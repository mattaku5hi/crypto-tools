//! Episode model. See ARCHITECTURE.md §10: "Episode — для wallet/asset
//! inventory от нулевой позиции до следующего нулевого состояния."

use scout_core::{AssetKey, Money};

/// A wallet/asset inventory episode: zero position -> ... -> zero
/// position (or still open). Per ARCHITECTURE.md §10, a partial sell
/// does not create multiple "won" trades — the episode spans the whole
/// zero-to-zero cycle.
#[derive(Debug, Clone)]
pub struct Episode {
    pub asset: AssetKey,
    pub realized_pnl: Money,
    pub is_closed_within_window: bool,
    /// True if the episode opened before the report window (left-
    /// censored) — tracked separately from the default cohort per
    /// ACCEPTANCE D05.
    pub opened_before_window: bool,
    /// True if any transfer/unknown-basis flow touched this episode's
    /// inventory, making it ineligible for win-rate (ARCHITECTURE.md
    /// §10: "Эпизод с непроверенными transfers/неизвестной себестоимостью
    /// не участвует в win rate").
    pub has_unresolved_flows: bool,
}

impl Episode {
    /// Whether this episode belongs to the **default cohort**: fully
    /// observed, opened and closed within the report window
    /// (ARCHITECTURE.md §10). Left-censored and still-open episodes are
    /// counted separately, never silently merged into this cohort.
    #[must_use]
    pub fn is_default_cohort(&self) -> bool {
        self.is_closed_within_window && !self.opened_before_window
    }

    /// Whether this episode is valid for win-rate/profit-factor
    /// computation at all: default cohort AND no unresolved flows.
    #[must_use]
    pub fn is_valid_for_win_rate(&self) -> bool {
        self.is_default_cohort() && !self.has_unresolved_flows
    }
}

/// A set of episodes for one wallet, with the cohort split already
/// applied so callers never have to re-derive it.
#[derive(Debug, Clone, Default)]
pub struct EpisodeCohort {
    pub episodes: Vec<Episode>,
}

impl EpisodeCohort {
    #[must_use]
    pub fn valid_closed_episodes(&self) -> Vec<&Episode> {
        self.episodes
            .iter()
            .filter(|e| e.is_valid_for_win_rate())
            .collect()
    }

    #[must_use]
    pub fn censored_or_open_count(&self) -> usize {
        self.episodes
            .iter()
            .filter(|e| !e.is_default_cohort())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> AssetKey {
        AssetKey::Native(scout_core::ChainKey {
            family: scout_core::ChainFamily::Evm,
            network_id: scout_core::NetworkId::EvmChainId(8453),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        })
    }

    #[test]
    fn left_censored_episode_is_excluded_from_default_cohort() {
        // ACCEPTANCE D05: episode opened before window is censored, not
        // merged into the default opened-and-closed-in-window cohort.
        let episode = Episode {
            asset: asset(),
            realized_pnl: Money::ZERO,
            is_closed_within_window: true,
            opened_before_window: true,
            has_unresolved_flows: false,
        };
        assert!(!episode.is_default_cohort());
    }

    #[test]
    fn still_open_episode_is_excluded_from_default_cohort() {
        let episode = Episode {
            asset: asset(),
            realized_pnl: Money::ZERO,
            is_closed_within_window: false,
            opened_before_window: false,
            has_unresolved_flows: false,
        };
        assert!(!episode.is_default_cohort());
    }

    #[test]
    fn episode_with_unresolved_flows_excluded_from_win_rate_but_visible_in_cohort() {
        // ARCHITECTURE.md §10: unresolved-basis episode does not
        // participate in win rate, even if otherwise a default-cohort
        // episode.
        let episode = Episode {
            asset: asset(),
            realized_pnl: Money::ZERO,
            is_closed_within_window: true,
            opened_before_window: false,
            has_unresolved_flows: true,
        };
        assert!(episode.is_default_cohort());
        assert!(!episode.is_valid_for_win_rate());
    }

    #[test]
    fn cohort_separates_valid_closed_from_censored_and_open() {
        let cohort = EpisodeCohort {
            episodes: vec![
                Episode {
                    asset: asset(),
                    realized_pnl: Money::ZERO,
                    is_closed_within_window: true,
                    opened_before_window: false,
                    has_unresolved_flows: false,
                },
                Episode {
                    asset: asset(),
                    realized_pnl: Money::ZERO,
                    is_closed_within_window: false,
                    opened_before_window: false,
                    has_unresolved_flows: false,
                },
            ],
        };
        assert_eq!(cohort.valid_closed_episodes().len(), 1);
        assert_eq!(cohort.censored_or_open_count(), 1);
    }
}
