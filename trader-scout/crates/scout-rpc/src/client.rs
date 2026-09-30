//! Thin HTTP client over `reqwest`, wired to `scout_api::ProviderError`.
//! This is the single place HTTP status codes and JSON-RPC error
//! bodies get mapped to the workspace's provider error contract — see
//! `map_status` for the exact table and its rationale.

use std::time::Duration;

use scout_api::ProviderError;
use serde::{Serialize, de::DeserializeOwned};

use crate::backoff::{JitterSource, RetryPolicy};
use crate::endpoint::RpcEndpoint;
use crate::jsonrpc::{JsonRpcEnvelopeError, JsonRpcRequest, JsonRpcResponse};

/// A JSON-RPC client bound to one endpoint, with retry/backoff. Reads
/// are idempotent by construction (this crate never issues a
/// mutating/send-transaction call), so automatic retry on transient
/// failure is safe here — a future write-capable client must not reuse
/// this retry policy unchanged.
#[derive(Debug)]
pub struct RpcClient {
    http: reqwest::Client,
    endpoint: RpcEndpoint,
    retry: RetryPolicy,
}

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
            .map_err(|err| ProviderError::Other(Box::new(err)))?;
        Ok(Self {
            http,
            endpoint,
            retry: RetryPolicy::from_config(max_attempts, request_timeout_ms),
        })
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
        let body = JsonRpcRequest::new(method, &params);
        let mut last_err: Option<ProviderError> = None;

        for attempt in 0..self.retry.max_attempts {
            if attempt > 0 {
                let delay = self.retry.delay_for_attempt(attempt, jitter);
                tokio::time::sleep(delay).await;
            }

            match self.try_once::<_, R>(&body).await {
                Ok(value) => return Ok(value),
                Err(Classified::Retryable(err)) => {
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
        let envelope: JsonRpcResponse<R> = response
            .json()
            .await
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
    use crate::backoff::NoJitter;

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
}
