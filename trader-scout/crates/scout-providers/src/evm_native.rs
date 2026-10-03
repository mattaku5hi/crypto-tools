//! Native-leg sources for EVM transactions (ADR-020 section 2, amended).
//!
//! The native currency (ETH/BNB) moves without logs when contracts forward
//! value, so a wallet's native proceeds of a sell are invisible in receipts.
//! This module establishes them, in this order, per transaction:
//!
//! a. **trace**: `debug_traceTransaction` with the `callTracer`; successful
//!    value transfers of the frame tree become `internal_transfers`
//!    (`NativeSource::Trace`);
//! b. **balance diff**: `eth_getBalance(W, block - 1)` versus
//!    `eth_getBalance(W, block)` when the node serves historical state AND
//!    the receipts of the block prove `W` is touched by no other transaction
//!    of that block (not `from`/`to` of another one, in no other
//!    transaction's log topics, emitter of none). The exact movement is that
//!    difference with the fee added back (`NativeSource::BalanceDiff`);
//! c. otherwise the leg stays **unobserved** (the trade keeps an Unknown
//!    consideration, never a number).
//!
//! Capabilities are feature-detected ONCE per resolver from a sample
//! transaction (the oldest one that needs resolving, the worst case for a
//! pruned node). A source that detection found unsupported is never tried
//! again; a per-transaction failure of a supported source degrades that
//! transaction to unobserved (counted with its reason) and never aborts.
//! Budget exhaustion is the one error that is propagated.
//!
//! Residual assumption of (b), documented in ADR-020: native value can be
//! pushed to `W` by another transaction's internal call without any log
//! naming `W` (e.g. `W` is the plain recipient of someone else's swap
//! proceeds). Receipts cannot exclude that; the check removes every case a
//! receipt can show. A trace source has no such assumption and is preferred.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, B256, I256, U256};
use futures::stream::{self, StreamExt};
use scout_core::{InternalTransfer, NativeBalanceDiff, NativeSource, RawEvmTransaction};
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::evm_rpc::{EvmRpcClient, EvmSourceError};
use crate::evm_wire::EvmReceiptInfo;

/// Upper bound on frames parsed from one trace and on the nesting depth.
const MAX_TRACE_FRAMES: usize = 100_000;
/// Matches serde_json's own recursion limit: a deeper tree cannot even be
/// deserialized from the wire (it surfaces as a failed trace).
const MAX_TRACE_DEPTH: usize = 128;
/// Cached blocks of receipts (bounded, invariant #13).
const MAX_CACHED_BLOCKS: usize = 256;

/// Whether a source works against this endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Supported,
    /// Not usable; the text is sanitized and carries no URL or key.
    Unsupported(String),
}

impl Capability {
    #[must_use]
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }

    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Supported => "supported".to_string(),
            Self::Unsupported(r) => format!("unsupported ({r})"),
        }
    }
}

/// Result of the one-time feature detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLegCapabilities {
    pub trace: Capability,
    pub archive_state: Capability,
}

/// Which sources a run may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeLegPolicy {
    pub allow_trace: bool,
    pub allow_archive: bool,
}

impl Default for NativeLegPolicy {
    fn default() -> Self {
        Self {
            allow_trace: true,
            allow_archive: true,
        }
    }
}

/// Why a transaction's native leg stays unobserved.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unobserved {
    /// Neither source is available on this endpoint.
    NoSource,
    /// A supported trace source failed for this transaction.
    TraceFailed(String),
    /// A supported archive source failed for this transaction.
    ArchiveFailed(String),
    /// The wallet is touched by another transaction of the block, so a
    /// balance difference would not be this transaction's.
    WalletNotAlone(&'static str),
    /// The fee cannot be added back exactly (Base receipt without `l1Fee`).
    FeeUnknown,
}

impl Unobserved {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::NoSource => "no_source".to_string(),
            Self::TraceFailed(_) => "trace_failed".to_string(),
            Self::ArchiveFailed(_) => "archive_failed".to_string(),
            Self::WalletNotAlone(r) => format!("wallet_not_alone:{r}"),
            Self::FeeUnknown => "fee_unknown".to_string(),
        }
    }
}

/// Per-transaction outcome, recorded for every resolved transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeLegOutcome {
    Observed(NativeSource),
    Unobserved(Unobserved),
}

impl NativeLegOutcome {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Observed(s) => s.label().to_string(),
            Self::Unobserved(u) => format!("unobserved:{}", u.label()),
        }
    }
}

/// Aggregate of one `resolve_many` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLegRun {
    pub capabilities: NativeLegCapabilities,
    /// `tx hash -> outcome` for every transaction that was asked about.
    pub outcomes: BTreeMap<B256, NativeLegOutcome>,
}

impl NativeLegRun {
    /// Counts by outcome label (`trace`, `balance_diff`, `unobserved:...`).
    #[must_use]
    pub fn counts(&self) -> BTreeMap<String, u64> {
        let mut m = BTreeMap::new();
        for o in self.outcomes.values() {
            *m.entry(o.label()).or_insert(0) += 1;
        }
        m
    }
}

fn sanitize(e: &EvmSourceError) -> String {
    crate::evm_blockscout::sanitize_text(&e.to_string())
}

fn u256_of(v: &Value) -> Option<U256> {
    let h = v.as_str()?.strip_prefix("0x")?;
    if h.is_empty() || h.len() > 64 {
        return None;
    }
    U256::from_str_radix(h, 16).ok()
}

fn address_of(v: &Value) -> Option<Address> {
    v.as_str()?.parse::<Address>().ok()
}

/// Failure to read a `callTracer` frame tree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TraceParseError {
    #[error("trace root is not a call frame object")]
    NotAFrame,
    #[error("a frame has a malformed `{0}` field")]
    BadField(&'static str),
    #[error("trace exceeds {MAX_TRACE_FRAMES} frames or depth {MAX_TRACE_DEPTH}")]
    TooLarge,
}

/// Value transfers of a `callTracer` tree that actually happened, depth >= 1
/// (the root's value is `tx.value`, accounted separately).
///
/// Rules: only `CALL`, `CREATE`, `CREATE2` and `SELFDESTRUCT` frames move
/// value; `DELEGATECALL`/`STATICCALL`/`CALLCODE` never do (their `value`
/// field, when present, is not a transfer). A frame carrying `error`
/// reverted: its transfer AND its whole subtree are discarded. Zero values
/// are dropped. Bounded iterative walk (no recursion).
pub fn parse_call_tree(root: &Value) -> Result<Vec<InternalTransfer>, TraceParseError> {
    if !root.is_object() || root.get("type").and_then(Value::as_str).is_none() {
        return Err(TraceParseError::NotAFrame);
    }
    let mut out = Vec::new();
    let mut frames = 0usize;
    // (frame, depth)
    let mut stack: Vec<(&Value, usize)> = vec![(root, 0)];
    while let Some((frame, depth)) = stack.pop() {
        frames += 1;
        if frames > MAX_TRACE_FRAMES || depth > MAX_TRACE_DEPTH {
            return Err(TraceParseError::TooLarge);
        }
        if frame.get("error").is_some_and(|e| !e.is_null()) {
            continue; // reverted: value and subtree never happened
        }
        if depth > 0 {
            let kind = frame
                .get("type")
                .and_then(Value::as_str)
                .ok_or(TraceParseError::BadField("type"))?;
            let moves_value = matches!(kind, "CALL" | "CREATE" | "CREATE2" | "SELFDESTRUCT");
            if moves_value && let Some(val) = frame.get("value").filter(|v| !v.is_null()) {
                let value = u256_of(val).ok_or(TraceParseError::BadField("value"))?;
                if !value.is_zero() {
                    let from = frame
                        .get("from")
                        .and_then(address_of)
                        .ok_or(TraceParseError::BadField("from"))?;
                    let to = frame
                        .get("to")
                        .and_then(address_of)
                        .ok_or(TraceParseError::BadField("to"))?;
                    out.push(InternalTransfer { from, to, value });
                }
            }
        }
        if let Some(calls) = frame.get("calls").and_then(Value::as_array) {
            // Reverse so pops come out in call order (deterministic).
            for c in calls.iter().rev() {
                stack.push((c, depth + 1));
            }
        }
    }
    Ok(out)
}

/// A resolved transaction's data, applied to the transaction afterwards.
struct Resolution {
    outcome: NativeLegOutcome,
    internal_transfers: Option<Vec<InternalTransfer>>,
    diff: Option<NativeBalanceDiff>,
}

impl Resolution {
    fn unobserved(u: Unobserved) -> Self {
        Self {
            outcome: NativeLegOutcome::Unobserved(u),
            internal_transfers: None,
            diff: None,
        }
    }
}

/// Resolves native legs for one chain endpoint. Cheap to share by reference;
/// the capability probe and the receipts cache live inside.
#[derive(Debug)]
pub struct NativeLegResolver {
    rpc: EvmRpcClient,
    policy: NativeLegPolicy,
    caps: OnceCell<NativeLegCapabilities>,
    blocks: Mutex<BTreeMap<u64, Arc<Vec<EvmReceiptInfo>>>>,
}

impl NativeLegResolver {
    #[must_use]
    pub fn new(rpc: EvmRpcClient, policy: NativeLegPolicy) -> Self {
        Self {
            rpc,
            policy,
            caps: OnceCell::new(),
            blocks: Mutex::new(BTreeMap::new()),
        }
    }

    /// The detected capabilities, if detection already ran.
    #[must_use]
    pub fn capabilities(&self) -> Option<&NativeLegCapabilities> {
        self.caps.get()
    }

    async fn detect(
        &self,
        sample: &RawEvmTransaction,
    ) -> Result<NativeLegCapabilities, EvmSourceError> {
        let trace = if self.policy.allow_trace {
            match self.rpc.trace_call_tree(sample.hash).await {
                Ok(v) if parse_call_tree(&v).is_ok() => Capability::Supported,
                Ok(_) => Capability::Unsupported("trace response is not a call frame".to_string()),
                Err(e) if e.is_budget_exhausted() => return Err(e),
                Err(e) => Capability::Unsupported(sanitize(&e)),
            }
        } else {
            Capability::Unsupported("disabled by policy".to_string())
        };
        let archive_state = if self.policy.allow_archive {
            match self
                .rpc
                .balance_at(sample.from, sample.block_number.saturating_sub(1))
                .await
            {
                Ok(_) => Capability::Supported,
                Err(e) if e.is_budget_exhausted() => return Err(e),
                Err(e) => Capability::Unsupported(sanitize(&e)),
            }
        } else {
            Capability::Unsupported("disabled by policy".to_string())
        };
        Ok(NativeLegCapabilities {
            trace,
            archive_state,
        })
    }

    async fn receipts_of(&self, block: u64) -> Result<Arc<Vec<EvmReceiptInfo>>, EvmSourceError> {
        if let Ok(c) = self.blocks.lock()
            && let Some(r) = c.get(&block)
        {
            return Ok(Arc::clone(r));
        }
        let r = Arc::new(self.rpc.block_receipts(block).await?);
        if let Ok(mut c) = self.blocks.lock() {
            while c.len() >= MAX_CACHED_BLOCKS {
                if c.pop_first().is_none() {
                    break;
                }
            }
            c.insert(block, Arc::clone(&r));
        }
        Ok(r)
    }

    /// Is `account` touched by any transaction of the block other than `hash`?
    fn wallet_alone(
        receipts: &[EvmReceiptInfo],
        hash: B256,
        account: Address,
    ) -> Result<(), &'static str> {
        let topic = account.into_word();
        let mut own = 0usize;
        for r in receipts {
            if r.tx_hash == hash {
                own += 1;
                continue;
            }
            if r.from == Some(account) || r.to == Some(account) {
                return Err("other_tx_from_or_to");
            }
            if r.logs
                .iter()
                .any(|l| l.address == account || l.topics.contains(&topic))
            {
                return Err("other_tx_log_mentions_wallet");
            }
        }
        if own == 1 {
            Ok(())
        } else {
            Err("tx_not_in_block_receipts")
        }
    }

    fn fee_of(&self, tx: &RawEvmTransaction) -> Option<U256> {
        let base = U256::from(tx.gas_used).checked_mul(tx.effective_gas_price)?;
        match (tx.l1_fee, self.rpc.profile().l1_fee_separate) {
            (Some(l1), _) => base.checked_add(l1),
            (None, true) => None,
            (None, false) => Some(base),
        }
    }

    async fn balance_diff(&self, tx: &RawEvmTransaction) -> Result<Resolution, EvmSourceError> {
        let receipts = match self.receipts_of(tx.block_number).await {
            Ok(r) => r,
            Err(e) if e.is_budget_exhausted() => return Err(e),
            Err(e) => {
                return Ok(Resolution::unobserved(Unobserved::ArchiveFailed(sanitize(
                    &e,
                ))));
            }
        };
        if let Err(why) = Self::wallet_alone(&receipts, tx.hash, tx.from) {
            return Ok(Resolution::unobserved(Unobserved::WalletNotAlone(why)));
        }
        let Some(fee) = self.fee_of(tx) else {
            return Ok(Resolution::unobserved(Unobserved::FeeUnknown));
        };
        let before_block = tx.block_number.saturating_sub(1);
        let (before, after) = match (
            self.rpc.balance_at(tx.from, before_block).await,
            self.rpc.balance_at(tx.from, tx.block_number).await,
        ) {
            (Ok(b), Ok(a)) => (b, a),
            (Err(e), _) | (_, Err(e)) => {
                if e.is_budget_exhausted() {
                    return Err(e);
                }
                return Ok(Resolution::unobserved(Unobserved::ArchiveFailed(sanitize(
                    &e,
                ))));
            }
        };
        let to_i = |x: U256| I256::try_from(x).ok();
        let net = match (to_i(before), to_i(after), to_i(fee)) {
            (Some(b), Some(a), Some(f)) => a.checked_sub(b).and_then(|d| d.checked_add(f)),
            _ => None,
        };
        let Some(net_excl_fee) = net else {
            return Ok(Resolution::unobserved(Unobserved::ArchiveFailed(
                "balance arithmetic overflow".to_string(),
            )));
        };
        Ok(Resolution {
            outcome: NativeLegOutcome::Observed(NativeSource::BalanceDiff),
            internal_transfers: None,
            diff: Some(NativeBalanceDiff {
                account: tx.from,
                net_excl_fee,
            }),
        })
    }

    async fn resolve_one(
        &self,
        tx: &RawEvmTransaction,
        caps: &NativeLegCapabilities,
    ) -> Result<Resolution, EvmSourceError> {
        let mut trace_error: Option<String> = None;
        if caps.trace.is_supported() {
            match self.rpc.trace_call_tree(tx.hash).await {
                Ok(v) => match parse_call_tree(&v) {
                    Ok(list) => {
                        return Ok(Resolution {
                            outcome: NativeLegOutcome::Observed(NativeSource::Trace),
                            internal_transfers: Some(list),
                            diff: None,
                        });
                    }
                    Err(e) => trace_error = Some(e.to_string()),
                },
                Err(e) if e.is_budget_exhausted() => return Err(e),
                Err(e) => trace_error = Some(sanitize(&e)),
            }
        }
        if caps.archive_state.is_supported() {
            let r = self.balance_diff(tx).await?;
            // A trace failure is reported when the archive path is not
            // exact either (the more informative reason for the report).
            return Ok(match (&r.outcome, trace_error) {
                (NativeLegOutcome::Unobserved(_), Some(t)) => {
                    Resolution::unobserved(Unobserved::TraceFailed(t))
                }
                _ => r,
            });
        }
        Ok(Resolution::unobserved(match trace_error {
            Some(t) => Unobserved::TraceFailed(t),
            None => Unobserved::NoSource,
        }))
    }

    /// Resolve the native legs of `txs[i]` for every `i` in `indices`
    /// (indices out of range are ignored). Transactions are patched in
    /// place; the returned run records capabilities and one outcome per
    /// asked transaction. At most the rpc client's concurrency in flight.
    ///
    /// # Errors
    /// Only request-budget exhaustion (or a malformed sample response that
    /// cannot even be classified) is returned; everything else degrades to
    /// an `Unobserved` outcome.
    pub async fn resolve_many(
        &self,
        txs: &mut [RawEvmTransaction],
        indices: &[usize],
    ) -> Result<NativeLegRun, EvmSourceError> {
        let mut wanted: Vec<usize> = indices.iter().copied().filter(|i| *i < txs.len()).collect();
        wanted.sort_unstable();
        wanted.dedup();
        // Probe sample: the oldest transaction (worst case for pruning).
        let sample = wanted
            .iter()
            .filter_map(|i| txs.get(*i))
            .min_by_key(|t| (t.block_number, t.transaction_index));
        let caps = match (self.caps.get(), sample) {
            (Some(c), _) => c.clone(),
            (None, Some(s)) => {
                let c = self.detect(s).await?;
                self.caps.get_or_init(|| async { c }).await.clone()
            }
            (None, None) => NativeLegCapabilities {
                trace: Capability::Unsupported("not checked: nothing to resolve".to_string()),
                archive_state: Capability::Unsupported(
                    "not checked: nothing to resolve".to_string(),
                ),
            },
        };
        let concurrency = self.rpc.config().concurrency;
        let view: &[RawEvmTransaction] = txs;
        let caps_ref = &caps;
        let results: Vec<(usize, Result<Resolution, EvmSourceError>)> = stream::iter(wanted)
            .map(|i| async move {
                match view.get(i) {
                    Some(t) => (i, self.resolve_one(t, caps_ref).await),
                    None => (i, Ok(Resolution::unobserved(Unobserved::NoSource))),
                }
            })
            .buffered(concurrency)
            .collect()
            .await;
        let mut outcomes = BTreeMap::new();
        for (i, r) in results {
            let r = r?;
            if let Some(tx) = txs.get_mut(i) {
                if let Some(list) = r.internal_transfers {
                    tx.internal_transfers = Some(list);
                    tx.native_source = Some(NativeSource::Trace);
                }
                if let Some(d) = r.diff {
                    tx.native_balance_diff = Some(d);
                }
                outcomes.insert(tx.hash, r.outcome);
            }
        }
        Ok(NativeLegRun {
            capabilities: caps,
            outcomes,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;
    use scout_core::EvmTxStatus;
    use scout_evm::ROBINHOOD;
    use scout_rpc::{RpcClient, RpcEndpoint};
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    const W: Address = address!("00000000000000000000000000000000000000a1");
    const OTHER: Address = address!("00000000000000000000000000000000000000b2");
    const ROUTER: Address = address!("00000000000000000000000000000000000000c3");

    fn a(addr: Address) -> String {
        format!("{addr:#x}")
    }

    // ---------------- trace parsing ----------------

    #[test]
    fn call_tree_keeps_successful_value_transfers_only() {
        let tree = json!({
            "type": "CALL", "from": a(W), "to": a(ROUTER), "value": "0x0",
            "calls": [
                {"type": "CALL", "from": a(ROUTER), "to": a(W), "value": "0x5"},
                {"type": "DELEGATECALL", "from": a(ROUTER), "to": a(OTHER), "value": "0x99"},
                {"type": "STATICCALL", "from": a(ROUTER), "to": a(OTHER)},
                {"type": "CALL", "from": a(ROUTER), "to": a(OTHER), "value": "0x7", "error": "execution reverted",
                 "calls": [{"type": "CALL", "from": a(OTHER), "to": a(W), "value": "0x3"}]},
                {"type": "CALL", "from": a(ROUTER), "to": a(OTHER), "value": "0x0"},
                {"type": "CALL", "from": a(ROUTER), "to": a(OTHER), "value": "0x2",
                 "calls": [{"type": "SELFDESTRUCT", "from": a(OTHER), "to": a(W), "value": "0x1"}]}
            ]
        });
        let got = parse_call_tree(&tree).unwrap();
        let pairs: Vec<(Address, Address, u64)> = got
            .iter()
            .map(|t| (t.from, t.to, u64::try_from(t.value).unwrap()))
            .collect();
        assert_eq!(
            pairs,
            vec![(ROUTER, W, 5), (ROUTER, OTHER, 2), (OTHER, W, 1)],
            "reverted subtree, delegate/static calls and zero values are dropped"
        );
    }

    #[test]
    fn call_tree_rejects_garbage_and_bounds_depth() {
        assert_eq!(parse_call_tree(&json!([])), Err(TraceParseError::NotAFrame));
        assert_eq!(
            parse_call_tree(&json!({"x": 1})),
            Err(TraceParseError::NotAFrame)
        );
        let bad =
            json!({"type":"CALL","calls":[{"type":"CALL","from":a(W),"to":a(OTHER),"value":"5"}]});
        assert_eq!(
            parse_call_tree(&bad),
            Err(TraceParseError::BadField("value"))
        );
        let mut deep = json!({"type":"CALL","from":a(W),"to":a(OTHER),"value":"0x0"});
        for _ in 0..(MAX_TRACE_DEPTH + 2) {
            deep = json!({"type":"CALL","from":a(W),"to":a(OTHER),"value":"0x0","calls":[deep]});
        }
        assert_eq!(parse_call_tree(&deep), Err(TraceParseError::TooLarge));
    }

    // ---------------- resolver over a mock node ----------------

    fn tx(hash: u8, block: u64, from: Address) -> RawEvmTransaction {
        RawEvmTransaction {
            chain: ROBINHOOD.verified_chain_key(),
            hash: B256::repeat_byte(hash),
            from,
            to: Some(ROUTER),
            block_number: block,
            transaction_index: 1,
            block_time: 1_000,
            value: U256::from(10u8),
            status: EvmTxStatus::Success,
            gas_used: 100,
            effective_gas_price: U256::from(2u8),
            l1_fee: None,
            logs: vec![],
            internal_transfers: None,
            native_source: None,
            native_balance_diff: None,
        }
    }

    fn receipt(
        hash: u8,
        block: u64,
        idx: u64,
        from: Address,
        to: Address,
        topic: Option<Address>,
    ) -> Value {
        let logs = topic.map_or_else(Vec::new, |t| {
            vec![json!({"address": a(ROUTER), "topics": [format!("{:#x}", B256::repeat_byte(9)), format!("{:#x}", t.into_word())],
                "data": "0x", "blockNumber": format!("{block:#x}"), "transactionIndex": format!("{idx:#x}"), "logIndex": "0x0"})]
        });
        json!({"transactionHash": format!("{:#x}", B256::repeat_byte(hash)),
            "blockNumber": format!("{block:#x}"), "transactionIndex": format!("{idx:#x}"),
            "status": "0x1", "gasUsed": "0x64", "effectiveGasPrice": "0x2",
            "from": a(from), "to": a(to), "logs": logs})
    }

    /// Behaviour switches of the mock node.
    struct Node {
        trace: Option<Value>,
        /// `(balance_before, balance_after)` of W; `None` = no historical state.
        balances: Option<(u64, u64)>,
        block_receipts: Value,
    }

    impl Respond for Node {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let err = |code: i64, msg: &str| {
                ResponseTemplate::new(200).set_body_json(
                    json!({"jsonrpc":"2.0","id":1,"error":{"code":code,"message":msg}}),
                )
            };
            let ok = |v: Value| {
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":v}))
            };
            match body["method"].as_str().unwrap() {
                "debug_traceTransaction" => match &self.trace {
                    Some(t) => ok(t.clone()),
                    None => err(
                        -32601,
                        "the method debug_traceTransaction does not exist/is not available",
                    ),
                },
                "eth_getBalance" => match self.balances {
                    Some((before, after)) => {
                        // block param 5 = after, 4 = before in these tests.
                        let blk = body["params"][1].as_str().unwrap();
                        ok(json!(format!(
                            "{:#x}",
                            if blk == "0x5" { after } else { before }
                        )))
                    }
                    None => err(-32000, "historical state is not available"),
                },
                "eth_getBlockReceipts" => ok(self.block_receipts.clone()),
                other => err(-32601, &format!("unexpected {other}")),
            }
        }
    }

    async fn resolver(server: &MockServer, policy: NativeLegPolicy) -> NativeLegResolver {
        let rpc = RpcClient::new(RpcEndpoint::new(server.uri()), 5_000, 1).unwrap();
        NativeLegResolver::new(EvmRpcClient::new(rpc, ROBINHOOD), policy)
    }

    async fn serve(node: Node) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(node)
            .mount(&s)
            .await;
        s
    }

    fn alone_block() -> Value {
        json!([
            receipt(1, 5, 1, W, ROUTER, None),
            receipt(2, 5, 2, OTHER, ROUTER, None)
        ])
    }

    #[tokio::test]
    async fn trace_path_fills_internal_transfers_and_is_preferred() {
        let trace = json!({"type":"CALL","from":a(W),"to":a(ROUTER),"value":"0xa",
            "calls":[{"type":"CALL","from":a(ROUTER),"to":a(W),"value":"0x5"}]});
        let s = serve(Node {
            trace: Some(trace),
            balances: Some((100, 200)),
            block_receipts: alone_block(),
        })
        .await;
        let r = resolver(&s, NativeLegPolicy::default()).await;
        let mut txs = vec![tx(1, 5, W)];
        let run = r.resolve_many(&mut txs, &[0]).await.unwrap();
        assert!(
            run.capabilities.trace.is_supported() && run.capabilities.archive_state.is_supported()
        );
        assert_eq!(
            run.outcomes[&txs[0].hash],
            NativeLegOutcome::Observed(NativeSource::Trace)
        );
        assert_eq!(txs[0].native_source, Some(NativeSource::Trace));
        assert_eq!(txs[0].internal_transfers.as_ref().unwrap().len(), 1);
        assert!(txs[0].native_balance_diff.is_none());
    }

    #[tokio::test]
    async fn balance_diff_adds_the_fee_back_exactly() {
        // W had 1000 before, 995 after: it paid value 10 + fee 200 and got 205 back.
        let s = serve(Node {
            trace: None,
            balances: Some((1_000, 995)),
            block_receipts: alone_block(),
        })
        .await;
        let r = resolver(&s, NativeLegPolicy::default()).await;
        let mut txs = vec![tx(1, 5, W)];
        let run = r.resolve_many(&mut txs, &[0]).await.unwrap();
        assert!(!run.capabilities.trace.is_supported());
        assert!(run.capabilities.archive_state.is_supported());
        assert_eq!(
            run.outcomes[&txs[0].hash],
            NativeLegOutcome::Observed(NativeSource::BalanceDiff)
        );
        // gas 100 * price 2 = fee 200 (Robinhood: no separate l1 fee).
        let d = txs[0].native_balance_diff.unwrap();
        assert_eq!(d.account, W);
        assert_eq!(
            d.net_excl_fee,
            I256::try_from(995i64 - 1_000 + 200).unwrap()
        );
        assert!(txs[0].internal_transfers.is_none());
    }

    #[tokio::test]
    async fn balance_diff_is_refused_when_the_wallet_is_not_alone_in_the_block() {
        let cases = [
            (
                json!([
                    receipt(1, 5, 1, W, ROUTER, None),
                    receipt(2, 5, 2, W, ROUTER, None)
                ]),
                "other_tx_from_or_to",
            ),
            (
                json!([
                    receipt(1, 5, 1, W, ROUTER, None),
                    receipt(2, 5, 2, OTHER, W, None)
                ]),
                "other_tx_from_or_to",
            ),
            (
                json!([
                    receipt(1, 5, 1, W, ROUTER, None),
                    receipt(2, 5, 2, OTHER, ROUTER, Some(W))
                ]),
                "other_tx_log_mentions_wallet",
            ),
        ];
        for (block, why) in cases {
            let s = serve(Node {
                trace: None,
                balances: Some((1_000, 995)),
                block_receipts: block,
            })
            .await;
            let r = resolver(&s, NativeLegPolicy::default()).await;
            let mut txs = vec![tx(1, 5, W)];
            let run = r.resolve_many(&mut txs, &[0]).await.unwrap();
            assert_eq!(
                run.outcomes[&txs[0].hash],
                NativeLegOutcome::Unobserved(Unobserved::WalletNotAlone(why))
            );
            assert!(
                txs[0].native_balance_diff.is_none(),
                "never a partial number"
            );
        }
    }

    #[tokio::test]
    async fn no_source_leaves_the_leg_unobserved() {
        let s = serve(Node {
            trace: None,
            balances: None,
            block_receipts: alone_block(),
        })
        .await;
        let r = resolver(&s, NativeLegPolicy::default()).await;
        let mut txs = vec![tx(1, 5, W)];
        let run = r.resolve_many(&mut txs, &[0]).await.unwrap();
        assert!(!run.capabilities.trace.is_supported());
        assert!(!run.capabilities.archive_state.is_supported());
        assert_eq!(
            run.outcomes[&txs[0].hash],
            NativeLegOutcome::Unobserved(Unobserved::NoSource)
        );
        assert!(txs[0].internal_transfers.is_none() && txs[0].native_balance_diff.is_none());
        assert_eq!(run.counts().get("unobserved:no_source"), Some(&1));
    }

    #[tokio::test]
    async fn policy_can_disable_a_source() {
        let s = serve(Node {
            trace: None,
            balances: Some((1_000, 995)),
            block_receipts: alone_block(),
        })
        .await;
        let r = resolver(
            &s,
            NativeLegPolicy {
                allow_trace: true,
                allow_archive: false,
            },
        )
        .await;
        let mut txs = vec![tx(1, 5, W)];
        let run = r.resolve_many(&mut txs, &[0]).await.unwrap();
        assert_eq!(
            run.outcomes[&txs[0].hash],
            NativeLegOutcome::Unobserved(Unobserved::NoSource)
        );
        assert!(
            run.capabilities
                .archive_state
                .describe()
                .contains("disabled")
        );
    }

    #[tokio::test]
    async fn detection_runs_once() {
        let s = serve(Node {
            trace: None,
            balances: None,
            block_receipts: alone_block(),
        })
        .await;
        let r = resolver(&s, NativeLegPolicy::default()).await;
        let mut txs = vec![tx(1, 5, W)];
        r.resolve_many(&mut txs, &[0]).await.unwrap();
        let before = s.received_requests().await.unwrap().len();
        r.resolve_many(&mut txs, &[0]).await.unwrap();
        // Second run: no probe requests at all (both sources known unsupported).
        assert_eq!(s.received_requests().await.unwrap().len(), before);
    }
}
