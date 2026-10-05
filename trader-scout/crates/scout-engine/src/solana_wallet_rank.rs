//! `wallet-rank` engine for Solana: a PURE function over the per-wallet
//! results of [`run_solana_wallet_stats`](crate::run_solana_wallet_stats)
//! (ARCHITECTURE §10: "one analyze function; rank and stats use the same
//! result"). No I/O, no clocks, no floats: every gate and every sort
//! comparison is exact integer arithmetic (AGENTS invariant #7).
//!
//! # Quote unit (ADR-013 §5)
//! [`RankPolicy::quote`] (`--quote sol|usdc|usdt`, default `sol`) selects
//! the unit whose known closed episodes feed the PnL / ROI / profit-factor
//! ranking metrics and the `min_closed_episodes` gate. Figures of other
//! units are never mixed in; they stay visible on the report's
//! `unit_blocks`. The unknown-episode share gate stays wallet-wide
//! (conservative).
//!
//! # USD (ADR-018)
//! `RankPolicy::quote = QuoteUnit::ReportCurrency` (`--quote usd`) selects
//! the USD view of each ledger (`SolanaWalletLedgerReport::usd`, applied by
//! `apply_usd_pricing` before ranking): metrics, the `min_closed_episodes`
//! gate, the unknown-episode share gate (USD counts: cross-quote episodes
//! valued in USD are known there) and the win-rate bound all read the USD
//! block, whose "raw" figures are the 1e-8 USD integers. A wallet without a
//! USD view has no metric (`metric_unknown`), never a zero. Failed-tx fees
//! (SOL) are priced at block time into the USD net (ADR-004); open USD unrealized is reported, never ranked.
//!
//! # Contract
//! * Every input wallet lands in exactly one of [`WalletRankReport::ranked`]
//!   or [`WalletRankReport::excluded`]; nothing is dropped silently.
//! * A wallet may fail several gates: ALL reasons are recorded, the
//!   primary one is the first in the fixed [`ExclusionReason`] order:
//!   `provider_error`, `not_scanned`, `incomplete_coverage`, `no_activity`,
//!   `no_pump_activity`, `unknown_episode_share`, `pnl_unbounded`,
//!   `metric_unknown`, `insufficient_closed_episodes`, `insufficient_active_days`,
//!   `activity_unknown`, `activity_ceiling_trades_per_day`,
//!   `activity_ceiling_mints_per_day`, `open_exposure`, `open_exposure_unvalued`,
//!   `below_top_n`.
//!   Wallets whose scan status is not `ok` carry only their status reason
//!   (their figures are a subset or absent, gates on them would mislead);
//!   their observations are still reported.
//! * Unknown is never a number: a wallet whose ranking metric is unknown
//!   goes to the exclusions with `metric_unknown`, it never takes part in
//!   the numeric sort (also under `--profile none`).
//!
//! # Unknown episodes (ADR-016)
//! * Share gate (`quality`/`insider`): a wallet passes iff
//!   `closed_unknown * 100 <= max_unknown_episode_share_percent *
//!   (closed_known + closed_unknown)` (exact integers, all units;
//!   left-censored episodes are in neither count). Default 10 %; `0` is
//!   the old strict rule. Failing it records `unknown_episode_share`.
//!   `has_unknown_basis_inventory` no longer excludes by itself.
//! * Rank keys are worst-case LOWER BOUNDS (every unknown episode is a
//!   loss; PnL bound = -known consumed basis): net PnL, cost ROI
//!   (`lower-bound PnL / (consumed basis + known basis of unknown
//!   episodes)`) and profit factor. An unknown episode can therefore
//!   never raise a wallet's keys.
//! * Tier 1 = bounded wallets (all-known wallets included); tier 2 = a
//!   wallet with an unbounded unknown episode (consumed lot with unknown
//!   basis, or lots of several quote units). Tier 2 sorts after every
//!   tier-1 wallet, by its known-subset values, labelled
//!   `pnl_status = known_subset_unbounded` / `rank_tier = 2`.
//!   [`RankPolicy::exclude_unbounded`] drops tier 2 (`pnl_unbounded`).
//! * Win-rate presentation uses `win_rate_lower_bound`; no gate in this
//!   module reads a win rate.
//!
//! # Sort (ARCHITECTURE §10 "Default rank")
//! Tier 1 before tier 2, then the keys below (lower bounds in tier 1,
//! known-subset values in tier 2).
//! `RealizedNetPnl`: net PnL desc, then `realized_cost_roi` desc (exact
//! rational), then known closed episode count desc, then wallet key asc.
//! `RealizedCostRoi` / `ProfitFactor` put their own metric first and then
//! continue with the same chain. A missing secondary ROI sorts last.
//! `ProfitFactor`: `no_observed_losses` above any finite PF (only gated
//! wallets reach the sort); undefined PF is `metric_unknown`. PF is the
//! analytics `Money`-scale (8 digits, floored) value: ties inside that
//! precision fall through to the next key.
//!
//! # Profiles (research starting policy, NOT statistical guarantees)
//! * `quality` (ARCHITECTURE defaults): >= 20 known closed episodes, >= 7
//!   active UTC days, complete coverage, unknown-episode share <= 10 %.
//! * `insider` (P4.3, concentrated discretionary early buyers): >= 5 known
//!   closed episodes, >= 3 active UTC days, unknown-episode share <= 10 %, plus an
//!   ACTIVITY CEILING of at most 10 distinct mints per active day and at
//!   most 30 trades per active day. P0.7 measured the K>=2 bonding-curve
//!   buyer set as HF/sniper dominated, median about 2,000 tokens/30d, about
//!   67 mints/day; 10 mints/day is roughly a seventh of that median, and
//!   30 trades/day allows a buy, a partial sell and a final sell on each of
//!   10 mints. Both numbers are a starting guess to be tuned on a separate
//!   validation set; they are not derived from outcomes.
//! * Windowed runs (ADR-011): the activity gates are evaluated over the
//!   window; `incomplete` wallets additionally carry the activity-ceiling
//!   reasons when the ceiling is exceeded on fully observed days alone
//!   (evidence only; `incomplete_coverage` stays primary).
//! * `none`: no sample/activity/unknown-share gates. Status gates (scan
//!   not `ok`) and `metric_unknown` still apply, and the `pnl_status`
//!   label (`observed` / `known_subset`) travels with the figure.
//!
//! # Open exposure (ADR-019)
//! The config key `require_resolved_open_exposure=true` and ARCHITECTURE
//! ("unresolved open exposure may exclude") would exclude every wallet
//! with an open position. Default here: INCLUDE such wallets and flag
//! `open_exposure` with the position count, the raw amounts and, when the
//! realizable valuation ran (`SolanaWalletLedgerReport::open_valuation`,
//! live runs only), the valued/unvalued counts and totals.
//! [`RankPolicy::require_no_open`] (`--require-no-open`) excludes every
//! wallet with an open position (`open_exposure`);
//! [`RankPolicy::require_valued_open`] (`--require-valued-open`) excludes
//! wallets with ANY open position that has no realizable value
//! (`open_exposure_unvalued`, also when valuation was not run).
//! Unrealized values are reported next to the rank, NEVER inside a rank
//! key, gate or sort: the rank layer reads realized figures only. An open
//! position with unknown basis is only reported (ADR-016).
//!
//! # Activity metrics
//! Trades/mints per active day use only trades with a verified event
//! timestamp ([`ActivityMetrics`]); untimed trades are not counted, so the
//! ceiling is a lower-bound check.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use scout_analytics::RatioStatus;
use scout_core::{MONEY_SCALE, Money, SolanaPubkey};

use crate::chain_display::ChainDisplay;
use crate::evm_open_valuation::{EvmOpenValuationTotals, EvmOpenValuationView};
use crate::solana_open_valuation::{OpenValuationTotals, OpenValuationView};
use crate::solana_wallet_ledger::{
    LowerBound, OpenPosition, QuoteUnit, SolanaWalletLedgerReport, WinRateLowerBound,
    money_to_unit_raw, quote_units_to_money,
};
use crate::solana_wallet_stats::{SolanaWalletStats, WalletScanStatus};

/// Default `--top`.
pub const DEFAULT_TOP: usize = 20;

/// ADR-016 default `--max-unknown-episode-share` (percent).
pub const DEFAULT_MAX_UNKNOWN_EPISODE_SHARE_PERCENT: u8 = 10;

/// Version tag of the ranking rules for report metadata.
pub const SOLANA_WALLET_RANK_VERSION: &str = "solana-wallet-rank/6 (ADR-004 --quote usd net = USD realized - USD failed-tx fees, USD unrealized in open exposure, ADR-019 open-exposure valuation totals + --require-valued-open, ADR-013 --quote, ADR-016 unknown-episode lower bounds, ADR-018 --quote usd)";

/// Ranking metric. `period-equity-pnl` needs a price source (P5.2) and is
/// deliberately not representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RankBy {
    #[default]
    RealizedNetPnl,
    RealizedCostRoi,
    ProfitFactor,
}

impl RankBy {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RealizedNetPnl => "realized-net-pnl",
            Self::RealizedCostRoi => "realized-cost-roi",
            Self::ProfitFactor => "profit-factor",
        }
    }
}

/// Gate profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RankProfile {
    #[default]
    Quality,
    Insider,
    None,
}

impl RankProfile {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Insider => "insider",
            Self::None => "none",
        }
    }
}

/// Effective policy: profile defaults with explicit overrides applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankPolicy {
    pub rank_by: RankBy,
    pub profile: RankProfile,
    /// Known closed episodes required (0 = no gate).
    pub min_closed_episodes: u64,
    /// Distinct active UTC days required (0 = no gate).
    pub min_active_days: u64,
    /// Ceiling on timestamped trades per active day (`None` = no ceiling).
    pub max_trades_per_day: Option<u64>,
    /// Ceiling on distinct mints per active day (`None` = no ceiling).
    pub max_mints_per_day: Option<u64>,
    /// ADR-016: apply the unknown-episode share gate (`quality`/`insider`).
    /// Left-censoring (ADR-011 §5) is in neither count.
    pub unknown_share_gate: bool,
    /// ADR-016: maximum `closed_unknown / (closed_known + closed_unknown)`
    /// in integer percent (0..=100); 0 reproduces the strict rule.
    pub max_unknown_episode_share_percent: u8,
    /// ADR-016: drop tier-2 wallets (`pnl_unbounded`).
    pub exclude_unbounded: bool,
    /// Strict variant: exclude wallets with any open position.
    pub require_no_open: bool,
    /// ADR-019: exclude wallets with any open position that has no
    /// realizable value (`open_exposure_unvalued`).
    pub require_valued_open: bool,
    /// Maximum number of ranked wallets (>= 1).
    pub top: usize,
    /// ADR-013 §5: quote unit of the ranking metrics and the episode gate.
    pub quote: QuoteUnit,
}

impl RankPolicy {
    /// Profile defaults (see the module docs for the numbers).
    #[must_use]
    pub fn for_profile(profile: RankProfile, rank_by: RankBy, top: usize) -> Self {
        let (min_closed_episodes, min_active_days, max_trades, max_mints, unknown) = match profile {
            RankProfile::Quality => (20, 7, None, None, true),
            RankProfile::Insider => (5, 3, Some(30), Some(10), true),
            RankProfile::None => (0, 0, None, None, false),
        };
        Self {
            rank_by,
            profile,
            min_closed_episodes,
            min_active_days,
            max_trades_per_day: max_trades,
            max_mints_per_day: max_mints,
            unknown_share_gate: unknown,
            max_unknown_episode_share_percent: DEFAULT_MAX_UNKNOWN_EPISODE_SHARE_PERCENT,
            exclude_unbounded: false,
            require_no_open: false,
            require_valued_open: false,
            top: top.max(1),
            quote: QuoteUnit::Lamports,
        }
    }

    /// The same policy ranking in `quote`.
    #[must_use]
    pub fn with_quote(mut self, quote: QuoteUnit) -> Self {
        self.quote = quote;
        self
    }
}

impl Default for RankPolicy {
    fn default() -> Self {
        Self::for_profile(RankProfile::Quality, RankBy::RealizedNetPnl, DEFAULT_TOP)
    }
}

/// Why a wallet is not in the ranking. Declaration order IS the primary
/// reason order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExclusionReason {
    /// Multi-chain run: the requested native/stable `--quote` unit does not
    /// exist on the wallet's chain, so the wallet is not ranked in it (and
    /// is not scanned for it).
    QuoteUnitNotOnChain,
    ProviderError,
    /// The run stopped (budget / terminal rate limit) before this wallet.
    NotScanned,
    IncompleteCoverage,
    NoActivity,
    NoPumpActivity,
    /// ADR-020: an EVM wallet with transactions but no booked trade.
    NoTradeActivity,
    /// ADR-016: too many `ClosedUnknown` episodes.
    UnknownEpisodeShare,
    /// ADR-016: `--exclude-unbounded` and the wallet is tier 2.
    PnlUnbounded,
    MetricUnknown,
    InsufficientClosedEpisodes,
    InsufficientActiveDays,
    /// A per-day ceiling is set but the wallet has no timestamped trades.
    ActivityUnknown,
    ActivityCeilingTradesPerDay,
    ActivityCeilingMintsPerDay,
    OpenExposure,
    /// ADR-019: `--require-valued-open` and an open position has no value.
    OpenExposureUnvalued,
    /// Passed every gate but is below the `--top` cut.
    BelowTopN,
}

impl ExclusionReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::QuoteUnitNotOnChain => "quote_unit_not_on_chain",
            Self::ProviderError => "provider_error",
            Self::NotScanned => "not_scanned",
            Self::IncompleteCoverage => "incomplete_coverage",
            Self::NoActivity => "no_activity",
            Self::NoPumpActivity => "no_pump_activity",
            Self::NoTradeActivity => "no_trade_activity",
            Self::UnknownEpisodeShare => "unknown_episode_share",
            Self::PnlUnbounded => "pnl_unbounded",
            Self::MetricUnknown => "metric_unknown",
            Self::InsufficientClosedEpisodes => "insufficient_closed_episodes",
            Self::InsufficientActiveDays => "insufficient_active_days",
            Self::ActivityUnknown => "activity_unknown",
            Self::ActivityCeilingTradesPerDay => "activity_ceiling_trades_per_day",
            Self::ActivityCeilingMintsPerDay => "activity_ceiling_mints_per_day",
            Self::OpenExposure => "open_exposure",
            Self::OpenExposureUnvalued => "open_exposure_unvalued",
            Self::BelowTopN => "below_top_n",
        }
    }
}

/// Exact rational `numerator / denominator` with `denominator > 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio {
    pub numerator: i128,
    pub denominator: i128,
}

impl Ratio {
    /// `None` when the denominator is not positive.
    #[must_use]
    pub fn new(numerator: i128, denominator: i128) -> Option<Self> {
        (denominator > 0).then_some(Self {
            numerator,
            denominator,
        })
    }

    /// Exact comparison by cross-multiplication in 256-bit arithmetic.
    #[must_use]
    pub fn cmp_exact(&self, other: &Self) -> Ordering {
        let l = widening_mul(self.numerator, other.denominator);
        let r = widening_mul(other.numerator, self.denominator);
        cmp_wide(l, r)
    }

    /// `numerator * 100 / denominator` rounded half up (away from zero for
    /// negatives) to `places` digits, as a decimal string; `None` on
    /// overflow of the intermediate (presentation only).
    #[must_use]
    pub fn percent_string(&self, places: u32) -> Option<String> {
        let scale = 10u128.checked_pow(places)?.checked_mul(100)?;
        let mag = self.numerator.unsigned_abs().checked_mul(scale)?;
        let den = self.denominator.unsigned_abs();
        let q = mag
            .checked_mul(2)?
            .checked_add(den)?
            .div_euclid(den.checked_mul(2)?);
        let q = i128::try_from(q).ok()?;
        let signed = if self.numerator < 0 { -q } else { q };
        Some(crate::solana_wallet_stats::format_scaled_decimal(
            signed, places,
        ))
    }
}

/// Signed 256-bit product as (negative, high, low) magnitude.
type Wide = (bool, u128, u128);

fn widening_mul(a: i128, b: i128) -> Wide {
    let (x, y) = (a.unsigned_abs(), b.unsigned_abs());
    const M: u128 = (1u128 << 64) - 1;
    let (x1, x0) = (x >> 64, x & M);
    let (y1, y0) = (y >> 64, y & M);
    let ll = x0 * y0;
    let m1 = x1 * y0;
    let m2 = x0 * y1;
    let hh = x1 * y1;
    let mid = (ll >> 64) + (m1 & M) + (m2 & M);
    let lo = (ll & M) | ((mid & M) << 64);
    let hi = hh + (m1 >> 64) + (m2 >> 64) + (mid >> 64);
    let zero = hi == 0 && lo == 0;
    ((a < 0) != (b < 0) && !zero, hi, lo)
}

fn cmp_wide(a: Wide, b: Wide) -> Ordering {
    match (a.0, b.0) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (neg, _) => {
            let mag = (a.1, a.2).cmp(&(b.1, b.2));
            if neg { mag.reverse() } else { mag }
        }
    }
}

/// Headline PnL classification (mirrors the wallet-stats card).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PnlStatus {
    /// Complete coverage and no `ClosedUnknown` episode.
    Observed,
    /// Known closed episodes only (incomplete coverage and/or unknown
    /// episodes whose worst case is bounded, ADR-016).
    KnownSubset,
    /// ADR-016: an unknown episode has no worst-case bound; the figure is
    /// the known subset only (tier 2).
    KnownSubsetUnbounded,
    /// Nothing known to report; never rendered as zero.
    NotAvailable,
}

impl PnlStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::KnownSubset => "known_subset",
            Self::KnownSubsetUnbounded => "known_subset_unbounded",
            Self::NotAvailable => "n_a",
        }
    }
}

/// Open exposure flag (ADR-019). Valuation is information only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenExposure {
    None,
    Open {
        positions: u64,
        positions_unknown_basis: u64,
        details: Vec<OpenPosition>,
        /// The realizable valuation of these positions; `None` = not run.
        valuation: Option<OpenValuationView>,
        /// ADR-019 EVM amendment: the exit-quote valuation of an EVM wallet.
        evm_valuation: Option<EvmOpenValuationView>,
    },
}

impl OpenExposure {
    /// `none`, `unvalued` (no valuation, or no position valued),
    /// `partially_valued` or `valued`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Open {
                valuation,
                evm_valuation,
                ..
            } => {
                let (positions, valued) = match (valuation, evm_valuation) {
                    (Some(v), _) => {
                        let t = v.totals();
                        (t.positions, t.valued)
                    }
                    (None, Some(v)) => {
                        let t = v.totals();
                        (t.positions, t.valued)
                    }
                    (None, None) => return "unvalued",
                };
                if positions > 0 && valued == positions {
                    "valued"
                } else if valued == 0 {
                    "unvalued"
                } else {
                    "partially_valued"
                }
            }
        }
    }

    /// EVM exit-quote totals (`None` for Solana or without a valuation).
    #[must_use]
    pub fn evm_totals(&self) -> Option<EvmOpenValuationTotals> {
        match self {
            Self::Open {
                evm_valuation: Some(v),
                ..
            } => Some(v.totals()),
            _ => None,
        }
    }

    /// Valuation totals (`None` without a valuation).
    #[must_use]
    pub fn totals(&self) -> Option<OpenValuationTotals> {
        match self {
            Self::Open {
                valuation: Some(v), ..
            } => Some(v.totals()),
            _ => None,
        }
    }
}

/// Everything the rank layer observed about one wallet (ranked or not).
#[derive(Debug, Clone)]
pub struct WalletRankObservation {
    pub wallet: SolanaPubkey,
    /// Chain of the wallet key / units (ADR-020).
    pub chain: ChainDisplay,
    pub status: WalletScanStatus,
    pub transactions_scanned: Option<u64>,
    /// Sanitized scan error text.
    pub error: Option<String>,
    pub incomplete_reasons: Vec<String>,
    /// Present unless the scan failed.
    pub ledger: Option<SolanaWalletLedgerReport>,
    pub pnl_status: PnlStatus,
    /// Quote unit of `net_pnl_raw`, `roi` and the closed-episode gate.
    pub quote: QuoteUnit,
    /// Realized net PnL in raw base units of `quote` (lamports for SOL,
    /// 6-dp units for USDC/USDT; net of failed-trade fees for SOL and USD)
    /// when known; `None` is N/A, not zero.
    pub net_pnl_raw: Option<i128>,
    /// `realized_trade_pnl / consumed_acquisition_basis` of `quote` (exact,
    /// scaled `Money` units on both sides); `None` when undefined.
    pub roi: Option<Ratio>,
    pub open_exposure: OpenExposure,
    /// ADR-016: 1 = bounded (all-known included), 2 = unbounded unknown episode.
    pub rank_tier: u8,
    /// ADR-016: worst-case net PnL (raw units of `quote`); `None` when
    /// `net_pnl_raw` is N/A. Equals `net_pnl_raw` without unknown episodes.
    pub net_pnl_lower_bound: Option<LowerBound<i128>>,
    /// ADR-016: worst-case ROI; `None` when unbounded or undefined.
    pub roi_lower_bound: Option<Ratio>,
    /// ADR-016: worst-case profit factor in `quote`; `None` without a block.
    pub pf_lower_bound: Option<LowerBound<RatioStatus<Money>>>,
    /// ADR-016: `wins / (closed_known + closed_unknown)`.
    pub win_rate_lower_bound: Option<WinRateLowerBound>,
    /// ADR-016 sort keys: lower bounds in tier 1, known-subset values in
    /// tier 2.
    pub key_net: Option<i128>,
    pub key_roi: Option<Ratio>,
    pub key_pf: Option<RatioStatus<Money>>,
}

#[derive(Debug, Clone)]
pub struct RankedWallet {
    /// 1-based.
    pub rank: usize,
    pub observation: WalletRankObservation,
}

#[derive(Debug, Clone)]
pub struct ExcludedWallet {
    /// Non-empty, in [`ExclusionReason`] order; `reasons[0]` is primary.
    pub reasons: Vec<ExclusionReason>,
    /// For `below_top_n`: the 1-based rank it would have had.
    pub eligible_rank: Option<usize>,
    pub observation: WalletRankObservation,
}

impl ExcludedWallet {
    #[must_use]
    pub fn primary_reason(&self) -> ExclusionReason {
        self.reasons
            .first()
            .copied()
            .unwrap_or(ExclusionReason::MetricUnknown)
    }
}

#[derive(Debug, Clone)]
pub struct WalletRankReport {
    pub policy: RankPolicy,
    pub ranked: Vec<RankedWallet>,
    /// Input order.
    pub excluded: Vec<ExcludedWallet>,
    /// Wallets that passed every gate (before `--top`).
    pub eligible_count: usize,
    pub input_count: usize,
}

impl WalletRankReport {
    /// Exclusions counted by PRIMARY reason (partition of `excluded`).
    #[must_use]
    pub fn primary_reason_counts(&self) -> BTreeMap<ExclusionReason, usize> {
        let mut m = BTreeMap::new();
        for e in &self.excluded {
            *m.entry(e.primary_reason()).or_insert(0) += 1;
        }
        m
    }

    /// Exclusions counted by EVERY reason (a wallet counts once per reason).
    #[must_use]
    pub fn all_reason_counts(&self) -> BTreeMap<ExclusionReason, usize> {
        let mut m = BTreeMap::new();
        for e in &self.excluded {
            for r in &e.reasons {
                *m.entry(*r).or_insert(0) += 1;
            }
        }
        m
    }
}

/// Known closed episodes of `unit` (0 when the report has no block).
fn unit_closed_known(l: &SolanaWalletLedgerReport, unit: QuoteUnit) -> u64 {
    l.unit_block(unit).map_or(0, |b| b.closed_episodes_known)
}

fn observe(w: &SolanaWalletStats, quote: QuoteUnit) -> WalletRankObservation {
    let mut obs = WalletRankObservation {
        wallet: w.wallet,
        chain: w.chain,
        status: w.status,
        transactions_scanned: w.transactions_scanned,
        error: w.error.clone(),
        incomplete_reasons: w.incomplete_reasons.clone(),
        ledger: w.ledger.clone(),
        pnl_status: PnlStatus::NotAvailable,
        quote,
        net_pnl_raw: None,
        roi: None,
        open_exposure: OpenExposure::None,
        rank_tier: 1,
        net_pnl_lower_bound: None,
        roi_lower_bound: None,
        pf_lower_bound: None,
        win_rate_lower_bound: None,
        key_net: None,
        key_roi: None,
        key_pf: None,
    };
    let Some(l) = &w.ledger else {
        return obs;
    };
    let block = l.unit_block(quote);
    let closed = unit_closed_known(l, quote);
    // ADR-018: under `--quote usd` the unknown counts are the USD view's.
    let (closed_unknown, win_lb) = if quote == QuoteUnit::ReportCurrency {
        l.usd.as_ref().map_or((0, None), |u| {
            (u.closed_episodes_unknown, u.win_rate_lower_bound)
        })
    } else {
        (l.closed_episodes_unknown, l.win_rate_lower_bound)
    };
    // The chain's native unit (lamports / wei): its net is fee-inclusive.
    let native = quote == l.quote_unit;
    let net = if native {
        // Failed-trade fees are native overhead (ADR-004): native only.
        (closed > 0 || l.failed_trade_fees_lamports != 0).then_some(l.realized_net_pnl_lamports)
    } else if quote == QuoteUnit::ReportCurrency {
        // ADR-004 in USD: realized - priced failed fees; unpriced fees make
        // the net unknown (never zero).
        l.usd.as_ref().and_then(|u| {
            let fee_txs = u.failed_fees.priced_txs + u.failed_fees.unpriced_txs;
            (closed > 0 || fee_txs > 0)
                .then_some(u.net.value)
                .flatten()
                .map(|m| m.scaled_units())
        })
    } else {
        block
            .filter(|_| closed > 0)
            .map(|b| b.realized_trade_pnl_raw)
    };
    let usd_fees_incomplete = quote == QuoteUnit::ReportCurrency
        && l.usd
            .as_ref()
            .is_some_and(|u| u.net.status != crate::solana_wallet_usd::UsdNetStatus::Known);
    let unbounded = block.is_some_and(|b| b.unknown_pnl_bound == LowerBound::Unbounded);
    let zero_bound = block.is_none_or(|b| b.unknown_pnl_bound == LowerBound::Bounded(Money::ZERO));
    obs.rank_tier = if unbounded { 2 } else { 1 };
    obs.pnl_status = match net {
        None => PnlStatus::NotAvailable,
        Some(_) if usd_fees_incomplete => PnlStatus::KnownSubset,
        Some(_) if w.coverage_complete() && closed_unknown == 0 => PnlStatus::Observed,
        Some(_) if unbounded => PnlStatus::KnownSubsetUnbounded,
        Some(_) => PnlStatus::KnownSubset,
    };
    obs.net_pnl_raw = net;
    obs.roi = block.filter(|_| closed > 0).and_then(|b| {
        Ratio::new(
            b.realized_trade_pnl_exact.scaled_units(),
            b.consumed_acquisition_basis_exact.scaled_units(),
        )
    });
    // Lower bounds (ADR-016). Without unknown episodes they equal the
    // known figures exactly.
    obs.net_pnl_lower_bound = net.map(|n| {
        if unbounded || usd_fees_incomplete {
            return LowerBound::Unbounded;
        }
        if zero_bound {
            return LowerBound::Bounded(n);
        }
        let pnl_lb = block.and_then(|b| b.realized_pnl_lower_bound().bounded().copied());
        let failed = if native {
            quote_units_to_money(l.quote_unit, l.failed_trade_fees_lamports).ok()
        } else if quote == QuoteUnit::ReportCurrency {
            l.usd.as_ref().map(|u| u.failed_fees.priced_usd)
        } else {
            Some(Money::ZERO)
        };
        match pnl_lb.zip(failed).and_then(|(m, f)| m.checked_sub(&f).ok()) {
            Some(m) => LowerBound::Bounded(money_to_unit_raw(quote, m)),
            None => LowerBound::Unbounded,
        }
    });
    obs.roi_lower_bound = block.filter(|_| closed > 0).and_then(|b| {
        if unbounded {
            return None;
        }
        if zero_bound {
            return obs.roi;
        }
        let num = b.realized_pnl_lower_bound().bounded().copied()?;
        let den = b.consumed_basis_with_unknown_exact()?;
        Ratio::new(num.scaled_units(), den.scaled_units())
    });
    let base_pf = if native {
        Some(l.profit_factor)
    } else {
        block.map(|b| b.profit_factor)
    };
    obs.pf_lower_bound = block
        .zip(base_pf)
        .map(|(b, base)| b.profit_factor_lower_bound_from(base));
    obs.win_rate_lower_bound = win_lb;
    if unbounded {
        obs.key_net = net;
        obs.key_roi = obs.roi;
        obs.key_pf = base_pf;
    } else {
        obs.key_net = match obs.net_pnl_lower_bound {
            Some(LowerBound::Bounded(v)) => Some(v),
            _ => None,
        };
        obs.key_roi = obs.roi_lower_bound;
        obs.key_pf = match obs.pf_lower_bound {
            Some(LowerBound::Bounded(v)) => Some(v),
            _ => None,
        };
    }
    obs.open_exposure = if l.open_positions.is_empty() {
        OpenExposure::None
    } else {
        OpenExposure::Open {
            positions: u64::try_from(l.open_positions.len()).unwrap_or(u64::MAX),
            positions_unknown_basis: l.open_positions_with_unknown_basis,
            details: l.open_positions.clone(),
            valuation: l.open_valuation.clone(),
            evm_valuation: l.evm.as_ref().and_then(|e| e.open_valuation.clone()),
        }
    };
    obs
}

fn status_reason(status: WalletScanStatus) -> Option<ExclusionReason> {
    match status {
        WalletScanStatus::Ok => None,
        WalletScanStatus::Error => Some(ExclusionReason::ProviderError),
        WalletScanStatus::NotScanned => Some(ExclusionReason::NotScanned),
        WalletScanStatus::Incomplete => Some(ExclusionReason::IncompleteCoverage),
        WalletScanStatus::NoActivity => Some(ExclusionReason::NoActivity),
        WalletScanStatus::NoPumpActivity => Some(ExclusionReason::NoPumpActivity),
        WalletScanStatus::NoTradeActivity => Some(ExclusionReason::NoTradeActivity),
    }
}

/// Ranking-metric key of a gated wallet; `None` = unknown for this metric.
#[derive(Debug, Clone, Copy)]
enum PfKey {
    Finite(i128),
    Unbounded,
}

fn pf_key(pf: RatioStatus<Money>) -> Option<PfKey> {
    match pf {
        RatioStatus::Value { value } => Some(PfKey::Finite(value.scaled_units())),
        RatioStatus::NoObservedLosses => Some(PfKey::Unbounded),
        RatioStatus::Undefined => None,
    }
}

fn cmp_pf(a: PfKey, b: PfKey) -> Ordering {
    match (a, b) {
        (PfKey::Unbounded, PfKey::Unbounded) => Ordering::Equal,
        (PfKey::Unbounded, PfKey::Finite(_)) => Ordering::Greater,
        (PfKey::Finite(_), PfKey::Unbounded) => Ordering::Less,
        (PfKey::Finite(x), PfKey::Finite(y)) => x.cmp(&y),
    }
}

fn cmp_opt_roi(a: Option<Ratio>, b: Option<Ratio>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp_exact(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

fn closed_known(o: &WalletRankObservation) -> u64 {
    o.ledger
        .as_ref()
        .map_or(0, |l| unit_closed_known(l, o.quote))
}

/// Descending comparator (best first) for the chosen metric.
fn cmp_best_first(by: RankBy, a: &WalletRankObservation, b: &WalletRankObservation) -> Ordering {
    let net = |o: &WalletRankObservation| o.key_net;
    let by_net = || match (net(a), net(b)) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    let by_roi = || cmp_opt_roi(b.key_roi, a.key_roi);
    let primary = match by {
        RankBy::RealizedNetPnl => by_net().then_with(by_roi),
        RankBy::RealizedCostRoi => by_roi().then_with(by_net),
        RankBy::ProfitFactor => {
            let key = |o: &WalletRankObservation| o.key_pf.and_then(pf_key);
            let pf = match (key(a), key(b)) {
                (Some(x), Some(y)) => cmp_pf(y, x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            pf.then_with(by_net).then_with(by_roi)
        }
    };
    // ADR-016: bounded wallets (tier 1) always precede unbounded ones.
    a.rank_tier
        .cmp(&b.rank_tier)
        .then(primary)
        .then_with(|| closed_known(b).cmp(&closed_known(a)))
        .then_with(|| a.wallet.cmp(&b.wallet))
        .then_with(|| a.chain.name.cmp(b.chain.name))
}

fn metric_known(by: RankBy, o: &WalletRankObservation) -> bool {
    match by {
        RankBy::RealizedNetPnl => o.key_net.is_some(),
        RankBy::RealizedCostRoi => o.key_roi.is_some(),
        RankBy::ProfitFactor => o.key_pf.and_then(pf_key).is_some(),
    }
}

/// ADR-016: `closed_unknown * 100 > percent * (closed_known +
/// closed_unknown)`, exact in `u128`.
fn exceeds_unknown_share(policy: &RankPolicy, l: &SolanaWalletLedgerReport) -> bool {
    // ADR-018: under `--quote usd` the share is the USD view's (cross-quote
    // episodes valued in USD are known there); no view = nothing is known.
    let (unknown, total) = if policy.quote == QuoteUnit::ReportCurrency {
        l.usd
            .as_ref()
            .map_or((0, 0), |u| u.unknown_episode_share_parts())
    } else {
        l.unknown_episode_share_parts()
    };
    let pct = u128::from(policy.max_unknown_episode_share_percent.min(100));
    u128::from(unknown) * 100 > pct * u128::from(total)
}

/// `count / days > max` by cross-multiplication (`days > 0`).
fn exceeds_per_day(count: u64, days: u64, max: u64) -> bool {
    u128::from(count) > u128::from(max) * u128::from(days)
}

/// ADR-011 §6: activity-ceiling evidence for an `incomplete` wallet. Every
/// observed UTC day except the oldest observed one is fully observed, so
/// the per-day count over those days is exact; an exceeded ceiling there is
/// recorded in addition to `incomplete_coverage` (it never makes the wallet
/// eligible). Days are the days with a timestamped trade, as in the
/// complete-window metric.
fn incomplete_ceiling_evidence(
    policy: &RankPolicy,
    l: &SolanaWalletLedgerReport,
) -> Vec<ExclusionReason> {
    let full: Vec<_> = l.daily_activity.iter().skip(1).collect();
    let days = u64::try_from(full.len()).unwrap_or(u64::MAX);
    if days == 0 {
        return Vec::new();
    }
    let trades: u64 = full.iter().map(|d| d.trades).sum();
    let pairs: u64 = full.iter().map(|d| d.distinct_mints).sum();
    let mut out = Vec::new();
    if let Some(max) = policy.max_trades_per_day
        && exceeds_per_day(trades, days, max)
    {
        out.push(ExclusionReason::ActivityCeilingTradesPerDay);
    }
    if let Some(max) = policy.max_mints_per_day
        && exceeds_per_day(pairs, days, max)
    {
        out.push(ExclusionReason::ActivityCeilingMintsPerDay);
    }
    out
}

fn gate_reasons(policy: &RankPolicy, o: &WalletRankObservation) -> Vec<ExclusionReason> {
    if let Some(r) = status_reason(o.status) {
        let mut out = vec![r];
        if o.status == WalletScanStatus::Incomplete
            && let Some(l) = &o.ledger
        {
            out.extend(incomplete_ceiling_evidence(policy, l));
        }
        return out;
    }
    let Some(l) = &o.ledger else {
        return vec![ExclusionReason::MetricUnknown];
    };
    let mut out = Vec::new();
    if policy.unknown_share_gate && exceeds_unknown_share(policy, l) {
        out.push(ExclusionReason::UnknownEpisodeShare);
    }
    if policy.exclude_unbounded && o.rank_tier == 2 {
        out.push(ExclusionReason::PnlUnbounded);
    }
    if !metric_known(policy.rank_by, o) {
        out.push(ExclusionReason::MetricUnknown);
    }
    if unit_closed_known(l, o.quote) < policy.min_closed_episodes {
        out.push(ExclusionReason::InsufficientClosedEpisodes);
    }
    let a = &l.activity;
    if a.active_utc_days < policy.min_active_days {
        out.push(ExclusionReason::InsufficientActiveDays);
    }
    let ceiling = policy.max_trades_per_day.is_some() || policy.max_mints_per_day.is_some();
    if ceiling && a.active_utc_days == 0 {
        out.push(ExclusionReason::ActivityUnknown);
    } else {
        if let Some(max) = policy.max_trades_per_day
            && exceeds_per_day(a.timestamped_trades, a.active_utc_days, max)
        {
            out.push(ExclusionReason::ActivityCeilingTradesPerDay);
        }
        if let Some(max) = policy.max_mints_per_day
            && exceeds_per_day(a.mint_day_pairs, a.active_utc_days, max)
        {
            out.push(ExclusionReason::ActivityCeilingMintsPerDay);
        }
    }
    if policy.require_no_open && !l.open_positions.is_empty() {
        out.push(ExclusionReason::OpenExposure);
    }
    if policy.require_valued_open && !l.open_positions.is_empty() {
        let evm = l.evm.as_ref().and_then(|e| e.open_valuation.as_ref());
        let all_valued = match evm {
            Some(v) => v.all_valued() && v.positions.len() == l.open_positions.len(),
            None => l
                .open_valuation
                .as_ref()
                .is_some_and(|v| v.all_valued() && v.positions.len() == l.open_positions.len()),
        };
        if !all_valued {
            out.push(ExclusionReason::OpenExposureUnvalued);
        }
    }
    out
}

/// Rank `wallets` (one entry per distinct wallet, as produced by the
/// stats engine) under `policy`. Deterministic; see the module docs.
#[must_use]
pub fn rank_solana_wallets(wallets: &[SolanaWalletStats], policy: &RankPolicy) -> WalletRankReport {
    let top = policy.top.max(1);
    let mut eligible: Vec<WalletRankObservation> = Vec::new();
    // (input index, excluded) so the final list keeps input order.
    let mut excluded: Vec<(usize, ExcludedWallet)> = Vec::new();
    for (idx, w) in wallets.iter().enumerate() {
        let obs = observe(w, policy.quote);
        let mut reasons = gate_reasons(policy, &obs);
        reasons.sort();
        reasons.dedup();
        if reasons.is_empty() {
            eligible.push(obs);
        } else {
            excluded.push((
                idx,
                ExcludedWallet {
                    reasons,
                    eligible_rank: None,
                    observation: obs,
                },
            ));
        }
    }
    // Invariant 3: the same address on two chains is two wallets.
    let input_index = |o: &WalletRankObservation| {
        wallets
            .iter()
            .position(|w| w.wallet == o.wallet && w.chain == o.chain)
    };
    eligible.sort_by(|a, b| cmp_best_first(policy.rank_by, a, b));
    let eligible_count = eligible.len();
    let mut ranked = Vec::new();
    for (i, obs) in eligible.into_iter().enumerate() {
        let rank = i + 1;
        if rank <= top {
            ranked.push(RankedWallet {
                rank,
                observation: obs,
            });
        } else {
            let idx = input_index(&obs).unwrap_or(usize::MAX);
            excluded.push((
                idx,
                ExcludedWallet {
                    reasons: vec![ExclusionReason::BelowTopN],
                    eligible_rank: Some(rank),
                    observation: obs,
                },
            ));
        }
    }
    excluded.sort_by_key(|(idx, _)| *idx);
    WalletRankReport {
        policy: *policy,
        ranked,
        excluded: excluded.into_iter().map(|(_, e)| e).collect(),
        eligible_count,
        input_count: wallets.len(),
    }
}

/// Multi-chain native-unit ranking: every wallet of a chain on which the
/// requested quote unit does not exist is excluded (never ranked, never
/// scanned) with the single reason [`ExclusionReason::QuoteUnitNotOnChain`].
/// `wallets` are the unscanned cards of that chain.
#[must_use]
pub fn exclude_quote_unit_not_on_chain(
    wallets: &[SolanaWalletStats],
    policy: &RankPolicy,
) -> WalletRankReport {
    WalletRankReport {
        policy: *policy,
        ranked: Vec::new(),
        excluded: wallets
            .iter()
            .map(|w| ExcludedWallet {
                reasons: vec![ExclusionReason::QuoteUnitNotOnChain],
                eligible_rank: None,
                observation: observe(w, policy.quote),
            })
            .collect(),
        eligible_count: 0,
        input_count: wallets.len(),
    }
}

/// Render exact `Money` units as SOL with `MONEY_SCALE + 9` digits.
#[must_use]
pub fn money_exact_sol_string(m: Money) -> String {
    crate::solana_wallet_stats::format_scaled_decimal(m.scaled_units(), MONEY_SCALE + 9)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i128, d: i128) -> Ratio {
        Ratio::new(n, d).unwrap()
    }

    #[test]
    fn ratio_comparison_is_exact() {
        // 1/3 > 333333/1000000 although both round to 0.3333 at 4 digits.
        assert_eq!(r(1, 3).cmp_exact(&r(333_333, 1_000_000)), Ordering::Greater);
        assert_eq!(r(2, 6).cmp_exact(&r(1, 3)), Ordering::Equal);
        assert_eq!(r(-1, 3).cmp_exact(&r(-333_333, 1_000_000)), Ordering::Less);
        assert_eq!(r(-1, 3).cmp_exact(&r(1, 1_000_000_000)), Ordering::Less);
        assert_eq!(r(0, 5).cmp_exact(&r(0, 7)), Ordering::Equal);
        // Products beyond i128 range still compare correctly.
        let big = i128::MAX;
        assert_eq!(
            r(big, big - 1).cmp_exact(&r(big - 1, big - 2)),
            Ordering::Less
        );
        assert_eq!(r(big, 1).cmp_exact(&r(big - 1, 1)), Ordering::Greater);
        assert_eq!(r(i128::MIN + 1, 1).cmp_exact(&r(-big, 1)), Ordering::Equal);
        assert!(Ratio::new(1, 0).is_none() && Ratio::new(1, -2).is_none());
    }

    #[test]
    fn widening_mul_matches_i128_where_it_fits() {
        for (a, b) in [
            (0i128, 5),
            (-7, 9),
            (123_456_789, -987_654_321),
            (1 << 40, 1 << 40),
        ] {
            let (neg, hi, lo) = widening_mul(a, b);
            assert_eq!(hi, 0);
            let v = i128::try_from(lo).unwrap();
            assert_eq!(if neg { -v } else { v }, a * b);
        }
    }

    #[test]
    fn percent_rounds_half_up_with_sign() {
        assert_eq!(r(1, 3).percent_string(2).unwrap(), "33.33");
        assert_eq!(r(2, 3).percent_string(2).unwrap(), "66.67");
        assert_eq!(r(-157_000, 1_020_000).percent_string(2).unwrap(), "-15.39");
        assert_eq!(r(0, 9).percent_string(2).unwrap(), "0.00");
    }

    #[test]
    fn per_day_ceiling_is_integer_cross_multiplication() {
        assert!(!exceeds_per_day(30, 1, 30));
        assert!(exceeds_per_day(31, 1, 30));
        assert!(!exceeds_per_day(100, 4, 25));
        assert!(exceeds_per_day(101, 4, 25));
    }
}
