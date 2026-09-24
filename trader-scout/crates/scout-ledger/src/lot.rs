//! Lot model. See ADR-004.

use scout_core::{AssetKey, Money, RawAmount, TrustLevel};

/// Whether a lot's acquisition cost is known or must be treated as
/// unknown (e.g. an external transfer with no provable lineage, or
/// Tier 2 unverified data admitted via `Ledger::acquire_unverified`).
/// Per AGENTS.md invariant #6, unknown basis is never treated as zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasisStatus {
    Known,
    Unknown { reason: String },
}

impl BasisStatus {
    /// The forced status for any lot acquired through the Tier 2 path
    /// (ADR-008): unverified data can never produce a `Known` basis,
    /// regardless of what a `NormalizedActivitySource` claims.
    #[must_use]
    pub fn unverified() -> Self {
        BasisStatus::Unknown {
            reason: "acquired via Tier 2 (ExternalUnverified) source".to_string(),
        }
    }
}

/// Which lot-acquisition path recorded a lot: verified (Tier 1, our own
/// decode) or unverified (Tier 2, `NormalizedActivitySource`). Kept
/// separate from `BasisStatus` because a `Known`-looking amount from an
/// external source is still not the same as an amount we decoded
/// ourselves — `Ledger::dispose` never has to inspect this field's
/// origin to decide `realized_trade_pnl`, `BasisStatus` alone already
/// carries that decision (see `BasisStatus::unverified`), but it is
/// exposed on `Lot` for reporting/audit trails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LotProvenance {
    Verified,
    Unverified,
}

impl From<&TrustLevel> for LotProvenance {
    fn from(trust: &TrustLevel) -> Self {
        if trust.is_verified() {
            LotProvenance::Verified
        } else {
            LotProvenance::Unverified
        }
    }
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
    pub provenance: LotProvenance,
}

impl Lot {
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.remaining_amount == RawAmount::ZERO
    }
}
