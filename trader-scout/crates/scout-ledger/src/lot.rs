//! Lot model. See ADR-004.

use scout_core::{AssetKey, Money, RawAmount};

/// Whether a lot's acquisition cost is known or must be treated as
/// unknown (e.g. an external transfer with no provable lineage).
/// Per AGENTS.md invariant #6, unknown basis is never treated as zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasisStatus {
    Known,
    Unknown { reason: String },
}

/// One FIFO acquisition lot. Consumed strictly in acquisition order;
/// `remaining_amount`/`remaining_basis` shrink as the lot is disposed of,
/// proportionally to the fraction consumed (ADR-004, ACCEPTANCE C02).
#[derive(Debug, Clone)]
pub struct Lot {
    pub asset: AssetKey,
    /// Monotonic acquisition sequence number, used for FIFO ordering.
    /// Backed by the same canonical-location ordering contract as
    /// ADR-002's `CanonicalLocation` (not wall-clock/fetch order).
    pub acquisition_sequence: u64,
    pub original_amount: RawAmount,
    pub remaining_amount: RawAmount,
    /// Capitalized acquisition cost (consideration + allocated acquisition
    /// fees), for the *original* full lot.
    pub original_basis: Money,
    /// Portion of `original_basis` not yet consumed by disposals.
    pub remaining_basis: Money,
    pub basis_status: BasisStatus,
}

impl Lot {
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.remaining_amount == RawAmount::ZERO
    }
}
