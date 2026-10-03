//! ADR-018 USD view of a Solana wallet ledger.
//!
//! The native quote-unit ledger (lamports / USDC / USDT, ADR-013) stays the
//! exact primary record and is never changed by this module. While the
//! ledger is built it journals, per disposal, the native net proceeds and
//! every consumed lot slice together with the PRICING TIME of each leg
//! (block time, else the verified event timestamp). After the build:
//!
//! 1. [`SolanaWalletLedgerReport::usd_price_requirements`] lists the minutes
//!    that must be priced (per quote asset);
//! 2. the caller awaits `PriceSource::prefetch` once (batched, cached);
//! 3. [`SolanaWalletLedgerReport::apply_usd_prices`] converts every leg and
//!    stores a [`UsdLedgerView`].
//!
//! # Conversion (one function)
//! [`convert_quote_to_usd`]: a native amount of `unit` (`Money` holding
//! `raw * 10^8`) times a decimal price `m / 10^s` USD per whole unit is
//! `amount * m / (10^s * 10^decimals)` in `Money` (USD at `MONEY_SCALE`),
//! computed with checked 128-bit integers and rounded HALF-EVEN exactly once
//! per leg (ties go to the even last digit). No float anywhere.
//!
//! # Episode rules (ADR-018 §4)
//! * Legs: the net proceeds of each disposal (gross minus the allocated sale
//!   fee, at the sale's pricing time) and every KNOWN-basis consumed lot
//!   slice (at the lot's acquisition pricing time). `usd_basis` of a slice
//!   is the slice's native basis (fee-capitalized) times the acquisition
//!   price; USD PnL of an episode is `sum(usd proceeds) - sum(usd basis)`.
//! * `ClosedKnown` in USD: the episode has no unknown reason other than
//!   `CrossQuoteUnit` (cross-quote episodes become known in USD) and every
//!   leg is priced. Any unpriced leg -> `ClosedUnknown` (never zero).
//! * Episodes whose only unknown cause is `LeftCensored` stay left-censored
//!   (never valued); open episodes stay open (no valuation of open
//!   positions in this slice).
//! * ADR-016 lower bound of a `ClosedUnknown` episode in USD:
//!   `-(sum of USD basis of its known consumed slices)`, `Unbounded` when
//!   any consumed lot had an unknown basis or a known slice is unpriced.
//!   Because every leg is USD, lots of several quote units no longer make
//!   the bound unbounded.
//! * The USD block reuses [`QuoteUnitBlock`] with unit `ReportCurrency`;
//!   its `Money` figures are USD at `MONEY_SCALE` and its `*_raw` figures
//!   are those scaled integers (1e-8 USD). Open-episode figures are not
//!   valued (zero, not rendered). Failed-transaction fees (ADR-004) are
//!   priced at their block time into [`UsdFailedFees`]; USD net = USD
//!   realized trade PnL - USD failed fees ([`UsdNet`]); an unpriced fee never
//!   counts as zero.

use std::collections::{BTreeMap, BTreeSet};

use scout_analytics::{Episode, EpisodeCohort, profit_factor, win_rate};
use scout_core::{Money, SolanaPubkey};
use scout_pricing::{DecimalPrice, PriceLabel, PriceSource, QuoteAsset, minute_start};

use crate::solana_wallet_ledger::{
    EpisodeOutcome, EpisodeRecord, LowerBound, QuoteUnit, QuoteUnitBlock, SolanaWalletLedgerError,
    SolanaWalletLedgerReport, UnknownReason, WinRateLowerBound, quote_unit_decimals,
    quote_units_to_money,
};
use crate::solana_wallet_stats::SolanaWalletStats;

/// Version tag of the USD view rules for report metadata (invariant #10).
pub const SOLANA_WALLET_USD_VERSION: &str = "solana-wallet-usd/2 (ADR-018: realized USD, failed-tx fees priced at block time -> USD net (ADR-004), open-position USD unrealized (ADR-019), half-even once per leg)";

/// Native legs of one disposal, journalled while the ledger is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdDisposal {
    /// Pricing time of the sale (`None`: the trade had no timestamp).
    pub price_ts: Option<i64>,
    /// Native net proceeds (gross - allocated sale fee) in their unit;
    /// `None` when the proceeds were unknown.
    pub net_proceeds: Option<(QuoteUnit, Money)>,
    /// Lot slices consumed, FIFO order.
    pub slices: Vec<UsdLotSlice>,
}

/// One consumed lot slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsdLotSlice {
    pub unit: QuoteUnit,
    /// Pricing time of the lot's acquisition.
    pub acquired_price_ts: Option<i64>,
    /// Native capitalized basis of the consumed slice.
    pub basis: Money,
    pub basis_known: bool,
}

/// All disposals of one episode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsdJournal {
    pub disposals: Vec<UsdDisposal>,
}

/// Outcome of one episode in USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsdOutcome {
    /// Every leg priced; exact USD PnL and consumed basis.
    ClosedKnown { pnl: Money, consumed_basis: Money },
    /// Closed, but a native cause or an unpriced leg makes the PnL unknown.
    ClosedUnknown,
    /// Pre-window inventory (ADR-011): never valued.
    LeftCensored,
    /// Still open: unvalued.
    Open,
}

impl UsdOutcome {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::ClosedKnown { .. } => "closed_known",
            Self::ClosedUnknown => "closed_unknown",
            Self::LeftCensored => "left_censored",
            Self::Open => "open",
        }
    }
}

/// USD view of one episode (parallel to `SolanaWalletLedgerReport::episodes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdEpisode {
    pub mint: SolanaPubkey,
    pub outcome: UsdOutcome,
    /// Native unknown causes other than `CrossQuoteUnit` / `LeftCensored`.
    pub native_unknown_reasons: BTreeSet<UnknownReason>,
    /// Labels of the unpriced legs' reasons (`price_unknown` causes).
    pub price_unknown_reasons: BTreeSet<String>,
    /// ADR-016 USD bound; `Some` exactly for `ClosedUnknown`.
    pub unknown_pnl_bound: Option<LowerBound<Money>>,
    /// Legs of this episode that were priced / not priced.
    pub legs_priced: u64,
    pub legs_unpriced: u64,
}

/// Price coverage of one wallet's USD view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsdCoverage {
    /// Conversions attempted (proceeds + known consumed slices of closed,
    /// non-left-censored episodes). A lot consumed by several disposals
    /// counts once per consumed slice.
    pub legs: u64,
    pub priced: u64,
    pub unpriced: u64,
    /// Histogram by quality label: `cex_reference_1m`, `stale_{k}m`,
    /// `usdc_par_assumed`, `price_unknown`.
    pub by_label: BTreeMap<String, u64>,
    /// Unpriced legs by reason label.
    pub unpriced_by_reason: BTreeMap<String, u64>,
    /// Priced legs valued at the USDC par assumption.
    pub usdc_par_legs: u64,
}

impl UsdCoverage {
    pub fn merge(&mut self, other: &Self) {
        self.legs = self.legs.saturating_add(other.legs);
        self.priced = self.priced.saturating_add(other.priced);
        self.unpriced = self.unpriced.saturating_add(other.unpriced);
        self.usdc_par_legs = self.usdc_par_legs.saturating_add(other.usdc_par_legs);
        for (k, v) in &other.by_label {
            let e = self.by_label.entry(k.clone()).or_insert(0);
            *e = e.saturating_add(*v);
        }
        for (k, v) in &other.unpriced_by_reason {
            let e = self.unpriced_by_reason.entry(k.clone()).or_insert(0);
            *e = e.saturating_add(*v);
        }
    }
}

/// ADR-004 failed-tx fees in USD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdFailedFees {
    /// Σ USD of the priced fees (`MONEY_SCALE`).
    pub priced_usd: Money,
    pub priced_txs: u64,
    pub unpriced_txs: u64,
    pub unpriced_by_reason: BTreeMap<String, u64>,
}

/// Status of the USD net figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsdNetStatus {
    /// Every failed fee priced: `net = realized - fees` exactly.
    Known,
    /// Some fees unpriced: `value = realized - priced fees` ONLY (the true net
    /// is lower by the unpriced fees; not a zero-fee claim).
    KnownSubset,
    /// All fees unpriced: no net value.
    Unknown,
}

impl UsdNetStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::KnownSubset => "known_subset",
            Self::Unknown => "unknown",
        }
    }
}

/// `usd realized trade pnl - usd failed fees` (ADR-004 in USD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsdNet {
    pub status: UsdNetStatus,
    /// `None` iff `Unknown`.
    pub value: Option<Money>,
}

/// The USD view of one wallet ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdLedgerView {
    pub version: &'static str,
    /// Parallel to `SolanaWalletLedgerReport::episodes`.
    pub episodes: Vec<UsdEpisode>,
    /// Known closed episodes (USD), unit `ReportCurrency`.
    pub block: QuoteUnitBlock,
    pub closed_episodes_unknown: u64,
    pub left_censored_episodes: u64,
    pub open_episodes: u64,
    /// ADR-016: `wins / (closed_known + closed_unknown)` in USD.
    pub win_rate_lower_bound: Option<WinRateLowerBound>,
    pub coverage: UsdCoverage,
    pub failed_fees: UsdFailedFees,
    pub net: UsdNet,
}

impl UsdLedgerView {
    /// ADR-016: `(closed_unknown, closed_known + closed_unknown)` in USD.
    #[must_use]
    pub fn unknown_episode_share_parts(&self) -> (u64, u64) {
        (
            self.closed_episodes_unknown,
            self.block
                .closed_episodes_known
                .saturating_add(self.closed_episodes_unknown),
        )
    }
}

/// The quote asset a native unit is valued through.
#[must_use]
pub const fn quote_asset_of(unit: QuoteUnit) -> Option<QuoteAsset> {
    match unit {
        QuoteUnit::Lamports => Some(QuoteAsset::Sol),
        QuoteUnit::UsdcUnits => Some(QuoteAsset::Usdc),
        QuoteUnit::UsdtUnits => Some(QuoteAsset::Usdt),
        // ADR-020 amendment: EVM native valued through ETH-USD (the native
        // symbol of Base/Robinhood; BNB chains need their own product once
        // enabled), USDG at par.
        QuoteUnit::Wei => Some(QuoteAsset::Eth),
        QuoteUnit::UsdgUnits => Some(QuoteAsset::Usdg),
        QuoteUnit::ReportCurrency => None,
    }
}

/// THE USD conversion (ADR-018 §4): `amount` is `Money` of a native
/// ledger of `unit` (`raw_base_units * 10^MONEY_SCALE`), `price` is USD per
/// whole unit (SOL, USDC, USDT). Result: USD `Money` at `MONEY_SCALE`,
/// `amount * price / 10^decimals`, rounded HALF-EVEN once.
pub fn convert_quote_to_usd(
    unit: QuoteUnit,
    amount: Money,
    price: &DecimalPrice,
) -> Result<Money, SolanaWalletLedgerError> {
    let overflow = SolanaWalletLedgerError::Overflow("usd conversion");
    let decimals = match quote_unit_decimals(unit) {
        Some(d) if unit != QuoteUnit::ReportCurrency => d,
        _ => return Err(SolanaWalletLedgerError::UnsupportedQuoteUnit(unit)),
    };
    let magnitude = amount.scaled_units().unsigned_abs();
    let num = magnitude
        .checked_mul(price.mantissa())
        .ok_or(SolanaWalletLedgerError::Overflow("usd conversion"))?;
    let den_exp = price
        .scale()
        .checked_add(decimals)
        .ok_or(SolanaWalletLedgerError::Overflow("usd conversion"))?;
    let den = 10u128
        .checked_pow(den_exp)
        .ok_or(SolanaWalletLedgerError::Overflow("usd conversion"))?;
    let q = num.div_euclid(den);
    let r = num.rem_euclid(den);
    let twice = r.checked_mul(2).ok_or(overflow)?;
    let rounded = match twice.cmp(&den) {
        std::cmp::Ordering::Greater => q.checked_add(1),
        std::cmp::Ordering::Equal if q.rem_euclid(2) == 1 => q.checked_add(1),
        _ => Some(q),
    }
    .ok_or(SolanaWalletLedgerError::Overflow("usd conversion"))?;
    let signed =
        i128::try_from(rounded).map_err(|_| SolanaWalletLedgerError::Overflow("usd conversion"))?;
    Ok(Money::from_scaled_units(if amount.is_negative() {
        -signed
    } else {
        signed
    }))
}

/// Reasons other than `CrossQuoteUnit` / `LeftCensored`.
fn other_reasons(rec: &EpisodeRecord) -> BTreeSet<UnknownReason> {
    rec.unknown_reasons
        .iter()
        .copied()
        .filter(|r| {
            !matches!(
                r,
                UnknownReason::CrossQuoteUnit | UnknownReason::LeftCensored
            )
        })
        .collect()
}

/// Whether the USD view values (or bounds) the episode's legs.
fn priced_episode(rec: &EpisodeRecord) -> bool {
    if matches!(rec.outcome, EpisodeOutcome::Open) {
        return false;
    }
    let left_censored_only =
        other_reasons(rec).is_empty() && rec.unknown_reasons.contains(&UnknownReason::LeftCensored);
    !left_censored_only
}

/// Pricing times of every leg of `rec` that needs a price (`None` times and
/// USDC legs excluded by the caller).
fn leg_specs(rec: &EpisodeRecord) -> Vec<(QuoteUnit, Option<i64>)> {
    let mut out = Vec::new();
    for d in &rec.usd_journal.disposals {
        if let Some((unit, _)) = d.net_proceeds {
            out.push((unit, d.price_ts));
        }
        for s in &d.slices {
            if s.basis_known {
                out.push((s.unit, s.acquired_price_ts));
            }
        }
    }
    out
}

/// Result of pricing one leg.
enum Leg {
    Priced(Money),
    Unpriced(String),
}

fn price_leg(
    source: &dyn PriceSource,
    unit: QuoteUnit,
    ts: Option<i64>,
    amount: Money,
    cov: &mut UsdCoverage,
) -> Leg {
    cov.legs += 1;
    let fail = |cov: &mut UsdCoverage, reason: String| {
        cov.unpriced += 1;
        *cov.by_label.entry("price_unknown".to_string()).or_insert(0) += 1;
        *cov.unpriced_by_reason.entry(reason.clone()).or_insert(0) += 1;
        Leg::Unpriced(reason)
    };
    let Some(t) = ts else {
        return fail(cov, "no_timestamp".to_string());
    };
    let Some(asset) = quote_asset_of(unit) else {
        return fail(cov, "unsupported_quote_unit".to_string());
    };
    let obs = source.usd_price(asset, t);
    let Some(price) = obs.value else {
        let reason = match obs.label {
            PriceLabel::Unknown { reason } => reason.label(),
            _ => "price_unknown".to_string(),
        };
        return fail(cov, reason);
    };
    match convert_quote_to_usd(unit, amount, &price) {
        Ok(m) => {
            cov.priced += 1;
            *cov.by_label.entry(obs.label.label()).or_insert(0) += 1;
            if obs.label == PriceLabel::UsdcParAssumed {
                cov.usdc_par_legs += 1;
            }
            Leg::Priced(m)
        }
        Err(_) => fail(cov, "conversion_overflow".to_string()),
    }
}

fn sum(a: Money, b: Money) -> Result<Money, SolanaWalletLedgerError> {
    Ok(a.checked_add(&b)?)
}

impl SolanaWalletLedgerReport {
    /// ADR-018: the minutes (unix seconds, minute-aligned) per quote asset
    /// whose USD price the view needs. USDC (par) is not listed.
    #[must_use]
    pub fn usd_price_requirements(&self) -> BTreeMap<QuoteAsset, BTreeSet<i64>> {
        let mut out: BTreeMap<QuoteAsset, BTreeSet<i64>> = BTreeMap::new();
        for rec in self.episodes.iter().filter(|r| priced_episode(r)) {
            for (unit, ts) in leg_specs(rec) {
                if let (Some(asset), Some(t)) = (quote_asset_of(unit), ts)
                    && asset != QuoteAsset::Usdc
                {
                    out.entry(asset).or_default().insert(minute_start(t));
                }
            }
        }
        for (ts, _) in &self.failed_fee_journal {
            if let (Some(t), Some(native)) = (ts, quote_asset_of(self.chain.native_unit)) {
                out.entry(native).or_default().insert(minute_start(*t));
            }
        }
        // Open-lot basis minutes: only for positions that are valued (the
        // USD unrealized needs a USD value first).
        for info in self.open_venues.iter().filter(|i| {
            self.open_valuation.as_ref().is_some_and(|v| {
                v.positions
                    .iter()
                    .any(|p| p.mint == i.mint && p.valued().is_some())
            })
        }) {
            for l in &info.lots {
                if l.basis_known
                    && let (Some(asset), Some(t)) = (quote_asset_of(l.unit), l.acquired_price_ts)
                    && asset != QuoteAsset::Usdc
                {
                    out.entry(asset).or_default().insert(minute_start(t));
                }
            }
        }
        out
    }

    /// ADR-018: compute the USD view from prefetched prices (no I/O).
    pub fn compute_usd_view(
        &self,
        source: &dyn PriceSource,
    ) -> Result<UsdLedgerView, SolanaWalletLedgerError> {
        let mut cov = UsdCoverage::default();
        let mut episodes = Vec::with_capacity(self.episodes.len());
        let mut block = QuoteUnitBlock::empty(QuoteUnit::ReportCurrency);
        let mut cohort = EpisodeCohort::default();
        let mut unknown_bound_sum = Money::ZERO;
        let mut any_unbounded = false;
        let (mut closed_unknown, mut left_censored, mut open) = (0u64, 0u64, 0u64);
        for rec in &self.episodes {
            let ep = usd_episode(rec, source, &mut cov)?;
            match ep.outcome {
                UsdOutcome::ClosedKnown {
                    pnl,
                    consumed_basis,
                } => {
                    block.closed_episodes_known += 1;
                    block.realized_trade_pnl_exact = sum(block.realized_trade_pnl_exact, pnl)?;
                    block.consumed_acquisition_basis_exact =
                        sum(block.consumed_acquisition_basis_exact, consumed_basis)?;
                    if pnl.is_positive() {
                        block.gross_profit_exact = sum(block.gross_profit_exact, pnl)?;
                        block.wins += 1;
                    } else if pnl.is_negative() {
                        block.gross_loss_abs_exact =
                            block.gross_loss_abs_exact.checked_sub(&pnl)?;
                        block.losses += 1;
                    } else {
                        block.breakeven += 1;
                    }
                    cohort.episodes.push(Episode {
                        asset: self.chain.asset_key(&rec.mint),
                        realized_pnl: pnl,
                        is_closed_within_window: true,
                        opened_before_window: false,
                        has_unresolved_flows: false,
                    });
                }
                UsdOutcome::ClosedUnknown => {
                    closed_unknown += 1;
                    match ep.unknown_pnl_bound {
                        Some(LowerBound::Bounded(b)) => {
                            unknown_bound_sum = sum(unknown_bound_sum, b)?;
                        }
                        Some(LowerBound::Unbounded) | None => any_unbounded = true,
                    }
                }
                UsdOutcome::LeftCensored => left_censored += 1,
                UsdOutcome::Open => open += 1,
            }
            episodes.push(ep);
        }
        block.realized_trade_pnl_raw = block.realized_trade_pnl_exact.scaled_units();
        block.consumed_acquisition_basis_raw =
            block.consumed_acquisition_basis_exact.scaled_units();
        block.win_rate = win_rate(&cohort)?;
        block.profit_factor = profit_factor(&cohort)?;
        block.unknown_pnl_bound = if any_unbounded {
            LowerBound::Unbounded
        } else {
            LowerBound::Bounded(unknown_bound_sum)
        };
        let total_closed = block.closed_episodes_known.saturating_add(closed_unknown);
        let mut failed_fees = UsdFailedFees {
            priced_usd: Money::ZERO,
            priced_txs: 0,
            unpriced_txs: 0,
            unpriced_by_reason: BTreeMap::new(),
        };
        for (ts, lamports) in &self.failed_fee_journal {
            let fee = quote_units_to_money(self.chain.native_unit, i128::from(*lamports))?;
            match price_leg(source, self.chain.native_unit, *ts, fee, &mut cov) {
                Leg::Priced(m) => {
                    failed_fees.priced_txs += 1;
                    failed_fees.priced_usd = sum(failed_fees.priced_usd, m)?;
                }
                Leg::Unpriced(reason) => {
                    failed_fees.unpriced_txs += 1;
                    *failed_fees.unpriced_by_reason.entry(reason).or_insert(0) += 1;
                }
            }
        }
        let net_value = block
            .realized_trade_pnl_exact
            .checked_sub(&failed_fees.priced_usd)?;
        let net = if failed_fees.unpriced_txs == 0 {
            UsdNet {
                status: UsdNetStatus::Known,
                value: Some(net_value),
            }
        } else if failed_fees.priced_txs > 0 {
            UsdNet {
                status: UsdNetStatus::KnownSubset,
                value: Some(net_value),
            }
        } else {
            UsdNet {
                status: UsdNetStatus::Unknown,
                value: None,
            }
        };
        Ok(UsdLedgerView {
            failed_fees,
            net,
            version: SOLANA_WALLET_USD_VERSION,
            episodes,
            win_rate_lower_bound: (total_closed > 0).then_some(WinRateLowerBound {
                wins: block.wins,
                episodes: total_closed,
            }),
            block,
            closed_episodes_unknown: closed_unknown,
            left_censored_episodes: left_censored,
            open_episodes: open,
            coverage: cov,
        })
    }

    /// ADR-018: compute and store the USD view. Native figures are untouched.
    pub fn apply_usd_prices(
        &mut self,
        source: &dyn PriceSource,
    ) -> Result<(), SolanaWalletLedgerError> {
        self.usd = Some(self.compute_usd_view(source)?);
        self.apply_open_usd_unrealized(source);
        Ok(())
    }

    /// ADR-019 x ADR-018: `usd unrealized = usd realizable value(as_of) -
    /// Σ usd basis of the remaining lots` per valued position. Each lot's
    /// remaining native basis (already pro-rata exact) is converted once,
    /// half-even, at its acquisition minute. Unknown (never zero) when the
    /// position is unvalued or unpriced, or any lot basis is unknown/unpriced.
    /// Never read by realized rank keys.
    pub fn apply_open_usd_unrealized(&mut self, source: &dyn PriceSource) {
        let Some(view) = self.open_valuation.as_mut() else {
            return;
        };
        for p in &mut view.positions {
            let crate::solana_open_valuation::ValuationOutcome::Valued(v) = &mut p.outcome else {
                continue;
            };
            v.usd_unrealized = None;
            v.usd_unrealized_reason = None;
            let Some(usd) = v.usd.as_ref() else {
                v.usd_unrealized_reason = Some(
                    v.usd_unpriced_reason
                        .clone()
                        .unwrap_or_else(|| "value_usd_unpriced".to_string()),
                );
                continue;
            };
            let Some(info) = self.open_venues.iter().find(|i| i.mint == p.mint) else {
                v.usd_unrealized_reason = Some("lots_unavailable".to_string());
                continue;
            };
            let mut cov = UsdCoverage::default();
            let mut basis = Money::ZERO;
            let mut reason: Option<String> = None;
            for l in &info.lots {
                if !l.basis_known {
                    reason = Some("unknown_basis".to_string());
                    break;
                }
                match price_leg(source, l.unit, l.acquired_price_ts, l.basis, &mut cov) {
                    Leg::Priced(m) => match sum(basis, m) {
                        Ok(b) => basis = b,
                        Err(_) => {
                            reason = Some("conversion_overflow".to_string());
                            break;
                        }
                    },
                    Leg::Unpriced(r) => {
                        reason = Some(format!("basis_{r}"));
                        break;
                    }
                }
            }
            match reason {
                Some(r) => v.usd_unrealized_reason = Some(r),
                None => match usd.value.checked_sub(&basis) {
                    Ok(m) => v.usd_unrealized = Some(m),
                    Err(_) => v.usd_unrealized_reason = Some("conversion_overflow".to_string()),
                },
            }
        }
    }
}

fn usd_episode(
    rec: &EpisodeRecord,
    source: &dyn PriceSource,
    cov: &mut UsdCoverage,
) -> Result<UsdEpisode, SolanaWalletLedgerError> {
    let native_other = other_reasons(rec);
    let mut ep = UsdEpisode {
        mint: rec.mint,
        outcome: UsdOutcome::ClosedUnknown,
        native_unknown_reasons: native_other.clone(),
        price_unknown_reasons: BTreeSet::new(),
        unknown_pnl_bound: None,
        legs_priced: 0,
        legs_unpriced: 0,
    };
    if matches!(rec.outcome, EpisodeOutcome::Open) {
        ep.outcome = UsdOutcome::Open;
        return Ok(ep);
    }
    if !priced_episode(rec) {
        ep.outcome = UsdOutcome::LeftCensored;
        return Ok(ep);
    }
    let mut proceeds_usd = Money::ZERO;
    let mut basis_usd = Money::ZERO;
    let mut structurally_known = native_other.is_empty();
    let mut all_priced = true;
    let mut any_unknown_slice = false;
    let mut slice_unpriced = false;
    for d in &rec.usd_journal.disposals {
        match d.net_proceeds {
            Some((unit, amount)) => match price_leg(source, unit, d.price_ts, amount, cov) {
                Leg::Priced(m) => {
                    ep.legs_priced += 1;
                    proceeds_usd = sum(proceeds_usd, m)?;
                }
                Leg::Unpriced(reason) => {
                    ep.legs_unpriced += 1;
                    ep.price_unknown_reasons.insert(reason);
                    all_priced = false;
                }
            },
            None => structurally_known = false,
        }
        for s in &d.slices {
            if !s.basis_known {
                any_unknown_slice = true;
                structurally_known = false;
                continue;
            }
            match price_leg(source, s.unit, s.acquired_price_ts, s.basis, cov) {
                Leg::Priced(m) => {
                    ep.legs_priced += 1;
                    basis_usd = sum(basis_usd, m)?;
                }
                Leg::Unpriced(reason) => {
                    ep.legs_unpriced += 1;
                    ep.price_unknown_reasons.insert(reason);
                    all_priced = false;
                    slice_unpriced = true;
                }
            }
        }
    }
    if structurally_known && all_priced && !rec.usd_journal.disposals.is_empty() {
        ep.outcome = UsdOutcome::ClosedKnown {
            pnl: proceeds_usd.checked_sub(&basis_usd)?,
            consumed_basis: basis_usd,
        };
        return Ok(ep);
    }
    ep.outcome = UsdOutcome::ClosedUnknown;
    ep.unknown_pnl_bound = Some(if any_unknown_slice || slice_unpriced {
        LowerBound::Unbounded
    } else {
        Money::ZERO
            .checked_sub(&basis_usd)
            .map_or(LowerBound::Unbounded, LowerBound::Bounded)
    });
    Ok(ep)
}

/// Aggregate of one pricing run over many wallets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsdPricingRun {
    pub prefetch: scout_pricing::PrefetchSummary,
    /// Wallets with a ledger that received a USD view.
    pub wallets_priced: u64,
    /// Wallets whose view could not be computed (arithmetic overflow).
    pub wallets_failed: u64,
    pub coverage: UsdCoverage,
    /// Distinct minutes requested per asset (after dedup across wallets).
    pub minutes_requested: BTreeMap<QuoteAsset, u64>,
}

/// ADR-018: price every wallet of a stats run: collect the required
/// minutes of ALL wallets, prefetch once, then apply the USD view to each
/// ledger. Pricing failures never fail a wallet: they surface as unknown
/// legs and in [`UsdPricingRun`].
pub async fn apply_usd_pricing(
    wallets: &mut [SolanaWalletStats],
    source: &dyn PriceSource,
) -> UsdPricingRun {
    let mut needs: BTreeMap<QuoteAsset, BTreeSet<i64>> = BTreeMap::new();
    for w in wallets.iter() {
        if let Some(l) = &w.ledger {
            for (asset, minutes) in l.usd_price_requirements() {
                needs.entry(asset).or_default().extend(minutes);
            }
        }
    }
    // ADR-019: the SOL price at `as_of` for the valued open positions.
    if let Some(as_of) = crate::open_usd_requirement(wallets) {
        needs
            .entry(QuoteAsset::Sol)
            .or_default()
            .insert(minute_start(as_of));
    }
    let mut run = UsdPricingRun {
        minutes_requested: needs
            .iter()
            .map(|(a, m)| (*a, u64::try_from(m.len()).unwrap_or(u64::MAX)))
            .collect(),
        ..UsdPricingRun::default()
    };
    run.prefetch = source.prefetch(&needs).await;
    for w in wallets.iter_mut() {
        let Some(l) = w.ledger.as_mut() else { continue };
        if let Some(v) = l.open_valuation.as_mut() {
            v.apply_usd_price(source);
        }
        match l.apply_usd_prices(source) {
            Ok(()) => {
                run.wallets_priced += 1;
                if let Some(u) = &l.usd {
                    run.coverage.merge(&u.coverage);
                }
            }
            Err(_) => run.wallets_failed += 1,
        }
    }
    run
}
