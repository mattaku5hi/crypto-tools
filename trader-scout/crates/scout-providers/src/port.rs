//! The `HistoryProvider` port. See ADR-006 and ARCHITECTURE.md §4 for the
//! contract; this mirrors `HistorySource`/`TxDecoder` from ARCHITECTURE.md
//! under this crate's naming.

use futures::stream::BoxStream;
use scout_core::{AssetKey, ScoutError, WalletKey};
use tokio_util::sync::CancellationToken;

use crate::capability::SourceCapabilities;

/// What is being scanned for: a token's buyers, a wallet's activity, or
/// an explicit block/slot range. Kept minimal here; full request shape
/// (period, finality, quality contract) is finalized in P1.4 alongside
/// the JSON Schema.
#[derive(Debug, Clone)]
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

/// One unit of scanning work, as scheduled by the engine (P2+). Left
/// minimal here — cursors/batching land with the real scheduler.
#[derive(Debug, Clone)]
pub struct ScanTask {
    pub description: String,
}

/// A raw batch from a provider, plus completeness observations. Per
/// ARCHITECTURE.md §4: "Объявление источником окончания диапазона — не
/// durable checkpoint" — this envelope carries observations, not a
/// commitment.
#[derive(Debug, Clone)]
pub struct ScanEnvelope {
    pub raw_payload_description: String,
}

/// Port every history/discovery source implements. Object-safe via
/// `BoxStream`/boxed futures so a registry of providers can be built
/// without a hidden runtime (ARCHITECTURE.md §4, AGENTS.md invariant #15).
#[async_trait::async_trait]
pub trait HistoryProvider: Send + Sync {
    fn capabilities(&self) -> SourceCapabilities;

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ScoutError>;

    fn scan(
        &self,
        task: ScanTask,
        cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ScoutError>>;
}
