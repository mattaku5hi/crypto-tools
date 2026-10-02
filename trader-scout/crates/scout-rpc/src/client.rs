//! Thin HTTP client over `reqwest`, wired to `scout_api::ProviderError`.
//! This is the single place HTTP status codes and JSON-RPC error
//! bodies get mapped to the workspace's provider error contract — see
//! `map_status` for the exact table and its rationale.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use scout_api::ProviderError;
use serde::{Serialize, de::DeserializeOwned};

use crate::backoff::{JitterSource, RetryPolicy, Sleeper, TokioSleeper};
use crate::endpoint::RpcEndpoint;
use crate::jsonrpc::{JsonRpcEnvelopeError, JsonRpcRequest, JsonRpcResponse};

/// A JSON-RPC client bound to one endpoint, with retry/backoff. Reads
/// are idempotent by construction (this crate never issues a
/// mutating/send-transaction call), so automatic retry on transient
/// failure is safe here — a future write-capable client must not reuse
/// this retry policy unchanged.
///
/// `Clone` is cheap and clones **share** the request budget (the
/// counter lives behind an `Arc`), so a cloned client cannot be used to
/// bypass `with_max_total_requests`.
#[derive(Debug, Clone)]
pub struct RpcClient {
    http: reqwest::Client,
    endpoint: RpcEndpoint,
    retry: RetryPolicy,
    max_response_bytes: usize,
    max_retry_after: Duration,
    budget: Arc<RequestBudget>,
}

/// Default cap on a single `Retry-After` wait: 60 s. A server asking
/// for longer is not waited out inside one call; `RateLimited` is
/// returned to the caller instead.
pub const DEFAULT_MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct RequestBudget {
    /// `None` = unlimited.
    limit: Option<u64>,
    /// HTTP attempts started (including retries), shared across calls.
    used: AtomicU64,
}

impl RequestBudget {
    fn is_exhausted(&self) -> bool {
        self.limit
            .is_some_and(|limit| self.used.load(Ordering::Acquire) >= limit)
    }

    /// Atomically reserves one attempt; `false` when the budget is spent.
    fn try_reserve(&self) -> bool {
        let Some(limit) = self.limit else {
            self.used.fetch_add(1, Ordering::AcqRel);
            return true;
        };
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < limit).then_some(used + 1)
            })
            .is_ok()
    }
}

/// The client's total HTTP-attempt budget (`with_max_total_requests`)
/// is spent. Terminal; no request was made. Carries no URL. Inside
/// `ProviderError::Other`; downcast to detect it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestBudgetExhausted {
    pub limit: u64,
}

impl std::fmt::Display for RequestBudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "request budget exhausted: limit of {} total HTTP requests reached",
            self.limit
        )
    }
}

impl std::error::Error for RequestBudgetExhausted {}

/// Default cap on a single HTTP response body: 16 MiB.
///
/// Grounded in `docs/p0/measurements/fixtures/pump_mint{1,2}_full.json`:
/// a Helius `getTransactionsForAddress` full-mode page of 5 pump.fun
/// transactions is ~90-110 KB (~20 KB/tx), so a 100-tx page is ~2 MB.
/// 16 MiB leaves ~8x headroom for heavier transactions while still
/// bounding memory against a hostile or broken endpoint (AGENTS.md
/// invariant #13). Override with `RpcClient::with_max_response_bytes`.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// A response body exceeded the configured cap. Terminal: the same
/// request would return the same oversized body. Never contains the
/// endpoint URL (it embeds the API key). Carried inside
/// `ProviderError::Other`; downcast to detect it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseTooLarge {
    pub cap_bytes: usize,
    /// Declared `Content-Length` (exact) or, for streamed bodies, the
    /// number of bytes received when reading was aborted (lower bound).
    pub observed_bytes: u64,
    /// `true` when `observed_bytes` is a lower bound (streamed abort).
    pub lower_bound: bool,
}

impl std::fmt::Display for ResponseTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let at_least = if self.lower_bound { "at least " } else { "" };
        write!(
            f,
            "response body too large: {at_least}{} bytes exceeds cap of {} bytes",
            self.observed_bytes, self.cap_bytes
        )
    }
}

impl std::error::Error for ResponseTooLarge {}

impl RpcClient {
    /// `request_timeout_ms` and `max_attempts` should come from
    /// `config/scout.example.toml`'s `[scan]` section once wired
    /// end-to-end, not be re-invented per call site.
    pub fn new(
        endpoint: RpcEndpoint,
        request_timeout_ms: u64,
        max_attempts: u32,
    ) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(request_timeout_ms))
            .build()
            .map_err(|err| ProviderError::Other(Box::new(err.without_url())))?;
        Ok(Self {
            http,
            endpoint,
            retry: RetryPolicy::from_config(max_attempts, request_timeout_ms),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_retry_after: DEFAULT_MAX_RETRY_AFTER,
            budget: Arc::new(RequestBudget::default()),
        })
    }

    /// Caps a single `Retry-After` wait (default
    /// `DEFAULT_MAX_RETRY_AFTER`). A 429 asking for more is terminal:
    /// `RateLimited { retry_after }` is returned without sleeping.
    #[must_use]
    pub fn with_max_retry_after(mut self, max_retry_after: Duration) -> Self {
        self.max_retry_after = max_retry_after;
        self
    }

    /// Limits the total number of HTTP attempts (retries included)
    /// this client, and all its clones, may make across all calls.
    /// `None` = unlimited. Resets the counter. When spent, calls fail
    /// with `ProviderError::Other(RequestBudgetExhausted)` without
    /// issuing a request.
    #[must_use]
    pub fn with_max_total_requests(mut self, limit: Option<u64>) -> Self {
        self.budget = Arc::new(RequestBudget {
            limit,
            used: AtomicU64::new(0),
        });
        self
    }

    /// HTTP attempts started so far (shared budget counter).
    #[must_use]
    pub fn total_requests_made(&self) -> u64 {
        self.budget.used.load(Ordering::Acquire)
    }

    fn budget_error(&self) -> ProviderError {
        ProviderError::Other(Box::new(RequestBudgetExhausted {
            limit: self.budget.limit.unwrap_or(0),
        }))
    }

    /// Sets the maximum accepted response body size in bytes (default
    /// `DEFAULT_MAX_RESPONSE_BYTES`). Exceeding it yields a terminal
    /// `ResponseTooLarge` error inside `ProviderError::Other`.
    #[must_use]
    pub fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes;
        self
    }

    /// Issue one JSON-RPC call, retrying on transient failure
    /// (429/5xx/network) up to `max_attempts` times. Non-transient
    /// failures (4xx other than 429, JSON-RPC `error` bodies, malformed
    /// responses) return immediately without retrying — retrying a
    /// request that is wrong will not make it right.
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R, ProviderError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.call_with_jitter(method, params, &crate::backoff::CounterJitter::default())
            .await
    }

    /// Same as `call`, but with an injectable jitter source — the
    /// production entry point (`call`) uses a real jitter source;
    /// tests use `NoJitter` or a fixed-seed `CounterJitter` for
    /// deterministic assertions on retry timing.
    pub async fn call_with_jitter<P, R>(
        &self,
        method: &str,
        params: P,
        jitter: &dyn JitterSource,
    ) -> Result<R, ProviderError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.call_with(method, params, jitter, &TokioSleeper).await
    }

    /// Fully injectable variant (jitter and sleeper), for tests.
    ///
    /// On a retryable `RateLimited { retry_after: Some(ra) }` the next
    /// wait is `max(ra, computed backoff)`; if `ra` exceeds
    /// `max_retry_after` the error is returned immediately instead.
    pub async fn call_with<P, R>(
        &self,
        method: &str,
        params: P,
        jitter: &dyn JitterSource,
        sleeper: &dyn Sleeper,
    ) -> Result<R, ProviderError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let body = JsonRpcRequest::new(method, &params);
        let mut last_err: Option<ProviderError> = None;

        for attempt in 0..self.retry.max_attempts {
            if attempt > 0 {
                if self.budget.is_exhausted() {
                    return Err(self.budget_error());
                }
                let mut delay = self.retry.delay_for_attempt(attempt, jitter);
                if let Some(ProviderError::RateLimited {
                    retry_after: Some(ra),
                }) = &last_err
                {
                    delay = delay.max(*ra);
                }
                sleeper.sleep(delay).await;
            }

            match self.try_once::<_, R>(&body).await {
                Ok(value) => return Ok(value),
                Err(Classified::Retryable(err)) => {
                    if let ProviderError::RateLimited {
                        retry_after: Some(ra),
                    } = &err
                        && *ra > self.max_retry_after
                    {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
                Err(Classified::Terminal(err)) => return Err(err),
            }
        }

        Err(
            last_err.unwrap_or(ProviderError::Other(Box::new(std::io::Error::other(
                "retry loop exited with no attempts made",
            )))),
        )
    }

    async fn try_once<P, R>(&self, body: &JsonRpcRequest<'_, P>) -> Result<R, Classified>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        if !self.budget.try_reserve() {
            return Err(Classified::Terminal(self.budget_error()));
        }
        let response = self
            .http
            .post(self.endpoint.as_str())
            .json(body)
            .send()
            .await
            .map_err(|err| Classified::Retryable(transport_error(err)))?;

        let status = response.status();

        if let Some(classified) = map_status(status, &response) {
            return Err(classified);
        }

        // HTTP 2xx: still must parse the JSON-RPC envelope and check
        // `error` before trusting `result` — a 200 with an `error`
        // body is not success (see jsonrpc.rs module docs).
        let bytes = read_capped(response, self.max_response_bytes).await?;
        let envelope: JsonRpcResponse<R> = serde_json::from_slice(&bytes)
            .map_err(|err| Classified::Terminal(ProviderError::Other(Box::new(err))))?;

        envelope.into_result().map_err(|err| match err {
            JsonRpcEnvelopeError::Rpc(rpc_err) => {
                Classified::Terminal(ProviderError::Other(Box::new(RpcErrorAdapter(rpc_err))))
            }
            JsonRpcEnvelopeError::EmptyEnvelope => Classified::Terminal(ProviderError::Other(
                Box::new(std::io::Error::other(err.to_string())),
            )),
        })
    }
}

/// Reads the body with a hard cap: rejects early on an oversized
/// `Content-Length`, otherwise accumulates chunks and aborts as soon as
/// the total exceeds `cap` (buffers at most `cap` + one chunk).
async fn read_capped(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, Classified> {
    let too_large = |observed_bytes: u64, lower_bound: bool| {
        Classified::Terminal(ProviderError::Other(Box::new(ResponseTooLarge {
            cap_bytes: cap,
            observed_bytes,
            lower_bound,
        })))
    };
    let cap_u64 = u64::try_from(cap).unwrap_or(u64::MAX);
    if let Some(len) = response.content_length()
        && len > cap_u64
    {
        return Err(too_large(len, false));
    }
    let mut buf: Vec<u8> = Vec::new();
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
            // Body read failures (reset, timeout mid-body) are
            // transient network faults: retryable, URL stripped.
            Err(err) => return Err(Classified::Retryable(transport_error(err))),
        }
    }
}

/// Internal classification of a failed attempt: retryable (try again)
/// vs terminal (return to the caller immediately).
enum Classified {
    Retryable(ProviderError),
    Terminal(ProviderError),
}

/// Maps an HTTP status to a `Classified` outcome. Returns `None` for
/// any 2xx status (caller proceeds to parse the JSON-RPC envelope).
///
/// Mapping table (decided once, documented here — not re-derived per
/// call site):
/// - `429` -> `RateLimited { retry_after }` (from the `Retry-After`
///   header when present), retryable.
/// - `5xx` -> `Transport`, retryable — the server may recover.
/// - `401`/`403` -> `ConfigurationRequired` (wrong/missing key),
///   terminal — retrying with the same bad key cannot succeed.
/// - other `4xx` -> `Other`, terminal — the request itself is bad;
///   retrying an unchanged bad request will not fix it.
fn map_status(status: reqwest::StatusCode, response: &reqwest::Response) -> Option<Classified> {
    if status.is_success() {
        return None;
    }

    if status.as_u16() == 429 {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(RetryPolicy::parse_retry_after);
        return Some(Classified::Retryable(ProviderError::RateLimited {
            retry_after,
        }));
    }

    if status.is_server_error() {
        return Some(Classified::Retryable(ProviderError::Transport(Box::new(
            std::io::Error::other(format!("server error: HTTP {status}")),
        ))));
    }

    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Some(Classified::Terminal(ProviderError::ConfigurationRequired {
            port: "rpc".to_string(),
            detail: format!("HTTP {status}: check API key/credentials"),
        }));
    }

    Some(Classified::Terminal(ProviderError::Other(Box::new(
        std::io::Error::other(format!("HTTP {status}")),
    ))))
}

fn transport_error(err: reqwest::Error) -> ProviderError {
    // `reqwest::Error` Display embeds the request URL, which carries
    // the `?api-key=` secret. Strip it before it can reach logs/errors.
    let err = err.without_url();
    if err.is_timeout() {
        ProviderError::Transport(Box::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            err.to_string(),
        )))
    } else {
        ProviderError::Transport(Box::new(err))
    }
}

/// Adapts `JsonRpcError` (from `jsonrpc.rs`, no `std::error::Error`
/// dependency there) into a boxable `std::error::Error` for
/// `ProviderError::Other`.
#[derive(Debug)]
struct RpcErrorAdapter(crate::jsonrpc::JsonRpcError);

impl std::fmt::Display for RpcErrorAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RpcErrorAdapter {}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::backoff::{NoJitter, Sleeper};

    async fn client_for(server: &MockServer, max_attempts: u32) -> RpcClient {
        RpcClient::new(RpcEndpoint::new(server.uri()), 5_000, max_attempts)
            .expect("client construction with a valid timeout must not fail")
    }

    #[tokio::test]
    async fn successful_call_returns_the_parsed_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"slot": 12345}
            })))
            .mount(&server)
            .await;

        let client = client_for(&server, 3).await;
        let result: serde_json::Value = client
            .call_with_jitter("getSlot", json!([]), &NoJitter)
            .await
            .unwrap();
        assert_eq!(result["slot"], 12345);
    }

    #[tokio::test]
    async fn http_200_with_jsonrpc_error_is_never_treated_as_success() {
        // The exact failure mode this crate exists to prevent: a
        // provider that returns 200 OK with an `error` field in the
        // body (observed with Codex in this workspace's own live
        // measurements) must not become Ok(_) here.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32602, "message": "invalid params"}
            })))
            .mount(&server)
            .await;

        let client = client_for(&server, 3).await;
        let result: Result<serde_json::Value, _> = client
            .call_with_jitter("getTransaction", json!(["bad-sig"]), &NoJitter)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rate_limit_retries_and_then_succeeds() {
        let server = MockServer::start().await;
        // First call: 429. Second call: success. wiremock serves mocks
        // in mount order with up_to_n_times, so stack two mocks.
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "ok"
            })))
            .mount(&server)
            .await;

        let client = client_for(&server, 3).await;
        let result: String = client
            .call_with_jitter("getHealth", json!([]), &NoJitter)
            .await
            .unwrap();
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn repeated_5xx_exhausts_retries_and_returns_transport_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = client_for(&server, 3).await;
        let result: Result<serde_json::Value, _> = client
            .call_with_jitter("getSlot", json!([]), &NoJitter)
            .await;
        assert!(matches!(result, Err(ProviderError::Transport(_))));
    }

    #[tokio::test]
    async fn unauthorized_is_terminal_configuration_required_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1) // must be called exactly once -- proves no retry happened
            .mount(&server)
            .await;

        let client = client_for(&server, 5).await;
        let result: Result<serde_json::Value, _> = client
            .call_with_jitter("getSlot", json!([]), &NoJitter)
            .await;
        assert!(matches!(
            result,
            Err(ProviderError::ConfigurationRequired { .. })
        ));
    }

    #[tokio::test]
    async fn bad_request_is_terminal_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server, 5).await;
        let result: Result<serde_json::Value, _> = client
            .call_with_jitter("getSlot", json!([]), &NoJitter)
            .await;
        assert!(result.is_err());
    }

    fn ok_body(payload_len: usize) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": 1, "result": "x".repeat(payload_len)
        }))
        .unwrap()
    }

    fn too_large(err: &ProviderError) -> &ResponseTooLarge {
        match err {
            ProviderError::Other(inner) => inner
                .downcast_ref::<ResponseTooLarge>()
                .expect("expected ResponseTooLarge"),
            other => panic!("expected Other(ResponseTooLarge), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn body_under_cap_is_accepted() {
        let server = MockServer::start().await;
        let body = ok_body(100);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
        let client = client_for(&server, 3)
            .await
            .with_max_response_bytes(body.len());
        let r: String = client
            .call_with_jitter("m", json!([]), &NoJitter)
            .await
            .unwrap();
        assert_eq!(r.len(), 100);
    }

    #[tokio::test]
    async fn content_length_over_cap_is_terminal_and_not_retried() {
        let server = MockServer::start().await;
        let body = ok_body(5_000);
        let len = body.len();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server, 5).await.with_max_response_bytes(1_000);
        let err = client
            .call_with_jitter::<_, String>("m", json!([]), &NoJitter)
            .await
            .unwrap_err();
        let e = too_large(&err);
        assert_eq!(e.cap_bytes, 1_000);
        assert_eq!(e.observed_bytes, u64::try_from(len).unwrap());
        assert!(!e.lower_bound);
        let msg = err.to_string();
        assert!(
            msg.contains("1000") && msg.contains(&len.to_string()),
            "{msg}"
        );
        // `expect(1)` is verified on server drop.
    }

    #[tokio::test]
    async fn chunked_body_without_content_length_over_cap_is_typed_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepts2 = accepts.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                accepts2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\
                          content-type: application/json\r\n\r\n",
                    )
                    .await;
                // Endless-ish stream of 1 KiB chunks; client must abort.
                let payload = "a".repeat(1024);
                for _ in 0..10_000 {
                    let chunk = format!("{:x}\r\n{payload}\r\n", payload.len());
                    if sock.write_all(chunk.as_bytes()).await.is_err() {
                        break;
                    }
                }
            }
        });
        let client = RpcClient::new(RpcEndpoint::new(format!("http://{addr}/")), 5_000, 5)
            .unwrap()
            .with_max_response_bytes(4_096);
        let err = client
            .call_with_jitter::<_, String>("m", json!([]), &NoJitter)
            .await
            .unwrap_err();
        let e = too_large(&err);
        assert!(e.lower_bound);
        assert!(e.observed_bytes > 4_096 && e.observed_bytes <= 4_096 + 64 * 1024);
        assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn errors_never_contain_endpoint_url_or_api_key() {
        // Oversized-body error via a wiremock endpoint with a secret query.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(ok_body(5_000)))
            .mount(&server)
            .await;
        let endpoint = RpcEndpoint::new(format!("{}/?api-key=SECRET123", server.uri()));
        let client = RpcClient::new(endpoint, 5_000, 1)
            .unwrap()
            .with_max_response_bytes(100);
        let err = client
            .call_with_jitter::<_, String>("m", json!([]), &NoJitter)
            .await
            .unwrap_err();
        assert!(!format!("{err} {err:?}").contains("SECRET123"));

        // Transport failure (connection refused) must have the URL stripped.
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = dead.local_addr().unwrap();
        drop(dead);
        let endpoint = RpcEndpoint::new(format!("http://{addr}/?api-key=SECRET123"));
        let client = RpcClient::new(endpoint, 1_000, 1).unwrap();
        let err = client
            .call_with_jitter::<_, String>("m", json!([]), &NoJitter)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains("SECRET123"), "{shown}");
        assert!(!shown.contains("api-key"), "{shown}");
    }

    #[derive(Default)]
    struct FakeSleeper(std::sync::Mutex<Vec<Duration>>);

    impl FakeSleeper {
        fn slept(&self) -> Vec<Duration> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Sleeper for FakeSleeper {
        fn sleep(&self, d: Duration) -> crate::backoff::SleepFuture<'_> {
            self.0.lock().unwrap().push(d);
            Box::pin(std::future::ready(()))
        }
    }

    fn ok_json() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": "ok"}))
    }

    #[tokio::test]
    async fn retry_after_is_honoured_when_larger_than_backoff() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "2"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ok_json())
            .mount(&server)
            .await;
        let sleeper = FakeSleeper::default();
        let client = client_for(&server, 3).await;
        let r: String = client
            .call_with("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap();
        assert_eq!(r, "ok");
        let slept = sleeper.slept();
        assert_eq!(slept.len(), 1);
        assert!(slept[0] >= Duration::from_secs(2), "{slept:?}");
    }

    #[tokio::test]
    async fn retry_after_above_cap_returns_rate_limited_immediately() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
            .expect(1)
            .mount(&server)
            .await;
        let sleeper = FakeSleeper::default();
        let client = client_for(&server, 5).await;
        let err = client
            .call_with::<_, String>("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::RateLimited {
                retry_after: Some(d)
            } if d == Duration::from_secs(3600)
        ));
        assert!(sleeper.slept().is_empty());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn backoff_wins_when_retry_after_is_smaller() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ok_json())
            .mount(&server)
            .await;
        let sleeper = FakeSleeper::default();
        let client = client_for(&server, 3).await;
        let _: String = client
            .call_with("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap();
        assert_eq!(sleeper.slept(), vec![Duration::from_millis(500)]);
    }

    #[tokio::test]
    async fn budget_of_three_with_endless_500s_makes_exactly_three_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let sleeper = FakeSleeper::default();
        let client = client_for(&server, 10)
            .await
            .with_max_total_requests(Some(3));
        let err = client
            .call_with::<_, String>("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::Other(e)
            if e.downcast_ref::<RequestBudgetExhausted>() == Some(&RequestBudgetExhausted { limit: 3 })));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
        assert_eq!(client.total_requests_made(), 3);
    }

    #[tokio::test]
    async fn budget_is_shared_across_calls_and_clones() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ok_json())
            .mount(&server)
            .await;
        let client = client_for(&server, 3)
            .await
            .with_max_total_requests(Some(2));
        let clone = client.clone();
        let sleeper = FakeSleeper::default();
        let _: String = client
            .call_with("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap();
        let _: String = clone
            .call_with("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap();
        let err = client
            .call_with::<_, String>("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::Other(e)
            if e.downcast_ref::<RequestBudgetExhausted>().is_some()));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn budget_error_has_no_url_or_api_key() {
        let server = MockServer::start().await;
        let endpoint = RpcEndpoint::new(format!("{}/?api-key=SECRET123", server.uri()));
        let client = RpcClient::new(endpoint, 5_000, 3)
            .unwrap()
            .with_max_total_requests(Some(0));
        let err = client
            .call_with::<_, String>("m", json!([]), &NoJitter, &FakeSleeper::default())
            .await
            .unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(
            !shown.contains("SECRET123") && !shown.contains("api-key"),
            "{shown}"
        );
        assert!(!shown.contains(&server.uri()), "{shown}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
