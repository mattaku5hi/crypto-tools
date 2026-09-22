//! Capability status taxonomy. See ADR-006.

use std::collections::BTreeMap;

use scout_core::AssetKey;

/// Maturity level of a claimed capability. Never promoted to
/// `LiveVerified` by documentation alone — requires an actual dated,
/// successful call against a live endpoint (ADR-006).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Documented,
    FixtureVerified,
    LiveVerified,
    Unsupported,
    Unknown,
}

/// What a provider can do, keyed by capability name. Deterministic
/// iteration order via `BTreeMap` so two providers with the same
/// capabilities always serialize identically regardless of insertion
/// order (workspace-wide determinism policy).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SourceCapabilities {
    pub by_capability: BTreeMap<String, CapabilityStatus>,
    /// Assets this provider has confirmed some capability for, if scoped
    /// to specific assets rather than a whole chain.
    pub scoped_assets: Vec<AssetKey>,
}

impl SourceCapabilities {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn status_for(&self, capability: &str) -> CapabilityStatus {
        self.by_capability
            .get(capability)
            .copied()
            .unwrap_or(CapabilityStatus::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_capability_reports_unknown_not_unsupported() {
        // ADR-006: Unknown must never be silently treated as Unsupported
        // or as a pass.
        let caps = SourceCapabilities::empty();
        assert_eq!(
            caps.status_for("wallet_activity"),
            CapabilityStatus::Unknown
        );
    }
}
