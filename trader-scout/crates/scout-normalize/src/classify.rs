//! Net-delta buy classification. See ADR-003 for the definition this
//! implements and ACCEPTANCE §B for the worked test cases.

use std::collections::BTreeMap;

use scout_core::{AssetKey, SignedAmount};

/// The economic kind of one decoded action, prior to buy classification.
/// Per AGENTS.md invariant #1, these are never conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Swap,
    Transfer,
    Wrap,
    Unwrap,
    Bridge,
    Mint,
    Burn,
    Airdrop,
    Reward,
    LiquidityAdd,
    LiquidityRemove,
    Fee,
    Unknown,
}

/// One asset's net delta for the confidently-attributed owner within a
/// single transaction, plus which kind of action produced flows of that
/// asset (an asset can be touched by more than one action kind within a
/// transaction, e.g. a swap plus a fee).
#[derive(Debug, Clone)]
pub struct AssetFlow {
    pub asset: AssetKey,
    pub net_delta: SignedAmount,
    pub kinds_observed: Vec<ActionKind>,
}

/// Everything [`classify_buy`] needs: the owner's net deltas per asset
/// within one transaction, keyed for deterministic iteration.
#[derive(Debug, Clone, Default)]
pub struct NetDeltaInput {
    pub flows: BTreeMap<AssetKey, AssetFlow>,
}

/// Determine which assets qualify as a "buy" for the confidently
/// attributed owner within one transaction, per ADR-003's net-acquisition
/// definition:
///
/// 1. A recognized exchange execution occurred (at least one `Swap`-kind
///    action touched the asset) — transfers/airdrops/rewards/wraps/
///    liquidity-removal alone never qualify (ACCEPTANCE B07).
/// 2. The asset's net delta across the owner's economic accounts in this
///    transaction is strictly positive (ACCEPTANCE B04/B06: intermediate
///    route tokens and atomic roundtrips that net to zero do not
///    qualify).
///
/// Returns the set of qualifying `AssetKey`s. Per ACCEPTANCE B02, this is
/// computed once per transaction per asset — callers must not call this
/// per decoded swap leg and count multiple hits from a single tx.
#[must_use]
pub fn classify_buy(input: &NetDeltaInput) -> Vec<AssetKey> {
    input
        .flows
        .iter()
        .filter(|(_, flow)| {
            let had_swap_execution = flow.kinds_observed.contains(&ActionKind::Swap);
            let net_positive =
                !flow.net_delta.is_negative() && flow.net_delta != SignedAmount::ZERO;
            had_swap_execution && net_positive
        })
        .map(|(asset, _)| asset.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(byte: u8) -> AssetKey {
        AssetKey::Token(
            scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            scout_core::AddressBytes::Evm([byte; 20]),
        )
    }

    fn positive_delta(units: i64) -> SignedAmount {
        SignedAmount::from_i256(alloy_primitives::I256::try_from(units).unwrap())
    }

    #[test]
    fn swap_with_positive_net_delta_qualifies_as_buy() {
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: positive_delta(100),
                kinds_observed: vec![ActionKind::Swap],
            },
        );
        let hits = classify_buy(&input);
        assert_eq!(hits, vec![asset(1)]);
    }

    #[test]
    fn transfer_receipt_never_qualifies_as_buy() {
        // ACCEPTANCE B07: transfer receipt is not a buy, even with a
        // positive net delta.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: positive_delta(100),
                kinds_observed: vec![ActionKind::Transfer],
            },
        );
        assert!(classify_buy(&input).is_empty());
    }

    #[test]
    fn airdrop_never_qualifies_as_buy() {
        // ACCEPTANCE B07.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: positive_delta(50),
                kinds_observed: vec![ActionKind::Airdrop],
            },
        );
        assert!(classify_buy(&input).is_empty());
    }

    #[test]
    fn zero_net_delta_intermediate_route_token_does_not_qualify() {
        // ACCEPTANCE B04: SOL->USDC->T1 routing; USDC nets to zero and
        // must not be a hit even though a Swap touched it.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(2), // USDC-equivalent intermediate
            AssetFlow {
                asset: asset(2),
                net_delta: SignedAmount::ZERO,
                kinds_observed: vec![ActionKind::Swap],
            },
        );
        assert!(classify_buy(&input).is_empty());
    }

    #[test]
    fn atomic_roundtrip_zero_net_end_state_does_not_qualify() {
        // ACCEPTANCE B06: atomic roundtrip with zero end net acquisition
        // is not a buy.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: SignedAmount::ZERO,
                kinds_observed: vec![ActionKind::Swap, ActionKind::Swap],
            },
        );
        assert!(classify_buy(&input).is_empty());
    }

    #[test]
    fn hundred_buys_of_one_token_yield_one_qualifying_asset_not_a_hundred() {
        // ACCEPTANCE B02: hit_count reflects distinct qualifying assets,
        // not the number of buy events. classify_buy operates on a
        // single already-aggregated net delta per asset, so this
        // invariant holds by construction — this test documents that
        // contract explicitly.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: positive_delta(100_000), // aggregate of many buys
                kinds_observed: vec![ActionKind::Swap],
            },
        );
        let hits = classify_buy(&input);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn bonding_curve_launch_buy_via_swap_kind_qualifies() {
        // ACCEPTANCE B07: "Поддерживаемая покупка на launch/bonding
        // curve дает hit." A bonding-curve buy decodes to ActionKind::Swap
        // in this workspace's taxonomy (it is a recognized exchange
        // execution), so it is covered by the same positive-net-delta
        // Swap-kind rule as an AMM swap.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: positive_delta(1),
                kinds_observed: vec![ActionKind::Swap],
            },
        );
        assert_eq!(classify_buy(&input), vec![asset(1)]);
    }

    #[test]
    fn liquidity_removal_never_qualifies_as_buy() {
        // ACCEPTANCE B07.
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset(1),
            AssetFlow {
                asset: asset(1),
                net_delta: positive_delta(10),
                kinds_observed: vec![ActionKind::LiquidityRemove],
            },
        );
        assert!(classify_buy(&input).is_empty());
    }
}
