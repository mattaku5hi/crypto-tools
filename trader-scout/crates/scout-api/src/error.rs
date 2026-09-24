//! Provider-facing error type. See ADR-008: replaces the previous
//! `ScoutError::ConfigurationRequired { port: &'static str, env_var:
//! &'static str }` constraint, which a third-party provider constructed
//! from runtime config data cannot produce (it requires `&'static str`
//! literals).
//!
//! `scout-engine` maps this into the workspace's own `ScoutError`/
//! exit-code contract (ADR-005) at the boundary where it consumes a
//! provider — `scout-core` itself never needs to know about
//! third-party error shapes.

use std::fmt;

/// Errors a `HistoryProvider` implementation may return. `#[non_exhaustive]`
/// so adding a variant later is not a breaking change for implementers
/// (the open question flagged in `docs/rust/07-library-ergonomics.md`).
#[derive(Debug)]
#[non_exhaustive]
pub enum ProviderError {
    /// No backend is configured for this provider. Per ADR-006, this
    /// must be returned instead of an empty successful result.
    ConfigurationRequired { port: String, detail: String },
    /// The provider is rate-limited. `retry_after`, when known, comes
    /// from the provider's own `Retry-After` signal. Kept as a distinct
    /// variant (not folded into `Transport`) because ACCEPTANCE E09
    /// requires the caller to back off and retry, not treat this the
    /// same as a permanent transport failure.
    RateLimited {
        retry_after: Option<std::time::Duration>,
    },
    /// This provider does not support the requested capability at all
    /// (distinct from `Unknown` in `CapabilityStatus`, which means "not
    /// yet investigated" — `Unsupported` here means "checked, does not
    /// work").
    Unsupported { capability: String },
    /// A network/transport-layer failure.
    Transport(Box<dyn std::error::Error + Send + Sync>),
    /// Any other provider-specific failure that does not fit the above.
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::ConfigurationRequired { port, detail } => {
                write!(f, "configuration required for port `{port}`: {detail}")
            }
            ProviderError::RateLimited { retry_after } => match retry_after {
                Some(duration) => write!(f, "rate limited, retry after {duration:?}"),
                None => write!(f, "rate limited"),
            },
            ProviderError::Unsupported { capability } => {
                write!(f, "capability `{capability}` is not supported")
            }
            ProviderError::Transport(err) => write!(f, "transport error: {err}"),
            ProviderError::Other(err) => write!(f, "provider error: {err}"),
        }
    }
}

impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProviderError::Transport(err) | ProviderError::Other(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_required_accepts_runtime_strings_not_just_static() {
        // The whole point of this type vs the old &'static str
        // constraint: a provider built from config at runtime must be
        // able to construct this variant.
        let port_name = format!("{}-history", "custom");
        let err = ProviderError::ConfigurationRequired {
            port: port_name.clone(),
            detail: "set MY_CUSTOM_API_KEY".to_string(),
        };
        assert!(err.to_string().contains(&port_name));
    }

    #[test]
    fn rate_limited_without_retry_after_still_displays() {
        let err = ProviderError::RateLimited { retry_after: None };
        assert!(err.to_string().contains("rate limited"));
    }

    #[test]
    fn transport_error_preserves_source_chain() {
        let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out");
        let err = ProviderError::Transport(Box::new(inner));
        assert!(std::error::Error::source(&err).is_some());
    }
}
