//! Blockscout PRO "Etherscan-compatible" source (ADR-020 §3):
//! `GET {base}/v2/api?chain_id=…&module=account&action=txlist|tokentx|txlistinternal&apikey=…`.
//!
//! - The API key travels in the `apikey` query parameter and is never logged,
//!   never in `Debug`, and is scrubbed from every error text.
//! - Response bodies are size-capped, requests are retried (429/5xx/
//!   transport) with backoff and counted against a shared budget.
//! - `txlistinternal` reports completeness: Blockscout answers `status "2"` /
//!   "not yet processed" while internal transactions are not indexed; that
//!   is surfaced as `complete = false` so callers set
//!   `internal_transfers = None` (never "no internal transfers").
//! - Listings are windows of at most `page_size * max_pages` rows; hitting
//!   that cap yields `complete = false` (never a silent truncation).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use scout_api::ProviderError;
use scout_rpc::{RateLimiter, RequestBudgetExhausted};
use serde_json::Value;

use crate::evm_rpc::EvmSourceError;
use crate::evm_wire::malformed;

const DEFAULT_BASE_URL: &str = "https://api.blockscout.com";
const MAX_TEXT_LEN: usize = 300;

/// Remove control characters, cap the length and scrub `apikey=` values.
pub(crate) fn sanitize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_TEXT_LEN));
    let mut rest = raw;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(pos) = ["apikey=", "api-key=", "api_key="]
            .iter()
            .filter_map(|k| lower.find(k).map(|p| (p, k.len())))
            .min()
        else {
            break;
        };
        let (p, klen) = pos;
        out.push_str(rest.get(..p).unwrap_or(""));
        out.push_str("apikey=<redacted>");
        let tail = rest.get(p + klen..).unwrap_or("");
        let skip = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
            .map_or(tail.len(), |(i, _)| i);
        rest = tail.get(skip..).unwrap_or("");
    }
    out.push_str(rest);
    out.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TEXT_LEN)
        .collect()
}

/// API key that never prints.
#[derive(Clone)]
pub struct BlockscoutApiKey(String);

impl BlockscoutApiKey {
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

impl fmt::Debug for BlockscoutApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BlockscoutApiKey(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub struct BlockscoutEvmConfig {
    /// Default `https://api.blockscout.com`; tests point this at wiremock.
    pub base_url: String,
    pub chain_id: u64,
    pub api_key: BlockscoutApiKey,
    pub timeout_ms: u64,
    /// Attempts per logical request (>= 1).
    pub max_attempts: u32,
    /// First retry delay; doubles per attempt (cap 30 s).
    pub base_delay_ms: u64,
    /// Rows per page (`offset`), clamped to 1..=10_000.
    pub page_size: u32,
    /// Max pages per listing.
    pub max_pages: u32,
    /// Body cap per response.
    pub max_response_bytes: usize,
    /// Shared HTTP-attempt budget (`None` = unlimited).
    pub max_total_requests: Option<u64>,
    /// Client-side limiter in front of every HTTP attempt (shared by clones;
    /// a 429 halves its rate). `None` = unthrottled.
    pub rate_limiter: Option<Arc<RateLimiter>>,
}

impl BlockscoutEvmConfig {
    #[must_use]
    pub fn new(chain_id: u64, api_key: BlockscoutApiKey) -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            chain_id,
            api_key,
            timeout_ms: 30_000,
            max_attempts: 3,
            base_delay_ms: 250,
            page_size: 1_000,
            max_pages: 20,
            max_response_bytes: 16 * 1024 * 1024,
            max_total_requests: None,
            rate_limiter: None,
        }
    }
}

/// One `txlist` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockscoutTx {
    pub hash: B256,
    pub block_number: u64,
    pub time_stamp: u64,
    pub from: Address,
    pub to: Option<Address>,
    pub value: U256,
    pub gas_used: u64,
    pub gas_price: U256,
    /// `isError == "1"` or `txreceipt_status == "0"`.
    pub failed: bool,
}

/// One `tokentx` row (ERC-20 transfer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockscoutTokenTransfer {
    pub hash: B256,
    pub block_number: u64,
    pub time_stamp: u64,
    pub from: Address,
    pub to: Address,
    pub contract: Address,
    pub value: U256,
}

/// One `txlistinternal` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockscoutInternal {
    pub hash: B256,
    pub block_number: u64,
    pub time_stamp: u64,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    /// Failed internal calls revert their value movement.
    pub failed: bool,
}

/// A listing and whether it is known to be complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing<T> {
    pub rows: Vec<T>,
    /// `false`: the source said "not yet processed" or the page cap was hit.
    pub complete: bool,
    pub pages: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closest {
    Before,
    After,
}

#[derive(Debug)]
struct Envelope {
    status: String,
    message: String,
    result: Value,
}

#[derive(Debug, Clone)]
pub struct BlockscoutEvmSource {
    http: reqwest::Client,
    cfg: BlockscoutEvmConfig,
    used: Arc<AtomicU64>,
}

impl BlockscoutEvmSource {
    pub fn new(cfg: BlockscoutEvmConfig) -> Result<Self, EvmSourceError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .map_err(|e| ProviderError::Other(Box::new(e.without_url())))?;
        Ok(Self {
            http,
            cfg,
            used: Arc::new(AtomicU64::new(0)),
        })
    }

    #[must_use]
    pub fn total_requests_made(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    fn reserve(&self) -> Result<(), EvmSourceError> {
        let limit = self.cfg.max_total_requests;
        let ok = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                limit.is_none_or(|l| u < l).then_some(u + 1)
            })
            .is_ok();
        if ok {
            Ok(())
        } else {
            Err(ProviderError::Other(Box::new(RequestBudgetExhausted {
                limit: limit.unwrap_or(0),
            }))
            .into())
        }
    }

    /// A 429 / "rate limit" answer: halve the client-side rate.
    fn note_rate_limited(&self) {
        if let Some(l) = &self.cfg.rate_limiter {
            l.on_rate_limited();
        }
    }

    fn scrub(&self, text: &str) -> String {
        sanitize_text(&text.replace(&self.cfg.api_key.0, "<redacted>"))
    }

    async fn get(&self, params: &[(&str, String)]) -> Result<Envelope, EvmSourceError> {
        let url = format!("{}/v2/api", self.cfg.base_url.trim_end_matches('/'));
        let mut last: Option<EvmSourceError> = None;
        for attempt in 0..self.cfg.max_attempts.max(1) {
            if attempt > 0 {
                let shift = attempt.min(7);
                let ms = self
                    .cfg
                    .base_delay_ms
                    .saturating_mul(1u64 << shift)
                    .min(30_000);
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            if let Some(l) = &self.cfg.rate_limiter {
                // Do not queue for a request the budget would refuse.
                if self
                    .cfg
                    .max_total_requests
                    .is_some_and(|m| self.total_requests_made() >= m)
                {
                    self.reserve()?; // spent: returns the typed error
                }
                l.acquire(1)
                    .await
                    .map_err(|e| ProviderError::Other(Box::new(e)))?;
            }
            self.reserve()?;
            let sent = self
                .http
                .get(&url)
                .query(&[("chain_id", self.cfg.chain_id.to_string())])
                .query(params)
                .query(&[("apikey", self.cfg.api_key.0.clone())])
                .send()
                .await;
            let mut response = match sent {
                Ok(r) => r,
                Err(e) => {
                    last = Some(ProviderError::Transport(Box::new(e.without_url())).into());
                    continue;
                }
            };
            let status = response.status();
            if status.as_u16() == 429 {
                self.note_rate_limited();
                last = Some(ProviderError::RateLimited { retry_after: None }.into());
                continue;
            }
            if status.is_server_error() {
                last = Some(
                    ProviderError::Transport(Box::new(std::io::Error::other(format!(
                        "server error: HTTP {status}"
                    ))))
                    .into(),
                );
                continue;
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::ConfigurationRequired {
                    port: "blockscout".to_string(),
                    detail: format!("HTTP {status}: check API key/plan"),
                }
                .into());
            }
            if !status.is_success() {
                // Includes 402 (plan does not cover this chain).
                return Err(ProviderError::Other(Box::new(std::io::Error::other(format!(
                    "blockscout HTTP {status}"
                ))))
                .into());
            }
            let body = self.read_capped(&mut response).await?;
            let v: Value = serde_json::from_slice(&body)
                .map_err(|e| malformed("blockscout response", self.scrub(&e.to_string())))?;
            let env = Envelope {
                status: v
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                message: v
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                result: v.get("result").cloned().unwrap_or(Value::Null),
            };
            let blob = format!("{} {}", env.message, env.result.as_str().unwrap_or(""));
            if blob.to_ascii_lowercase().contains("rate limit") {
                self.note_rate_limited();
                last = Some(ProviderError::RateLimited { retry_after: None }.into());
                continue;
            }
            return Ok(env);
        }
        Err(last.unwrap_or_else(|| {
            ProviderError::Other(Box::new(std::io::Error::other("no attempt made"))).into()
        }))
    }

    async fn read_capped(
        &self,
        response: &mut reqwest::Response,
    ) -> Result<Vec<u8>, EvmSourceError> {
        let cap = self.cfg.max_response_bytes;
        let too_large = |observed: u64, lower: bool| {
            EvmSourceError::from(ProviderError::Other(Box::new(
                scout_rpc::ResponseTooLarge {
                    cap_bytes: cap,
                    observed_bytes: observed,
                    lower_bound: lower,
                },
            )))
        };
        let cap_u64 = u64::try_from(cap).unwrap_or(u64::MAX);
        if let Some(len) = response.content_length()
            && len > cap_u64
        {
            return Err(too_large(len, false));
        }
        let mut buf = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    let total = u64::try_from(buf.len())
                        .unwrap_or(u64::MAX)
                        .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
                    if total > cap_u64 {
                        return Err(too_large(total, true));
                    }
                    buf.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(buf),
                Err(e) => {
                    return Err(ProviderError::Transport(Box::new(e.without_url())).into());
                }
            }
        }
    }

    /// Paged account listing. `internal` toggles the completeness-flag
    /// handling of `txlistinternal`.
    async fn list<T>(
        &self,
        action: &str,
        extra: &[(&str, String)],
        from_block: u64,
        to_block: u64,
        parse: impl Fn(&Value) -> Result<T, EvmSourceError>,
    ) -> Result<Listing<T>, EvmSourceError> {
        let offset = self.cfg.page_size.clamp(1, 10_000);
        let mut rows = Vec::new();
        let mut pages = 0u32;
        for page in 1..=self.cfg.max_pages.max(1) {
            let mut params: Vec<(&str, String)> = vec![
                ("module", "account".to_string()),
                ("action", action.to_string()),
                ("startblock", from_block.to_string()),
                ("endblock", to_block.to_string()),
                ("page", page.to_string()),
                ("offset", offset.to_string()),
                ("sort", "asc".to_string()),
            ];
            params.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
            let env = self.get(&params).await?;
            let text = format!("{} {}", env.message, env.result.as_str().unwrap_or(""))
                .to_ascii_lowercase();
            if env.status == "2"
                || text.contains("not yet processed")
                || text.contains("not processed")
            {
                return Ok(Listing {
                    rows,
                    complete: false,
                    pages,
                });
            }
            let arr = match (env.status.as_str(), env.result.as_array()) {
                ("1", Some(a)) => a,
                ("0", Some(a)) if a.is_empty() => {
                    return Ok(Listing {
                        rows,
                        complete: true,
                        pages: pages + 1,
                    });
                }
                _ if text.contains("no transactions found")
                    || text.contains("no records found") =>
                {
                    return Ok(Listing {
                        rows,
                        complete: true,
                        pages: pages + 1,
                    });
                }
                _ => {
                    return Err(ProviderError::Other(Box::new(std::io::Error::other(format!(
                        "blockscout {action}: status `{}`: {}",
                        self.scrub(&env.status),
                        self.scrub(&env.message)
                    ))))
                    .into());
                }
            };
            pages += 1;
            for item in arr {
                rows.push(parse(item)?);
            }
            if u32::try_from(arr.len()).unwrap_or(u32::MAX) < offset {
                return Ok(Listing {
                    rows,
                    complete: true,
                    pages,
                });
            }
        }
        // Page cap reached with a full last page: more rows may exist.
        Ok(Listing {
            rows,
            complete: false,
            pages,
        })
    }

    /// Wallet's `txlist` (both directions; filter `from == wallet` for
    /// signer-only semantics).
    pub async fn txlist(
        &self,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Listing<BlockscoutTx>, EvmSourceError> {
        self.list(
            "txlist",
            &[("address", format!("{wallet:#x}"))],
            from_block,
            to_block,
            parse_tx_row,
        )
        .await
    }

    /// Wallet's ERC-20 transfers, optionally for one token contract.
    pub async fn token_transfers(
        &self,
        wallet: Address,
        token: Option<Address>,
        from_block: u64,
        to_block: u64,
    ) -> Result<Listing<BlockscoutTokenTransfer>, EvmSourceError> {
        let mut extra = vec![("address", format!("{wallet:#x}"))];
        if let Some(t) = token {
            extra.push(("contractaddress", format!("{t:#x}")));
        }
        self.list("tokentx", &extra, from_block, to_block, parse_token_row)
            .await
    }

    /// Wallet's internal (call-value) transfers. `complete == false` means
    /// Blockscout has not finished processing: use `internal_transfers =
    /// None`.
    pub async fn internal_transfers(
        &self,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Listing<BlockscoutInternal>, EvmSourceError> {
        self.list(
            "txlistinternal",
            &[("address", format!("{wallet:#x}"))],
            from_block,
            to_block,
            parse_internal_row,
        )
        .await
    }

    /// `module=block&action=getblocknobytime`.
    pub async fn block_no_by_time(
        &self,
        timestamp: u64,
        closest: Closest,
    ) -> Result<u64, EvmSourceError> {
        let env = self
            .get(&[
                ("module", "block".to_string()),
                ("action", "getblocknobytime".to_string()),
                ("timestamp", timestamp.to_string()),
                (
                    "closest",
                    match closest {
                        Closest::Before => "before",
                        Closest::After => "after",
                    }
                    .to_string(),
                ),
            ])
            .await?;
        if env.status != "1" {
            return Err(ProviderError::Other(Box::new(std::io::Error::other(format!(
                "blockscout getblocknobytime: {}",
                self.scrub(&env.message)
            ))))
            .into());
        }
        env.result
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| env.result.as_u64())
            .ok_or_else(|| malformed("getblocknobytime", "result is not a block number"))
    }
}

fn s<'a>(v: &'a Value, key: &str, what: &'static str) -> Result<&'a str, EvmSourceError> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(what, format!("missing string field `{key}`")))
}

fn num(v: &Value, key: &str, what: &'static str) -> Result<u64, EvmSourceError> {
    s(v, key, what)?
        .parse::<u64>()
        .map_err(|_| malformed(what, format!("field `{key}` is not a decimal integer")))
}

fn dec_u256(v: &Value, key: &str, what: &'static str) -> Result<U256, EvmSourceError> {
    U256::from_str_radix(s(v, key, what)?, 10)
        .map_err(|_| malformed(what, format!("field `{key}` is not a decimal integer")))
}

fn addr(text: &str, what: &'static str) -> Result<Address, EvmSourceError> {
    text.parse::<Address>()
        .map_err(|_| malformed(what, "invalid address"))
}

fn hash(v: &Value, what: &'static str) -> Result<B256, EvmSourceError> {
    s(v, "hash", what)?
        .parse::<B256>()
        .map_err(|_| malformed(what, "invalid hash"))
}

fn parse_tx_row(v: &Value) -> Result<BlockscoutTx, EvmSourceError> {
    const W: &str = "txlist row";
    let to = s(v, "to", W)?;
    let is_error = s(v, "isError", W).map(|x| x == "1").unwrap_or(false);
    let receipt_failed = v
        .get("txreceipt_status")
        .and_then(Value::as_str)
        .is_some_and(|x| x == "0");
    Ok(BlockscoutTx {
        hash: hash(v, W)?,
        block_number: num(v, "blockNumber", W)?,
        time_stamp: num(v, "timeStamp", W)?,
        from: addr(s(v, "from", W)?, W)?,
        to: if to.is_empty() {
            None
        } else {
            Some(addr(to, W)?)
        },
        value: dec_u256(v, "value", W)?,
        gas_used: num(v, "gasUsed", W)?,
        gas_price: dec_u256(v, "gasPrice", W)?,
        failed: is_error || receipt_failed,
    })
}

fn parse_token_row(v: &Value) -> Result<BlockscoutTokenTransfer, EvmSourceError> {
    const W: &str = "tokentx row";
    Ok(BlockscoutTokenTransfer {
        hash: hash(v, W)?,
        block_number: num(v, "blockNumber", W)?,
        time_stamp: num(v, "timeStamp", W)?,
        from: addr(s(v, "from", W)?, W)?,
        to: addr(s(v, "to", W)?, W)?,
        contract: addr(s(v, "contractAddress", W)?, W)?,
        value: dec_u256(v, "value", W)?,
    })
}

fn parse_internal_row(v: &Value) -> Result<BlockscoutInternal, EvmSourceError> {
    const W: &str = "txlistinternal row";
    Ok(BlockscoutInternal {
        hash: v
            .get("hash")
            .or_else(|| v.get("transactionHash"))
            .and_then(Value::as_str)
            .and_then(|x| x.parse::<B256>().ok())
            .ok_or_else(|| malformed(W, "missing/invalid hash"))?,
        block_number: num(v, "blockNumber", W)?,
        time_stamp: num(v, "timeStamp", W)?,
        from: addr(s(v, "from", W)?, W)?,
        to: addr(s(v, "to", W)?, W)?,
        value: dec_u256(v, "value", W)?,
        failed: s(v, "isError", W).map(|x| x == "1").unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const KEY: &str = "sekrit-key-123";
    const W: &str = "0x00000000000000000000000000000000000000aa";

    fn source(
        server: &MockServer,
        tweak: impl FnOnce(&mut BlockscoutEvmConfig),
    ) -> BlockscoutEvmSource {
        let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new(KEY));
        cfg.base_url = server.uri();
        cfg.base_delay_ms = 1;
        tweak(&mut cfg);
        BlockscoutEvmSource::new(cfg).unwrap()
    }

    fn hash_hex(n: u8) -> String {
        format!("{:#x}", B256::repeat_byte(n))
    }

    fn tx_row(n: u8) -> Value {
        json!({"hash":hash_hex(n),"blockNumber":"10","timeStamp":"1777567931","from":W,
            "to":"0x00000000000000000000000000000000000000bb","value":"1000","gasUsed":"21000",
            "gasPrice":"7","isError":"0","txreceipt_status":"1"})
    }

    #[test]
    fn debug_hides_key_and_sanitize_scrubs_it() {
        let cfg = BlockscoutEvmConfig::new(1, BlockscoutApiKey::new(KEY));
        assert!(!format!("{cfg:?}").contains(KEY));
        let t = sanitize_text("error at https://x/v2/api?chain_id=1&apikey=abc123&x=1\n");
        assert!(!t.contains("abc123") && t.contains("apikey=<redacted>") && t.contains("x=1"));
    }

    #[tokio::test]
    async fn txlist_sends_params_and_parses_rows() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/api"))
            .and(query_param("chain_id", "4663"))
            .and(query_param("module", "account"))
            .and(query_param("action", "txlist"))
            .and(query_param("address", W))
            .and(query_param("apikey", KEY))
            .and(query_param("startblock", "5"))
            .and(query_param("endblock", "50"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status":"1","message":"OK","result":[tx_row(1)]})),
            )
            .mount(&s)
            .await;
        let l = source(&s, |_| {})
            .txlist(W.parse().unwrap(), 5, 50)
            .await
            .unwrap();
        assert!(l.complete);
        assert_eq!(l.rows.len(), 1);
        assert_eq!(l.rows[0].gas_used, 21_000);
        assert_eq!(l.rows[0].value, U256::from(1000u64));
        assert!(!l.rows[0].failed);
    }

    #[tokio::test]
    async fn pagination_and_page_cap_flag_incomplete() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"status":"1","message":"OK","result":[tx_row(1), tx_row(2)]}),
            ))
            .mount(&s)
            .await;
        // page_size 2, max 3 pages, every page full => capped, incomplete.
        let l = source(&s, |c| {
            c.page_size = 2;
            c.max_pages = 3;
        })
        .txlist(W.parse().unwrap(), 0, 100)
        .await
        .unwrap();
        assert_eq!((l.rows.len(), l.pages, l.complete), (6, 3, false));
    }

    #[tokio::test]
    async fn no_transactions_found_is_complete_empty() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"status":"0","message":"No transactions found","result":[]}),
                ),
            )
            .mount(&s)
            .await;
        let l = source(&s, |_| {})
            .token_transfers(W.parse().unwrap(), Some(Address::repeat_byte(1)), 0, 1)
            .await
            .unwrap();
        assert!(l.complete && l.rows.is_empty());
    }

    #[tokio::test]
    async fn internal_not_yet_processed_is_incomplete() {
        let s = MockServer::start().await;
        for body in [
            json!({"status":"2","message":"internal transactions not yet processed","result":[]}),
            json!({"status":"0","message":"NOTOK","result":"Internal transactions for this address are not yet processed"}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            let l = source(&server, |_| {})
                .internal_transfers(W.parse().unwrap(), 0, 10)
                .await
                .unwrap();
            assert!(!l.complete && l.rows.is_empty());
        }
        drop(s);
    }

    #[tokio::test]
    async fn internal_rows_parse_when_complete() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("action", "txlistinternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"1","message":"OK",
                "result":[{"hash":hash_hex(3),"blockNumber":"10","timeStamp":"5","from":"0x00000000000000000000000000000000000000bb",
                "to":W,"value":"42","isError":"0"}]})))
            .mount(&s)
            .await;
        let l = source(&s, |_| {})
            .internal_transfers(W.parse().unwrap(), 0, 10)
            .await
            .unwrap();
        assert!(l.complete);
        assert_eq!(l.rows[0].value, U256::from(42u8));
    }

    #[tokio::test]
    async fn retries_429_then_succeeds_and_counts_budget() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status":"1","message":"OK","result":"123"})),
            )
            .mount(&s)
            .await;
        let src = source(&s, |_| {});
        assert_eq!(
            src.block_no_by_time(99, Closest::Before).await.unwrap(),
            123
        );
        assert_eq!(src.total_requests_made(), 2);
    }

    #[tokio::test]
    async fn limiter_paces_attempts_and_429_halves_the_rate() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status":"1","message":"OK","result":"7"})),
            )
            .mount(&s)
            .await;
        let limiter = Arc::new(RateLimiter::new(40, 1));
        let lim = limiter.clone();
        let src = source(&s, move |c| {
            c.rate_limiter = Some(lim);
            c.max_total_requests = Some(5);
        });
        let t0 = std::time::Instant::now();
        assert_eq!(src.block_no_by_time(1, Closest::After).await.unwrap(), 7);
        let st = limiter.stats();
        assert_eq!((st.acquired, st.halvings), (2, 1));
        assert_eq!(st.rate_milli, 20_000);
        // second attempt waited for a token at the halved rate (50 ms)
        assert!(t0.elapsed() >= std::time::Duration::from_millis(45));
        assert_eq!(src.total_requests_made(), 2);
    }

    #[tokio::test]
    async fn budget_exhaustion_is_typed_and_makes_no_request() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&s)
            .await;
        let src = source(&s, |c| c.max_total_requests = Some(2));
        let e = src.txlist(W.parse().unwrap(), 0, 1).await.unwrap_err();
        assert!(e.is_budget_exhausted(), "{e:?}");
        assert_eq!(src.total_requests_made(), 2);
    }

    #[tokio::test]
    async fn http_402_and_401_are_terminal_and_do_not_leak_the_key() {
        for (code, key_error) in [(402u16, false), (401, true)] {
            let s = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(code))
                .mount(&s)
                .await;
            let src = source(&s, |_| {});
            let e = src.txlist(W.parse().unwrap(), 0, 1).await.unwrap_err();
            assert_eq!(src.total_requests_made(), 1);
            assert!(!e.to_string().contains(KEY));
            assert_eq!(
                matches!(
                    e,
                    EvmSourceError::Provider(ProviderError::ConfigurationRequired { .. })
                ),
                key_error
            );
        }
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(5_000)))
            .mount(&s)
            .await;
        let e = source(&s, |c| c.max_response_bytes = 1_000)
            .txlist(W.parse().unwrap(), 0, 1)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("too large"), "{e}");
    }

    #[tokio::test]
    async fn error_status_text_is_scrubbed() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"status":"0","message":format!("Invalid API Key {KEY}"),"result":"Invalid API Key"}),
            ))
            .mount(&s)
            .await;
        let e = source(&s, |_| {})
            .txlist(W.parse().unwrap(), 0, 1)
            .await
            .unwrap_err();
        assert!(!e.to_string().contains(KEY), "{e}");
    }
}
