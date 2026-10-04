//! Chain identity of a ledger report, shared by the Solana and EVM paths
//! (ADR-020 step 2). The wallet/mint keys of the shared report types are
//! 32-byte arrays: Solana pubkeys as they are, EVM addresses left-padded
//! with 12 zero bytes. This type says which one a report holds and how to
//! render addresses, the native unit and its labels, so no output code has
//! to guess from byte patterns.

use scout_core::{AddressBytes, AssetKey, ChainFamily, ChainKey};
use scout_evm::EvmChainProfile;
use scout_ledger::QuoteUnit;

/// How addresses/units of one report are interpreted and displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainDisplay {
    pub family: ChainFamily,
    /// Input-syntax name (`solana`, `robinhood`, ...).
    pub name: &'static str,
    /// EVM chain id (`None` for Solana).
    pub chain_id: Option<u64>,
    /// Quote unit of the chain's native currency (`Lamports` / `Wei`).
    pub native_unit: QuoteUnit,
    /// Lower-case native label (`sol`, `eth`, `bnb`) used in column names.
    pub native_label: &'static str,
    /// Upper-case native symbol (`SOL`, `ETH`).
    pub native_symbol: &'static str,
}

/// Solana mainnet.
pub const SOLANA_DISPLAY: ChainDisplay = ChainDisplay {
    family: ChainFamily::Solana,
    name: "solana",
    chain_id: None,
    native_unit: QuoteUnit::Lamports,
    native_label: "sol",
    native_symbol: "SOL",
};

const SOLANA_UNITS: [QuoteUnit; 3] = [
    QuoteUnit::Lamports,
    QuoteUnit::UsdcUnits,
    QuoteUnit::UsdtUnits,
];
const EVM_UNITS: [QuoteUnit; 2] = [QuoteUnit::Wei, QuoteUnit::UsdgUnits];
const BASE_UNITS: [QuoteUnit; 2] = [QuoteUnit::Wei, QuoteUnit::UsdcUnits];

impl ChainDisplay {
    /// Display of an EVM chain profile.
    #[must_use]
    pub fn evm(profile: &EvmChainProfile) -> Self {
        let (label, symbol) = match profile.native_symbol {
            "BNB" => ("bnb", "BNB"),
            _ => ("eth", "ETH"),
        };
        Self {
            family: ChainFamily::Evm,
            name: profile.name,
            chain_id: Some(profile.chain_id),
            native_unit: QuoteUnit::Wei,
            native_label: label,
            native_symbol: symbol,
        }
    }

    /// Display of the EVM chain with `chain_id`, `None` outside the profiles.
    #[must_use]
    pub fn evm_by_chain_id(chain_id: u64) -> Option<Self> {
        EvmChainProfile::by_chain_id(chain_id).map(|p| Self::evm(&p))
    }

    #[must_use]
    pub fn is_evm(&self) -> bool {
        self.family == ChainFamily::Evm
    }

    /// Quote units a report of this chain carries one block for, in order.
    #[must_use]
    pub fn quote_units(&self) -> &'static [QuoteUnit] {
        if self.is_evm() {
            // Base quotes in USDC (Circle), Robinhood in USDG.
            if self.name == "base" {
                &BASE_UNITS
            } else {
                &EVM_UNITS
            }
        } else {
            &SOLANA_UNITS
        }
    }

    /// Render a 32-byte report key: base58 for Solana, `0x` lowercase hex of
    /// the low 20 bytes for EVM.
    #[must_use]
    pub fn address(&self, key: &[u8; 32]) -> String {
        if self.is_evm() {
            let tail = key.get(12..).unwrap_or(&[]);
            let mut s = String::with_capacity(42);
            s.push_str("0x");
            for b in tail {
                s.push_str(&format!("{b:02x}"));
            }
            s
        } else {
            bs58::encode(key).into_string()
        }
    }

    /// Identity key of a token of this chain (`AssetKey::Token`). EVM keys
    /// carry the chain id with an unverified genesis fingerprint (analytics
    /// grouping only; the verified key lives in the extraction layer).
    #[must_use]
    pub fn asset_key(&self, key: &[u8; 32]) -> AssetKey {
        match self.chain_id {
            Some(id) if self.is_evm() => AssetKey::Token(
                ChainKey {
                    family: ChainFamily::Evm,
                    network_id: scout_core::NetworkId::EvmChainId(id),
                    genesis_identity: scout_core::GenesisIdentity::Unverified,
                },
                AddressBytes::Evm(evm_address_of_key(key).into_array()),
            ),
            _ => AssetKey::Token(
                crate::solana_buy_qualification::solana_mainnet_chain(),
                AddressBytes::Solana(*key),
            ),
        }
    }

    /// Wallet/token identity text with the chain prefix (`robinhood:0x..`).
    #[must_use]
    pub fn prefixed_address(&self, key: &[u8; 32]) -> String {
        format!("{}:{}", self.name, self.address(key))
    }
}

/// 32-byte report key of an EVM address (left-padded with zeros).
#[must_use]
pub fn evm_key(address: alloy_primitives::Address) -> [u8; 32] {
    let mut k = [0u8; 32];
    if let Some(tail) = k.get_mut(12..) {
        tail.copy_from_slice(address.as_slice());
    }
    k
}

/// Inverse of [`evm_key`] (low 20 bytes).
#[must_use]
pub fn evm_address_of_key(key: &[u8; 32]) -> alloy_primitives::Address {
    alloy_primitives::Address::from_slice(key.get(12..).unwrap_or(&[0u8; 20]))
}

/// `AssetKey` of an EVM token on `chain`.
#[must_use]
pub fn evm_asset(chain: &ChainKey, token: alloy_primitives::Address) -> AssetKey {
    AssetKey::Token(chain.clone(), AddressBytes::Evm(token.into_array()))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;
    use scout_evm::ROBINHOOD;

    use super::*;

    #[test]
    fn evm_key_round_trips_and_renders_lowercase_hex() {
        let a = address!("0Bd7D308f8E1639FAb988df18A8011f41EAcAD73");
        let k = evm_key(a);
        assert_eq!(&k[..12], &[0u8; 12]);
        assert_eq!(evm_address_of_key(&k), a);
        let d = ChainDisplay::evm(&ROBINHOOD);
        assert_eq!(d.address(&k), "0x0bd7d308f8e1639fab988df18a8011f41eacad73");
        assert_eq!(
            d.prefixed_address(&k),
            "robinhood:0x0bd7d308f8e1639fab988df18a8011f41eacad73"
        );
        assert_eq!(d.native_unit, QuoteUnit::Wei);
        assert_eq!(d.quote_units(), &[QuoteUnit::Wei, QuoteUnit::UsdgUnits]);
        let base = ChainDisplay::evm(&scout_evm::BASE);
        assert_eq!(base.quote_units(), &[QuoteUnit::Wei, QuoteUnit::UsdcUnits]);
        let sol = SOLANA_DISPLAY;
        assert_eq!(
            sol.address(&[1u8; 32]),
            bs58::encode([1u8; 32]).into_string()
        );
    }
}
