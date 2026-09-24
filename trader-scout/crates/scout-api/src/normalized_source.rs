//! Tier 2: `NormalizedActivitySource` — already-classified economic
//! actions from an external system (a caller's own indexer, a Dune
//! export, etc.). See ADR-008 for the two-tier taxonomy.
//!
//! `TrustLevel`/`ExternalDataOptIn`/`ExternalDataAcknowledgement` live
//! in `scout-core` (not here) so `scout-ledger` can gate on trust
//! without depending on `scout-api`'s async-provider machinery
//! (`docs/rust/01-workspace-and-crates.md`). Re-exported from here so
//! existing `scout_api::TrustLevel` import paths keep working.
//!
//! Design note: `fetch_activity` returns `ExternalActivityClaim` — a
//! type with **no trust field at all**. Only
//! [`ExternalActivity::from_claim`], which requires the caller to
//! already hold an `ExternalDataOptIn` token, can produce a
//! `TrustLevel::ExternalUnverified` activity. A Tier 2 source cannot
//! declare itself `Verified` because nothing in this module can attach
//! `TrustLevel::Verified` to a claim — that variant is only ever
//! constructed by `scout-engine` for data that actually passed through
//! the Tier 1 decode path.

use scout_core::{AssetKey, WalletKey};

pub use scout_core::{ExternalDataAcknowledgement, ExternalDataOptIn, TrustLevel};

/// One economic action as **claimed** by an external, Tier 2 source.
/// Deliberately carries no trust information — see the module doc for
/// why. Not a `RawPayload`: this is already a classification claim
/// (e.g. "this was a buy") that our own pipeline did not derive itself.
#[derive(Debug, Clone)]
pub struct ExternalActivityClaim {
    pub wallet: WalletKey,
    pub asset: AssetKey,
    pub description: String,
}

/// An [`ExternalActivityClaim`] with a [`TrustLevel`] attached by the
/// caller. The only constructor, [`ExternalActivity::from_claim`],
/// requires an `ExternalDataOptIn` token and always produces
/// `TrustLevel::ExternalUnverified` — there is no public constructor
/// that can attach `TrustLevel::Verified` to a Tier 2 claim from this
/// module.
#[derive(Debug, Clone)]
pub struct ExternalActivity {
    pub claim: ExternalActivityClaim,
    pub trust: TrustLevel,
}

impl ExternalActivity {
    /// Attach trust to a claim. Requires proof (the token) that the
    /// caller explicitly opted into unverified data — the claim itself
    /// carries no say in the matter.
    #[must_use]
    pub fn from_claim(claim: ExternalActivityClaim, opt_in: ExternalDataOptIn) -> Self {
        Self {
            claim,
            trust: TrustLevel::ExternalUnverified(opt_in),
        }
    }

    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.trust.is_verified()
    }
}

/// Port a Tier 2 external activity source implements. Kept synchronous
/// (unlike `HistoryProvider`) since a caller's own indexer/export is
/// typically already-materialized data, not something requiring a
/// streaming network round-trip — a source needing async I/O can still
/// implement this by doing its own blocking-safe fetch internally
/// before returning.
///
/// Returns `ExternalActivityClaim`, not `ExternalActivity` — a source
/// has no way to attach a trust level to its own output. Only the
/// caller holding an `ExternalDataOptIn` (via
/// `ExternalActivity::from_claim`) can do that.
pub trait NormalizedActivitySource: Send + Sync {
    fn fetch_activity(&self, wallet: &WalletKey) -> Vec<ExternalActivityClaim>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_wallet() -> WalletKey {
        WalletKey {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            address: scout_core::AddressBytes::Evm([0x11; 20]),
        }
    }

    fn test_asset() -> AssetKey {
        AssetKey::Token(
            scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            scout_core::AddressBytes::Evm([0x22; 20]),
        )
    }

    #[test]
    fn claim_carries_no_trust_field() {
        let claim = ExternalActivityClaim {
            wallet: test_wallet(),
            asset: test_asset(),
            description: "external buy claim".to_string(),
        };
        assert_eq!(claim.description, "external buy claim");
    }

    #[test]
    fn from_claim_always_produces_external_unverified() {
        let claim = ExternalActivityClaim {
            wallet: test_wallet(),
            asset: test_asset(),
            description: "external buy claim".to_string(),
        };
        let opt_in = ExternalDataOptIn::acknowledge(
            scout_core::ExternalDataAcknowledgement::ExploratoryUse {
                description: "research pass".to_string(),
            },
        );
        let activity = ExternalActivity::from_claim(claim, opt_in);
        assert!(!activity.is_verified());
        assert!(matches!(activity.trust, TrustLevel::ExternalUnverified(_)));
    }

    #[test]
    fn verified_trust_level_reports_as_verified() {
        let trust = TrustLevel::Verified;
        assert!(trust.is_verified());
    }

    struct StubSource;
    impl NormalizedActivitySource for StubSource {
        fn fetch_activity(&self, wallet: &WalletKey) -> Vec<ExternalActivityClaim> {
            vec![ExternalActivityClaim {
                wallet: wallet.clone(),
                asset: test_asset(),
                description: "stub".to_string(),
            }]
        }
    }

    #[test]
    fn normalized_activity_source_is_object_safe() {
        let source: Box<dyn NormalizedActivitySource> = Box::new(StubSource);
        let claims = source.fetch_activity(&test_wallet());
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].description, "stub");
    }
}
