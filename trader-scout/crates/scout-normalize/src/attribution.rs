//! Owner attribution. See ADR-003.

use scout_core::WalletKey;

/// Why an attribution could not reach `Confident`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmbiguityReason {
    /// Only a relayer/router/bundler/fee-payer signal was available, with
    /// no independent evidence tying the transaction to an economic
    /// owner (ACCEPTANCE B05).
    OnlyRelayerSignal,
    /// More than one wallet is an equally plausible economic owner.
    MultipleCandidates,
}

/// Which signal(s) justified a `Confident` classification. Kept as data
/// (not just a bool) so downstream reporting can show *why* a hit was
/// confident, per ARCHITECTURE.md §6's attribution-classes requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionEvidence {
    /// The recipient token account's tracked owner matches, and no
    /// router/relayer pattern was detected for this protocol/version.
    RecipientOwnerMatch,
    /// The transaction's sole signer is also the recipient, and the
    /// protocol/version is known not to use a relayer pattern.
    SoleSignerNoRelayerPattern,
}

/// Owner attribution result for one action. `tx.from`, the first signer,
/// and `Swap.sender`/`recipient` are never auto-promoted to this — they
/// are candidate signals consumed by classification logic elsewhere that
/// produces this typed result (ADR-003, AGENTS.md invariant #2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionStatus {
    Confident {
        owner: WalletKey,
        evidence: AttributionEvidence,
    },
    Ambiguous {
        candidates: Vec<WalletKey>,
        reason: AmbiguityReason,
    },
}

impl AttributionStatus {
    #[must_use]
    pub fn is_confident(&self) -> bool {
        matches!(self, AttributionStatus::Confident { .. })
    }

    /// The confidently-attributed owner, if any. Never returns a wallet
    /// for an `Ambiguous` status — callers must not fall back to "pick
    /// the first candidate."
    #[must_use]
    pub fn confident_owner(&self) -> Option<&WalletKey> {
        match self {
            AttributionStatus::Confident { owner, .. } => Some(owner),
            AttributionStatus::Ambiguous { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallet(byte: u8) -> WalletKey {
        WalletKey {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            address: scout_core::AddressBytes::Evm([byte; 20]),
        }
    }

    #[test]
    fn ambiguous_status_never_exposes_a_confident_owner() {
        // ACCEPTANCE B05: relayer sender with no independent evidence
        // must not silently become a confident hit owner.
        let status = AttributionStatus::Ambiguous {
            candidates: vec![wallet(1), wallet(2)],
            reason: AmbiguityReason::OnlyRelayerSignal,
        };
        assert_eq!(status.confident_owner(), None);
        assert!(!status.is_confident());
    }

    #[test]
    fn confident_status_exposes_its_owner_and_evidence() {
        let owner = wallet(3);
        let status = AttributionStatus::Confident {
            owner: owner.clone(),
            evidence: AttributionEvidence::RecipientOwnerMatch,
        };
        assert_eq!(status.confident_owner(), Some(&owner));
        assert!(status.is_confident());
    }
}
