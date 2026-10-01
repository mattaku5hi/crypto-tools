//! Aggregating Solana SPL token balance changes into net-delta inputs
//! for `classify_buy`. See `docs/TICKETS.md` P0.13 and
//! `docs/p0/deployment-registry.md`'s census notes for why this exists
//! and what it deliberately does NOT claim.

use std::collections::BTreeMap;

use alloy_primitives::I256;
use scout_core::{
    AddressBytes, AssetKey, ChainKey, SignedAmount, SolanaPubkey, SolanaTokenBalanceChange,
    WalletKey,
};

use crate::attribution::{AmbiguityReason, AttributionStatus};
use crate::classify::{ActionKind, AssetFlow, NetDeltaInput};

/// Aggregation of one transaction's `SolanaTokenBalanceChange` entries
/// into `classify_buy`-ready net deltas, with any mint this transaction
/// cannot attribute confidently set aside instead of guessed at.
#[derive(Debug, Clone, Default)]
pub struct SolanaBalanceAggregation {
    /// `(wallet, asset) -> net delta` for mints where exactly one
    /// owner's balance increased in this transaction -- the only case
    /// where "the owner of the increase" is an unambiguous candidate.
    ///
    /// Every flow's `kinds_observed` is `[ActionKind::Unknown]`, NEVER
    /// `Swap`: a balance increase alone does not prove a recognized
    /// exchange execution occurred (ACCEPTANCE B07 requires `Swap`
    /// specifically). `docs/p0/deployment-registry.md` has zero
    /// confirmed Solana deployments, so nothing in this workspace can
    /// honestly supply `Swap` for Solana yet (`docs/TICKETS.md` P0.12).
    /// `classify_buy` will therefore correctly report zero qualifying
    /// buys for every flow built here until a real decoder lands --
    /// this is the honest result, not a bug to route around by
    /// hardcoding `Swap` to make a pipeline "work end to end" (that
    /// would repeat `scout-dex-solana`'s event-CPI false-positive
    /// mistake one layer up).
    pub flows: BTreeMap<WalletKey, NetDeltaInput>,
    /// Mints where more than one owner's balance increased in the same
    /// transaction (fee recipients, LP accounts, routers, or genuinely
    /// more than one buyer in one transaction) -- per
    /// `AttributionStatus::Ambiguous`, never resolved by picking the
    /// largest delta. Pubkey bytes alone cannot distinguish a wallet
    /// from a program-owned vault/PDA; `docs/TICKETS.md` P0.15 tracks
    /// the exclusion-set work that would shrink this set in practice.
    pub ambiguous: BTreeMap<AssetKey, AttributionStatus>,
}

/// The only way aggregation can fail: a delta exceeds `SignedAmount`'s
/// representable range. Unreachable for Solana `u64` token amounts in
/// practice (the widest possible delta is far smaller than `i128`'s
/// range, itself far smaller than `SignedAmount`'s 256-bit range) --
/// kept as a typed `Result` rather than `unwrap`/`expect` per the
/// workspace's deny-by-default lint policy: AGENTS.md invariant #7's
/// "never silently assume" extends to arithmetic that cannot currently
/// fail but must not panic if the assumption ever breaks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SolanaBalanceAggregationError {
    #[error("token balance delta exceeds SignedAmount's representable range")]
    DeltaOutOfRange,
}

/// Aggregate one transaction's `SolanaTokenBalanceChange` entries (see
/// `scout_core::RawSolanaTransaction::token_balance_changes`) into net
/// deltas keyed by `(mint, owner)` -- never by token account, since one
/// wallet can hold several associated token accounts for the same
/// mint, and routers create scratch ones that must not be counted as
/// separate participants.
///
/// Entries with `owner: None` are excluded from attribution entirely,
/// never bucketed under a sentinel owner. `chain` identifies which
/// Solana cluster these balances belong to -- not carried by the raw
/// balance-change data itself, supplied by the caller's own context
/// (mirrors `HeliusProvider::parse_solana_wallet`'s handling of the
/// same gap).
pub fn aggregate_solana_token_balance_changes(
    changes: &[SolanaTokenBalanceChange],
    chain: &ChainKey,
) -> Result<SolanaBalanceAggregation, SolanaBalanceAggregationError> {
    // mint -> owner -> cumulative i128 delta. i128 (not u64) because the
    // delta is routinely negative (the pool/seller side of a swap) and
    // post_amount - pre_amount on raw u64s would underflow for exactly
    // that case.
    let mut deltas: BTreeMap<SolanaPubkey, BTreeMap<SolanaPubkey, i128>> = BTreeMap::new();

    for change in changes {
        let Some(owner) = change.owner else {
            // No owner reported for this account -- cannot attribute,
            // not guessed at, not bucketed under a sentinel.
            continue;
        };
        let pre = i128::from(change.pre_amount.unwrap_or(0));
        let post = i128::from(change.post_amount);
        let delta = post
            .checked_sub(pre)
            .ok_or(SolanaBalanceAggregationError::DeltaOutOfRange)?;

        let owner_deltas = deltas.entry(change.mint).or_default();
        let accumulated = owner_deltas.entry(owner).or_insert(0);
        *accumulated = accumulated
            .checked_add(delta)
            .ok_or(SolanaBalanceAggregationError::DeltaOutOfRange)?;
    }

    let mut result = SolanaBalanceAggregation::default();

    for (mint, owner_deltas) in deltas {
        let asset = AssetKey::Token(chain.clone(), AddressBytes::Solana(mint));
        let mut positive_iter = owner_deltas.into_iter().filter(|(_, delta)| *delta > 0);

        match (positive_iter.next(), positive_iter.next()) {
            (None, _) => {
                // No owner gained this mint in this transaction (pure
                // sell/transfer-out, or every delta was exactly zero)
                // -- not a buy signal, not an error, nothing to record.
            }
            (Some((owner, delta)), None) => {
                let wallet = WalletKey {
                    chain: chain.clone(),
                    address: AddressBytes::Solana(owner),
                };
                let net_delta = SignedAmount::from_i256(
                    I256::try_from(delta)
                        .map_err(|_| SolanaBalanceAggregationError::DeltaOutOfRange)?,
                );
                let flow = AssetFlow {
                    asset: asset.clone(),
                    net_delta,
                    kinds_observed: vec![ActionKind::Unknown],
                };
                result
                    .flows
                    .entry(wallet)
                    .or_default()
                    .flows
                    .insert(asset, flow);
            }
            (Some(first), Some(second)) => {
                let mut candidates_raw = vec![first, second];
                candidates_raw.extend(positive_iter);
                let candidates = candidates_raw
                    .into_iter()
                    .map(|(owner, _)| WalletKey {
                        chain: chain.clone(),
                        address: AddressBytes::Solana(owner),
                    })
                    .collect();
                result.ambiguous.insert(
                    asset,
                    AttributionStatus::Ambiguous {
                        candidates,
                        reason: AmbiguityReason::MultipleCandidates,
                    },
                );
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::classify_buy;

    // Synthetic byte arrays throughout: this module's correctness is
    // about the aggregation ALGORITHM (grouping, sign filtering,
    // ambiguity detection), not about any specific chain's real
    // addresses -- the real-data fidelity burden for the actual
    // numbers/addresses already lives in
    // crates/scout-providers/src/helius.rs's tests against the
    // committed fixture. Mirrors the existing synthetic-helper idiom
    // in classify.rs/attribution.rs's own tests.
    fn chain() -> ChainKey {
        ChainKey {
            family: scout_core::ChainFamily::Solana,
            network_id: scout_core::NetworkId::SolanaCluster(scout_core::SolanaCluster::Mainnet),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        }
    }

    fn change(
        mint: u8,
        owner: Option<u8>,
        pre: Option<u64>,
        post: u64,
    ) -> SolanaTokenBalanceChange {
        SolanaTokenBalanceChange {
            mint: [mint; 32],
            owner: owner.map(|b| [b; 32]),
            decimals: 6,
            pre_amount: pre,
            post_amount: post,
        }
    }

    #[test]
    fn single_positive_owner_is_a_confident_flow_tagged_unknown_not_swap() {
        let changes = vec![
            change(0x01, Some(0x11), Some(1_000), 5_000), // buyer: +4000
            change(0x01, Some(0x22), Some(9_000), 5_000), // pool: -4000
        ];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        assert!(aggregation.ambiguous.is_empty());
        assert_eq!(aggregation.flows.len(), 1);

        let wallet = WalletKey {
            chain: chain(),
            address: AddressBytes::Solana([0x11; 32]),
        };
        let asset = AssetKey::Token(chain(), AddressBytes::Solana([0x01; 32]));
        let flow = &aggregation.flows[&wallet].flows[&asset];
        assert_eq!(flow.kinds_observed, vec![ActionKind::Unknown]);
        assert!(!flow.net_delta.is_negative());
    }

    #[test]
    fn classify_buy_yields_zero_hits_for_an_unknown_kind_flow() {
        // The contract this module exists to guarantee: aggregating a
        // real balance increase does NOT, by itself, produce a
        // confident buy hit. If this test ever starts failing because
        // someone wired ActionKind::Swap into this module to make
        // buyer-intersect "find" Solana buys, that change must be
        // reverted -- Swap may only come from a confirmed decoder
        // (docs/TICKETS.md P0.12), never fabricated here.
        let changes = vec![change(0x01, Some(0x11), Some(1_000), 5_000)];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        let wallet = WalletKey {
            chain: chain(),
            address: AddressBytes::Solana([0x11; 32]),
        };
        let input = aggregation.flows.get(&wallet).unwrap();
        assert!(classify_buy(input).is_empty());
    }

    #[test]
    fn owner_none_is_excluded_from_attribution_entirely() {
        let changes = vec![change(0x01, None, Some(0), 999_999_999)];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        assert!(aggregation.flows.is_empty());
        assert!(aggregation.ambiguous.is_empty());
    }

    #[test]
    fn multiple_positive_owners_for_one_mint_are_ambiguous_not_picked() {
        let changes = vec![
            change(0x01, Some(0x11), Some(0), 100), // candidate A
            change(0x01, Some(0x22), Some(0), 500), // candidate B, larger
        ];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        assert!(aggregation.flows.is_empty());
        let asset = AssetKey::Token(chain(), AddressBytes::Solana([0x01; 32]));
        let status = aggregation.ambiguous.get(&asset).unwrap();
        match status {
            AttributionStatus::Ambiguous { candidates, reason } => {
                assert_eq!(candidates.len(), 2);
                assert_eq!(*reason, AmbiguityReason::MultipleCandidates);
            }
            AttributionStatus::Confident { .. } => panic!("must not resolve to Confident"),
        }
    }

    #[test]
    fn zero_delta_owner_does_not_count_as_a_buy_candidate() {
        let changes = vec![change(0x01, Some(0x11), Some(500), 500)];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        assert!(aggregation.flows.is_empty());
        assert!(aggregation.ambiguous.is_empty());
    }

    #[test]
    fn negative_delta_owner_alone_produces_no_flow_and_no_ambiguity() {
        let changes = vec![change(0x01, Some(0x22), Some(9_000), 5_000)];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        assert!(aggregation.flows.is_empty());
        assert!(aggregation.ambiguous.is_empty());
    }

    #[test]
    fn duplicate_owner_entries_for_the_same_mint_are_summed_not_overwritten() {
        // Defensive: this function's contract doesn't forbid a caller
        // concatenating changes from more than one source for the same
        // mint+owner pair. Checked accumulation, not last-write-wins.
        let changes = vec![
            change(0x01, Some(0x11), Some(0), 100),
            change(0x01, Some(0x11), Some(100), 250),
        ];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        let wallet = WalletKey {
            chain: chain(),
            address: AddressBytes::Solana([0x11; 32]),
        };
        let asset = AssetKey::Token(chain(), AddressBytes::Solana([0x01; 32]));
        let flow = &aggregation.flows[&wallet].flows[&asset];
        // 100 + 150 = 250 total positive delta across both entries.
        assert_eq!(
            flow.net_delta,
            SignedAmount::from_i256(I256::try_from(250).unwrap())
        );
    }

    #[test]
    fn independent_mints_in_one_transaction_are_aggregated_separately() {
        let changes = vec![
            change(0x01, Some(0x11), Some(0), 100),
            change(0x02, Some(0x11), Some(0), 200),
        ];
        let aggregation = aggregate_solana_token_balance_changes(&changes, &chain()).unwrap();

        let wallet = WalletKey {
            chain: chain(),
            address: AddressBytes::Solana([0x11; 32]),
        };
        let flows_for_wallet = aggregation.flows.get(&wallet).unwrap();
        assert_eq!(flows_for_wallet.flows.len(), 2);
    }
}
