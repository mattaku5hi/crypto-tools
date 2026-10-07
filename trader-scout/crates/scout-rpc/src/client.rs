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
use crate::ratelimit::RateLimiter;

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
    limiter: Option<MethodLimiter>,
    /// Second endpoint for the same chain (own limiter, shared budget): a
    /// call the primary cannot serve (see [`is_fallbackable`]) is repeated
    /// there once.
    fallback: Option<Arc<RpcClient>>,
}

/// A shared limiter plus the per-method cost table of this endpoint.
#[derive(Clone)]
struct MethodLimiter {
    limiter: Arc<RateLimiter>,
    cost: fn(&str) -> u64,
}

impl std::fmt::Debug for MethodLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MethodLimiter")
            .field("limiter", &self.limiter)
            .finish_non_exhaustive()
    }
}

fn unit_cost(_method: &str) -> u64 {
    1
}

/// Default cap on a single `Retry-After` wait: 60 s. A server asking
/// for longer is not waited out inside one call; `RateLimited` is
/// returned to the caller instead.
pub const DEFAULT_MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Extra attempts a call may spend on 429 answers beyond the generic
/// `max_attempts` (which only counts non-429 transient failures): a rate
/// limit is waited out with exponential backoff (capped at
/// [`RATE_LIMIT_MAX_BACKOFF`]) while the limiter halves its rate, instead of
/// turning into a terminal error after a few attempts. All attempts still
/// count against `--max-requests`.
pub const RATE_LIMIT_EXTRA_RETRIES: u32 = 6;
/// Cap of one backoff step between attempts after a 429.
pub const RATE_LIMIT_MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
struct RequestBudget {
    /// `None` = unlimited.
    limit: Option<u64>,
    /// HTTP attempts started (including retries), shared across calls.
    used: AtomicU64,
    /// Retries scheduled after a 429 (own budget, see
    /// [`RATE_LIMIT_EXTRA_RETRIES`]), shared across calls and clones.
    rate_limit_retries: AtomicU64,
    /// Calls that ended `RateLimited` after spending that budget.
    rate_limit_failures: AtomicU64,
    /// Calls the primary could not serve that were repeated on the
    /// fallback endpoint.
    fallback_calls: AtomicU64,
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
            limiter: None,
            fallback: None,
        })
    }

    /// Puts a client-side rate limiter in front of EVERY HTTP attempt of
    /// this client and its clones (retries included). `cost` maps a method
    /// name to its weight in limiter units (`None` = 1 unit per request).
    /// A 429 without `Retry-After` additionally halves the limiter's rate.
    #[must_use]
    pub fn with_rate_limiter(
        mut self,
        limiter: Arc<RateLimiter>,
        cost: Option<fn(&str) -> u64>,
    ) -> Self {
        self.limiter = Some(MethodLimiter {
            limiter,
            cost: cost.unwrap_or(unit_cost),
        });
        self
    }

    /// Makes this client count against the SAME request budget as `other`
    /// (several endpoints of one run, one exact `--max-requests`).
    #[must_use]
    pub fn sharing_budget_with(mut self, other: &RpcClient) -> Self {
        self.budget = Arc::clone(&other.budget);
        self
    }

    /// Repeat calls the primary cannot serve on `fallback` (another endpoint
    /// of the SAME chain; the caller checks its identity). The fallback keeps
    /// its own limiter and shares this client's request budget — call after
    /// [`Self::with_max_total_requests`], which resets the budget.
    #[must_use]
    pub fn with_fallback(mut self, fallback: RpcClient) -> Self {
        self.fallback = Some(Arc::new(fallback.sharing_budget_with(&self)));
        self
    }

    /// Calls answered by the fallback endpoint so far (shared counter).
    #[must_use]
    pub fn fallback_calls(&self) -> u64 {
        self.budget.fallback_calls.load(Ordering::Acquire)
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
            ..RequestBudget::default()
        });
        self
    }

    /// HTTP attempts started so far (shared budget counter).
    #[must_use]
    pub fn total_requests_made(&self) -> u64 {
        self.budget.used.load(Ordering::Acquire)
    }

    /// `(retries after a 429, calls that failed rate limited after spending
    /// the rate-limit retry budget)` so far (shared counters).
    #[must_use]
    pub fn rate_limit_counts(&self) -> (u64, u64) {
        (
            self.budget.rate_limit_retries.load(Ordering::Acquire),
            self.budget.rate_limit_failures.load(Ordering::Acquire),
        )
    }

    /// The request budget limit (`None` = unlimited).
    #[must_use]
    pub fn request_limit(&self) -> Option<u64> {
        self.budget.limit
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
        match self.call_endpoint(method, &params, jitter, sleeper).await {
            Err(err) if is_fallbackable(&err) => match &self.fallback {
                Some(fb) => {
                    self.budget.fallback_calls.fetch_add(1, Ordering::AcqRel);
                    tracing::warn!(method, "primary endpoint failed; repeating on the fallback");
                    fb.call_endpoint(method, &params, jitter, sleeper).await
                }
                None => Err(err),
            },
            other => other,
        }
    }

    /// One endpoint, with retries (the body of [`Self::call_with`]).
    async fn call_endpoint<P, R>(
        &self,
        method: &str,
        params: &P,
        jitter: &dyn JitterSource,
        sleeper: &dyn Sleeper,
    ) -> Result<R, ProviderError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let body = JsonRpcRequest::new(method, params);
        let mut last_err: Option<ProviderError> = None;
        // Generic transient failures (5xx/network) spend `max_attempts`;
        // 429 answers spend their own `max_attempts + EXTRA` budget.
        let mut generic: u32 = 0;
        let mut limited: u32 = 0;
        let mut attempt: u32 = 0;
        let rl_policy = RetryPolicy {
            max_delay_ms: u64::try_from(RATE_LIMIT_MAX_BACKOFF.as_millis()).unwrap_or(u64::MAX),
            ..self.retry.clone()
        };

        loop {
            if attempt > 0 {
                if self.budget.is_exhausted() {
                    return Err(self.budget_error());
                }
                let was_limited = matches!(last_err, Some(ProviderError::RateLimited { .. }));
                let policy = if was_limited { &rl_policy } else { &self.retry };
                let mut delay = policy.delay_for_attempt(attempt, jitter);
                if let Some(ProviderError::RateLimited {
                    retry_after: Some(ra),
                }) = &last_err
                {
                    delay = delay.max(*ra);
                }
                sleeper.sleep(delay).await;
            }
            attempt = attempt.saturating_add(1);

            match self.try_once::<_, R>(&body, method).await {
                Ok(value) => return Ok(value),
                Err(Classified::Retryable(err)) => {
                    if matches!(err, ProviderError::RateLimited { retry_after: None })
                        && let Some(l) = &self.limiter
                        && let Some(new_milli) = l.limiter.on_rate_limited()
                    {
                        tracing::warn!(
                            new_rate_milli_per_sec = new_milli,
                            "provider answered 429 without Retry-After: client rate halved"
                        );
                    }
                    if let ProviderError::RateLimited {
                        retry_after: Some(ra),
                    } = &err
                        && *ra > self.max_retry_after
                    {
                        return Err(err);
                    }
                    let exhausted = if matches!(err, ProviderError::RateLimited { .. }) {
                        limited = limited.saturating_add(1);
                        limited
                            >= self
                                .retry
                                .max_attempts
                                .saturating_add(RATE_LIMIT_EXTRA_RETRIES)
                    } else {
                        generic = generic.saturating_add(1);
                        generic >= self.retry.max_attempts
                    };
                    if exhausted {
                        if matches!(err, ProviderError::RateLimited { .. }) {
                            self.budget
                                .rate_limit_failures
                                .fetch_add(1, Ordering::AcqRel);
                        }
                        return Err(err);
                    }
                    if matches!(err, ProviderError::RateLimited { .. }) {
                        self.budget
                            .rate_limit_retries
                            .fetch_add(1, Ordering::AcqRel);
                    }
                    last_err = Some(err);
                }
                Err(Classified::Terminal(err)) => return Err(err),
            }
        }
    }

    async fn try_once<P, R>(
        &self,
        body: &JsonRpcRequest<'_, P>,
        method: &str,
    ) -> Result<R, Classified>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        if let Some(l) = &self.limiter {
            // Do not queue for a request the budget would refuse anyway.
            if self.budget.is_exhausted() {
                return Err(Classified::Terminal(self.budget_error()));
            }
            l.limiter
                .acquire((l.cost)(method))
                .await
                .map_err(|e| Classified::Terminal(ProviderError::Other(Box::new(e))))?;
        }
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

        // Providers (Alchemy) answer plan limits with HTTP 400 and a JSON-RPC
        // error body ("up to a 10 block range", "not available on the Free
        // tier"). Keep that message (bounded read; no URL in it) instead of
        // a bare `HTTP 400`, so callers can react to range/plan errors.
        if status.is_client_error() && !matches!(status.as_u16(), 401 | 403 | 429) {
            let fallback = Classified::Terminal(ProviderError::Other(Box::new(
                std::io::Error::other(format!("HTTP {status}")),
            )));
            let Ok(bytes) = read_capped(response, 16 * 1024).await else {
                return Err(fallback);
            };
            return Err(
                match serde_json::from_slice::<JsonRpcResponse<serde_json::Value>>(&bytes)
                    .ok()
                    .and_then(|e| e.into_result().err())
                {
                    Some(JsonRpcEnvelopeError::Rpc(rpc_err)) => Classified::Terminal(
                        ProviderError::Other(Box::new(RpcErrorAdapter(rpc_err))),
                    ),
                    _ => fallback,
                },
            );
        }

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

/// Does a failure (after this endpoint's own retries) justify repeating the
/// call on the fallback endpoint? Endpoint-level failures do: transport and
/// 5xx, rate limiting past the retry budget, rejected credentials, quota /
/// capacity answers. Request-level answers do not (JSON-RPC errors such as a
/// range cap are the caller's to handle), nor does the run's own budget.
#[must_use]
pub fn is_fallbackable(err: &ProviderError) -> bool {
    match err {
        ProviderError::Transport(_)
        | ProviderError::RateLimited { .. }
        | ProviderError::ConfigurationRequired { .. } => true,
        ProviderError::Other(inner) => {
            if inner.downcast_ref::<RequestBudgetExhausted>().is_some() {
                return false;
            }
            let t = inner.to_string().to_ascii_lowercase();
            ["capacity", "quota", "credits", "http 402", "exceeded your"]
                .iter()
                .any(|k| t.contains(k))
        }
        _ => false,
    }
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
    async fn endpoint_failures_are_repeated_on_the_fallback_request_errors_are_not() {
        let primary = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&primary)
            .await;
        let fallback = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": "0x10"
            })))
            .mount(&fallback)
            .await;
        let fb = client_for(&fallback, 1).await;
        let client = client_for(&primary, 2)
            .await
            .with_max_total_requests(Some(10))
            .with_fallback(fb);
        let v: String = client
            .call_with(
                "eth_blockNumber",
                json!([]),
                &NoJitter,
                &FakeSleeper::default(),
            )
            .await
            .unwrap();
        assert_eq!(v, "0x10");
        assert_eq!(client.fallback_calls(), 1);
        // 2 primary attempts + 1 fallback attempt, one shared budget
        assert_eq!(client.total_requests_made(), 3);

        // a JSON-RPC error body is the request's problem: no fallback
        let bad = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1,
                "error": {"code": -32600, "message": "up to a 10 block range"}
            })))
            .mount(&bad)
            .await;
        let client = client_for(&bad, 1)
            .await
            .with_fallback(client_for(&fallback, 1).await);
        let e = client
            .call_with::<_, String>("eth_getLogs", json!([]), &NoJitter, &FakeSleeper::default())
            .await
            .unwrap_err();
        assert!(!is_fallbackable(&e), "{e}");
        assert_eq!(client.fallback_calls(), 0);
    }

    #[test]
    fn fallback_classification() {
        assert!(is_fallbackable(&ProviderError::RateLimited {
            retry_after: None
        }));
        assert!(is_fallbackable(&ProviderError::ConfigurationRequired {
            port: "rpc".into(),
            detail: "HTTP 403".into()
        }));
        assert!(is_fallbackable(&ProviderError::Other(Box::new(
            std::io::Error::other("Monthly capacity limit exceeded")
        ))));
        assert!(!is_fallbackable(&ProviderError::Other(Box::new(
            RequestBudgetExhausted { limit: 1 }
        ))));
        assert!(!is_fallbackable(&ProviderError::Other(Box::new(
            std::io::Error::other("execution reverted")
        ))));
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

    fn ok_resp() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": "ok"}))
    }

    #[tokio::test]
    async fn limiter_paces_real_requests_and_weights_apply() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ok_resp())
            .mount(&server)
            .await;
        // 20 units/s, bucket 1: 5 unit-cost calls need >= 4 * 50 ms.
        let limiter = Arc::new(RateLimiter::new(20, 1));
        let client = client_for(&server, 1)
            .await
            .with_rate_limiter(limiter.clone(), None);
        let t0 = std::time::Instant::now();
        for _ in 0..5 {
            let _: String = client.call("m", json!([])).await.unwrap();
        }
        assert!(
            t0.elapsed() >= Duration::from_millis(190),
            "{:?}",
            t0.elapsed()
        );
        assert_eq!(limiter.stats().acquired, 5);
        // a weighted method costs its weight
        fn cost(m: &str) -> u64 {
            if m == "heavy" { 3 } else { 1 }
        }
        let limiter = Arc::new(RateLimiter::new(30, 3));
        let client = client_for(&server, 1)
            .await
            .with_rate_limiter(limiter.clone(), Some(cost));
        let t0 = std::time::Instant::now();
        for _ in 0..3 {
            let _: String = client.call("heavy", json!([])).await.unwrap();
        }
        // 9 units at 30/s with 3 free: 200 ms
        assert!(
            t0.elapsed() >= Duration::from_millis(180),
            "{:?}",
            t0.elapsed()
        );
    }

    #[tokio::test]
    async fn rate_limited_without_retry_after_backs_off_and_halves_the_rate() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ok_resp())
            .mount(&server)
            .await;
        let limiter = Arc::new(RateLimiter::new(1_000, 10));
        let client = client_for(&server, 4)
            .await
            .with_max_total_requests(Some(10))
            .with_rate_limiter(limiter.clone(), None);
        let sleeper = FakeSleeper(std::sync::Mutex::new(Vec::new()));
        let r: String = client
            .call_with("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap();
        assert_eq!(r, "ok");
        // exponential backoff between the three attempts (doubling)
        assert_eq!(
            sleeper.slept(),
            vec![Duration::from_millis(500), Duration::from_millis(1_000)]
        );
        // two 429s inside the debounce window count once
        let st = limiter.stats();
        assert_eq!((st.halvings, st.rate_milli), (1, 500_000));
        // every attempt is counted against the budget
        assert_eq!(client.total_requests_made(), 3);
    }

    #[tokio::test]
    async fn repeated_429s_get_their_own_retry_budget_then_succeed() {
        // 8 x 429 with max_attempts 3: the generic budget alone would fail
        // at the 3rd; the rate-limit budget (3 + 6 = 9 attempts) rides it out.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(8)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ok_resp())
            .mount(&server)
            .await;
        let limiter = Arc::new(RateLimiter::new(1_000, 10));
        let client = client_for(&server, 3)
            .await
            .with_max_total_requests(Some(20))
            .with_rate_limiter(limiter, None);
        let sleeper = FakeSleeper(std::sync::Mutex::new(Vec::new()));
        let r: String = client
            .call_with("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap();
        assert_eq!(r, "ok");
        assert_eq!(client.total_requests_made(), 9);
        assert_eq!(client.rate_limit_counts(), (8, 0));
        let slept = sleeper.slept();
        assert_eq!(slept.len(), 8);
        // exponential, capped at 30 s
        assert_eq!(slept[0], Duration::from_millis(500));
        assert!(slept.iter().all(|d| *d <= RATE_LIMIT_MAX_BACKOFF));
        assert_eq!(slept[7], RATE_LIMIT_MAX_BACKOFF);
    }

    #[tokio::test]
    async fn endless_429s_fail_after_the_rate_limit_budget_and_count_exactly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let client = client_for(&server, 2)
            .await
            .with_max_total_requests(Some(100));
        let sleeper = FakeSleeper(std::sync::Mutex::new(Vec::new()));
        let err = client
            .call_with::<_, String>("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::RateLimited { retry_after: None }
        ));
        // max_attempts (2) + 6 extra = 8 attempts, 7 retries, 1 failed call
        assert_eq!(client.total_requests_made(), 8);
        assert_eq!(client.rate_limit_counts(), (7, 1));
    }

    #[tokio::test]
    async fn rate_limit_retries_never_exceed_the_request_budget() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let client = client_for(&server, 3)
            .await
            .with_max_total_requests(Some(4));
        let sleeper = FakeSleeper(std::sync::Mutex::new(Vec::new()));
        let err = client
            .call_with::<_, String>("m", json!([]), &NoJitter, &sleeper)
            .await
            .unwrap_err();
        let ProviderError::Other(inner) = err else {
            panic!("expected budget error");
        };
        assert!(inner.downcast_ref::<RequestBudgetExhausted>().is_some());
        assert_eq!(client.total_requests_made(), 4);
    }

    #[tokio::test]
    async fn retry_after_429_does_not_halve_and_budget_stays_exact() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .mount(&server)
            .await;
        let limiter = Arc::new(RateLimiter::new(1_000, 10));
        let client = client_for(&server, 10)
            .await
            .with_max_total_requests(Some(3))
            .with_rate_limiter(limiter.clone(), None);
        let sleeper = FakeSleeper(std::sync::Mutex::new(Vec::new()));
        let r: Result<String, _> = client.call_with("m", json!([]), &NoJitter, &sleeper).await;
        assert!(r.is_err());
        assert_eq!(limiter.stats().halvings, 0);
        assert_eq!(client.total_requests_made(), 3);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn clients_sharing_a_budget_count_together() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ok_resp())
            .mount(&server)
            .await;
        let a = client_for(&server, 1)
            .await
            .with_max_total_requests(Some(2));
        let b = client_for(&server, 1).await.sharing_budget_with(&a);
        let _: String = a.call("m", json!([])).await.unwrap();
        let _: String = b.call("m", json!([])).await.unwrap();
        assert!(a.call::<_, String>("m", json!([])).await.is_err());
        assert!(b.call::<_, String>("m", json!([])).await.is_err());
        assert_eq!((a.total_requests_made(), b.total_requests_made()), (2, 2));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
