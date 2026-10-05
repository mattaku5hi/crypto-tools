//! JSON-RPC 2.0 request/response envelope. The critical property this
//! module exists for: **a JSON-RPC error arrives as HTTP 200** with an
//! `error` field in the body, not as a non-2xx status code. A caller
//! that only checks `response.status().is_success()` will silently
//! treat a JSON-RPC error as a successful empty result — exactly the
//! "fake empty history instead of honest failure" outcome ADR-006
//! forbids. Every response must be parsed as `JsonRpcResponse<T>` and
//! its `error` field checked before `result` is trusted, never the
//! reverse.

use serde::{Deserialize, Serialize};

/// A JSON-RPC 2.0 request. `id` is a fixed literal (`1`) — this client
/// issues one request per HTTP call, never batches, so a constant id
/// is sufficient and avoids a shared counter needing synchronization.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest<'a, P> {
    pub jsonrpc: &'a str,
    pub id: u32,
    pub method: &'a str,
    pub params: P,
}

impl<'a, P> JsonRpcRequest<'a, P> {
    #[must_use]
    pub fn new(method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        }
    }
}

/// A JSON-RPC 2.0 error object, per spec: `code` + `message`, optional
/// `data` with provider-specific detail.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// Longest `error.data` hex string (with `0x`) echoed into the error text.
const MAX_ERROR_DATA_HEX_CHARS: usize = 2 + 2 * 1024;

impl JsonRpcError {
    /// `error.data` when it is a bounded `0x`-hex string, else `None`.
    #[must_use]
    pub fn revert_data_hex(&self) -> Option<&str> {
        let s = self.data.as_ref()?.as_str()?;
        let h = s.strip_prefix("0x")?;
        (s.len() <= MAX_ERROR_DATA_HEX_CHARS && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(s)
    }
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)?;
        // `eth_call` revert data (`error.data` = 0x-hex): kept, bounded and
        // hex-only, so callers can decode the revert reason.
        if let Some(d) = self.revert_data_hex() {
            write!(f, " (data {d})")?;
        }
        Ok(())
    }
}

/// The full JSON-RPC 2.0 response envelope. Per spec, exactly one of
/// `result`/`error` is present — modeled as `Option` on both rather
/// than an enum because some providers (observed with Codex in this
/// workspace's own measurements) return non-conformant bodies with
/// both fields absent-but-200, and a permissive `Option`/`Option`
/// shape degrades to a clear "neither present" error instead of a
/// deserialize failure that would hide the real problem.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse<T> {
    // No #[serde(default)] here: an `Option<T>` field already
    // deserializes to `None` when absent, for any `T` — adding the
    // attribute makes serde's derive conservatively require `T:
    // Default` (it can't see that `Option<T>: Default` holds
    // regardless of `T`), which would force every JSON-RPC result type
    // in this crate to implement `Default` for no real reason.
    pub result: Option<T>,
    pub error: Option<JsonRpcError>,
}

impl<T> JsonRpcResponse<T> {
    /// Consume the envelope, returning `Ok(result)` only if `error` is
    /// absent and `result` is present. This is the single required
    /// call site for every JSON-RPC response in this crate — nothing
    /// downstream should destructure `result`/`error` manually and
    /// risk skipping the error check.
    pub fn into_result(self) -> Result<T, JsonRpcEnvelopeError> {
        match (self.result, self.error) {
            (_, Some(err)) => Err(JsonRpcEnvelopeError::Rpc(err)),
            (Some(result), None) => Ok(result),
            (None, None) => Err(JsonRpcEnvelopeError::EmptyEnvelope),
        }
    }
}

/// What can go wrong extracting a value from a `JsonRpcResponse`,
/// before any mapping to `scout_api::ProviderError` happens (that
/// mapping lives in `client.rs`, which knows about HTTP status codes
/// too — this type only knows about the JSON-RPC envelope itself).
#[derive(Debug)]
pub enum JsonRpcEnvelopeError {
    /// The provider returned a JSON-RPC `error` object.
    Rpc(JsonRpcError),
    /// HTTP 200, but neither `result` nor `error` was present — a
    /// non-conformant response this crate refuses to silently treat as
    /// an empty success.
    EmptyEnvelope,
}

impl std::fmt::Display for JsonRpcEnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonRpcEnvelopeError::Rpc(err) => write!(f, "{err}"),
            JsonRpcEnvelopeError::EmptyEnvelope => {
                write!(f, "JSON-RPC response had neither `result` nor `error`")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_with_fixed_jsonrpc_and_id() {
        let request = JsonRpcRequest::new("getBlock", serde_json::json!([1000000]));
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 1);
        assert_eq!(value["method"], "getBlock");
    }

    #[test]
    fn response_with_result_and_no_error_extracts_cleanly() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"slot":1000000}}"#;
        let response: JsonRpcResponse<serde_json::Value> = serde_json::from_str(body).unwrap();
        let result = response.into_result().unwrap();
        assert_eq!(result["slot"], 1000000);
    }

    #[test]
    fn response_with_error_is_never_silently_treated_as_success() {
        // The whole point of this module: a 200-OK body carrying an
        // `error` field must not become Ok(_) anywhere in this crate.
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"invalid params"}}"#;
        let response: JsonRpcResponse<serde_json::Value> = serde_json::from_str(body).unwrap();
        let result = response.into_result();
        assert!(matches!(result, Err(JsonRpcEnvelopeError::Rpc(_))));
    }

    #[test]
    fn response_with_both_result_and_error_prefers_error() {
        // Non-conformant per spec, but if a provider ever sends both,
        // treating it as success would be the more dangerous failure
        // mode.
        let body =
            r#"{"jsonrpc":"2.0","id":1,"result":"ignored","error":{"code":-1,"message":"x"}}"#;
        let response: JsonRpcResponse<serde_json::Value> = serde_json::from_str(body).unwrap();
        assert!(matches!(
            response.into_result(),
            Err(JsonRpcEnvelopeError::Rpc(_))
        ));
    }

    #[test]
    fn response_with_neither_field_is_an_explicit_error_not_a_default() {
        let body = r#"{"jsonrpc":"2.0","id":1}"#;
        let response: JsonRpcResponse<serde_json::Value> = serde_json::from_str(body).unwrap();
        assert!(matches!(
            response.into_result(),
            Err(JsonRpcEnvelopeError::EmptyEnvelope)
        ));
    }

    #[test]
    fn rpc_error_display_includes_code_and_message() {
        let err = JsonRpcError {
            code: -32602,
            message: "invalid params".to_string(),
            data: None,
        };
        let text = err.to_string();
        assert!(text.contains("-32602"));
        assert!(text.contains("invalid params"));
    }
}
