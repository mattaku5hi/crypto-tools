//! Alchemy Enhanced API `alchemy_getAssetTransfers` as a WALLET-LISTING
//! indexer (ADR-020 amendment 4): for a wallet `W` and a block range, the
//! `external` (top-level ETH/BNB value), `erc20` and, where the chain supports
//! it, `internal` (call-value) transfers from AND to `W`.
//!
//! Semantics and limits (measured 2026-10-04, free tier):
//! - works on Base, BSC and Robinhood for `external` + `erc20`; `internal`
//!   works on Base only, the other chains answer JSON-RPC -32602 "The
//!   'internal' category is not supported for this network". Support is
//!   feature-detected from the first internal request of a chain and cached
//!   (one atomic per source family);
//! - amounts are read from `rawContract.value` (hex, exact integers) and
//!   NEVER from the float `value` field;
//! - pages are bounded (`maxCount` 0x3e8, [`AlchemyConfig::max_pages`] pages
//!   per stream); a `pageKey` left over after the cap makes the listing
//!   incomplete (never silent);
//! - requests go through the shared [`EvmRpcClient`] (one budget, one rate
//!   limiter, CU weight of the method from the approximate table);
//! - LIMITATION: a transaction that `W` signed but that moved neither ETH nor
//!   tokens to/from `W` (a failed swap, an approve) does not appear (the
//!   external stream asks for zero-value rows too, but a reverted call is not
//!   a transfer). Failed-transaction overhead is therefore possibly
//!   incomplete: reports name `listing_kind = alchemy_transfers` and carry
//!   [`ALCHEMY_COVERAGE_NOTE`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use alloy_primitives::{Address, B256, U256};
use scout_api::ProviderError;
use serde_json::{Value, json};

use crate::evm_cost::ALCHEMY_TRANSFERS_METHOD;
use crate::evm_rpc::{EvmRpcClient, EvmSourceError};
use crate::evm_wire::{address, b256, malformed, quantity_u64, quantity_u256};

/// `listing_kind` label of reports.
pub const ALCHEMY_LISTING_KIND: &str = "alchemy_transfers";

/// Coverage sentence reports carry whenever this indexer listed a wallet.
pub const ALCHEMY_COVERAGE_NOTE: &str = "listing via alchemy_getAssetTransfers: a transaction \
    signed by the wallet that moved neither ETH nor tokens to/from it (a failed swap, an approve) \
    does not appear, so failed-transaction fee overhead may be incomplete; trades are unaffected";

/// Largest page the API serves for these categories.
const MAX_COUNT_HEX: &str = "0x3e8";

/// Tunables (hard bounds, invariant #13).
#[derive(Debug, Clone, Copy)]
pub struct AlchemyConfig {
    /// Max pages per stream (`from`/`to` x category group).
    pub max_pages: u32,
}

impl Default for AlchemyConfig {
    fn default() -> Self {
        Self { max_pages: 20 }
    }
}

/// Which `alchemy_getAssetTransfers` category a row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlchemyCategory {
    External,
    Erc20,
    Internal,
}

/// One transfer row with exact integers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlchemyTransfer {
    pub hash: B256,
    pub block_number: u64,
    /// Unix seconds from `metadata.blockTimestamp` (`withMetadata`).
    pub time_stamp: Option<u64>,
    pub category: AlchemyCategory,
    pub from: Address,
    /// `None` = contract creation.
    pub to: Option<Address>,
    /// Token contract (`rawContract.address`) of an `erc20` row.
    pub contract: Option<Address>,
    /// `rawContract.value`, raw integer (wei / token units).
    pub value: U256,
    /// Provider's `uniqueId` (hash:external, hash:log:N, hash:internal:N).
    pub unique_id: String,
}

/// A wallet's listing over a block range.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlchemyListing {
    /// External + erc20 rows, deduplicated by `uniqueId`, `(block, hash)` order.
    pub transfers: Vec<AlchemyTransfer>,
    /// Internal rows; `None` = the chain does not support the category.
    pub internal: Option<Vec<AlchemyTransfer>>,
    /// `false`: a stream hit [`AlchemyConfig::max_pages`] with a `pageKey` left.
    pub transfers_complete: bool,
    /// `true` only when the category is supported AND its pagination finished.
    pub internal_complete: bool,
    /// Provider requests spent on this listing.
    pub requests: u32,
}

const CAP_UNKNOWN: u8 = 0;
const CAP_SUPPORTED: u8 = 1;
const CAP_UNSUPPORTED: u8 = 2;

/// The listing source for one chain (clone it freely: the internal-category
/// capability cache is shared).
#[derive(Debug, Clone)]
pub struct AlchemyTransfersSource {
    rpc: EvmRpcClient,
    cfg: AlchemyConfig,
    internal_cap: Arc<AtomicU8>,
}

/// Address filter of a stream.
#[derive(Debug, Clone, Copy)]
enum Dir {
    From,
    To,
    /// `contractAddresses: [token]` (every transfer of one token).
    Contract,
}

impl Dir {
    fn filter(self, address: Address) -> (&'static str, Value) {
        let a = format!("{address:#x}");
        match self {
            Self::From => ("fromAddress", Value::String(a)),
            Self::To => ("toAddress", Value::String(a)),
            Self::Contract => ("contractAddresses", json!([a])),
        }
    }
}

/// Every ERC-20 transfer of one token over a block range (token-centric
/// listing without `eth_getLogs`, for range-capped providers).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlchemyTokenListing {
    /// Distinct `(block, tx hash)` of the transfers, ascending.
    pub transactions: Vec<(u64, B256)>,
    /// Transfer rows seen (zero-value ones included).
    pub transfer_rows: usize,
    /// `false`: the page bound was hit with a `pageKey` left.
    pub complete: bool,
    pub requests: u32,
}

/// `true` for the JSON-RPC "category/method not supported" family.
fn is_unsupported_error(e: &EvmSourceError) -> bool {
    let EvmSourceError::Provider(ProviderError::Other(inner)) = e else {
        return false;
    };
    if inner
        .downcast_ref::<scout_rpc::RequestBudgetExhausted>()
        .is_some()
    {
        return false;
    }
    let t = inner.to_string().to_ascii_lowercase();
    t.contains("not supported")
        || t.contains("not available")
        || t.contains("does not exist")
        || t.contains("method not found")
        || t.contains("-32601")
        || t.contains("-32602")
}

/// `true` for an error answered BY the node as a JSON-RPC error body (not a
/// budget, transport or rate-limit failure).
fn is_rpc_error_body(e: &EvmSourceError) -> bool {
    matches!(e, EvmSourceError::Provider(ProviderError::Other(inner))
        if inner.downcast_ref::<scout_rpc::RequestBudgetExhausted>().is_none()
            && inner.downcast_ref::<scout_rpc::ResponseTooLarge>().is_none())
}

impl AlchemyTransfersSource {
    #[must_use]
    pub fn new(rpc: EvmRpcClient, cfg: AlchemyConfig) -> Self {
        Self {
            rpc,
            cfg,
            internal_cap: Arc::new(AtomicU8::new(CAP_UNKNOWN)),
        }
    }

    /// `Some(true/false)` once the internal category was probed.
    #[must_use]
    pub fn internal_supported(&self) -> Option<bool> {
        match self.internal_cap.load(Ordering::Acquire) {
            CAP_SUPPORTED => Some(true),
            CAP_UNSUPPORTED => Some(false),
            _ => None,
        }
    }

    /// Does the endpoint answer `alchemy_getAssetTransfers`? One request
    /// (counted). `Ok(false)` for a JSON-RPC "unknown method / not available"
    /// answer; budget, rate-limit and transport failures are errors (the
    /// answer is unknown, not "no").
    ///
    /// # Errors
    /// Budget exhaustion, rate limiting, transport failure.
    pub async fn probe(&self) -> Result<bool, EvmSourceError> {
        let params = json!([{
            "fromBlock": "0x0", "toBlock": "0x0",
            "fromAddress": format!("{:#x}", Address::ZERO),
            "category": ["external"], "maxCount": "0x1",
        }]);
        match self.rpc.call_raw(ALCHEMY_TRANSFERS_METHOD, params).await {
            Ok(v) => Ok(v.get("transfers").is_some_and(Value::is_array)),
            Err(e) if is_rpc_error_body(&e) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// One bounded paginated stream. Returns `(rows, complete, requests)`.
    #[allow(clippy::too_many_arguments)]
    async fn stream(
        &self,
        wallet: Address,
        dir: Dir,
        categories: &[&str],
        exclude_zero: bool,
        from_block: u64,
        to_block: u64,
        max_pages: u32,
    ) -> Result<(Vec<AlchemyTransfer>, bool, u32), EvmSourceError> {
        let mut rows = Vec::new();
        let mut page_key: Option<String> = None;
        let mut requests = 0u32;
        let (filter_key, filter_value) = dir.filter(wallet);
        for _ in 0..max_pages.max(1) {
            let mut p = json!({
                "fromBlock": format!("{from_block:#x}"),
                "toBlock": format!("{to_block:#x}"),
                filter_key: filter_value.clone(),
                "category": categories,
                "withMetadata": true,
                "excludeZeroValue": exclude_zero,
                "maxCount": MAX_COUNT_HEX,
                "order": "asc",
            });
            if let (Some(k), Some(obj)) = (&page_key, p.as_object_mut()) {
                obj.insert("pageKey".to_string(), Value::String(k.clone()));
            }
            let v = self
                .rpc
                .call_raw(ALCHEMY_TRANSFERS_METHOD, json!([p]))
                .await?;
            requests = requests.saturating_add(1);
            let arr = v
                .get("transfers")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("alchemy_getAssetTransfers", "no `transfers` array"))?;
            for item in arr {
                if let Some(t) = parse_transfer(item)? {
                    rows.push(t);
                }
            }
            match v.get("pageKey").and_then(Value::as_str) {
                Some(k) if !k.is_empty() => page_key = Some(k.to_string()),
                _ => return Ok((rows, true, requests)),
            }
        }
        Ok((rows, false, requests))
    }

    /// List `wallet` over `[from_block, to_block]` in both directions.
    ///
    /// # Errors
    /// Provider failures (budget, transport) and malformed rows.
    pub async fn list(
        &self,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<AlchemyListing, EvmSourceError> {
        let mut requests = 0u32;
        let mut complete = true;
        let mut all: Vec<AlchemyTransfer> = Vec::new();
        // `from`: zero-value external rows are asked for too (a signed
        // contract call with value 0 is still a transaction of the wallet);
        // zero-value ERC-20 rows are dropped below. `to`: provider default.
        for (dir, exclude_zero) in [(Dir::From, false), (Dir::To, true)] {
            let (rows, done, n) = self
                .stream(
                    wallet,
                    dir,
                    &["external", "erc20"],
                    exclude_zero,
                    from_block,
                    to_block,
                    self.cfg.max_pages,
                )
                .await?;
            requests = requests.saturating_add(n);
            complete &= done;
            all.extend(rows);
        }
        all.retain(|t| t.category != AlchemyCategory::Erc20 || !t.value.is_zero());
        let transfers = dedup_sorted(all);

        let mut internal: Option<Vec<AlchemyTransfer>> = None;
        let mut internal_complete = false;
        if self.internal_cap.load(Ordering::Acquire) != CAP_UNSUPPORTED {
            let mut rows_all: Vec<AlchemyTransfer> = Vec::new();
            let mut done_all = true;
            let mut unsupported = false;
            for dir in [Dir::From, Dir::To] {
                match self
                    .stream(
                        wallet,
                        dir,
                        &["internal"],
                        true,
                        from_block,
                        to_block,
                        self.cfg.max_pages,
                    )
                    .await
                {
                    Ok((rows, done, n)) => {
                        self.internal_cap.store(CAP_SUPPORTED, Ordering::Release);
                        requests = requests.saturating_add(n);
                        done_all &= done;
                        rows_all.extend(rows);
                    }
                    Err(e) if is_unsupported_error(&e) => {
                        self.internal_cap.store(CAP_UNSUPPORTED, Ordering::Release);
                        unsupported = true;
                        requests = requests.saturating_add(1);
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if !unsupported {
                internal = Some(dedup_sorted(rows_all));
                internal_complete = done_all;
            }
        }
        Ok(AlchemyListing {
            transfers,
            internal,
            transfers_complete: complete,
            internal_complete,
            requests,
        })
    }
}

impl AlchemyTransfersSource {
    /// Every ERC-20 transfer of `token` over `[from_block, to_block]`
    /// (`contractAddresses` filter, zero-value rows included), at most
    /// `max_pages` pages of 1,000 rows. About 150 CU per page on Alchemy.
    ///
    /// # Errors
    /// Provider failures (budget, transport) and malformed rows.
    pub async fn list_token(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
        max_pages: u32,
    ) -> Result<AlchemyTokenListing, EvmSourceError> {
        let (rows, complete, requests) = self
            .stream(
                token,
                Dir::Contract,
                &["erc20"],
                false,
                from_block,
                to_block,
                max_pages,
            )
            .await?;
        let transfer_rows = rows.len();
        let mut transactions: Vec<(u64, B256)> =
            rows.iter().map(|t| (t.block_number, t.hash)).collect();
        transactions.sort_unstable();
        transactions.dedup();
        Ok(AlchemyTokenListing {
            transactions,
            transfer_rows,
            complete,
            requests,
        })
    }
}

fn dedup_sorted(mut rows: Vec<AlchemyTransfer>) -> Vec<AlchemyTransfer> {
    rows.sort_by(|a, b| {
        (a.block_number, a.hash, &a.unique_id).cmp(&(b.block_number, b.hash, &b.unique_id))
    });
    rows.dedup_by(|a, b| a.unique_id == b.unique_id);
    rows
}

/// Seconds since 1970-01-01 of `YYYY-MM-DDTHH:MM:SS[.fff]Z` (UTC).
// Calendar arithmetic (Hinnant's days-from-civil) is integer division by design.
#[allow(clippy::integer_division)]
fn parse_iso_utc(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let time = time.trim_end_matches('Z');
    let time = time.split(['.', '+']).next()?;
    let mut t = time.split(':');
    let (hh, mm, ss): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3_600 + mm * 60 + ss).ok()
}

/// One API row. `Ok(None)` = a category this source does not use (erc721,
/// erc1155, specialnft: not fungible flows; they never reach the listing).
fn parse_transfer(v: &Value) -> Result<Option<AlchemyTransfer>, EvmSourceError> {
    const W: &str = "alchemy transfer";
    let category = match v.get("category").and_then(Value::as_str) {
        Some("external") => AlchemyCategory::External,
        Some("erc20") => AlchemyCategory::Erc20,
        Some("internal") => AlchemyCategory::Internal,
        Some(_) => return Ok(None),
        None => return Err(malformed(W, "missing `category`")),
    };
    let get = |k: &str| {
        v.get(k)
            .filter(|x| !x.is_null())
            .ok_or_else(|| malformed(W, format!("missing field `{k}`")))
    };
    // the field name goes into the error (live: one BSC wallet failed with an
    // unnamed "expected 0x-prefixed hex string")
    let field = |k: &'static str| move |e: EvmSourceError| malformed(W, format!("`{k}`: {e}"));
    let hash = b256(get("hash")?, W).map_err(field("hash"))?;
    let block_number = quantity_u64(get("blockNum")?, W).map_err(field("blockNum"))?;
    let from = address(get("from")?, W).map_err(field("from"))?;
    // a contract creation has no recipient: `null`, or `""` (live on BSC)
    let to = match v
        .get("to")
        .filter(|x| !x.is_null() && x.as_str() != Some(""))
    {
        Some(x) => Some(address(x, W).map_err(|e| {
            // untrusted text: show a bounded, alphanumeric-only sample
            let sample: String = x
                .to_string()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(48)
                .collect();
            malformed(W, format!("`to` ({sample:?}): {e}"))
        })?),
        None => None,
    };
    let raw = v
        .get("rawContract")
        .filter(|x| x.is_object())
        .ok_or_else(|| malformed(W, "missing `rawContract` (exact amounts are read from it)"))?;
    // Exact integer only: the float `value` field is never consulted.
    let value = quantity_u256(
        raw.get("value")
            .filter(|x| !x.is_null())
            .ok_or_else(|| malformed(W, "missing `rawContract.value`"))?,
        W,
    )
    .map_err(field("rawContract.value"))?;
    let contract = match (category, raw.get("address").filter(|x| !x.is_null())) {
        (AlchemyCategory::Erc20, Some(a)) => {
            Some(address(a, W).map_err(field("rawContract.address"))?)
        }
        (AlchemyCategory::Erc20, None) => {
            return Err(malformed(W, "erc20 row without `rawContract.address`"));
        }
        _ => None,
    };
    let time_stamp = v
        .get("metadata")
        .and_then(|m| m.get("blockTimestamp"))
        .and_then(Value::as_str)
        .and_then(parse_iso_utc);
    let unique_id = v.get("uniqueId").and_then(Value::as_str).map_or_else(
        || {
            // Fallback identity: stable and specific enough for dedup.
            format!(
                "{hash:#x}:{category:?}:{from:#x}:{}:{value}",
                to.map(|t| format!("{t:#x}")).unwrap_or_default()
            )
        },
        str::to_string,
    );
    Ok(Some(AlchemyTransfer {
        hash,
        block_number,
        time_stamp,
        category,
        from,
        to,
        contract,
        value,
        unique_id,
    }))
}

#[cfg(test)]
mod tests {
    use scout_evm::BASE;
    use scout_rpc::{RpcClient, RpcEndpoint};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    #[test]
    fn empty_to_is_a_contract_creation() {
        let row = serde_json::json!({
            "category": "external",
            "hash": format!("0x{}", "ab".repeat(32)),
            "blockNum": "0x10",
            "from": "0x1111111111111111111111111111111111111111",
            "to": "",
            "rawContract": {"value": "0x0", "address": null},
        });
        let t = parse_transfer(&row).unwrap().unwrap();
        assert_eq!(t.to, None);
        let mut bad = row.clone();
        bad["to"] = serde_json::json!("nothex");
        assert!(parse_transfer(&bad).is_err());
    }

    const W: &str = "0x00000000000000000000000000000000000000aa";
    const OTHER: &str = "0x00000000000000000000000000000000000000bb";
    const USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";

    fn hx(n: u8) -> String {
        format!("{:#x}", B256::repeat_byte(n))
    }

    fn row(cat: &str, n: u8, block: u64, from: &str, to: &str, raw: &str, uid: &str) -> Value {
        let mut r = json!({
            "category": cat, "hash": hx(n), "blockNum": format!("{block:#x}"),
            "from": from, "to": to, "uniqueId": uid,
            // The float field is deliberately WRONG: it must never be read.
            "value": 1.5,
            "rawContract": {"value": raw, "address": null, "decimal": "0x12"},
            "metadata": {"blockTimestamp": "2026-10-04T12:00:00.000Z"},
        });
        if cat == "erc20" {
            r["rawContract"]["address"] = json!(USDC);
        }
        r
    }

    fn source(server: &MockServer, max_pages: u32) -> AlchemyTransfersSource {
        let rpc = RpcClient::new(RpcEndpoint::new(server.uri()), 5_000, 1).unwrap();
        AlchemyTransfersSource::new(EvmRpcClient::new(rpc, BASE), AlchemyConfig { max_pages })
    }

    /// Scripted node: `handler(params) -> Result<result, (code, message)>`.
    struct Node<F: Fn(&Value) -> Result<Value, (i64, &'static str)> + Send + Sync>(F);
    impl<F: Fn(&Value) -> Result<Value, (i64, &'static str)> + Send + Sync> Respond for Node<F> {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let b: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(b["method"], ALCHEMY_TRANSFERS_METHOD);
            let body = match (self.0)(&b["params"][0]) {
                Ok(r) => json!({"jsonrpc":"2.0","id":b["id"],"result":r}),
                Err((c, m)) => {
                    json!({"jsonrpc":"2.0","id":b["id"],"error":{"code":c,"message":m}})
                }
            };
            ResponseTemplate::new(200).set_body_json(body)
        }
    }

    async fn serve<F>(f: F) -> MockServer
    where
        F: Fn(&Value) -> Result<Value, (i64, &'static str)> + Send + Sync + 'static,
    {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node(f))
            .mount(&s)
            .await;
        s
    }

    fn cat0(p: &Value) -> String {
        p["category"][0].as_str().unwrap().to_string()
    }

    #[test]
    fn iso_timestamps_parse_to_unix_seconds() {
        assert_eq!(parse_iso_utc("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_utc("2026-10-04T12:00:00.000Z"),
            Some(1_791_115_200)
        );
        assert_eq!(parse_iso_utc("2000-03-01T00:00:01Z"), Some(951_868_801));
        assert_eq!(parse_iso_utc("garbage"), None);
    }

    #[tokio::test]
    async fn both_directions_paginate_dedup_and_parse_exact_hex() {
        // Huge raw value > 2^64 and > f64 precision: must survive exactly.
        let big = "0xde0b6b3a7640001"; // 1e18 + 1
        let huge = "0x1ffffffffffffffffffffffff"; // 2^97 - 1
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let c2 = calls.clone();
        let s = serve(move |p| {
            c2.lock().unwrap().push(p.clone());
            let from_dir = p.get("fromAddress").is_some();
            let key = p.get("pageKey").and_then(Value::as_str);
            Ok(match (cat0(p).as_str(), from_dir, key) {
                ("internal", _, _) => json!({"transfers": []}),
                // from W: two pages
                ("external", true, None) => json!({
                "pageKey": "k1",
                "transfers": [
                    row("external", 1, 10, W, OTHER, big, "0x01:external"),
                    row("erc20", 2, 11, W, OTHER, huge, "0x02:log:1"),
                    // zero-value token row of `from` is dropped
                    row("erc20", 3, 11, W, OTHER, "0x0", "0x03:log:1"),
                ]}),
                ("external", true, Some("k1")) => json!({
                    "transfers": [row("external", 4, 12, W, OTHER, "0x0", "0x04:external")]}),
                // to W: one page; its first row duplicates a from-row (W -> W)
                ("external", false, None) => json!({
                "transfers": [
                    row("erc20", 2, 11, W, OTHER, huge, "0x02:log:1"),
                    row("erc20", 5, 9, OTHER, W, "0x7", "0x05:log:2"),
                ]}),
                _ => panic!("unexpected {p}"),
            })
        })
        .await;
        let src = source(&s, 5);
        let l = src.list(W.parse().unwrap(), 5, 50).await.unwrap();
        // 3 external/erc20 requests (from p1, from p2, to p1) + internal
        // from/to = 5.
        assert_eq!(l.requests, 5);
        assert!(l.transfers_complete && l.internal_complete);
        assert_eq!(l.internal, Some(vec![]));
        let ids: Vec<&str> = l.transfers.iter().map(|t| t.unique_id.as_str()).collect();
        // dedup by uniqueId, sorted by (block, hash), zero erc20 dropped.
        assert_eq!(
            ids,
            vec!["0x05:log:2", "0x01:external", "0x02:log:1", "0x04:external"]
        );
        let by = |id: &str| l.transfers.iter().find(|t| t.unique_id == id).unwrap();
        assert_eq!(
            by("0x01:external").value,
            U256::from(1_000_000_000_000_000_001u128)
        );
        assert_eq!(
            by("0x02:log:1").value,
            (U256::from(1u8) << 97) - U256::from(1u8)
        );
        assert_eq!(by("0x02:log:1").contract, Some(USDC.parse().unwrap()));
        assert_eq!(by("0x01:external").time_stamp, Some(1_791_115_200));
        // request shape: hex block range, maxCount, order, metadata, key.
        let calls = calls.lock().unwrap();
        let first = &calls[0];
        assert_eq!(first["fromBlock"], "0x5");
        assert_eq!(first["toBlock"], "0x32");
        assert_eq!(first["maxCount"], "0x3e8");
        assert_eq!(first["order"], "asc");
        assert_eq!(first["withMetadata"], true);
        assert_eq!(first["fromAddress"], W);
        assert_eq!(first["excludeZeroValue"], false);
        assert_eq!(calls[1]["pageKey"], "k1");
        assert_eq!(calls[2]["toAddress"], W);
        assert_eq!(calls[2]["excludeZeroValue"], true);
        assert_eq!(src.internal_supported(), Some(true));
    }

    #[tokio::test]
    async fn page_cap_makes_the_listing_incomplete() {
        let s = serve(|p| {
            Ok(match cat0(p).as_str() {
                "external" => json!({"pageKey": "more", "transfers":
                    [row("external", 1, 10, W, OTHER, "0x1", &format!("u{}", p["pageKey"]))]}),
                _ => json!({"transfers": []}),
            })
        })
        .await;
        let l = source(&s, 2)
            .list(W.parse().unwrap(), 0, 100)
            .await
            .unwrap();
        assert!(!l.transfers_complete, "pageKey left after the cap");
        // 2 pages per direction + 2 internal
        assert_eq!(l.requests, 6);
        assert!(l.internal_complete);
    }

    #[tokio::test]
    async fn internal_unsupported_is_detected_once_and_cached() {
        let s = serve(|p| match cat0(p).as_str() {
            "internal" => Err((
                -32602,
                "The 'internal' category is not supported for this network",
            )),
            _ => Ok(json!({"transfers": [row("external", 1, 10, W, OTHER, "0x5", "e1")]})),
        })
        .await;
        let src = source(&s, 5);
        let l1 = src.list(W.parse().unwrap(), 0, 100).await.unwrap();
        assert_eq!(l1.internal, None);
        assert!(!l1.internal_complete && l1.transfers_complete);
        assert_eq!(src.internal_supported(), Some(false));
        let n1 = s.received_requests().await.unwrap().len();
        // 2 transfer streams + 1 failed internal probe (the first direction).
        assert_eq!(n1, 3);
        assert_eq!(l1.requests, 3);
        // Cached: the second listing never asks for `internal` again.
        let l2 = src.list(W.parse().unwrap(), 0, 100).await.unwrap();
        assert_eq!(l2.internal, None);
        assert_eq!(s.received_requests().await.unwrap().len(), n1 + 2);
    }

    #[tokio::test]
    async fn internal_page_cap_is_not_complete() {
        let s = serve(|p| {
            Ok(match cat0(p).as_str() {
                "internal" => json!({"pageKey": "x", "transfers":
                    [row("internal", 9, 10, OTHER, W, "0x9", &format!("i{}", p["pageKey"]))]}),
                _ => json!({"transfers": []}),
            })
        })
        .await;
        let l = source(&s, 1).list(W.parse().unwrap(), 0, 9).await.unwrap();
        assert!(l.internal.is_some() && !l.internal_complete);
    }

    #[tokio::test]
    async fn malformed_rows_and_missing_raw_value_are_typed_errors() {
        let s = serve(|_| {
            let mut r = row("external", 1, 1, W, OTHER, "0x1", "u");
            r["rawContract"] = json!({"value": null});
            Ok(json!({"transfers": [r]}))
        })
        .await;
        let e = source(&s, 1)
            .list(W.parse().unwrap(), 0, 9)
            .await
            .unwrap_err();
        assert!(matches!(e, EvmSourceError::Malformed { .. }), "{e}");
        // Non-fungible categories are skipped, not errors.
        assert_eq!(
            parse_transfer(&json!({"category": "erc721"})).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn probe_distinguishes_unknown_method_from_transport_and_budget() {
        let yes = serve(|_| Ok(json!({"transfers": []}))).await;
        assert!(source(&yes, 1).probe().await.unwrap());
        let no = serve(|_| {
            Err((
                -32601,
                "the method alchemy_getAssetTransfers does not exist",
            ))
        })
        .await;
        assert!(!source(&no, 1).probe().await.unwrap());
        // A spent budget is an error (unknown answer), not "unsupported".
        let rpc = RpcClient::new(RpcEndpoint::new(yes.uri()), 5_000, 1)
            .unwrap()
            .with_max_total_requests(Some(0));
        let src =
            AlchemyTransfersSource::new(EvmRpcClient::new(rpc, BASE), AlchemyConfig::default());
        assert!(src.probe().await.unwrap_err().is_budget_exhausted());
    }

    #[tokio::test]
    async fn listing_requests_share_the_budget_and_the_limiter() {
        use scout_rpc::RateLimiter;
        let s = serve(|_| Ok(json!({"transfers": []}))).await;
        let limiter = std::sync::Arc::new(RateLimiter::new(1_500, 150));
        let rpc = RpcClient::new(RpcEndpoint::new(s.uri()), 5_000, 1)
            .unwrap()
            .with_max_total_requests(Some(5))
            .with_rate_limiter(limiter.clone(), Some(crate::approx_method_cu));
        let src = AlchemyTransfersSource::new(
            EvmRpcClient::new(rpc.clone(), BASE),
            AlchemyConfig::default(),
        );
        // A listing needs 4 requests (2 transfer streams + 2 internal).
        let started = std::time::Instant::now();
        src.list(W.parse().unwrap(), 0, 9).await.unwrap();
        let elapsed = started.elapsed();
        assert_eq!(rpc.total_requests_made(), 4);
        // Bucket of 150 units at 1,500 units/s: with the method's CU weight
        // the 4 requests are paced ~70-100 ms apart (> 200 ms for the
        // listing); unit cost would never wait. Wall time, not the limiter's
        // own wait counter: on a loaded machine slow requests refill the
        // bucket and shorten the waits, but never the span.
        let st = limiter.stats();
        assert_eq!(st.acquired, 4);
        assert!(
            elapsed >= std::time::Duration::from_millis(150),
            "CU weight paces requests: {elapsed:?} {st:?}"
        );
        // The second listing needs 4 more but only 1 request is left.
        let e = src.list(W.parse().unwrap(), 0, 9).await.unwrap_err();
        assert!(e.is_budget_exhausted(), "{e}");
        assert_eq!(rpc.total_requests_made(), 5);
    }
}
