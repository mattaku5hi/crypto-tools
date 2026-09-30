//! Redacted URL wrapper — Helius (and similar) put the API key directly
//! in the URL query string (`?api-key=...`). Any `Debug`/`Display` of
//! the raw URL in a log line or error message leaks the key into
//! stdout/CI logs/issue trackers. `RpcEndpoint` wraps a URL and only
//! ever exposes the full value to the HTTP client itself — every other
//! consumer (logging, error text) gets `<redacted>`.

use std::fmt;

/// An RPC endpoint URL that may embed a secret (API key in the query
/// string). `Debug`/`Display` never print the real value — only
/// `as_str()` does, and that method is reserved for the HTTP client
/// building the actual request.
#[derive(Clone)]
pub struct RpcEndpoint(String);

impl RpcEndpoint {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    /// The real URL, for building an HTTP request only. Never pass this
    /// to a logging macro, `format!` for a user-facing message, or a
    /// `ProviderError` variant.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RpcEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RpcEndpoint").field(&"<redacted>").finish()
    }
}

impl fmt::Display for RpcEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_real_url() {
        let endpoint = RpcEndpoint::new("https://mainnet.helius-rpc.com/?api-key=secret123");
        let debug_output = format!("{endpoint:?}");
        assert!(!debug_output.contains("secret123"));
        assert!(debug_output.contains("redacted"));
    }

    #[test]
    fn display_never_prints_the_real_url() {
        let endpoint = RpcEndpoint::new("https://mainnet.helius-rpc.com/?api-key=secret123");
        let display_output = format!("{endpoint}");
        assert!(!display_output.contains("secret123"));
    }

    #[test]
    fn as_str_gives_the_real_url_for_request_building() {
        let endpoint = RpcEndpoint::new("https://mainnet.helius-rpc.com/?api-key=secret123");
        assert_eq!(
            endpoint.as_str(),
            "https://mainnet.helius-rpc.com/?api-key=secret123"
        );
    }
}
