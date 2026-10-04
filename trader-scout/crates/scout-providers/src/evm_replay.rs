//! Offline replay of recorded EVM JSON-RPC exchanges (`evm-capture`
//! fixtures) as a wiremock responder. Feature `test-support` only: it exists
//! so downstream tests and binaries' end-to-end tests drive the REAL client
//! stack against captured live responses without any network.
//!
//! A request is answered from the fixture when `(method, params)` equals a
//! recorded call. Anything else is answered like the measured public
//! Robinhood endpoint: `debug_traceTransaction` is "method not found" and
//! `eth_getBalance` has "no historical state"; a handler hook lets a test
//! script other behaviour (a trace-capable node, an archive node, `eth_call`
//! of `decimals()`), consulted first.

use std::collections::HashMap;

use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

/// A scripted reply.
#[derive(Debug, Clone)]
pub enum ReplayReply {
    Result(Value),
    Error { code: i64, message: String },
}

/// Hook consulted before the recorded calls: `(method, params) -> reply`.
pub type ReplayHandler = Box<dyn Fn(&str, &Value) -> Option<ReplayReply> + Send + Sync>;

/// Recorded-exchange responder.
pub struct EvmFixtureReplay {
    recorded: HashMap<String, Value>,
    /// Receipts of recorded `eth_getBlockReceipts` results by lowercase tx
    /// hash, so `eth_getTransactionReceipt` is answered from the same data.
    receipts_by_hash: HashMap<String, Value>,
    handler: Option<ReplayHandler>,
    /// `alchemy_getAssetTransfers` answers derived from the fixture.
    alchemy: Option<AlchemyReplay>,
    /// `(first_block, its timestamp, seconds per block)`: any block but
    /// genesis gets this linear time.
    linear_time: Option<(u64, u64, u64)>,
    /// `eth_getLogs` served from the recorded block receipts.
    receipt_logs: Option<ReceiptLogs>,
    /// Receipts of recorded blocks by tx hash and the recorded tx hashes:
    /// unrecorded transactions are synthesized (`value = 0`).
    synthetic_txs: Option<(HashMap<String, Value>, std::collections::HashSet<String>)>,
}

#[derive(Debug, Clone)]
struct ReceiptLogs {
    /// `(block, log)` of every receipt log.
    logs: Vec<(u64, Value)>,
    /// Provider block-range cap (free-tier Alchemy: 10).
    cap: Option<u64>,
}

impl std::fmt::Debug for EvmFixtureReplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmFixtureReplay")
            .field("recorded_calls", &self.recorded.len())
            .field("has_handler", &self.handler.is_some())
            .finish()
    }
}

fn key(method: &str, params: &Value) -> String {
    format!("{method}|{params}")
}

/// `eth_call` key without the block parameter: pool metadata (`factory()`,
/// `token0()`, ...) is immutable, so a fixture recorded at the capture's head
/// block answers the same call at `latest`.
fn call_key(params: &Value) -> Option<String> {
    let call = params.get(0)?;
    Some(format!(
        "eth_call*|{}|{}",
        call.get("to")?.as_str()?.to_ascii_lowercase(),
        call.get("data").or_else(|| call.get("input"))?.as_str()?
    ))
}

impl EvmFixtureReplay {
    /// From a parsed `evm-capture` fixture (`{"calls": [{method, params,
    /// result}, ..]}`). Malformed entries are skipped (a test then fails on
    /// the missing call, loudly).
    #[must_use]
    pub fn from_fixture(fixture: &Value) -> Self {
        let mut recorded = HashMap::new();
        let mut receipts_by_hash = HashMap::new();
        if let Some(calls) = fixture.get("calls").and_then(Value::as_array) {
            for c in calls {
                if let (Some(m), Some(p), Some(r)) = (
                    c.get("method").and_then(Value::as_str),
                    c.get("params"),
                    c.get("result"),
                ) {
                    recorded.insert(key(m, p), r.clone());
                    if m == "eth_call"
                        && let Some(k) = call_key(p)
                    {
                        recorded.entry(k).or_insert_with(|| r.clone());
                    }
                    if m == "eth_getBlockReceipts"
                        && let Some(rs) = r.as_array()
                    {
                        for rc in rs {
                            if let Some(h) = rc.get("transactionHash").and_then(Value::as_str) {
                                receipts_by_hash.insert(h.to_ascii_lowercase(), rc.clone());
                            }
                        }
                    }
                }
            }
        }
        Self {
            recorded,
            receipts_by_hash,
            handler: None,
            alchemy: None,
            linear_time: None,
            receipt_logs: None,
            synthetic_txs: None,
        }
    }

    /// Answer `eth_getBlockByNumber` of every block but genesis with the
    /// exactly linear time `first_ts + secs_per_block * (n - first_block)`
    /// (the fixture only records the blocks it scanned; true for Base,
    /// whose blocks are exactly 2 s apart).
    #[must_use]
    pub fn with_linear_block_times(
        mut self,
        first_block: u64,
        first_ts: u64,
        secs_per_block: u64,
    ) -> Self {
        self.linear_time = Some((first_block, first_ts, secs_per_block));
        self
    }

    /// Serve `eth_getLogs` from the logs of the fixture's recorded
    /// `eth_getBlockReceipts` (other blocks have no logs), filtered by
    /// address and topic0. With `cap`, a range wider than `cap` blocks is
    /// refused like the measured Alchemy free tier (JSON-RPC error with a
    /// suggested range), which the scanner follows.
    #[must_use]
    pub fn with_receipt_logs(mut self, fixture: &Value, cap: Option<u64>) -> Self {
        let mut logs = Vec::new();
        for c in fixture
            .get("calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if c.get("method").and_then(Value::as_str) != Some("eth_getBlockReceipts") {
                continue;
            }
            for r in c
                .get("result")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for l in r
                    .get("logs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(b) = l.get("blockNumber").and_then(hex_u64) {
                        logs.push((b, l.clone()));
                    }
                }
            }
        }
        self.receipt_logs = Some(ReceiptLogs { logs, cap });
        self
    }

    /// Answer `eth_getTransactionByHash` of a transaction the capture did
    /// NOT record (a token-centric scan meets every transaction of the
    /// blocks) from its receipt, with SYNTHETIC `value = 0`: fine for
    /// buyer-intersect sides, never for per-wallet native amounts.
    #[must_use]
    pub fn with_synthetic_unrecorded_txs(mut self, fixture: &Value) -> Self {
        let mut recorded = std::collections::HashSet::new();
        let mut receipts = HashMap::new();
        for c in fixture
            .get("calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match c.get("method").and_then(Value::as_str) {
                Some("eth_getTransactionByHash") => {
                    if let Some(h) = c
                        .get("params")
                        .and_then(|p| p.get(0))
                        .and_then(Value::as_str)
                    {
                        recorded.insert(h.to_ascii_lowercase());
                    }
                }
                Some("eth_getBlockReceipts") => {
                    for r in c
                        .get("result")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(h) = r.get("transactionHash").and_then(Value::as_str) {
                            receipts.insert(h.to_ascii_lowercase(), r.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        self.synthetic_txs = Some((receipts, recorded));
        self
    }

    /// Answer `alchemy_getAssetTransfers` from the fixture's recorded
    /// transactions and block receipts (see [`AlchemyReplay`]).
    #[must_use]
    pub fn with_alchemy_transfers(
        mut self,
        fixture: &Value,
        internal: AlchemyInternalMode,
    ) -> Self {
        self.alchemy = Some(AlchemyReplay::from_fixture(fixture, internal));
        self
    }

    /// Like [`Self::with_alchemy_transfers`] with a page size smaller than
    /// the request's `maxCount` (forces `pageKey` pagination).
    #[must_use]
    pub fn with_alchemy_page_size(mut self, page_size: usize) -> Self {
        if let Some(a) = self.alchemy.as_mut() {
            a.page_size = Some(page_size.max(1));
        }
        self
    }

    /// Drop zero-value `external` rows from the replayed index (an indexer
    /// that does not list value-less contract calls).
    #[must_use]
    pub fn with_alchemy_zero_external_dropped(mut self) -> Self {
        if let Some(a) = self.alchemy.as_mut() {
            a.rows
                .retain(|r| !(r.category == "external" && r.is_zero()));
        }
        self
    }

    /// The Alchemy rows, for tests that compute expectations independently.
    #[must_use]
    pub fn alchemy(&self) -> Option<&AlchemyReplay> {
        self.alchemy.as_ref()
    }

    /// Read and parse a fixture file.
    ///
    /// # Errors
    /// I/O or JSON errors as text.
    pub fn from_path(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        Ok(Self::from_fixture(&v))
    }

    /// Script extra behaviour (consulted before recorded calls).
    #[must_use]
    pub fn with_handler(mut self, handler: ReplayHandler) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Number of recorded calls.
    #[must_use]
    pub fn recorded_calls(&self) -> usize {
        self.recorded.len()
    }

    /// Replies of the derived behaviours (linear block times, receipt logs,
    /// synthetic unrecorded transactions), consulted after the handler.
    fn derived_reply(&self, method: &str, params: &Value) -> Option<ReplayReply> {
        match method {
            "eth_getBlockByNumber" => {
                let (b0, t0, step) = self.linear_time?;
                let n = hex_u64(params.get(0)?)?;
                if n == 0 {
                    return None; // the recorded genesis block
                }
                let ts = i128::from(t0) + i128::from(step) * (i128::from(n) - i128::from(b0));
                Some(ReplayReply::Result(
                    json!({"number": format!("{n:#x}"), "timestamp": format!("{ts:#x}")}),
                ))
            }
            "eth_getTransactionByHash" => {
                let (receipts, recorded) = self.synthetic_txs.as_ref()?;
                let h = params.get(0)?.as_str()?.to_ascii_lowercase();
                if recorded.contains(&h) {
                    return None;
                }
                let r = receipts.get(&h)?;
                Some(ReplayReply::Result(json!({
                    "hash": h, "from": r["from"], "to": r["to"], "value": "0x0",
                    "gasPrice": r["effectiveGasPrice"], "blockNumber": r["blockNumber"],
                    "transactionIndex": r["transactionIndex"],
                })))
            }
            "eth_getLogs" => {
                let rl = self.receipt_logs.as_ref()?;
                let f = params.get(0)?;
                let (lo, hi) = (hex_u64(f.get("fromBlock")?)?, hex_u64(f.get("toBlock")?)?);
                if let Some(cap) = rl.cap
                    && hi.saturating_sub(lo).saturating_add(1) > cap
                {
                    return Some(ReplayReply::Error {
                        code: -32600,
                        message: format!(
                            "Under the Free tier plan, you can make eth_getLogs requests with up \
                             to a {cap} block range. Based on your parameters, this block range \
                             should work: [{lo:#x}, {:#x}]",
                            lo + cap - 1
                        ),
                    });
                }
                let addr = f
                    .get("address")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase);
                let t0 = f.get("topics").and_then(|t| t.get(0));
                let topics: Vec<String> = match t0 {
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_ascii_lowercase))
                        .collect(),
                    Some(Value::String(s)) => vec![s.to_ascii_lowercase()],
                    _ => Vec::new(),
                };
                let hits: Vec<Value> = rl
                    .logs
                    .iter()
                    .filter(|(b, _)| (lo..=hi).contains(b))
                    .filter(|(_, l)| {
                        addr.as_ref().is_none_or(|a| {
                            l.get("address")
                                .and_then(Value::as_str)
                                .is_some_and(|x| x.eq_ignore_ascii_case(a))
                        })
                    })
                    .filter(|(_, l)| {
                        topics.is_empty()
                            || l.get("topics")
                                .and_then(|t| t.get(0))
                                .and_then(Value::as_str)
                                .is_some_and(|x| topics.contains(&x.to_ascii_lowercase()))
                    })
                    .map(|(_, l)| l.clone())
                    .collect();
                Some(ReplayReply::Result(Value::Array(hits)))
            }
            _ => None,
        }
    }

    fn reply(&self, method: &str, params: &Value) -> ReplayReply {
        if let Some(r) = self.handler.as_ref().and_then(|h| h(method, params)) {
            return r;
        }
        if let Some(r) = self.derived_reply(method, params) {
            return r;
        }
        if method == "alchemy_getAssetTransfers"
            && let Some(a) = &self.alchemy
        {
            return a.answer(params);
        }
        let hit = self.recorded.get(&key(method, params)).or_else(|| {
            (method == "eth_call")
                .then(|| call_key(params).and_then(|k| self.recorded.get(&k)))
                .flatten()
        });
        if let Some(v) = hit {
            // A recorded revert (`{"reverted": true}`, see `eth_call`).
            if method == "eth_call" && v.get("reverted").and_then(Value::as_bool) == Some(true) {
                return ReplayReply::Error {
                    code: 3,
                    message: "execution reverted".to_string(),
                };
            }
            return ReplayReply::Result(v.clone());
        }
        if method == "eth_getTransactionReceipt"
            && let Some(h) = params.get(0).and_then(Value::as_str)
            && let Some(r) = self.receipts_by_hash.get(&h.to_ascii_lowercase())
        {
            return ReplayReply::Result(r.clone());
        }
        match method {
            "debug_traceTransaction" => ReplayReply::Error {
                code: -32601,
                message: "the method debug_traceTransaction does not exist/is not available"
                    .to_string(),
            },
            "eth_getBalance" => ReplayReply::Error {
                code: -32000,
                message: "historical state not available".to_string(),
            },
            other => ReplayReply::Error {
                code: -32601,
                message: format!("replay: no recorded call for {other} with these params"),
            },
        }
    }
}

impl Respond for EvmFixtureReplay {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
            return ResponseTemplate::new(400);
        };
        let (Some(method), Some(params)) = (
            body.get("method").and_then(Value::as_str),
            body.get("params"),
        ) else {
            return ResponseTemplate::new(400);
        };
        let id = body.get("id").cloned().unwrap_or(json!(1));
        let envelope = match self.reply(method, params) {
            ReplayReply::Result(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            ReplayReply::Error { code, message } => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        ResponseTemplate::new(200).set_body_json(envelope)
    }
}

/// Blockscout (`/v2/api`, Etherscan-compatible) responder built from the
/// transactions of an `evm-capture` fixture: `txlist` of an address returns
/// the recorded transactions it sent or received; `txlistinternal` answers
/// "not yet processed" exactly like the measured Robinhood deployment
/// (internal transfers are then NOT observed). Anything else is an empty
/// listing.
#[derive(Debug, Clone)]
pub struct ExplorerReplay {
    /// `(hash, block, from, to, value_dec)` of every recorded transaction.
    txs: Vec<ExplorerTx>,
    /// Make `txlist` hit the page cap (truncated listing), for tests.
    internal_complete_empty: bool,
}

#[derive(Debug, Clone)]
struct ExplorerTx {
    hash: String,
    block: u64,
    time: u64,
    from: String,
    to: String,
    value: String,
}

fn hex_u64(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.strip_prefix("0x")?, 16).ok()
}

impl ExplorerReplay {
    /// From a parsed fixture (`eth_getTransactionByHash` results + block
    /// timestamps are read from the recorded calls).
    #[must_use]
    pub fn from_fixture(fixture: &Value) -> Self {
        let mut times: HashMap<u64, u64> = HashMap::new();
        let mut txs = Vec::new();
        if let Some(calls) = fixture.get("calls").and_then(Value::as_array) {
            for c in calls {
                let r = c.get("result");
                match c.get("method").and_then(Value::as_str) {
                    Some("eth_getBlockByNumber") => {
                        if let (Some(b), Some(t)) = (
                            r.and_then(|r| r.get("number")).and_then(hex_u64),
                            r.and_then(|r| r.get("timestamp")).and_then(hex_u64),
                        ) {
                            times.insert(b, t);
                        }
                    }
                    Some("eth_getTransactionByHash") => {
                        let Some(r) = r else { continue };
                        let value = r
                            .get("value")
                            .and_then(Value::as_str)
                            .and_then(|h| h.strip_prefix("0x"))
                            .and_then(|h| u128::from_str_radix(h, 16).ok())
                            .unwrap_or(0);
                        if let (Some(h), Some(f), Some(b)) = (
                            r.get("hash").and_then(Value::as_str),
                            r.get("from").and_then(Value::as_str),
                            r.get("blockNumber").and_then(hex_u64),
                        ) {
                            txs.push(ExplorerTx {
                                hash: h.to_string(),
                                block: b,
                                time: 0,
                                from: f.to_ascii_lowercase(),
                                to: r
                                    .get("to")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_ascii_lowercase(),
                                value: value.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        for t in &mut txs {
            t.time = times.get(&t.block).copied().unwrap_or(0);
        }
        txs.sort_by(|a, b| (a.block, &a.hash).cmp(&(b.block, &b.hash)));
        Self {
            txs,
            internal_complete_empty: false,
        }
    }

    /// Wallets (senders) that have at least one recorded transaction, with
    /// their transaction counts, most active first (ties by address).
    #[must_use]
    pub fn senders(&self) -> Vec<(String, usize)> {
        let mut m: HashMap<&str, usize> = HashMap::new();
        for t in &self.txs {
            *m.entry(t.from.as_str()).or_insert(0) += 1;
        }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Answer `txlistinternal` with a COMPLETE empty listing instead of
    /// "not yet processed" (an explorer that finished indexing).
    #[must_use]
    pub fn with_complete_internal_listing(mut self) -> Self {
        self.internal_complete_empty = true;
        self
    }
}

impl Respond for ExplorerReplay {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let q: HashMap<String, String> = req.url.query_pairs().into_owned().collect();
        let action = q.get("action").map_or("", String::as_str);
        let address = q
            .get("address")
            .map(|a| a.to_ascii_lowercase())
            .unwrap_or_default();
        let page: u64 = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
        let body = match action {
            "txlist" => {
                let rows: Vec<Value> = self
                    .txs
                    .iter()
                    .filter(|t| t.from == address || t.to == address)
                    .map(|t| {
                        json!({"hash": t.hash, "blockNumber": t.block.to_string(),
                            "timeStamp": t.time.to_string(), "from": t.from, "to": t.to,
                            "value": t.value, "gasUsed": "0", "gasPrice": "0",
                            "isError": "0", "txreceipt_status": "1"})
                    })
                    .collect();
                if page > 1 || rows.is_empty() {
                    json!({"status": "0", "message": "No transactions found", "result": []})
                } else {
                    json!({"status": "1", "message": "OK", "result": rows})
                }
            }
            "txlistinternal" if self.internal_complete_empty => {
                json!({"status": "0", "message": "No transactions found", "result": []})
            }
            "txlistinternal" => json!({"status": "2",
                "message": "internal transactions are not yet processed", "result": []}),
            _ => json!({"status": "0", "message": "No records found", "result": []}),
        };
        ResponseTemplate::new(200).set_body_json(body)
    }
}

/// How the replayed Alchemy node treats `category = internal`.
#[derive(Debug, Clone)]
pub enum AlchemyInternalMode {
    /// BSC / Robinhood: JSON-RPC -32602 "not supported for this network".
    Unsupported,
    /// Base: these rows (the fixture holds no traces, so tests script them).
    Supported(Vec<AlchemyReplayRow>),
}

/// One replayed `alchemy_getAssetTransfers` row.
#[derive(Debug, Clone)]
pub struct AlchemyReplayRow {
    pub category: &'static str,
    pub hash: String,
    pub block: u64,
    pub time: Option<u64>,
    pub from: String,
    pub to: Option<String>,
    /// ERC-20 contract (`erc20` rows).
    pub contract: Option<String>,
    /// Hex quantity as served in `rawContract.value`.
    pub raw_value: String,
    pub unique_id: String,
}

impl AlchemyReplayRow {
    fn is_zero(&self) -> bool {
        self.raw_value
            .strip_prefix("0x")
            .is_some_and(|h| h.chars().all(|c| c == '0'))
    }

    fn json(&self) -> Value {
        json!({
            "blockNum": format!("{:#x}", self.block),
            "uniqueId": self.unique_id,
            "hash": self.hash,
            "from": self.from,
            "to": self.to,
            // A wrong float on purpose: readers must use rawContract.value.
            "value": 0.123_456_789,
            "asset": if self.category == "erc20" { "TKN" } else { "ETH" },
            "category": self.category,
            "rawContract": {"value": self.raw_value, "address": self.contract, "decimal": "0x12"},
            "metadata": self.time.map(|t| json!({"blockTimestamp": iso_utc(t)})),
        })
    }
}

/// `alchemy_getAssetTransfers` responder built from an `evm-capture`
/// fixture: ERC-20 rows from the Transfer logs of the recorded block
/// receipts, `external` rows from the recorded transactions (value as
/// recorded, zero-value included). Serves the fixture's blocks only.
#[derive(Debug, Clone)]
pub struct AlchemyReplay {
    rows: Vec<AlchemyReplayRow>,
    internal: AlchemyInternalMode,
    page_size: Option<usize>,
}

const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

fn topic_addr(t: &Value) -> Option<String> {
    let h = t.as_str()?.strip_prefix("0x")?;
    (h.len() == 64).then(|| format!("0x{}", h.get(24..).unwrap_or("")).to_ascii_lowercase())
}

/// `YYYY-MM-DDTHH:MM:SS.000Z`.
// Calendar arithmetic (Hinnant's civil-from-days) is integer division by design.
#[allow(clippy::integer_division)]
fn iso_utc(t: u64) -> String {
    let days = i64::try_from(t / 86_400).unwrap_or(0);
    let secs = t % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3_600,
        secs % 3_600 / 60,
        secs % 60
    )
}

impl AlchemyReplay {
    /// Build the rows of a parsed fixture.
    #[must_use]
    pub fn from_fixture(fixture: &Value, internal: AlchemyInternalMode) -> Self {
        let mut times: HashMap<u64, u64> = HashMap::new();
        let mut rows: Vec<AlchemyReplayRow> = Vec::new();
        let calls = fixture
            .get("calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for c in &calls {
            if c.get("method").and_then(Value::as_str) == Some("eth_getTransactionByHash")
                && let Some(r) = c.get("result")
                && let (Some(b), Some(t)) = (
                    r.get("blockNumber").and_then(hex_u64),
                    r.get("blockTimestamp").and_then(hex_u64),
                )
            {
                times.insert(b, t);
            }
            if c.get("method").and_then(Value::as_str) == Some("eth_getBlockByNumber")
                && let Some(r) = c.get("result")
                && let (Some(b), Some(t)) = (
                    r.get("number").and_then(hex_u64),
                    r.get("timestamp").and_then(hex_u64),
                )
            {
                times.insert(b, t);
            }
        }
        for c in &calls {
            let (Some(m), Some(r)) = (c.get("method").and_then(Value::as_str), c.get("result"))
            else {
                continue;
            };
            match m {
                "eth_getTransactionByHash" => {
                    if let (Some(h), Some(f), Some(b), Some(v)) = (
                        r.get("hash").and_then(Value::as_str),
                        r.get("from").and_then(Value::as_str),
                        r.get("blockNumber").and_then(hex_u64),
                        r.get("value").and_then(Value::as_str),
                    ) {
                        rows.push(AlchemyReplayRow {
                            category: "external",
                            hash: h.to_ascii_lowercase(),
                            block: b,
                            time: times.get(&b).copied(),
                            from: f.to_ascii_lowercase(),
                            to: r
                                .get("to")
                                .and_then(Value::as_str)
                                .map(str::to_ascii_lowercase),
                            contract: None,
                            raw_value: v.to_string(),
                            unique_id: format!("{}:external", h.to_ascii_lowercase()),
                        });
                    }
                }
                "eth_getBlockReceipts" => {
                    for rc in r.as_array().into_iter().flatten() {
                        let (Some(h), Some(b)) = (
                            rc.get("transactionHash").and_then(Value::as_str),
                            rc.get("blockNumber").and_then(hex_u64),
                        ) else {
                            continue;
                        };
                        for l in rc
                            .get("logs")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            let topics = l.get("topics").and_then(Value::as_array);
                            let (Some(t), Some(data)) =
                                (topics, l.get("data").and_then(Value::as_str))
                            else {
                                continue;
                            };
                            // An ERC-20 Transfer has 3 topics (ERC-721 has 4).
                            let [t0, t1, t2] = t.as_slice() else {
                                continue;
                            };
                            if t0.as_str() != Some(TRANSFER_TOPIC) || data.len() != 66 {
                                continue;
                            }
                            let (Some(from), Some(to)) = (topic_addr(t1), topic_addr(t2)) else {
                                continue;
                            };
                            rows.push(AlchemyReplayRow {
                                category: "erc20",
                                hash: h.to_ascii_lowercase(),
                                block: b,
                                time: times.get(&b).copied(),
                                from,
                                to: Some(to),
                                contract: l
                                    .get("address")
                                    .and_then(Value::as_str)
                                    .map(str::to_ascii_lowercase),
                                raw_value: data.to_string(),
                                unique_id: format!(
                                    "{}:log:{}",
                                    h.to_ascii_lowercase(),
                                    l.get("logIndex").and_then(Value::as_str).unwrap_or("0x0")
                                ),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        rows.sort_by(|a, b| (a.block, &a.unique_id).cmp(&(b.block, &b.unique_id)));
        rows.dedup_by(|a, b| a.unique_id == b.unique_id);
        Self {
            rows,
            internal,
            page_size: None,
        }
    }

    /// Every row (all categories) of the replayed chain slice.
    #[must_use]
    pub fn rows(&self) -> &[AlchemyReplayRow] {
        &self.rows
    }

    fn answer(&self, params: &Value) -> ReplayReply {
        let Some(p) = params.get(0) else {
            return ReplayReply::Error {
                code: -32602,
                message: "missing params".to_string(),
            };
        };
        let cats: Vec<&str> = p
            .get("category")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let internal_rows: &[AlchemyReplayRow] = match &self.internal {
            AlchemyInternalMode::Supported(r) => r,
            AlchemyInternalMode::Unsupported => {
                if cats.contains(&"internal") {
                    return ReplayReply::Error {
                        code: -32602,
                        message: "The 'internal' category is not supported for this network"
                            .to_string(),
                    };
                }
                &[]
            }
        };
        let lo = p.get("fromBlock").and_then(hex_u64).unwrap_or(0);
        let hi = p.get("toBlock").and_then(hex_u64).unwrap_or(u64::MAX);
        let from = p
            .get("fromAddress")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
        let to = p
            .get("toAddress")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
        let exclude_zero = p
            .get("excludeZeroValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let mut hits: Vec<&AlchemyReplayRow> = self
            .rows
            .iter()
            .chain(internal_rows.iter())
            .filter(|r| cats.contains(&r.category))
            .filter(|r| r.block >= lo && r.block <= hi)
            .filter(|r| from.as_ref().is_none_or(|a| &r.from == a))
            .filter(|r| to.as_ref().is_none_or(|a| r.to.as_ref() == Some(a)))
            .filter(|r| !(exclude_zero && r.is_zero()))
            .collect();
        hits.sort_by(|a, b| (a.block, &a.unique_id).cmp(&(b.block, &b.unique_id)));
        let max_count = p
            .get("maxCount")
            .and_then(hex_u64)
            .and_then(|m| usize::try_from(m).ok())
            .unwrap_or(1_000);
        let page = self
            .page_size
            .map_or(max_count, |s| s.min(max_count))
            .max(1);
        let offset: usize = p
            .get("pageKey")
            .and_then(Value::as_str)
            .and_then(|k| k.parse().ok())
            .unwrap_or(0);
        let slice: Vec<Value> = hits
            .iter()
            .skip(offset)
            .take(page)
            .map(|r| r.json())
            .collect();
        let next = offset + slice.len();
        let mut out = serde_json::Map::new();
        out.insert("transfers".to_string(), Value::Array(slice));
        if next < hits.len() {
            out.insert("pageKey".to_string(), json!(next.to_string()));
        }
        ReplayReply::Result(Value::Object(out))
    }
}
