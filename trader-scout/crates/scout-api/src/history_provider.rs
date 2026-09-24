//! Tier 1: `HistoryProvider` — raw, undecoded chain data. See ADR-008
//! for the tier taxonomy and ADR-006 for the capability-status
//! vocabulary this reuses.

use std::collections::BTreeMap;

use futures::stream::BoxStream;
use scout_core::{AssetKey, RawPayload, WalletKey};
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;

/// Maturity level of a claimed capability. Moved here from
/// `scout-providers` (ADR-006's taxonomy) so a third-party provider can
/// report capabilities without depending on our crate. Never promoted
/// to `LiveVerified` by documentation alone — requires an actual dated,
/// successful call against a live endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum CapabilityStatus {
    Documented,
    FixtureVerified,
    LiveVerified,
    Unsupported,
    Unknown,
}

/// What a provider can do, keyed by capability name. `BTreeMap` for
/// deterministic iteration (workspace-wide determinism policy, see
/// `docs/rust/03-numeric-and-determinism.md`).
#[derive(Debug, Clone, Default)]
pub struct SourceCapabilities {
    pub by_capability: BTreeMap<String, CapabilityStatus>,
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

/// What is being scanned for: a token's buyers, a wallet's activity, or
/// an explicit block/slot range. Kept minimal; full request shape
/// (period, finality, quality contract) lands alongside the JSON Schema
/// (P1.4).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ScanRequest {
    TokenMarketActivity { asset: AssetKey },
    WalletActivity { wallet: WalletKey },
}

/// A provider's plan for satisfying a `ScanRequest`: what it can do, at
/// what granularity. Distinct from the request itself so a provider can
/// report a partial/degraded plan without lying about full coverage.
#[derive(Debug, Clone)]
pub struct ScanPlan {
    pub request_echo: String,
    pub capabilities: SourceCapabilities,
}

/// One unit of scanning work, as scheduled by the engine. Left minimal
/// here — cursors/batching land with the real scheduler.
#[derive(Debug, Clone)]
pub struct ScanTask {
    pub description: String,
}

/// A raw batch from a provider, plus completeness observations. Per
/// ARCHITECTURE.md §4: "Объявление источником окончания диапазона — не
/// durable checkpoint" — this envelope carries observations, not a
/// commitment. `payload` is a real `RawPayload` (ADR-008 S1/S3), not a
/// description string — a caller can actually decode it.
#[derive(Debug, Clone)]
pub struct ScanEnvelope {
    pub payload: RawPayload,
}

/// Port every Tier 1 history/discovery source implements — ours and
/// third parties' alike (ADR-008: there is no privileged internal-only
/// interface). Object-safe via `BoxStream`/boxed futures so a registry
/// of heterogeneous providers can be built without a hidden runtime
/// (ARCHITECTURE.md §4, AGENTS.md invariant #15).
#[async_trait::async_trait]
pub trait HistoryProvider: Send + Sync {
    fn capabilities(&self) -> SourceCapabilities;

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError>;

    fn scan(
        &self,
        task: ScanTask,
        cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_capability_reports_unknown_not_unsupported() {
        // ADR-006: Unknown must never be silently treated as Unsupported.
        let caps = SourceCapabilities::empty();
        assert_eq!(
            caps.status_for("wallet_activity"),
            CapabilityStatus::Unknown
        );
    }
}
