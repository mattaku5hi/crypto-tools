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
        }
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

    fn reply(&self, method: &str, params: &Value) -> ReplayReply {
        if let Some(r) = self.handler.as_ref().and_then(|h| h(method, params)) {
            return r;
        }
        if let Some(v) = self.recorded.get(&key(method, params)) {
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
