//! EVM JSON-RPC client on top of `scout_rpc::RpcClient` (body cap, retries
//! and the shared request budget all come from there).
//!
//! What lives here (ADR-020 §3):
//! - chain identity preflight (`eth_chainId` + block-0 hash) before any scan;
//! - `eth_getLogs` with adaptive range halving on range/result-cap errors;
//! - `eth_getBlockReceipts`, `eth_getTransactionReceipt`,
//!   `eth_getTransactionByHash` (bounded concurrency, input order kept);
//! - block timestamps with a bounded cache and a binary search from a
//!   timestamp to a block number.
//!
//! JSON-RPC batch requests are not used: `RpcClient` issues one request per
//! HTTP call so retries/budget stay exact; throughput comes from bounded
//! concurrency instead.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, B256, U256};
use futures::stream::{self, StreamExt};
use scout_api::ProviderError;
use scout_core::ChainKey;
use scout_evm::EvmChainProfile;
use scout_rpc::{RequestBudgetExhausted, ResponseTooLarge, RpcClient};
use serde_json::{Value, json};

use crate::evm_wire::{
    EvmReceiptInfo, EvmTxInfo, b256, malformed, parse_logs, parse_receipt, parse_tx, quantity_u64,
};

/// Typed failures of the EVM sources.
#[derive(Debug, thiserror::Error)]
pub enum EvmSourceError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("chain id mismatch: expected {expected}, endpoint reports {actual}")]
    ChainIdMismatch { expected: u64, actual: u64 },
    #[error(
        "genesis hash mismatch for chain {chain_id}: expected {expected:#x}, endpoint reports {actual:#x}"
    )]
    GenesisMismatch {
        chain_id: u64,
        expected: B256,
        actual: B256,
    },
    #[error("eth_getLogs range cannot be narrowed below one block (block {block}): {detail}")]
    RangeUnresolvable { block: u64, detail: String },
    #[error("eth_getLogs returned more than the configured cap of {cap} logs")]
    TooManyLogs { cap: usize },
    #[error("eth_getLogs split limit of {limit} reached")]
    TooManySplits { limit: u32 },
    #[error("malformed {what}: {detail}")]
    Malformed { what: &'static str, detail: String },
    #[error("{what} not found at the endpoint")]
    NotFound { what: String },
    #[error("too many transactions: more than the configured cap of {cap}")]
    TooManyTransactions { cap: usize },
}

impl EvmSourceError {
    /// `true` when the shared request budget is spent (incomplete coverage,
    /// not an infrastructure failure).
    #[must_use]
    pub fn is_budget_exhausted(&self) -> bool {
        matches!(self, Self::Provider(ProviderError::Other(e))
            if e.downcast_ref::<RequestBudgetExhausted>().is_some())
    }
}

/// Tunables; all caps are hard bounds (invariant #13).
#[derive(Debug, Clone, Copy)]
pub struct EvmRpcConfig {
    /// Max in-flight requests for fan-out calls (clamped to 1..=64).
    pub concurrency: usize,
    /// Max logs a single `get_logs` may accumulate.
    pub max_logs: usize,
    /// Max range splits per `get_logs`.
    pub max_splits: u32,
    /// Max cached block timestamps.
    pub timestamp_cache_cap: usize,
}

impl Default for EvmRpcConfig {
    fn default() -> Self {
        Self {
            concurrency: 8,
            max_logs: 200_000,
            max_splits: 100_000,
            timestamp_cache_cap: 65_536,
        }
    }
}

/// `eth_getLogs` filter (topic positions 0..=3; `None` = wildcard, several
/// values = OR).
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub addresses: Vec<Address>,
    pub topics: [Option<Vec<B256>>; 4],
}

impl LogFilter {
    fn to_json(&self, from: u64, to: u64) -> Value {
        let mut topics: Vec<Value> = self
            .topics
            .iter()
            .map(|t| match t {
                None => Value::Null,
                Some(v) if v.len() == 1 => json!(v.first().map(|h| format!("{h:#x}"))),
                Some(v) => json!(v.iter().map(|h| format!("{h:#x}")).collect::<Vec<_>>()),
            })
            .collect();
        while topics.last().is_some_and(Value::is_null) {
            topics.pop();
        }
        let mut obj = json!({
            "fromBlock": format!("{from:#x}"),
            "toBlock": format!("{to:#x}"),
            "topics": topics,
        });
        let address = match self.addresses.as_slice() {
            [] => None,
            [one] => Some(json!(format!("{one:#x}"))),
            many => Some(json!(
                many.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>()
            )),
        };
        if let (Some(address), Some(map)) = (address, obj.as_object_mut()) {
            map.insert("address".to_string(), address);
        }
        obj
    }
}

/// Logs plus accounting of how they were obtained.
#[derive(Debug, Clone)]
pub struct LogsResult {
    pub logs: Vec<scout_core::RawEvmLog>,
    /// Successful `eth_getLogs` calls.
    pub requests: u32,
    /// Range halvings performed.
    pub splits: u32,
}

/// Bounded recorder of raw JSON-RPC exchanges (golden-fixture capture).
/// Holds only method/params/result; the endpoint URL and keys never enter.
#[derive(Debug)]
pub struct CallRecorder {
    max_calls: usize,
    max_bytes: usize,
    inner: Mutex<RecorderState>,
}

#[derive(Debug, Default)]
struct RecorderState {
    calls: Vec<Value>,
    bytes: usize,
    dropped: u64,
}

impl CallRecorder {
    #[must_use]
    pub fn new(max_calls: usize, max_bytes: usize) -> Self {
        Self {
            max_calls,
            max_bytes,
            inner: Mutex::new(RecorderState::default()),
        }
    }

    fn record(&self, method: &str, params: &Value, result: &Value) {
        let entry = json!({"method": method, "params": params, "result": result});
        let size = entry.to_string().len();
        let Ok(mut st) = self.inner.lock() else {
            return;
        };
        if st.calls.len() >= self.max_calls || st.bytes.saturating_add(size) > self.max_bytes {
            st.dropped = st.dropped.saturating_add(1);
            return;
        }
        st.bytes += size;
        st.calls.push(entry);
    }

    /// `(calls, dropped_over_cap)`.
    #[must_use]
    pub fn snapshot(&self) -> (Vec<Value>, u64) {
        self.inner
            .lock()
            .map(|s| (s.calls.clone(), s.dropped))
            .unwrap_or_default()
    }
}

/// EVM RPC client bound to one chain profile.
#[derive(Debug, Clone)]
pub struct EvmRpcClient {
    rpc: RpcClient,
    /// Separate endpoint for `eth_getLogs` only (per-method routing); it
    /// shares the main client's request budget. `None` = logs go to `rpc`.
    logs_rpc: Option<RpcClient>,
    profile: EvmChainProfile,
    cfg: EvmRpcConfig,
    timestamps: Arc<Mutex<TimestampCache>>,
    recorder: Option<Arc<CallRecorder>>,
    /// Whole-block receipts fetched so far (shared by clones): the native-leg
    /// sole-touch check and the receipt of the transaction itself come from
    /// ONE `eth_getBlockReceipts`. Bounded by total receipt count.
    block_cache: Arc<Mutex<BlockReceiptCache>>,
    /// Logical calls by JSON-RPC method (a retry is not a new call).
    call_counts: Arc<Mutex<BTreeMap<String, u64>>>,
}

/// Max receipts held in the shared block cache (oldest blocks evicted).
const MAX_CACHED_RECEIPTS: usize = 50_000;

#[derive(Debug, Default)]
struct BlockReceiptCache {
    map: BTreeMap<u64, Arc<Vec<EvmReceiptInfo>>>,
    receipts: usize,
}

#[derive(Debug, Default)]
struct TimestampCache {
    map: BTreeMap<u64, u64>,
    order: VecDeque<u64>,
}

/// Is this error a "range too wide / too many results" signal where
/// narrowing the window can help? Budget exhaustion is never one (its text
/// contains "limit").
#[must_use]
pub fn is_range_or_cap_error(err: &ProviderError) -> bool {
    match err {
        ProviderError::Other(e) => {
            if e.downcast_ref::<RequestBudgetExhausted>().is_some() {
                return false;
            }
            if e.downcast_ref::<ResponseTooLarge>().is_some() {
                return true;
            }
            let t = e.to_string().to_ascii_lowercase();
            // Base "max range 2000", Robinhood "-32000 logs matched by query
            // exceeds limit of 10000", BSC "limit exceeded", publicnode
            // "query exceeds max results", "response too large".
            [
                "range",
                "limit",
                "exceed",
                "too large",
                "too many",
                "max results",
                "more than",
            ]
            .iter()
            .any(|k| t.contains(k))
        }
        _ => false,
    }
}

/// The block range a provider suggests in a range-cap error, e.g. Alchemy
/// free tier: `... this block range should work: [0x10, 0x19]`.
#[must_use]
pub fn suggested_range(text: &str) -> Option<(u64, u64)> {
    let tail = text.split_once("should work:")?.1;
    let inner = tail.split_once('[')?.1.split_once(']')?.0;
    let (a, b) = inner.split_once(',')?;
    let hex = |x: &str| u64::from_str_radix(x.trim().strip_prefix("0x")?, 16).ok();
    let (a, b) = (hex(a)?, hex(b)?);
    (a <= b).then_some((a, b))
}

/// The block span a range-cap error announces: the suggested range's length
/// (`should work: [0x10, 0x19]`), else the number in `up to a N block range`.
#[must_use]
pub fn capped_span_from_error(text: &str) -> Option<u64> {
    if let Some((a, b)) = suggested_range(text) {
        return b.checked_sub(a).map(|d| d.saturating_add(1));
    }
    let tail = text.split_once("up to a ")?.1;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn is_empty_envelope(err: &ProviderError) -> bool {
    matches!(err, ProviderError::Other(e) if e.to_string().contains("neither `result` nor `error`"))
}

impl EvmRpcClient {
    #[must_use]
    pub fn new(rpc: RpcClient, profile: EvmChainProfile) -> Self {
        Self::with_config(rpc, profile, EvmRpcConfig::default())
    }

    #[must_use]
    pub fn with_config(rpc: RpcClient, profile: EvmChainProfile, mut cfg: EvmRpcConfig) -> Self {
        cfg.concurrency = cfg.concurrency.clamp(1, 64);
        Self {
            rpc,
            logs_rpc: None,
            profile,
            cfg,
            timestamps: Arc::new(Mutex::new(TimestampCache::default())),
            recorder: None,
            block_cache: Arc::new(Mutex::new(BlockReceiptCache::default())),
            call_counts: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Route `eth_getLogs` (and only it) to `logs_rpc`; receipts, archive
    /// state, traces and everything else stay on the main endpoint. The two
    /// endpoints share ONE request budget.
    #[must_use]
    pub fn with_logs_endpoint(mut self, logs_rpc: RpcClient) -> Self {
        self.logs_rpc = Some(logs_rpc.sharing_budget_with(&self.rpc));
        self
    }

    /// Logical JSON-RPC calls made so far by method (retries not counted).
    #[must_use]
    pub fn calls_by_method(&self) -> BTreeMap<String, u64> {
        self.call_counts
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// `true` when `eth_getLogs` goes to a separate endpoint.
    #[must_use]
    pub fn has_logs_endpoint(&self) -> bool {
        self.logs_rpc.is_some()
    }

    #[must_use]
    pub fn with_recorder(mut self, recorder: Arc<CallRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    #[must_use]
    pub fn profile(&self) -> &EvmChainProfile {
        &self.profile
    }

    #[must_use]
    pub fn config(&self) -> &EvmRpcConfig {
        &self.cfg
    }

    #[must_use]
    pub fn total_requests_made(&self) -> u64 {
        self.rpc.total_requests_made()
    }

    /// One call; a JSON `null` result (legit for unknown tx/block) comes
    /// back as `Value::Null` instead of the transport's "empty envelope".
    async fn call_json(&self, method: &str, params: Value) -> Result<Value, EvmSourceError> {
        if let Ok(mut c) = self.call_counts.lock() {
            *c.entry(method.to_string()).or_insert(0) += 1;
        }
        let rpc = match (&self.logs_rpc, method) {
            (Some(logs), "eth_getLogs") => logs,
            _ => &self.rpc,
        };
        let result = match rpc.call::<_, Value>(method, &params).await {
            Ok(v) => v,
            Err(e) if is_empty_envelope(&e) => Value::Null,
            Err(e) => return Err(e.into()),
        };
        if let Some(r) = &self.recorder {
            r.record(method, &params, &result);
        }
        Ok(result)
    }

    pub async fn chain_id(&self) -> Result<u64, EvmSourceError> {
        let v = self.call_json("eth_chainId", json!([])).await?;
        quantity_u64(&v, "eth_chainId")
    }

    pub async fn block_number(&self) -> Result<u64, EvmSourceError> {
        let v = self.call_json("eth_blockNumber", json!([])).await?;
        quantity_u64(&v, "eth_blockNumber")
    }

    /// Hash of block 0.
    pub async fn genesis_hash(&self) -> Result<B256, EvmSourceError> {
        let v = self
            .call_json("eth_getBlockByNumber", json!(["0x0", false]))
            .await?;
        b256(
            v.get("hash")
                .ok_or_else(|| malformed("block 0", "missing `hash`"))?,
            "block 0 hash",
        )
    }

    /// Chain identity preflight (invariants #3/#4): `eth_chainId` and the
    /// block-0 hash must both equal the profile's. Returns the verified
    /// `ChainKey`; any mismatch is a typed error and nothing may be scanned.
    pub async fn preflight(&self) -> Result<ChainKey, EvmSourceError> {
        let actual = self.chain_id().await?;
        if actual != self.profile.chain_id {
            return Err(EvmSourceError::ChainIdMismatch {
                expected: self.profile.chain_id,
                actual,
            });
        }
        let genesis = self.genesis_hash().await?;
        if genesis != self.profile.genesis_hash {
            return Err(EvmSourceError::GenesisMismatch {
                chain_id: actual,
                expected: self.profile.genesis_hash,
                actual: genesis,
            });
        }
        Ok(self.profile.verified_chain_key())
    }

    /// `eth_getLogs` over `[from, to]` (inclusive). On a range/result-cap
    /// error the window is halved and both halves retried (explicit stack,
    /// ascending order), down to single blocks.
    pub async fn get_logs(
        &self,
        filter: &LogFilter,
        from: u64,
        to: u64,
    ) -> Result<LogsResult, EvmSourceError> {
        let mut stack = vec![(from, to)];
        let mut out: Vec<scout_core::RawEvmLog> = Vec::new();
        let (mut requests, mut splits) = (0u32, 0u32);
        // Provider-announced maximum span (blocks) once a range error showed
        // one: later windows are cut to it up front instead of failing again.
        let mut span_cap: Option<u64> = None;
        while let Some((a, b)) = stack.pop() {
            if a > b {
                continue;
            }
            if let Some(cap) = span_cap
                && b - a >= cap
            {
                stack.push((a + cap, b));
                stack.push((a, a + cap - 1));
                continue;
            }
            match self
                .call_json("eth_getLogs", json!([filter.to_json(a, b)]))
                .await
            {
                Ok(v) => {
                    let logs = parse_logs(&v)?;
                    if out.len().saturating_add(logs.len()) > self.cfg.max_logs {
                        return Err(EvmSourceError::TooManyLogs {
                            cap: self.cfg.max_logs,
                        });
                    }
                    requests += 1;
                    out.extend(logs);
                }
                Err(EvmSourceError::Provider(e)) if is_range_or_cap_error(&e) => {
                    if a == b {
                        return Err(EvmSourceError::RangeUnresolvable {
                            block: a,
                            detail: crate::evm_blockscout::sanitize_text(&e.to_string()),
                        });
                    }
                    if splits >= self.cfg.max_splits {
                        return Err(EvmSourceError::TooManySplits {
                            limit: self.cfg.max_splits,
                        });
                    }
                    splits += 1;
                    // A suggested range that is strictly smaller than the
                    // failed one fixes the span for the rest of the scan.
                    if let Some((sa, sb)) = suggested_range(&e.to_string())
                        && sb >= sa
                        && sb - sa < b - a
                    {
                        span_cap = Some(sb - sa + 1);
                        stack.push((a + (sb - sa + 1), b));
                        stack.push((a, a + (sb - sa)));
                        continue;
                    }
                    let mid = a + ((b - a) >> 1);
                    stack.push((mid + 1, b));
                    stack.push((a, mid));
                }
                Err(e) => return Err(e),
            }
        }
        out.sort_by_key(|l| (l.block_number, l.log_index));
        out.dedup_by_key(|l| (l.block_number, l.log_index));
        Ok(LogsResult {
            logs: out,
            requests,
            splits,
        })
    }

    /// Does the MAIN endpoint cap the `eth_getLogs` block range? Issues one
    /// 100-block query (after `eth_blockNumber`): a range error naming a span
    /// (Alchemy free: "up to a 10 block range" / "should work: [a, b]")
    /// yields `Some(span)`; success or an error that names no span yields
    /// `None` (adaptive splitting still copes with the latter).
    ///
    /// # Errors
    /// Transport/budget errors (anything that is not a range/cap error).
    pub async fn probe_logs_span(&self) -> Result<Option<u64>, EvmSourceError> {
        let head = self.block_number().await?;
        let from = head.saturating_sub(99);
        let filter = LogFilter {
            addresses: vec![Address::ZERO],
            topics: [Some(vec![scout_evm::TRANSFER_TOPIC0]), None, None, None],
        };
        match self
            .rpc
            .call::<_, Value>("eth_getLogs", json!([filter.to_json(from, head)]))
            .await
        {
            Ok(_) => Ok(None),
            Err(e) if is_range_or_cap_error(&e) => {
                Ok(capped_span_from_error(&e.to_string()).filter(|s| *s > 0))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Native balance of `account` at the END of `block`
    /// (`eth_getBalance`). Needs historical state for past blocks; a node
    /// without it answers with a JSON-RPC error.
    pub async fn balance_at(&self, account: Address, block: u64) -> Result<U256, EvmSourceError> {
        let v = self
            .call_json(
                "eth_getBalance",
                json!([format!("{account:#x}"), format!("{block:#x}")]),
            )
            .await?;
        crate::evm_wire::quantity_u256(&v, "eth_getBalance")
    }

    /// `debug_traceTransaction` with the built-in `callTracer`. The raw frame
    /// tree is returned as JSON; parsing is `evm_native::parse_call_tree`.
    pub async fn trace_call_tree(&self, hash: B256) -> Result<Value, EvmSourceError> {
        self.call_json(
            "debug_traceTransaction",
            json!([format!("{hash:#x}"), {"tracer": "callTracer"}]),
        )
        .await
    }

    /// `eth_call` of `decimals()` on an ERC-20 (`0x313ce567`) at `latest`.
    pub async fn erc20_decimals(&self, token: Address) -> Result<u8, EvmSourceError> {
        let v = self
            .call_json(
                "eth_call",
                json!([{"to": format!("{token:#x}"), "data": "0x313ce567"}, "latest"]),
            )
            .await?;
        let q = crate::evm_wire::quantity_u256(&v, "decimals()")?;
        u8::try_from(q).map_err(|_| malformed("decimals()", "does not fit u8"))
    }

    /// All receipts of a block (`eth_getBlockReceipts`).
    pub async fn block_receipts(&self, block: u64) -> Result<Vec<EvmReceiptInfo>, EvmSourceError> {
        let v = self
            .call_json("eth_getBlockReceipts", json!([format!("{block:#x}")]))
            .await?;
        if v.is_null() {
            return Err(EvmSourceError::NotFound {
                what: format!("receipts of block {block}"),
            });
        }
        v.as_array()
            .ok_or_else(|| malformed("eth_getBlockReceipts", "result is not an array"))?
            .iter()
            .map(parse_receipt)
            .collect()
    }

    /// Receipts of a block, cached and shared (bounded; see
    /// `MAX_CACHED_RECEIPTS`).
    pub async fn block_receipts_shared(
        &self,
        block: u64,
    ) -> Result<Arc<Vec<EvmReceiptInfo>>, EvmSourceError> {
        if let Ok(c) = self.block_cache.lock()
            && let Some(r) = c.map.get(&block)
        {
            return Ok(Arc::clone(r));
        }
        let r = Arc::new(self.block_receipts(block).await?);
        if let Ok(mut c) = self.block_cache.lock() {
            while c.receipts.saturating_add(r.len()) > MAX_CACHED_RECEIPTS {
                let Some((_, old)) = c.map.pop_first() else {
                    break;
                };
                c.receipts = c.receipts.saturating_sub(old.len());
            }
            c.receipts = c.receipts.saturating_add(r.len());
            if let Some(prev) = c.map.insert(block, Arc::clone(&r)) {
                c.receipts = c.receipts.saturating_sub(prev.len());
            }
        }
        Ok(r)
    }

    pub async fn transaction_receipt(&self, hash: B256) -> Result<EvmReceiptInfo, EvmSourceError> {
        let v = self
            .call_json("eth_getTransactionReceipt", json!([format!("{hash:#x}")]))
            .await?;
        if v.is_null() {
            return Err(EvmSourceError::NotFound {
                what: format!("receipt {hash:#x}"),
            });
        }
        parse_receipt(&v)
    }

    pub async fn transaction_by_hash(&self, hash: B256) -> Result<EvmTxInfo, EvmSourceError> {
        let v = self
            .call_json("eth_getTransactionByHash", json!([format!("{hash:#x}")]))
            .await?;
        if v.is_null() {
            return Err(EvmSourceError::NotFound {
                what: format!("transaction {hash:#x}"),
            });
        }
        parse_tx(&v)
    }

    /// Transactions for `hashes`, in input order, at most
    /// `config.concurrency` in flight. Any missing/failed one fails the call.
    pub async fn transactions_by_hashes(
        &self,
        hashes: &[B256],
    ) -> Result<Vec<EvmTxInfo>, EvmSourceError> {
        stream::iter(hashes.iter().copied())
            .map(|h| async move { self.transaction_by_hash(h).await })
            .buffered(self.cfg.concurrency)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    /// Receipts per hash, in input order, bounded concurrency.
    pub async fn receipts_by_hashes(
        &self,
        hashes: &[B256],
    ) -> Result<Vec<EvmReceiptInfo>, EvmSourceError> {
        stream::iter(hashes.iter().copied())
            .map(|h| async move { self.transaction_receipt(h).await })
            .buffered(self.cfg.concurrency)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    /// Block receipts for distinct blocks, `(block, receipts)` in input order.
    pub async fn receipts_by_blocks(
        &self,
        blocks: &[u64],
    ) -> Result<Vec<(u64, Vec<EvmReceiptInfo>)>, EvmSourceError> {
        stream::iter(blocks.iter().copied())
            .map(|b| async move {
                self.block_receipts_shared(b)
                    .await
                    .map(|r| (b, r.as_ref().clone()))
            })
            .buffered(self.cfg.concurrency)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    fn cached_timestamp(&self, block: u64) -> Option<u64> {
        self.timestamps.lock().ok()?.map.get(&block).copied()
    }

    fn cache_timestamp(&self, block: u64, ts: u64) {
        let Ok(mut c) = self.timestamps.lock() else {
            return;
        };
        if c.map.insert(block, ts).is_none() {
            c.order.push_back(block);
            while c.order.len() > self.cfg.timestamp_cache_cap {
                if let Some(old) = c.order.pop_front() {
                    c.map.remove(&old);
                }
            }
        }
    }

    /// Pre-fill the timestamp cache from a source that already carries block
    /// times (explorer listings), saving one `eth_getBlockByNumber` per
    /// block. Zero timestamps are ignored; entries already cached win.
    pub fn seed_block_timestamps(&self, pairs: impl IntoIterator<Item = (u64, u64)>) {
        for (block, ts) in pairs {
            if ts > 0 && self.cached_timestamp(block).is_none() {
                self.cache_timestamp(block, ts);
            }
        }
    }

    /// Block timestamp (unix seconds), cached.
    pub async fn block_timestamp(&self, block: u64) -> Result<u64, EvmSourceError> {
        if let Some(ts) = self.cached_timestamp(block) {
            return Ok(ts);
        }
        let v = self
            .call_json(
                "eth_getBlockByNumber",
                json!([format!("{block:#x}"), false]),
            )
            .await?;
        if v.is_null() {
            return Err(EvmSourceError::NotFound {
                what: format!("block {block}"),
            });
        }
        let ts = quantity_u64(
            v.get("timestamp")
                .ok_or_else(|| malformed("block", "missing `timestamp`"))?,
            "block timestamp",
        )?;
        self.cache_timestamp(block, ts);
        Ok(ts)
    }

    /// Timestamps of distinct blocks (bounded concurrency).
    pub async fn block_timestamps(
        &self,
        blocks: &[u64],
    ) -> Result<BTreeMap<u64, u64>, EvmSourceError> {
        let pairs: Vec<(u64, u64)> = stream::iter(blocks.iter().copied())
            .map(|b| async move { self.block_timestamp(b).await.map(|t| (b, t)) })
            .buffered(self.cfg.concurrency)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;
        Ok(pairs.into_iter().collect())
    }

    /// First block in `[0, latest]` with `timestamp >= ts`; `latest + 1`
    /// when every block is older. Binary search over block times (monotone
    /// non-decreasing on all three chains), ~log2(latest) calls.
    pub async fn first_block_at_or_after(
        &self,
        ts: u64,
        latest: u64,
    ) -> Result<u64, EvmSourceError> {
        let (mut lo, mut hi) = (0u64, latest.saturating_add(1));
        while lo < hi {
            let mid = lo + ((hi - lo) >> 1);
            if self.block_timestamp(mid).await? >= ts {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        Ok(lo)
    }
}

#[cfg(test)]
mod tests {
    use scout_evm::{BASE, ROBINHOOD};
    use scout_rpc::RpcEndpoint;
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn client(server: &MockServer, profile: EvmChainProfile, budget: Option<u64>) -> EvmRpcClient {
        let rpc = RpcClient::new(RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
            .with_max_total_requests(budget);
        EvmRpcClient::new(rpc, profile)
    }

    fn ok(result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }

    fn err(code: i64, msg: &str) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"jsonrpc":"2.0","id":1,"error":{"code":code,"message":msg}}))
    }

    fn log_json(block: u64, idx: u64) -> Value {
        json!({"address":"0x0000000000000000000000000000000000000001","topics":[],"data":"0x",
            "blockNumber":format!("{block:#x}"),"transactionIndex":"0x0","logIndex":format!("{idx:#x}")})
    }

    async fn mount_method(server: &MockServer, m: &str, resp: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": m})))
            .respond_with(resp)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn preflight_accepts_matching_chain_and_marks_genesis_verified() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_chainId", ok(json!("0x1237"))).await;
        mount_method(
            &s,
            "eth_getBlockByNumber",
            ok(json!({"hash": format!("{:#x}", ROBINHOOD.genesis_hash), "timestamp":"0x0"})),
        )
        .await;
        let key = client(&s, ROBINHOOD, None).preflight().await.unwrap();
        assert_eq!(key, ROBINHOOD.verified_chain_key());
    }

    #[tokio::test]
    async fn preflight_chain_id_mismatch_is_typed() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_chainId", ok(json!("0x2105"))).await;
        let e = client(&s, ROBINHOOD, None).preflight().await.unwrap_err();
        assert!(matches!(
            e,
            EvmSourceError::ChainIdMismatch {
                expected: 4663,
                actual: 8453
            }
        ));
    }

    #[tokio::test]
    async fn preflight_genesis_mismatch_is_typed() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_chainId", ok(json!("0x2105"))).await;
        mount_method(
            &s,
            "eth_getBlockByNumber",
            ok(json!({"hash": format!("{:#x}", B256::repeat_byte(7))})),
        )
        .await;
        let e = client(&s, BASE, None).preflight().await.unwrap_err();
        assert!(matches!(
            e,
            EvmSourceError::GenesisMismatch { chain_id: 8453, .. }
        ));
    }

    #[tokio::test]
    async fn get_logs_halves_on_base_range_error_and_orders_output() {
        let s = MockServer::start().await;
        // Whole range 0..=3 rejected; each half accepted.
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"method":"eth_getLogs","params":[{"fromBlock":"0x0","toBlock":"0x3"}]}),
            ))
            .respond_with(err(-32614, "eth_getLogs is limited to a 2,000 range"))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"method":"eth_getLogs","params":[{"fromBlock":"0x0","toBlock":"0x1"}]}),
            ))
            .respond_with(ok(json!([log_json(1, 1), log_json(0, 0)])))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"method":"eth_getLogs","params":[{"fromBlock":"0x2","toBlock":"0x3"}]}),
            ))
            .respond_with(ok(json!([log_json(3, 5)])))
            .mount(&s)
            .await;
        let r = client(&s, BASE, None)
            .get_logs(&LogFilter::default(), 0, 3)
            .await
            .unwrap();
        assert_eq!((r.requests, r.splits), (2, 1));
        let order: Vec<_> = r
            .logs
            .iter()
            .map(|l| (l.block_number, l.log_index))
            .collect();
        assert_eq!(order, vec![(0, 0), (1, 1), (3, 5)]);
    }

    #[tokio::test]
    async fn get_logs_halves_on_robinhood_and_bsc_error_shapes() {
        for msg in [
            "logs matched by query exceeds limit of 10000",
            "limit exceeded",
            "query exceeds max results 20000, retry with the range 0-0",
        ] {
            let s = MockServer::start().await;
            Mock::given(method("POST"))
                .and(body_partial_json(
                    json!({"params":[{"fromBlock":"0x0","toBlock":"0x1"}]}),
                ))
                .respond_with(err(-32000, msg))
                .mount(&s)
                .await;
            mount_method(&s, "eth_getLogs", ok(json!([]))).await;
            let r = client(&s, ROBINHOOD, None)
                .get_logs(&LogFilter::default(), 0, 1)
                .await
                .unwrap();
            assert_eq!(r.splits, 1, "{msg}");
        }
    }

    #[test]
    fn suggested_range_is_parsed_from_the_alchemy_message() {
        let m = "Under the Free tier plan, you can make eth_getLogs requests with up to a 10 block range. Based on your parameters, this block range should work: [0x64, 0x6d]";
        assert_eq!(suggested_range(m), Some((100, 109)));
        assert_eq!(suggested_range("limit exceeded"), None);
        assert_eq!(suggested_range("should work: [0x9, 0x1]"), None);
    }

    /// Alchemy free tier: HTTP 400 + JSON-RPC -32600 with a suggested range.
    /// The scan must follow the suggestion (and then chunk to that span up
    /// front), not halve blindly from the huge window: exactly one failing
    /// request, then 10-block requests only.
    #[tokio::test]
    async fn alchemy_ten_block_cap_over_http_400_is_followed_not_halved() {
        use wiremock::{Request, Respond};
        struct Cap;
        impl Respond for Cap {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let p = &body["params"][0];
                let from = u64::from_str_radix(
                    p["fromBlock"].as_str().unwrap().trim_start_matches("0x"),
                    16,
                )
                .unwrap();
                let to = u64::from_str_radix(
                    p["toBlock"].as_str().unwrap().trim_start_matches("0x"),
                    16,
                )
                .unwrap();
                if to - from + 1 > 10 {
                    let msg = format!(
                        "Under the Free tier plan, you can make eth_getLogs requests with up to a 10 block range. Based on your parameters, this block range should work: [{from:#x}, {:#x}]",
                        from + 9
                    );
                    return ResponseTemplate::new(400).set_body_json(
                        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":msg}}),
                    );
                }
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":[log_json(from, 0)]}))
            }
        }
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Cap)
            .mount(&s)
            .await;
        let r = client(&s, BASE, None)
            .get_logs(&LogFilter::default(), 1_000, 1_099)
            .await
            .unwrap();
        // 100 blocks -> 10 windows of 10 -> 10 logs, one failed probe.
        assert_eq!(r.logs.len(), 10);
        assert_eq!(r.requests, 10);
        assert_eq!(r.splits, 1);
        let total = s.received_requests().await.unwrap().len();
        assert_eq!(total, 11, "one failing request + ten 10-block windows");
    }

    #[tokio::test]
    async fn single_block_range_error_is_typed_unresolvable() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_getLogs", err(-32005, "limit exceeded")).await;
        let e = client(&s, ROBINHOOD, None)
            .get_logs(&LogFilter::default(), 5, 5)
            .await
            .unwrap_err();
        assert!(
            matches!(e, EvmSourceError::RangeUnresolvable { block: 5, .. }),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn unrelated_rpc_error_is_not_halved() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_getLogs", err(-32601, "method not found")).await;
        let e = client(&s, ROBINHOOD, None)
            .get_logs(&LogFilter::default(), 0, 100)
            .await
            .unwrap_err();
        assert!(matches!(e, EvmSourceError::Provider(_)));
    }

    #[tokio::test]
    async fn log_cap_is_enforced() {
        let s = MockServer::start().await;
        mount_method(
            &s,
            "eth_getLogs",
            ok(json!([log_json(1, 1), log_json(1, 2)])),
        )
        .await;
        let rpc = RpcClient::new(RpcEndpoint::new(s.uri()), 5_000, 1).unwrap();
        let c = EvmRpcClient::with_config(
            rpc,
            ROBINHOOD,
            EvmRpcConfig {
                max_logs: 1,
                ..EvmRpcConfig::default()
            },
        );
        let e = c.get_logs(&LogFilter::default(), 0, 1).await.unwrap_err();
        assert!(matches!(e, EvmSourceError::TooManyLogs { cap: 1 }));
    }

    #[tokio::test]
    async fn budget_exhaustion_is_not_treated_as_a_range_error() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_getLogs", err(-32000, "limit of 10000 exceeded")).await;
        // Budget of 2 requests: whole range fails, first half fails, then
        // the budget is spent.
        let e = client(&s, ROBINHOOD, Some(2))
            .get_logs(&LogFilter::default(), 0, 1_000_000)
            .await
            .unwrap_err();
        assert!(e.is_budget_exhausted(), "{e:?}");
    }

    #[tokio::test]
    async fn filter_json_trims_trailing_wildcards_and_supports_or() {
        let t0 = B256::repeat_byte(1);
        let a = B256::repeat_byte(2);
        let b = B256::repeat_byte(3);
        let f = LogFilter {
            addresses: vec![Address::repeat_byte(9)],
            topics: [Some(vec![t0]), None, Some(vec![a, b]), None],
        };
        let j = f.to_json(1, 2);
        assert_eq!(j["topics"].as_array().unwrap().len(), 3);
        assert!(j["topics"][1].is_null());
        assert_eq!(j["topics"][2].as_array().unwrap().len(), 2);
        assert!(j["address"].is_string());
    }

    #[tokio::test]
    async fn receipts_txs_and_null_result() {
        let s = MockServer::start().await;
        let h = B256::repeat_byte(0xaa);
        mount_method(
            &s,
            "eth_getBlockReceipts",
            ok(json!([{
                "transactionHash": format!("{h:#x}"), "blockNumber":"0x5","transactionIndex":"0x1",
                "status":"0x1","gasUsed":"0x5208","effectiveGasPrice":"0x3b9aca00","l1Fee":"0x10",
                "logs":[log_json(5,0)]
            }])),
        )
        .await;
        mount_method(
            &s,
            "eth_getTransactionByHash",
            ok(json!({"hash": format!("{h:#x}"),"from":"0x0000000000000000000000000000000000000002",
                "to":null,"value":"0xde0b6b3a7640000","blockNumber":"0x5","transactionIndex":"0x1"})),
        )
        .await;
        let c = client(&s, BASE, None);
        let r = c.block_receipts(5).await.unwrap();
        assert_eq!(r[0].gas_used, 21_000);
        assert_eq!(r[0].l1_fee, Some(alloy_primitives::U256::from(16u8)));
        assert_eq!(r[0].logs.len(), 1);
        let txs = c.transactions_by_hashes(&[h]).await.unwrap();
        assert_eq!(txs[0].to, None);
        assert_eq!(
            txs[0].value,
            alloy_primitives::U256::from(10u64).pow(alloy_primitives::U256::from(18u8))
        );

        let s2 = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":null})),
            )
            .mount(&s2)
            .await;
        let e = client(&s2, BASE, None)
            .transaction_by_hash(h)
            .await
            .unwrap_err();
        assert!(matches!(e, EvmSourceError::NotFound { .. }), "{e:?}");
    }

    #[tokio::test]
    async fn timestamps_are_cached_and_binary_search_finds_boundary() {
        let s = MockServer::start().await;
        // timestamp(n) = n * 10 via a responder.
        struct Ts;
        impl wiremock::Respond for Ts {
            fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let n = u64::from_str_radix(
                    body["params"][0].as_str().unwrap().trim_start_matches("0x"),
                    16,
                )
                .unwrap();
                ok(json!({"timestamp": format!("{:#x}", n * 10)}))
            }
        }
        Mock::given(method("POST")).respond_with(Ts).mount(&s).await;
        let c = client(&s, BASE, None);
        assert_eq!(c.first_block_at_or_after(95, 1000).await.unwrap(), 10);
        assert_eq!(c.first_block_at_or_after(100, 1000).await.unwrap(), 10);
        assert_eq!(c.first_block_at_or_after(0, 1000).await.unwrap(), 0);
        assert_eq!(c.first_block_at_or_after(99_999, 1000).await.unwrap(), 1001);
        let before = c.total_requests_made();
        c.block_timestamp(10).await.unwrap();
        assert_eq!(c.total_requests_made(), before, "cached");
        let m = c.block_timestamps(&[10, 11]).await.unwrap();
        assert_eq!(m[&11], 110);
    }

    #[tokio::test]
    async fn recorder_keeps_method_params_result_only() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_chainId", ok(json!("0x2105"))).await;
        let rec = Arc::new(CallRecorder::new(10, 10_000));
        let c = client(&s, BASE, None).with_recorder(rec.clone());
        c.chain_id().await.unwrap();
        let (calls, dropped) = rec.snapshot();
        assert_eq!(dropped, 0);
        assert_eq!(calls[0]["method"], "eth_chainId");
        assert_eq!(calls[0]["result"], "0x2105");
        assert!(!calls[0].to_string().contains("127.0.0.1"));
    }

    async fn methods_called(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .filter_map(|b| b["method"].as_str().map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn logs_go_to_the_logs_endpoint_everything_else_to_the_main_one() {
        let main = MockServer::start().await;
        let logs = MockServer::start().await;
        mount_method(&main, "eth_getBalance", ok(json!("0x5"))).await;
        mount_method(&main, "eth_getBlockReceipts", ok(json!([]))).await;
        mount_method(&main, "eth_blockNumber", ok(json!("0x10"))).await;
        mount_method(&logs, "eth_getLogs", ok(json!([log_json(3, 0)]))).await;
        let logs_rpc = RpcClient::new(RpcEndpoint::new(logs.uri()), 5_000, 1).unwrap();
        let c = client(&main, ROBINHOOD, Some(4)).with_logs_endpoint(logs_rpc);
        assert!(c.has_logs_endpoint());
        let r = c.get_logs(&LogFilter::default(), 0, 10).await.unwrap();
        assert_eq!(r.logs.len(), 1);
        assert_eq!(
            c.balance_at(Address::ZERO, 5).await.unwrap(),
            U256::from(5u8)
        );
        c.block_receipts(5).await.unwrap();
        assert_eq!(c.block_number().await.unwrap(), 16);
        assert_eq!(methods_called(&logs).await, vec!["eth_getLogs"]);
        let m = methods_called(&main).await;
        assert!(!m.iter().any(|x| x == "eth_getLogs"), "{m:?}");
        assert_eq!(m.len(), 3);
        // ONE budget across both endpoints: 4 requests made, the 5th refused
        assert_eq!(c.total_requests_made(), 4);
        assert!(c.block_number().await.is_err());
        assert_eq!(
            methods_called(&main).await.len() + methods_called(&logs).await.len(),
            4
        );
    }

    #[tokio::test]
    async fn probe_detects_alchemy_style_range_caps() {
        let s = MockServer::start().await;
        mount_method(&s, "eth_blockNumber", ok(json!("0x1000"))).await;
        mount_method(
            &s,
            "eth_getLogs",
            err(
                -32600,
                "Under the Free tier plan, you can make eth_getLogs requests with up to a 10 block range. Based on your parameters, this block range should work: [0xf00, 0xf09]",
            ),
        )
        .await;
        assert_eq!(
            client(&s, ROBINHOOD, None).probe_logs_span().await.unwrap(),
            Some(10)
        );
        let w = MockServer::start().await;
        mount_method(&w, "eth_blockNumber", ok(json!("0x1000"))).await;
        mount_method(&w, "eth_getLogs", ok(json!([]))).await;
        assert_eq!(
            client(&w, ROBINHOOD, None).probe_logs_span().await.unwrap(),
            None
        );
        assert_eq!(capped_span_from_error("up to a 25 block range"), Some(25));
        assert_eq!(capped_span_from_error("limit exceeded"), None);
    }
}
