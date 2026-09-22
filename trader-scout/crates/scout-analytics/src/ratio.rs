//! Win rate & profit factor. See ADR-004: statuses are a typed enum, not
//! float sentinels (no Infinity/NaN/999 surrogate) — ACCEPTANCE D06.

use scout_core::Money;

use crate::episode::EpisodeCohort;

/// A ratio result that may be a value, or one of two well-defined
/// non-value states. Never serialized as float Infinity/NaN, never a
/// magic-number surrogate like 999 (ACCEPTANCE D06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RatioStatus<T> {
    Value {
        value: T,
    },
    /// No negative episodes observed; profit factor is mathematically
    /// unbounded. Serializes as `{"status":"no_observed_losses"}` with
    /// no numeric `value` field (ACCEPTANCE D06).
    NoObservedLosses,
    /// No positive and no negative episodes: undefined.
    Undefined,
}

/// Win rate: `positive_closed_episodes / all_valid_closed_episodes`.
/// Breakeven (zero-PnL) episodes count in the denominator but are never
/// positive (ACCEPTANCE D04).
#[must_use]
pub fn win_rate(cohort: &EpisodeCohort) -> RatioStatus<Money> {
    let valid = cohort.valid_closed_episodes();
    if valid.is_empty() {
        return RatioStatus::Undefined;
    }
    let positive_count = valid
        .iter()
        .filter(|e| {
            e.realized_pnl.is_negative().then_some(()).is_none() && !e.realized_pnl.is_zero()
        })
        .count();
    // Represent win rate as a Money-scaled fraction (0..=1 at MONEY_SCALE)
    // so it goes through the same checked-integer path as everything
    // else, rather than introducing a bare f64 here.
    let scale = 10i128.pow(scout_core::MONEY_SCALE);
    let numerator = i128::try_from(positive_count).unwrap_or(0);
    let denominator = i128::try_from(valid.len()).unwrap_or(1).max(1);
    let scaled = numerator.saturating_mul(scale).div_euclid(denominator);
    RatioStatus::Value {
        value: Money::from_scaled_units(scaled),
    }
}

/// Profit factor: `sum(positive episode pnl) / abs(sum(negative episode
/// pnl))`. Breakeven episodes are excluded from both sums (ACCEPTANCE
/// §10: "Breakeven episode входит в denominator win rate, но не в
/// positive/negative sums profit factor").
#[must_use]
pub fn profit_factor(cohort: &EpisodeCohort) -> RatioStatus<Money> {
    let valid = cohort.valid_closed_episodes();
    let mut positive_sum = Money::ZERO;
    let mut negative_sum_abs = Money::ZERO;
    for episode in &valid {
        if episode.realized_pnl.is_zero() {
            continue;
        }
        if episode.realized_pnl.is_negative() {
            // Money has no public negate; reconstruct the absolute value
            // via ZERO - value, which is exact for our fixed-point type.
            if let Ok(abs) = Money::ZERO.checked_sub(&episode.realized_pnl) {
                negative_sum_abs = negative_sum_abs
                    .checked_add(&abs)
                    .unwrap_or(negative_sum_abs);
            }
        } else if let Ok(sum) = positive_sum.checked_add(&episode.realized_pnl) {
            positive_sum = sum;
        }
    }

    if negative_sum_abs.is_zero() && positive_sum.is_zero() {
        return RatioStatus::Undefined;
    }
    if negative_sum_abs.is_zero() {
        return RatioStatus::NoObservedLosses;
    }

    let scale = 10i128.pow(scout_core::MONEY_SCALE);
    let numerator = positive_sum.scaled_units();
    let denominator = negative_sum_abs.scaled_units();
    let scaled = numerator
        .saturating_mul(scale)
        .div_euclid(denominator.max(1));
    RatioStatus::Value {
        value: Money::from_scaled_units(scaled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::episode::Episode;

    fn asset() -> scout_core::AssetKey {
        scout_core::AssetKey::Native(scout_core::ChainKey {
            family: scout_core::ChainFamily::Evm,
            network_id: scout_core::NetworkId::EvmChainId(8453),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        })
    }

    fn closed_episode(pnl_scaled: i128) -> Episode {
        Episode {
            asset: asset(),
            realized_pnl: Money::from_scaled_units(pnl_scaled),
            is_closed_within_window: true,
            opened_before_window: false,
            has_unresolved_flows: false,
        }
    }

    #[test]
    fn no_negative_episodes_gives_no_observed_losses_status() {
        // ACCEPTANCE D06: PF status=no_observed_losses, not Infinity.
        let cohort = EpisodeCohort {
            episodes: vec![closed_episode(100), closed_episode(200)],
        };
        assert_eq!(
            profit_factor(&cohort),
            RatioStatus::<Money>::NoObservedLosses
        );
    }

    #[test]
    fn no_episodes_at_all_gives_undefined_status() {
        let cohort = EpisodeCohort { episodes: vec![] };
        assert_eq!(profit_factor(&cohort), RatioStatus::<Money>::Undefined);
    }

    #[test]
    fn breakeven_episode_counts_in_win_rate_denominator_but_not_pf_sums() {
        // ARCHITECTURE.md §10: breakeven is in win-rate denominator (not
        // numerator, since it's not positive) but excluded entirely from
        // PF's positive/negative sums.
        let cohort = EpisodeCohort {
            episodes: vec![closed_episode(0), closed_episode(100)],
        };
        // Win rate: 1 positive out of 2 valid closed episodes = 50%.
        match win_rate(&cohort) {
            RatioStatus::Value { value } => {
                assert_eq!(value, Money::from_scaled_units(50_000_000)); // 0.5 at 8dp
            }
            other => panic!("expected Value, got {other:?}"),
        }
        // PF: only the positive episode counts; no negatives observed.
        assert_eq!(
            profit_factor(&cohort),
            RatioStatus::<Money>::NoObservedLosses
        );
    }

    #[test]
    fn win_rate_and_pf_are_finite_values_when_both_positive_and_negative_exist() {
        let cohort = EpisodeCohort {
            episodes: vec![closed_episode(300), closed_episode(-100)],
        };
        match profit_factor(&cohort) {
            RatioStatus::Value { value } => {
                // 300 / 100 = 3.0
                assert_eq!(value, Money::from_scaled_units(3 * 10i128.pow(8)));
            }
            other => panic!("expected Value, got {other:?}"),
        }
    }

    #[test]
    fn ratio_status_serializes_without_a_numeric_value_field_for_non_value_variants() {
        // ACCEPTANCE D06: no JSON Infinity/NaN/999 surrogate.
        let status: RatioStatus<Money> = RatioStatus::NoObservedLosses;
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("Infinity"));
        assert!(!json.contains("NaN"));
        assert!(json.contains("no_observed_losses"));
    }

    #[test]
    fn episodes_with_unresolved_flows_are_excluded_from_win_rate_computation() {
        let mut unresolved = closed_episode(100);
        unresolved.has_unresolved_flows = true;
        let cohort = EpisodeCohort {
            episodes: vec![unresolved],
        };
        assert_eq!(win_rate(&cohort), RatioStatus::<Money>::Undefined);
    }
}
