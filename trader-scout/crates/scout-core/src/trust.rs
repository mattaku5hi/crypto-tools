//! Data trust level — a domain concept, not a provider-port detail.
//! Lives in `scout-core` (not `scout-api`) so `scout-ledger` can gate on
//! it without pulling in `async-trait`/`futures`/`tokio-util`
//! (`docs/rust/01-workspace-and-crates.md`: a crate depends only on
//! what its own types need). See ADR-008 for the two-tier taxonomy this
//! implements.

/// Why a caller is opting into unverified external (Tier 2) data.
/// Threaded into the report/manifest (ADR-008) rather than discarded —
/// this is the concrete mechanism behind "callers must justify why."
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExternalDataAcknowledgement {
    /// The caller has their own indexer/data pipeline they trust for a
    /// stated reason.
    TrustedExternalSource { description: String },
    /// Research/exploratory use where strict verification is not
    /// required for the caller's purpose.
    ExploratoryUse { description: String },
}

/// An unforgeable token proving a caller explicitly opted into
/// unverified Tier 2 data. Deliberately not `Default`, not
/// constructible from a literal, not `Clone`-from-nothing — the only
/// constructor is [`ExternalDataOptIn::acknowledge`], which requires a
/// reason. `#[non_exhaustive]` so this type can never be constructed
/// via struct-literal syntax even from within this crate's own future
/// code without going through the constructor.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExternalDataOptIn {
    reason: ExternalDataAcknowledgement,
}

impl ExternalDataOptIn {
    /// The only way to construct this token. Per ADR-008, only the
    /// composition root (an application's `main`, or a library caller
    /// explicitly wiring Tier 2 support) should call this — and only
    /// when the corresponding config/CLI flag has been explicitly set,
    /// which is enforced by `scout-sdk`'s `external-data` feature gate
    /// existing as the second, independent barrier (this token alone is
    /// the first).
    #[must_use]
    pub fn acknowledge(reason: ExternalDataAcknowledgement) -> Self {
        Self { reason }
    }

    #[must_use]
    pub fn reason(&self) -> &ExternalDataAcknowledgement {
        &self.reason
    }
}

/// The trust level of one piece of data flowing through the pipeline.
/// `Verified` is only ever constructed by `scout-engine` for data that
/// actually passed through a Tier 1 `HistoryProvider` + a
/// scope-checked `TxDecoder`. `ExternalUnverified` can only be
/// constructed by holding an [`ExternalDataOptIn`] token — making
/// "unverified data with no acknowledgment" and "a Tier 2 source
/// self-declaring Verified" both unrepresentable states, not runtime
/// checks someone could forget.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TrustLevel {
    Verified,
    ExternalUnverified(ExternalDataOptIn),
}

impl TrustLevel {
    #[must_use]
    pub fn is_verified(&self) -> bool {
        matches!(self, TrustLevel::Verified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_token_requires_a_stated_reason() {
        let token =
            ExternalDataOptIn::acknowledge(ExternalDataAcknowledgement::TrustedExternalSource {
                description: "our own indexer, cross-checked weekly".to_string(),
            });
        match token.reason() {
            ExternalDataAcknowledgement::TrustedExternalSource { description } => {
                assert!(description.contains("indexer"));
            }
            other => panic!("expected TrustedExternalSource, got {other:?}"),
        }
    }

    #[test]
    fn external_unverified_is_never_reported_as_verified() {
        let opt_in = ExternalDataOptIn::acknowledge(ExternalDataAcknowledgement::ExploratoryUse {
            description: "research pass".to_string(),
        });
        let trust = TrustLevel::ExternalUnverified(opt_in);
        assert!(!trust.is_verified());
    }

    #[test]
    fn verified_reports_as_verified() {
        assert!(TrustLevel::Verified.is_verified());
    }
}
