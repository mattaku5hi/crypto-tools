//! `wallet-rank` engine for Solana: a PURE function over the per-wallet
//! results of [`run_solana_wallet_stats`](crate::run_solana_wallet_stats)
//! (ARCHITECTURE §10: "one analyze function; rank and stats use the same
//! result"). No I/O, no clocks, no floats: every gate and every sort
//! comparison is exact integer arithmetic (AGENTS invariant #7).
//!
//! # Contract
//! * Every input wallet lands in exactly one of [`WalletRankReport::ranked`]
//!   or [`WalletRankReport::excluded`]; nothing is dropped silently.
//! * A wallet may fail several gates: ALL reasons are recorded, the
//!   primary one is the first in the fixed [`ExclusionReason`] order:
//!   `provider_error`, `not_scanned`, `incomplete_coverage`, `no_activity`,
//!   `no_pump_activity`, `unknown_basis`, `metric_unknown`,
//!   `insufficient_closed_episodes`, `insufficient_active_days`,
//!   `activity_unknown`, `activity_ceiling_trades_per_day`,
//!   `activity_ceiling_mints_per_day`, `open_exposure`, `below_top_n`.
//!   Wallets whose scan status is not `ok` carry only their status reason
//!   (their figures are a subset or absent, gates on them would mislead);
//!   their observations are still reported.
//! * Unknown is never a number: a wallet whose ranking metric is unknown
//!   goes to the exclusions with `metric_unknown`, it never takes part in
//!   the numeric sort (also under `--profile none`).
//!
//! # Sort (ARCHITECTURE §10 "Default rank")
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
//!   active UTC days, complete coverage, no unknown-basis episode/inventory.
//! * `insider` (P4.3, concentrated discretionary early buyers): >= 5 known
//!   closed episodes, >= 3 active UTC days, no unknown basis, plus an
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
//! * `none`: no sample/activity/unknown-basis gates. Status gates (scan
//!   not `ok`) and `metric_unknown` still apply, and the `pnl_status`
//!   label (`observed` / `known_subset`) travels with the figure.
//!
//! # Open exposure (explicit deviation, pending P5.2)
//! The config key `require_resolved_open_exposure=true` and ARCHITECTURE
//! ("unresolved open exposure may exclude") would exclude every wallet
//! with an open position, but there is no price source until P5.2, so no
//! open position can be valued. Default here: INCLUDE such wallets and
//! flag `open_exposure = unvalued` with the position count and raw
//! amounts; [`RankPolicy::require_no_open`] (`--require-no-open`) is the
//! strict variant that excludes them with `open_exposure`. An open
//! position with unknown basis is `unknown_basis` under quality/insider.
//!
//! # Activity metrics
//! Trades/mints per active day use only trades with a verified event
//! timestamp ([`ActivityMetrics`]); untimed trades are not counted, so the
//! ceiling is a lower-bound check.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use scout_analytics::RatioStatus;
use scout_core::{MONEY_SCALE, Money, SolanaPubkey};

use crate::solana_wallet_ledger::{OpenPosition, SolanaWalletLedgerReport};
use crate::solana_wallet_stats::{SolanaWalletStats, WalletScanStatus};

/// Default `--top`.
pub const DEFAULT_TOP: usize = 20;

/// Version tag of the ranking rules for report metadata.
pub const SOLANA_WALLET_RANK_VERSION: &str = "solana-wallet-rank/1";

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
    /// Exclude wallets with any `ClosedUnknown` episode or unknown-basis
    /// open inventory from IN-WINDOW causes. Left-censoring (ADR-011 §5)
    /// does not exclude: it is reported, not gated.
    pub exclude_unknown_basis: bool,
    /// Strict variant: exclude wallets with any open position.
    pub require_no_open: bool,
    /// Maximum number of ranked wallets (>= 1).
    pub top: usize,
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
            exclude_unknown_basis: unknown,
            require_no_open: false,
            top: top.max(1),
        }
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
    ProviderError,
    /// The run stopped (budget / terminal rate limit) before this wallet.
    NotScanned,
    IncompleteCoverage,
    NoActivity,
    NoPumpActivity,
    UnknownBasis,
    MetricUnknown,
    InsufficientClosedEpisodes,
    InsufficientActiveDays,
    /// A per-day ceiling is set but the wallet has no timestamped trades.
    ActivityUnknown,
    ActivityCeilingTradesPerDay,
    ActivityCeilingMintsPerDay,
    OpenExposure,
    /// Passed every gate but is below the `--top` cut.
    BelowTopN,
}

impl ExclusionReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProviderError => "provider_error",
            Self::NotScanned => "not_scanned",
            Self::IncompleteCoverage => "incomplete_coverage",
            Self::NoActivity => "no_activity",
            Self::NoPumpActivity => "no_pump_activity",
            Self::UnknownBasis => "unknown_basis",
            Self::MetricUnknown => "metric_unknown",
            Self::InsufficientClosedEpisodes => "insufficient_closed_episodes",
            Self::InsufficientActiveDays => "insufficient_active_days",
            Self::ActivityUnknown => "activity_unknown",
            Self::ActivityCeilingTradesPerDay => "activity_ceiling_trades_per_day",
            Self::ActivityCeilingMintsPerDay => "activity_ceiling_mints_per_day",
            Self::OpenExposure => "open_exposure",
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
    /// Known closed episodes only (incomplete coverage and/or unknown episodes).
    KnownSubset,
    /// Nothing known to report; never rendered as zero.
    NotAvailable,
}

impl PnlStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::KnownSubset => "known_subset",
            Self::NotAvailable => "n_a",
        }
    }
}

/// Open exposure flag. No price source exists (P5.2): never valued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenExposure {
    None,
    Unvalued {
        positions: u64,
        positions_unknown_basis: u64,
        details: Vec<OpenPosition>,
    },
}

impl OpenExposure {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unvalued { .. } => "unvalued",
        }
    }
}

/// Everything the rank layer observed about one wallet (ranked or not).
#[derive(Debug, Clone)]
pub struct WalletRankObservation {
    pub wallet: SolanaPubkey,
    pub status: WalletScanStatus,
    pub transactions_scanned: Option<u64>,
    /// Sanitized scan error text.
    pub error: Option<String>,
    pub incomplete_reasons: Vec<String>,
    /// Present unless the scan failed.
    pub ledger: Option<SolanaWalletLedgerReport>,
    pub pnl_status: PnlStatus,
    /// Realized net PnL (lamports) when known; `None` is N/A, not zero.
    pub net_pnl_lamports: Option<i128>,
    /// `realized_trade_pnl / consumed_acquisition_basis` (exact, lamport-scaled
    /// `Money` units on both sides); `None` when undefined.
    pub roi: Option<Ratio>,
    pub open_exposure: OpenExposure,
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

fn observe(w: &SolanaWalletStats) -> WalletRankObservation {
    let (pnl_status, net, roi, open) = match &w.ledger {
        None => (PnlStatus::NotAvailable, None, None, OpenExposure::None),
        Some(l) => {
            let net = (l.closed_episodes_known > 0 || l.failed_trade_fees_lamports != 0)
                .then_some(l.realized_net_pnl_lamports);
            let status = match net {
                None => PnlStatus::NotAvailable,
                Some(_) if w.coverage_complete() && l.closed_episodes_unknown == 0 => {
                    PnlStatus::Observed
                }
                Some(_) => PnlStatus::KnownSubset,
            };
            let roi = if l.closed_episodes_known > 0 {
                Ratio::new(
                    l.realized_trade_pnl_exact.scaled_units(),
                    l.consumed_acquisition_basis_exact.scaled_units(),
                )
            } else {
                None
            };
            let open = if l.open_positions.is_empty() {
                OpenExposure::None
            } else {
                OpenExposure::Unvalued {
                    positions: u64::try_from(l.open_positions.len()).unwrap_or(u64::MAX),
                    positions_unknown_basis: l.open_positions_with_unknown_basis,
                    details: l.open_positions.clone(),
                }
            };
            (status, net, roi, open)
        }
    };
    WalletRankObservation {
        wallet: w.wallet,
        status: w.status,
        transactions_scanned: w.transactions_scanned,
        error: w.error.clone(),
        incomplete_reasons: w.incomplete_reasons.clone(),
        ledger: w.ledger.clone(),
        pnl_status,
        net_pnl_lamports: net,
        roi,
        open_exposure: open,
    }
}

fn status_reason(status: WalletScanStatus) -> Option<ExclusionReason> {
    match status {
        WalletScanStatus::Ok => None,
        WalletScanStatus::Error => Some(ExclusionReason::ProviderError),
        WalletScanStatus::NotScanned => Some(ExclusionReason::NotScanned),
        WalletScanStatus::Incomplete => Some(ExclusionReason::IncompleteCoverage),
        WalletScanStatus::NoActivity => Some(ExclusionReason::NoActivity),
        WalletScanStatus::NoPumpActivity => Some(ExclusionReason::NoPumpActivity),
    }
}

/// Ranking-metric key of a gated wallet; `None` = unknown for this metric.
#[derive(Debug, Clone, Copy)]
enum PfKey {
    Finite(i128),
    Unbounded,
}

fn pf_key(l: &SolanaWalletLedgerReport) -> Option<PfKey> {
    match l.profit_factor {
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
    o.ledger.as_ref().map_or(0, |l| l.closed_episodes_known)
}

/// Descending comparator (best first) for the chosen metric.
fn cmp_best_first(by: RankBy, a: &WalletRankObservation, b: &WalletRankObservation) -> Ordering {
    let net = |o: &WalletRankObservation| o.net_pnl_lamports;
    let by_net = || match (net(a), net(b)) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    let by_roi = || cmp_opt_roi(b.roi, a.roi);
    let primary = match by {
        RankBy::RealizedNetPnl => by_net().then_with(by_roi),
        RankBy::RealizedCostRoi => by_roi().then_with(by_net),
        RankBy::ProfitFactor => {
            let key = |o: &WalletRankObservation| o.ledger.as_ref().and_then(pf_key);
            let pf = match (key(a), key(b)) {
                (Some(x), Some(y)) => cmp_pf(y, x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            pf.then_with(by_net).then_with(by_roi)
        }
    };
    primary
        .then_with(|| closed_known(b).cmp(&closed_known(a)))
        .then_with(|| a.wallet.cmp(&b.wallet))
}

fn metric_known(by: RankBy, o: &WalletRankObservation) -> bool {
    match by {
        RankBy::RealizedNetPnl => o.net_pnl_lamports.is_some(),
        RankBy::RealizedCostRoi => o.roi.is_some(),
        RankBy::ProfitFactor => o.ledger.as_ref().and_then(pf_key).is_some(),
    }
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
    if policy.exclude_unknown_basis
        && (l.closed_episodes_unknown > 0 || l.has_unknown_basis_inventory)
    {
        out.push(ExclusionReason::UnknownBasis);
    }
    if !metric_known(policy.rank_by, o) {
        out.push(ExclusionReason::MetricUnknown);
    }
    if l.closed_episodes_known < policy.min_closed_episodes {
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
        let obs = observe(w);
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
    let input_index = |wallet: &SolanaPubkey| wallets.iter().position(|w| w.wallet == *wallet);
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
            let idx = input_index(&obs.wallet).unwrap_or(usize::MAX);
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
