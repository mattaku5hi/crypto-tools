//! Chain/wallet/asset identity types. See ADR-002 for the rationale.
//!
//! Network identity is part of every key here — same address bytes on two
//! different chains are two different records (AGENTS.md invariant #3).

use std::fmt;

/// Which blockchain family an address/chain belongs to.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ChainFamily {
    Solana,
    Evm,
}

/// Network identifier within a family: EVM chain_id, or a Solana cluster
/// tag. Not sufficient identity proof by itself — see [`GenesisIdentity`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum NetworkId {
    EvmChainId(u64),
    SolanaCluster(SolanaCluster),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SolanaCluster {
    Mainnet,
    Devnet,
    Testnet,
}

/// Opaque verified genesis fingerprint. Obtained via preflight (P2.1) and
/// cached; a chain profile without one is not eligible for live scanning
/// (ADR-002). `Unverified` is the only variant available before any
/// network credentials exist, and it is a strong type-level marker that
/// this chain identity has not passed the genesis check.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum GenesisIdentity {
    Unverified,
    Verified(String),
}

/// Full chain identity: family + network + verified genesis fingerprint.
///
/// Two `ChainKey`s are equal only if all three fields match — a Solana
/// mainnet and devnet key with byte-identical addresses are never equal
/// (ACCEPTANCE A03).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ChainKey {
    pub family: ChainFamily,
    pub network_id: NetworkId,
    pub genesis_identity: GenesisIdentity,
}

/// Fixed-capacity address bytes, tagged by chain family so a 20-byte EVM
/// address and a 32-byte Solana address are never comparable as raw bytes
/// alone.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum AddressBytes {
    Evm([u8; 20]),
    Solana([u8; 32]),
}

impl fmt::Display for AddressBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Lowercase hex is the canonical stored/compared form; EIP-55
            // mixed-case is a presentation-layer concern applied only when
            // formatting for a human, never part of this Display impl or
            // this type's Eq/Hash/Ord (ADR-002).
            AddressBytes::Evm(bytes) => write!(f, "0x{}", hex_lower(bytes)),
            AddressBytes::Solana(bytes) => write!(f, "{}", bs58::encode(bytes).into_string()),
        }
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A wallet's canonical identity: which chain, which address.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct WalletKey {
    pub chain: ChainKey,
    pub address: AddressBytes,
}

/// An asset's canonical identity: native currency of a chain, or a
/// specific token contract/mint on that chain. Same ticker on different
/// chains is never the same `AssetKey` (AGENTS.md invariant #3).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum AssetKey {
    Native(ChainKey),
    Token(ChainKey, AddressBytes),
}

/// Result of resolving a bare address to a chain when no explicit chain
/// was given. Never silently picks the first responder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainResolution {
    Resolved(ChainKey),
    /// Address is a valid candidate on more than one enabled chain.
    Ambiguous(Vec<ChainKey>),
    /// Not found in any available index — not proof of absence.
    NotFoundOrUnobserved,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mainnet_chain() -> ChainKey {
        ChainKey {
            family: ChainFamily::Solana,
            network_id: NetworkId::SolanaCluster(SolanaCluster::Mainnet),
            genesis_identity: GenesisIdentity::Verified("mainnet-genesis-hash".to_string()),
        }
    }

    fn devnet_chain() -> ChainKey {
        ChainKey {
            family: ChainFamily::Solana,
            network_id: NetworkId::SolanaCluster(SolanaCluster::Devnet),
            genesis_identity: GenesisIdentity::Verified("devnet-genesis-hash".to_string()),
        }
    }

    #[test]
    fn same_address_bytes_on_different_solana_clusters_are_different_wallet_keys() {
        // ACCEPTANCE A03: Solana mainnet/devnet address bytes collide but
        // must not be treated as the same wallet.
        let bytes = AddressBytes::Solana([7u8; 32]);
        let mainnet_wallet = WalletKey {
            chain: mainnet_chain(),
            address: bytes.clone(),
        };
        let devnet_wallet = WalletKey {
            chain: devnet_chain(),
            address: bytes,
        };
        assert_ne!(mainnet_wallet, devnet_wallet);
    }

    #[test]
    fn same_ticker_different_chains_are_different_asset_keys() {
        // AGENTS.md invariant #3: identical tickers on different networks
        // must not collapse into one AssetKey.
        let evm_chain_a = ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(8453), // Base
            genesis_identity: GenesisIdentity::Verified("base-genesis".to_string()),
        };
        let evm_chain_b = ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(56), // BSC
            genesis_identity: GenesisIdentity::Verified("bsc-genesis".to_string()),
        };
        let same_bytes = AddressBytes::Evm([0xAA; 20]);
        let asset_a = AssetKey::Token(evm_chain_a, same_bytes.clone());
        let asset_b = AssetKey::Token(evm_chain_b, same_bytes);
        assert_ne!(asset_a, asset_b);
    }

    #[test]
    fn evm_address_display_is_lowercase_canonical_form() {
        let addr = AddressBytes::Evm([0xAB; 20]);
        assert_eq!(addr.to_string(), format!("0x{}", "ab".repeat(20)));
    }

    #[test]
    fn ambiguous_chain_resolution_lists_every_candidate() {
        // ACCEPTANCE A01: never silently pick the first responder.
        let candidates = vec![mainnet_chain(), devnet_chain()];
        let resolution = ChainResolution::Ambiguous(candidates.clone());
        match resolution {
            ChainResolution::Ambiguous(found) => assert_eq!(found, candidates),
            _ => panic!("expected Ambiguous variant"),
        }
    }
}
