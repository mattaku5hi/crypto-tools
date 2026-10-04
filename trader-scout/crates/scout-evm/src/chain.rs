//! Per-chain constants for the EVM networks in scope (ADR-020, research doc
//! §1). Chain identity is `(chain_id, genesis hash)`: the genesis hash is
//! checked by a preflight before any scan (invariants #3/#4).

use alloy_primitives::{Address, B256, address, b256};
use scout_core::{ChainFamily, ChainKey, GenesisIdentity, NetworkId};

/// How a quote token is valued in USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuoteUsdPolicy {
    /// Valued at exactly 1 USD and labelled `<symbol>_par_assumed` (no FX,
    /// like USDC in ADR-018). The depeg risk is the user's, and visible.
    ParAssumed,
}

/// A stable quote token pinned for one chain (ADR-020 amendment). Native
/// ETH/BNB and its wrapped form are NOT listed here: they are merged and
/// handled by `EvmChainProfile::wrapped_native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteAssetSpec {
    pub symbol: &'static str,
    pub address: Address,
    /// ERC-20 decimals the ledger scales by. A run preflight compares this
    /// with the live `decimals()` and refuses to run on a mismatch.
    pub decimals: u8,
    pub usd: QuoteUsdPolicy,
}

/// Static profile of one supported EVM chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmChainProfile {
    pub name: &'static str,
    pub chain_id: u64,
    /// Expected hash of block 0 (research doc, measured 2026-10-03).
    pub genesis_hash: B256,
    pub native_symbol: &'static str,
    /// WETH/WBNB (WETH9 semantics).
    pub wrapped_native: Address,
    /// `true` for OP-stack chains whose receipts carry a separate `l1Fee`
    /// that is NOT part of `gasUsed * effectiveGasPrice` (Base).
    pub l1_fee_separate: bool,
    /// Pinned stable quote tokens (empty = none verified yet: the chain's
    /// trades quote only in native/wrapped-native until addresses are
    /// verified; Base/BSC are deliberately empty, ADR-020 amendment).
    pub quote_assets: &'static [QuoteAssetSpec],
}

/// Robinhood Chain stable quote: USDG (Global Dollar). The address is the
/// one seen in the busiest v4 swaps of the 2026-10-03 live fixture; its
/// decimals (6) are checked live by the run preflight.
pub const ROBINHOOD_USDG: QuoteAssetSpec = QuoteAssetSpec {
    symbol: "USDG",
    address: address!("5fc5360d0400a0fd4f2af552add042d716f1d168"),
    decimals: 6,
    usd: QuoteUsdPolicy::ParAssumed,
};

/// Base stable quote: native USDC issued by Circle
/// (developers.circle.com/stablecoins/usdc-contract-addresses, checked
/// 2026-10-04). `decimals()` (6) is verified live by the run preflight.
pub const BASE_USDC: QuoteAssetSpec = QuoteAssetSpec {
    symbol: "USDC",
    address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
    decimals: 6,
    usd: QuoteUsdPolicy::ParAssumed,
};

pub const ROBINHOOD: EvmChainProfile = EvmChainProfile {
    name: "robinhood",
    chain_id: 4663,
    genesis_hash: b256!("aad15f3d702aaea00caf3e9bb56395efe9127bc3b31b24921abf1eee3409305c"),
    native_symbol: "ETH",
    wrapped_native: address!("0Bd7D308f8E1639FAb988df18A8011f41EAcAD73"),
    // Arbitrum Orbit: gasUsed already includes gasUsedForL1.
    l1_fee_separate: false,
    quote_assets: &[ROBINHOOD_USDG],
};

pub const BASE: EvmChainProfile = EvmChainProfile {
    name: "base",
    chain_id: 8453,
    genesis_hash: b256!("f712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd"),
    native_symbol: "ETH",
    wrapped_native: address!("4200000000000000000000000000000000000006"),
    l1_fee_separate: true,
    // WETH is the pinned `wrapped_native` (canonical OP-stack WETH9, merged
    // with native ETH); USDC is Circle's official token. USDbC (bridged) is
    // deliberately not a quote asset.
    quote_assets: &[BASE_USDC],
};

pub const BSC: EvmChainProfile = EvmChainProfile {
    name: "bsc",
    chain_id: 56,
    genesis_hash: b256!("0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b"),
    native_symbol: "BNB",
    wrapped_native: address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"),
    l1_fee_separate: false,
    // TODO(ADR-020 step 3): USDT/USDC (18-decimals on BSC!) not verified yet.
    quote_assets: &[],
};

impl EvmChainProfile {
    /// Profile by CLI name (`robinhood|base|bsc`).
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        [ROBINHOOD, BASE, BSC]
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    #[must_use]
    pub fn by_chain_id(chain_id: u64) -> Option<Self> {
        [ROBINHOOD, BASE, BSC]
            .into_iter()
            .find(|p| p.chain_id == chain_id)
    }

    /// Pinned quote asset by address.
    #[must_use]
    pub fn quote_asset(&self, token: &Address) -> Option<&'static QuoteAssetSpec> {
        self.quote_assets.iter().find(|q| q.address == *token)
    }

    /// `ChainKey` with the genesis fingerprint marked verified. Only call
    /// after the preflight compared the live block-0 hash.
    #[must_use]
    pub fn verified_chain_key(&self) -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(self.chain_id),
            genesis_identity: GenesisIdentity::Verified(format!("{:#x}", self.genesis_hash)),
        }
    }

    /// `ChainKey` without a genesis check (offline tests, unverified data).
    #[must_use]
    pub fn unverified_chain_key(&self) -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(self.chain_id),
            genesis_identity: GenesisIdentity::Unverified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_and_keys_carry_network_identity() {
        assert_eq!(EvmChainProfile::by_name("Base"), Some(BASE));
        assert_eq!(EvmChainProfile::by_chain_id(4663), Some(ROBINHOOD));
        assert_eq!(EvmChainProfile::by_chain_id(1), None);
        assert_ne!(BASE.verified_chain_key(), ROBINHOOD.verified_chain_key());
        assert_ne!(BASE.verified_chain_key(), BASE.unverified_chain_key());
    }

    #[test]
    fn robinhood_pins_usdg_base_pins_usdc_and_bsc_pins_nothing_yet() {
        assert_eq!(ROBINHOOD.quote_assets.len(), 1);
        let usdg = ROBINHOOD.quote_asset(&ROBINHOOD_USDG.address).unwrap();
        assert_eq!((usdg.symbol, usdg.decimals), ("USDG", 6));
        // Native-wrapped is merged, never a quote token entry.
        assert!(ROBINHOOD.quote_asset(&ROBINHOOD.wrapped_native).is_none());
        assert_eq!(BASE.quote_assets, &[BASE_USDC]);
        let usdc = BASE.quote_asset(&BASE_USDC.address).unwrap();
        assert_eq!((usdc.symbol, usdc.decimals), ("USDC", 6));
        assert!(BASE.quote_asset(&BASE.wrapped_native).is_none());
        assert!(BSC.quote_assets.is_empty());
    }
}
