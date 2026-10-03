//! ADR-019: realizable valuation of OPEN positions at the run's `as_of`.
//!
//! A position is valued as the quote the wallet would receive by selling its
//! WHOLE remaining amount now with the venue's own swap math on live state
//! (never the spot price): the pump.fun bonding curve (virtual reserves of
//! the `BondingCurve` account) or the PumpSwap pool (vault balances +
//! `virtual_quote_reserves`). The swap math lives in
//! `scout_dex_solana::swap_math` and is verified exactly against every paired
//! live sell event of the committed fixtures. Fee bps are those of the most
//! recent event observed in the run on the same curve/pool (carried on the
//! ledger as [`OpenVenueInfo`]); with none the position is
//! `unvalued { fee_unknown }`.
//!
//! Layers:
//! * pure: [`value_position`] (state -> [`PositionValuation`]), no I/O;
//! * I/O: [`apply_open_valuation`] reads the needed accounts with
//!   `getMultipleAccounts` (bounded by the provider's request budget, the
//!   context slot recorded) and stores an [`OpenValuationView`] on each
//!   ledger; [`OpenValuationView::apply_usd_price`] adds USD at `as_of`
//!   (ADR-018 price source, one SOL price per run).
//!
//! Unrealized PnL = realizable net - known remaining basis, only when EVERY
//! open lot has a known basis in lamports; otherwise `None` (never zero).
//! Nothing here touches the realized figures or the rank keys.

use std::collections::{BTreeMap, BTreeSet};

use scout_core::{Money, SolanaPubkey};
use scout_dex_solana::{
    BondingCurveAccount, PoolAccount, WRAPPED_SOL_MINT, amm_sell_quote, curve_sell_quote,
    decode_bonding_curve, decode_pool, decode_token_account, effective_quote_reserve,
    price_impact_bps,
};
use scout_pricing::{PriceLabel, PriceSource, QuoteAsset};
use scout_providers::HeliusProvider;

use crate::analysis_window::AnalysisWindow;
use crate::solana_buyer_intersect::{ScanFailureKind, classify_provider_error};
use crate::solana_wallet_ledger::{QuoteUnit, Venue, lamports_to_money};
use crate::solana_wallet_stats::SolanaWalletStats;
use crate::solana_wallet_usd::convert_quote_to_usd;

/// Version tag of the valuation rules for report metadata (invariant #10).
pub const SOLANA_OPEN_VALUATION_VERSION: &str = "solana-open-valuation/1 (ADR-019: realizable constant-product sell of the whole open amount on live pump.fun curve / PumpSwap pool state, fee bps of the latest observed event, swap math verified exactly on paired live events)";

/// The label of a valued position (ADR-019 §4).
pub const LABEL_REALIZABLE_CP_QUOTE: &str = "realizable_cp_quote";

/// Commitment of the account reads (recorded in `run_meta`).
pub const VALUATION_COMMITMENT: &str = "confirmed";

/// Fee bps of the latest observed event on a curve / pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeObservation {
    Curve {
        protocol_bps: u64,
        creator_bps: u64,
    },
    Amm {
        lp_bps: u64,
        protocol_bps: u64,
        creator_bps: u64,
    },
}

impl FeeObservation {
    /// Sum of all fee bps (presentation).
    #[must_use]
    pub fn total_bps(&self) -> u64 {
        match *self {
            Self::Curve {
                protocol_bps,
                creator_bps,
            } => protocol_bps.saturating_add(creator_bps),
            Self::Amm {
                lp_bps,
                protocol_bps,
                creator_bps,
            } => lp_bps
                .saturating_add(protocol_bps)
                .saturating_add(creator_bps),
        }
    }
}

/// A venue account the wallet traded on, with the latest fee observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueAddress {
    pub address: SolanaPubkey,
    /// Unix seconds of the event the fee bps came from.
    pub fee_observed_at: Option<i64>,
    pub fee: Option<FeeObservation>,
}

/// Known remaining basis of an open position, native lamports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenBasis {
    /// Σ remaining basis of the KNOWN-basis lamport lots (lamport-scaled).
    pub known_sol_basis: Money,
    /// Every open lot is known-basis AND lamport-denominated: the sum above
    /// is the whole basis and an unrealized PnL may be formed.
    pub fully_known_sol: bool,
}

/// Ledger-side input of the valuation of one open position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenVenueInfo {
    pub mint: SolanaPubkey,
    pub open_amount_raw: u128,
    /// Venue of the wallet's latest DIRECT pump trade of this mint
    /// (`BondingCurve` or `PumpAmm`); `None` = only route/unknown trades.
    pub last_venue: Option<Venue>,
    /// Latest curve account the wallet traded this mint on.
    pub curve: Option<VenueAddress>,
    /// Latest PumpSwap pool the wallet traded this mint on.
    pub pool: Option<VenueAddress>,
    pub basis: OpenBasis,
    /// ADR-018/019: remaining open lots (native basis + acquisition pricing
    /// time) for the USD unrealized PnL.
    pub lots: Vec<crate::solana_wallet_usd::UsdLotSlice>,
}

/// Why a position is not valued (ADR-019 §§1-3). Never a zero value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnvaluedReason {
    /// The window ends before `as_of`: no historical account state is read.
    HistoricalWindow,
    /// Valuation was not run (`--no-valuation`).
    NotRun,
    /// No direct pump.fun / PumpSwap trade of the mint in the run (route
    /// swaps or transfers only).
    NoVenueObserved,
    /// Venue/pool shape not supported (mint is not the pool base, ...).
    VenueNotSupported,
    /// No event with fee bps was observed on the curve/pool in the run.
    FeeUnknown,
    /// The curve graduated and no PumpSwap pool of the wallet is known.
    MigratedPoolUnknown,
    AccountMissing,
    AccountInvalid,
    /// The quote asset is not SOL/wSOL.
    QuoteNotSol,
    /// The account read failed (provider error).
    StateFetchFailed,
    /// The request budget was spent before the state could be read.
    RequestBudgetExhausted,
    /// Open amount above `u64::MAX` (cannot be a token balance).
    AmountTooLarge,
    /// Checked arithmetic failed (empty reserve, overflow).
    MathFailure,
}

impl UnvaluedReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::HistoricalWindow => "historical_window",
            Self::NotRun => "not_run",
            Self::NoVenueObserved => "no_venue_observed",
            Self::VenueNotSupported => "venue_not_supported",
            Self::FeeUnknown => "fee_unknown",
            Self::MigratedPoolUnknown => "migrated_pool_unknown",
            Self::AccountMissing => "account_missing",
            Self::AccountInvalid => "account_invalid",
            Self::QuoteNotSol => "quote_not_sol",
            Self::StateFetchFailed => "state_fetch_failed",
            Self::RequestBudgetExhausted => "request_budget_exhausted",
            Self::AmountTooLarge => "amount_too_large",
            Self::MathFailure => "math_failure",
        }
    }
}

/// USD value of a valued position at `as_of` (ADR-018).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenUsd {
    /// USD at `MONEY_SCALE` of the realizable net.
    pub value: Money,
    /// ADR-018 price label (`cex_reference_1m`, `stale_3m`, ...).
    pub price_label: String,
}

/// A valued open position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuedPosition {
    /// `BondingCurve` or `PumpAmm`.
    pub venue: Venue,
    pub address: SolanaPubkey,
    pub label: &'static str,
    /// Context slot of the account read (curve or pool account).
    pub account_slot: u64,
    /// Context slot of the vault read (PumpSwap only).
    pub vault_slot: Option<u64>,
    /// Constant-product output before fees (lamports).
    pub gross_lamports: u64,
    /// Σ lp + protocol + creator fees (lamports).
    pub fee_lamports: u64,
    /// What the wallet would receive (lamports): the realizable value.
    pub realizable_lamports: u64,
    /// Zero-size (spot) value of the same amount before fees (lamports).
    pub marginal_lamports: u64,
    /// `(marginal - gross) / marginal` in bps; `None` when marginal is 0.
    pub price_impact_bps: Option<u64>,
    /// Quote reserve the swap ran against (curve: virtual SOL; pool: vault
    /// quote + virtual quote), lamports.
    pub quote_reserve_lamports: u128,
    /// Token reserve the swap ran against (raw base units).
    pub token_reserve_raw: u128,
    pub fee: FeeObservation,
    pub fee_observed_at: Option<i64>,
    /// `realizable - known remaining basis` (lamport-scaled `Money`); `None`
    /// when any open lot has an unknown or non-SOL basis.
    pub unrealized_pnl: Option<Money>,
    /// `Some` after [`OpenValuationView::apply_usd_price`] priced it.
    pub usd: Option<OpenUsd>,
    /// Why the USD figure is missing (after pricing was attempted).
    pub usd_unpriced_reason: Option<String>,
    /// `usd value - Σ usd basis of remaining lots` (USD `MONEY_SCALE`);
    /// `Some` only when every lot basis and the value are USD-priced.
    pub usd_unrealized: Option<Money>,
    /// Why `usd_unrealized` is missing (after USD pricing was attempted).
    pub usd_unrealized_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValuationOutcome {
    Valued(Box<ValuedPosition>),
    Unvalued { reason: UnvaluedReason },
}

/// One open position and its valuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionValuation {
    pub mint: SolanaPubkey,
    pub open_amount_raw: u128,
    /// Known remaining basis, lamports (see [`OpenBasis`]).
    pub basis: OpenBasis,
    pub outcome: ValuationOutcome,
}

impl PositionValuation {
    #[must_use]
    pub fn valued(&self) -> Option<&ValuedPosition> {
        match &self.outcome {
            ValuationOutcome::Valued(v) => Some(v),
            ValuationOutcome::Unvalued { .. } => None,
        }
    }

    #[must_use]
    pub fn unvalued_reason(&self) -> Option<UnvaluedReason> {
        match &self.outcome {
            ValuationOutcome::Unvalued { reason } => Some(*reason),
            ValuationOutcome::Valued(_) => None,
        }
    }
}

/// Totals over the positions of one wallet (or several, via `merge`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenValuationTotals {
    pub positions: u64,
    pub valued: u64,
    pub unvalued: u64,
    /// Σ realizable lamports of the valued positions.
    pub realizable_lamports: u128,
    /// Σ unrealized PnL (lamport-scaled `Money` units) over valued
    /// positions whose basis is fully known; the count is next to it.
    pub unrealized_known_scaled: i128,
    pub unrealized_known_positions: u64,
    /// Valued positions without a known unrealized PnL (unknown basis).
    pub unrealized_unknown_positions: u64,
    /// Σ USD value (`MONEY_SCALE` scaled integer) of the USD-priced positions.
    pub usd_value_scaled: i128,
    pub usd_priced_positions: u64,
    /// Σ USD unrealized PnL (`MONEY_SCALE`) over positions where it is known.
    pub usd_unrealized_known_scaled: i128,
    pub usd_unrealized_known_positions: u64,
    /// Valued positions whose USD unrealized PnL is unknown (any cause).
    pub usd_unrealized_unknown_positions: u64,
    pub unvalued_by_reason: BTreeMap<&'static str, u64>,
}

impl OpenValuationTotals {
    pub fn merge(&mut self, o: &Self) {
        self.positions = self.positions.saturating_add(o.positions);
        self.valued = self.valued.saturating_add(o.valued);
        self.unvalued = self.unvalued.saturating_add(o.unvalued);
        self.realizable_lamports = self
            .realizable_lamports
            .saturating_add(o.realizable_lamports);
        self.unrealized_known_scaled = self
            .unrealized_known_scaled
            .saturating_add(o.unrealized_known_scaled);
        self.unrealized_known_positions = self
            .unrealized_known_positions
            .saturating_add(o.unrealized_known_positions);
        self.unrealized_unknown_positions = self
            .unrealized_unknown_positions
            .saturating_add(o.unrealized_unknown_positions);
        self.usd_value_scaled = self.usd_value_scaled.saturating_add(o.usd_value_scaled);
        self.usd_priced_positions = self
            .usd_priced_positions
            .saturating_add(o.usd_priced_positions);
        self.usd_unrealized_known_scaled = self
            .usd_unrealized_known_scaled
            .saturating_add(o.usd_unrealized_known_scaled);
        self.usd_unrealized_known_positions = self
            .usd_unrealized_known_positions
            .saturating_add(o.usd_unrealized_known_positions);
        self.usd_unrealized_unknown_positions = self
            .usd_unrealized_unknown_positions
            .saturating_add(o.usd_unrealized_unknown_positions);
        for (k, v) in &o.unvalued_by_reason {
            let e = self.unvalued_by_reason.entry(k).or_insert(0);
            *e = e.saturating_add(*v);
        }
    }
}

/// The valuation of one wallet's open positions (stored on its ledger).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenValuationView {
    pub version: &'static str,
    /// Run start the positions are valued at (unix seconds).
    pub as_of: i64,
    pub commitment: &'static str,
    /// Highest context slot of any read that fed this view.
    pub state_slot: Option<u64>,
    /// Ascending mint order.
    pub positions: Vec<PositionValuation>,
}

impl OpenValuationView {
    #[must_use]
    pub fn totals(&self) -> OpenValuationTotals {
        let mut t = OpenValuationTotals::default();
        for p in &self.positions {
            t.positions += 1;
            match &p.outcome {
                ValuationOutcome::Unvalued { reason } => {
                    t.unvalued += 1;
                    *t.unvalued_by_reason.entry(reason.label()).or_insert(0) += 1;
                }
                ValuationOutcome::Valued(v) => {
                    t.valued += 1;
                    t.realizable_lamports = t
                        .realizable_lamports
                        .saturating_add(u128::from(v.realizable_lamports));
                    match v.unrealized_pnl {
                        Some(m) => {
                            t.unrealized_known_positions += 1;
                            t.unrealized_known_scaled =
                                t.unrealized_known_scaled.saturating_add(m.scaled_units());
                        }
                        None => t.unrealized_unknown_positions += 1,
                    }
                    match v.usd_unrealized {
                        Some(m) => {
                            t.usd_unrealized_known_positions += 1;
                            t.usd_unrealized_known_scaled = t
                                .usd_unrealized_known_scaled
                                .saturating_add(m.scaled_units());
                        }
                        None => t.usd_unrealized_unknown_positions += 1,
                    }
                    if let Some(u) = &v.usd {
                        t.usd_priced_positions += 1;
                        t.usd_value_scaled =
                            t.usd_value_scaled.saturating_add(u.value.scaled_units());
                    }
                }
            }
        }
        t
    }

    /// True when every position has a realizable value.
    #[must_use]
    pub fn all_valued(&self) -> bool {
        self.positions.iter().all(|p| p.valued().is_some())
    }

    /// The minute whose SOL price [`apply_usd_price`](Self::apply_usd_price)
    /// needs (`None` without a valued position).
    #[must_use]
    pub fn usd_price_requirement(&self) -> Option<i64> {
        self.positions
            .iter()
            .any(|p| p.valued().is_some())
            .then_some(self.as_of)
    }

    /// ADR-018: value every valued position in USD at `as_of` (one SOL
    /// price, one half-even rounding per position). A missing price leaves
    /// the position valued in SOL with `usd_unpriced_reason` set.
    pub fn apply_usd_price(&mut self, source: &dyn PriceSource) {
        let obs = source.usd_price(QuoteAsset::Sol, self.as_of);
        for p in &mut self.positions {
            let ValuationOutcome::Valued(v) = &mut p.outcome else {
                continue;
            };
            v.usd = None;
            v.usd_unpriced_reason = None;
            let Some(price) = obs.value else {
                v.usd_unpriced_reason = Some(match obs.label {
                    PriceLabel::Unknown { reason } => reason.label(),
                    _ => "price_unknown".to_string(),
                });
                continue;
            };
            let converted = lamports_to_money(i128::from(v.realizable_lamports))
                .ok()
                .and_then(|m| convert_quote_to_usd(QuoteUnit::Lamports, m, &price).ok());
            match converted {
                Some(value) => {
                    v.usd = Some(OpenUsd {
                        value,
                        price_label: obs.label.label(),
                    });
                }
                None => v.usd_unpriced_reason = Some("conversion_overflow".to_string()),
            }
        }
    }
}

// ---------------------------------------------------------------------
// Pure valuation
// ---------------------------------------------------------------------

/// Decoded state of one curve address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurveRead {
    Missing,
    Invalid,
    Ok(Box<BondingCurveAccount>),
}

/// Decoded state of one pool address with its vault balances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolRead {
    Missing,
    Invalid,
    /// The pool decoded, the vaults are not read (yet).
    NoVaults(Box<PoolAccount>),
    /// A vault is missing/invalid.
    VaultInvalid,
    Ok {
        pool: Box<PoolAccount>,
        base_balance: u64,
        quote_balance: u64,
    },
    /// The read of this part failed (budget/provider).
    Failed(UnvaluedReason),
}

/// Everything read from the chain for one valuation pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateSnapshot {
    pub account_slot: u64,
    pub vault_slot: Option<u64>,
    pub curves: BTreeMap<SolanaPubkey, CurveRead>,
    pub pools: BTreeMap<SolanaPubkey, PoolRead>,
}

fn unvalued(info: &OpenVenueInfo, reason: UnvaluedReason) -> PositionValuation {
    PositionValuation {
        mint: info.mint,
        open_amount_raw: info.open_amount_raw,
        basis: info.basis,
        outcome: ValuationOutcome::Unvalued { reason },
    }
}

fn unrealized(basis: &OpenBasis, realizable_lamports: u64) -> Option<Money> {
    if !basis.fully_known_sol {
        return None;
    }
    let value = lamports_to_money(i128::from(realizable_lamports)).ok()?;
    value.checked_sub(&basis.known_sol_basis).ok()
}

fn is_sol_quote(q: Option<SolanaPubkey>) -> bool {
    q.is_none_or(|q| q == [0u8; 32] || q == WRAPPED_SOL_MINT)
}

/// Value ONE open position against `snap` (pure).
#[must_use]
pub fn value_position(info: &OpenVenueInfo, snap: &StateSnapshot) -> PositionValuation {
    let Ok(amount) = u64::try_from(info.open_amount_raw) else {
        return unvalued(info, UnvaluedReason::AmountTooLarge);
    };
    match info.last_venue {
        Some(Venue::BondingCurve) => {
            let Some(c) = info.curve else {
                return unvalued(info, UnvaluedReason::NoVenueObserved);
            };
            match snap.curves.get(&c.address) {
                None => unvalued(info, UnvaluedReason::StateFetchFailed),
                Some(CurveRead::Missing) => unvalued(info, UnvaluedReason::AccountMissing),
                Some(CurveRead::Invalid) => unvalued(info, UnvaluedReason::AccountInvalid),
                Some(CurveRead::Ok(acct)) if acct.complete => match info.pool {
                    Some(p) => value_pool(info, amount, p, snap),
                    None => unvalued(info, UnvaluedReason::MigratedPoolUnknown),
                },
                Some(CurveRead::Ok(acct)) => value_curve(info, amount, c, acct, snap),
            }
        }
        Some(Venue::PumpAmm) => match info.pool {
            Some(p) => value_pool(info, amount, p, snap),
            None => unvalued(info, UnvaluedReason::NoVenueObserved),
        },
        Some(Venue::Route) | None => unvalued(info, UnvaluedReason::NoVenueObserved),
    }
}

fn value_curve(
    info: &OpenVenueInfo,
    amount: u64,
    venue: VenueAddress,
    acct: &BondingCurveAccount,
    snap: &StateSnapshot,
) -> PositionValuation {
    if !is_sol_quote(acct.quote_mint) {
        return unvalued(info, UnvaluedReason::QuoteNotSol);
    }
    let Some(
        fee @ FeeObservation::Curve {
            protocol_bps,
            creator_bps,
        },
    ) = venue.fee
    else {
        return unvalued(info, UnvaluedReason::FeeUnknown);
    };
    let Some(q) = curve_sell_quote(
        amount,
        acct.virtual_quote_reserves,
        acct.virtual_token_reserves,
        protocol_bps,
        creator_bps,
    ) else {
        return unvalued(info, UnvaluedReason::MathFailure);
    };
    let marginal = u128::from(amount)
        .checked_mul(u128::from(acct.virtual_quote_reserves))
        .and_then(|n| n.checked_div(u128::from(acct.virtual_token_reserves)))
        .and_then(|m| u64::try_from(m).ok());
    let Some(marginal) = marginal else {
        return unvalued(info, UnvaluedReason::MathFailure);
    };
    let impact = price_impact_bps(
        amount,
        u128::from(acct.virtual_token_reserves),
        u128::from(acct.virtual_quote_reserves),
        q.gross,
    );
    let fees = q.protocol_fee.saturating_add(q.creator_fee);
    PositionValuation {
        mint: info.mint,
        open_amount_raw: info.open_amount_raw,
        basis: info.basis,
        outcome: ValuationOutcome::Valued(Box::new(ValuedPosition {
            venue: Venue::BondingCurve,
            address: venue.address,
            label: LABEL_REALIZABLE_CP_QUOTE,
            account_slot: snap.account_slot,
            vault_slot: None,
            gross_lamports: q.gross,
            fee_lamports: fees,
            realizable_lamports: q.net,
            marginal_lamports: marginal,
            price_impact_bps: impact,
            quote_reserve_lamports: u128::from(acct.virtual_quote_reserves),
            token_reserve_raw: u128::from(acct.virtual_token_reserves),
            fee,
            fee_observed_at: venue.fee_observed_at,
            unrealized_pnl: unrealized(&info.basis, q.net),
            usd: None,
            usd_unpriced_reason: None,
            usd_unrealized: None,
            usd_unrealized_reason: None,
        })),
    }
}

fn value_pool(
    info: &OpenVenueInfo,
    amount: u64,
    venue: VenueAddress,
    snap: &StateSnapshot,
) -> PositionValuation {
    let (pool, base_balance, quote_balance) = match snap.pools.get(&venue.address) {
        None => return unvalued(info, UnvaluedReason::StateFetchFailed),
        Some(PoolRead::Missing) => return unvalued(info, UnvaluedReason::AccountMissing),
        Some(PoolRead::Invalid | PoolRead::VaultInvalid) => {
            return unvalued(info, UnvaluedReason::AccountInvalid);
        }
        Some(PoolRead::NoVaults(_)) => return unvalued(info, UnvaluedReason::StateFetchFailed),
        Some(PoolRead::Failed(reason)) => return unvalued(info, *reason),
        Some(PoolRead::Ok {
            pool,
            base_balance,
            quote_balance,
        }) => (pool, *base_balance, *quote_balance),
    };
    if pool.base_mint != info.mint {
        // A reversed pool (wSOL base) or a different mint.
        return unvalued(info, UnvaluedReason::VenueNotSupported);
    }
    if pool.quote_mint != WRAPPED_SOL_MINT {
        return unvalued(info, UnvaluedReason::QuoteNotSol);
    }
    let Some(
        fee @ FeeObservation::Amm {
            lp_bps,
            protocol_bps,
            creator_bps,
        },
    ) = venue.fee
    else {
        return unvalued(info, UnvaluedReason::FeeUnknown);
    };
    let Some(quote_reserve) = effective_quote_reserve(quote_balance, pool.virtual_quote_reserves)
    else {
        return unvalued(info, UnvaluedReason::MathFailure);
    };
    let Some(q) = amm_sell_quote(
        amount,
        base_balance,
        quote_reserve,
        lp_bps,
        protocol_bps,
        creator_bps,
    ) else {
        return unvalued(info, UnvaluedReason::MathFailure);
    };
    let marginal = if base_balance == 0 {
        None
    } else {
        u128::from(amount)
            .checked_mul(quote_reserve)
            .and_then(|n| n.checked_div(u128::from(base_balance)))
            .and_then(|m| u64::try_from(m).ok())
    };
    let Some(marginal) = marginal else {
        return unvalued(info, UnvaluedReason::MathFailure);
    };
    let impact = price_impact_bps(amount, u128::from(base_balance), quote_reserve, q.raw_out);
    let fees = q
        .lp_fee
        .saturating_add(q.protocol_fee)
        .saturating_add(q.creator_fee);
    PositionValuation {
        mint: info.mint,
        open_amount_raw: info.open_amount_raw,
        basis: info.basis,
        outcome: ValuationOutcome::Valued(Box::new(ValuedPosition {
            venue: Venue::PumpAmm,
            address: venue.address,
            label: LABEL_REALIZABLE_CP_QUOTE,
            account_slot: snap.account_slot,
            vault_slot: snap.vault_slot,
            gross_lamports: q.raw_out,
            fee_lamports: fees,
            realizable_lamports: q.net,
            marginal_lamports: marginal,
            price_impact_bps: impact,
            quote_reserve_lamports: quote_reserve,
            token_reserve_raw: u128::from(base_balance),
            fee,
            fee_observed_at: venue.fee_observed_at,
            unrealized_pnl: unrealized(&info.basis, q.net),
            usd: None,
            usd_unpriced_reason: None,
            usd_unrealized: None,
            usd_unrealized_reason: None,
        })),
    }
}

// ---------------------------------------------------------------------
// I/O: read state and apply to the wallets of a run
// ---------------------------------------------------------------------

/// Aggregate of one valuation run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenValuationRun {
    /// `false` when valuation was skipped for the whole run.
    pub ran: bool,
    /// The window ended before `as_of`: nothing was read.
    pub historical: bool,
    pub as_of: i64,
    /// `getMultipleAccounts` HTTP calls made (chunks), retries not counted.
    pub account_calls: u64,
    /// Highest / lowest context slot among the reads.
    pub state_slot: Option<u64>,
    pub state_slot_min: Option<u64>,
    pub wallets_with_open: u64,
    pub totals: OpenValuationTotals,
    /// The request budget was spent: affected positions are unvalued and
    /// the run is incomplete.
    pub budget_exhausted: bool,
    /// Sanitized text of the first read failure, if any.
    pub fetch_error: Option<String>,
}

/// True when open positions can be valued: the window ends at `as_of`
/// (or there is no window: full history up to the run start).
#[must_use]
pub fn window_is_live(window: &AnalysisWindow) -> bool {
    !window.is_bounded() || window.until >= window.as_of
}

fn failure_reason(kind: ScanFailureKind) -> UnvaluedReason {
    match kind {
        ScanFailureKind::BudgetExhausted { .. } => UnvaluedReason::RequestBudgetExhausted,
        ScanFailureKind::RateLimited { .. } | ScanFailureKind::Other => {
            UnvaluedReason::StateFetchFailed
        }
    }
}

struct Fetcher<'a> {
    provider: &'a HeliusProvider,
    run: &'a mut OpenValuationRun,
}

impl Fetcher<'_> {
    /// Read `addrs`; on failure every address maps to the failure reason.
    async fn read(
        &mut self,
        addrs: &[SolanaPubkey],
    ) -> Result<scout_providers::AccountsRead, UnvaluedReason> {
        if addrs.is_empty() {
            return Err(UnvaluedReason::StateFetchFailed);
        }
        match self
            .provider
            .get_multiple_accounts(addrs, VALUATION_COMMITMENT)
            .await
        {
            Ok(r) => {
                self.run.account_calls = self.run.account_calls.saturating_add(r.calls);
                self.run.state_slot = Some(self.run.state_slot.map_or(r.slot, |s| s.max(r.slot)));
                self.run.state_slot_min = Some(
                    self.run
                        .state_slot_min
                        .map_or(r.min_slot, |s| s.min(r.min_slot)),
                );
                Ok(r)
            }
            Err(e) => {
                let kind = classify_provider_error(&e);
                if matches!(kind, ScanFailureKind::BudgetExhausted { .. }) {
                    self.run.budget_exhausted = true;
                }
                if self.run.fetch_error.is_none() {
                    self.run.fetch_error = Some(crate::sanitize_provider_text(&e.to_string()));
                }
                Err(failure_reason(kind))
            }
        }
    }
}

/// Read the curve and pool state `infos` need (at most two passes of
/// `getMultipleAccounts`: accounts, then pool vaults).
async fn read_state(
    fetcher: &mut Fetcher<'_>,
    infos: &[&OpenVenueInfo],
) -> Result<StateSnapshot, UnvaluedReason> {
    let mut curves: BTreeSet<SolanaPubkey> = BTreeSet::new();
    let mut pools: BTreeSet<SolanaPubkey> = BTreeSet::new();
    for i in infos {
        match i.last_venue {
            Some(Venue::BondingCurve) => {
                curves.extend(i.curve.map(|c| c.address));
                // Fallback after graduation.
                pools.extend(i.pool.map(|p| p.address));
            }
            Some(Venue::PumpAmm) => pools.extend(i.pool.map(|p| p.address)),
            Some(Venue::Route) | None => {}
        }
    }
    let mut snap = StateSnapshot::default();
    let stage1: Vec<SolanaPubkey> = curves.iter().chain(pools.iter()).copied().collect();
    if stage1.is_empty() {
        return Ok(snap);
    }
    let read = fetcher.read(&stage1).await?;
    snap.account_slot = read.slot;
    for (addr, acct) in stage1.iter().zip(read.accounts.iter()) {
        if curves.contains(addr) {
            let state = match acct {
                None => CurveRead::Missing,
                Some(a) => decode_bonding_curve(&a.owner, &a.data)
                    .map_or(CurveRead::Invalid, |c| CurveRead::Ok(Box::new(c))),
            };
            snap.curves.insert(*addr, state);
        } else {
            let state = match acct {
                None => PoolRead::Missing,
                Some(a) => decode_pool(&a.owner, &a.data)
                    .map_or(PoolRead::Invalid, |p| PoolRead::NoVaults(Box::new(p))),
            };
            snap.pools.insert(*addr, state);
        }
    }
    // Which pools are actually needed: primary AMM positions, and the
    // fallback pool of a position whose curve graduated.
    let mut needed: BTreeSet<SolanaPubkey> = BTreeSet::new();
    for i in infos {
        let Some(pool) = i.pool else { continue };
        match i.last_venue {
            Some(Venue::PumpAmm) => {
                needed.insert(pool.address);
            }
            Some(Venue::BondingCurve) => {
                let graduated = i
                    .curve
                    .and_then(|c| snap.curves.get(&c.address))
                    .is_some_and(|r| matches!(r, CurveRead::Ok(c) if c.complete));
                if graduated {
                    needed.insert(pool.address);
                }
            }
            Some(Venue::Route) | None => {}
        }
    }
    // Vault addresses of the decoded, needed pools.
    let mut vaults: BTreeSet<SolanaPubkey> = BTreeSet::new();
    for addr in &needed {
        if let Some(PoolRead::NoVaults(p)) = snap.pools.get(addr) {
            vaults.insert(p.pool_base_token_account);
            vaults.insert(p.pool_quote_token_account);
        }
    }
    if vaults.is_empty() {
        return Ok(snap);
    }
    let vault_list: Vec<SolanaPubkey> = vaults.iter().copied().collect();
    match fetcher.read(&vault_list).await {
        Err(reason) => {
            for addr in &needed {
                if matches!(snap.pools.get(addr), Some(PoolRead::NoVaults(_))) {
                    snap.pools.insert(*addr, PoolRead::Failed(reason));
                }
            }
        }
        Ok(vread) => {
            snap.vault_slot = Some(vread.slot);
            let mut balances: BTreeMap<SolanaPubkey, Option<(SolanaPubkey, u64)>> = BTreeMap::new();
            for (addr, acct) in vault_list.iter().zip(vread.accounts.iter()) {
                let b = acct.as_ref().and_then(|a| {
                    decode_token_account(&a.owner, &a.data)
                        .ok()
                        .map(|t| (t.mint, t.amount))
                });
                balances.insert(*addr, b);
            }
            for addr in &needed {
                let Some(PoolRead::NoVaults(p)) = snap.pools.get(addr).cloned() else {
                    continue;
                };
                let base = balances.get(&p.pool_base_token_account).copied().flatten();
                let quote = balances.get(&p.pool_quote_token_account).copied().flatten();
                let state = match (base, quote) {
                    (Some((bm, ba)), Some((qm, qa))) if bm == p.base_mint && qm == p.quote_mint => {
                        PoolRead::Ok {
                            pool: p,
                            base_balance: ba,
                            quote_balance: qa,
                        }
                    }
                    _ => PoolRead::VaultInvalid,
                };
                snap.pools.insert(*addr, state);
            }
        }
    }
    Ok(snap)
}

/// ADR-019: value the open positions of every wallet ledger of a stats run
/// and store an [`OpenValuationView`] on each ledger that has positions.
///
/// * Historical windows (`until < as_of`): positions are
///   `unvalued { historical_window }` and nothing is read.
/// * Otherwise one batched read (+ one vault read) through `provider`, so
///   every HTTP attempt counts against the run's request budget; a spent
///   budget leaves the affected positions `unvalued { request_budget_exhausted }`
///   and sets [`OpenValuationRun::budget_exhausted`].
pub async fn apply_open_valuation(
    wallets: &mut [SolanaWalletStats],
    provider: &HeliusProvider,
    window: &AnalysisWindow,
) -> OpenValuationRun {
    let mut run = OpenValuationRun {
        ran: true,
        as_of: window.as_of,
        ..OpenValuationRun::default()
    };
    let live = window_is_live(window);
    run.historical = !live;
    let infos: Vec<&OpenVenueInfo> = wallets
        .iter()
        .filter_map(|w| w.ledger.as_ref())
        .flat_map(|l| l.open_venues.iter())
        .collect();
    let snapshot = if live && !infos.is_empty() {
        let mut fetcher = Fetcher {
            provider,
            run: &mut run,
        };
        Some(read_state(&mut fetcher, &infos).await)
    } else {
        None
    };
    for w in wallets.iter_mut() {
        let Some(l) = w.ledger.as_mut() else { continue };
        if l.open_venues.is_empty() {
            l.open_valuation = None;
            continue;
        }
        run.wallets_with_open += 1;
        let positions: Vec<PositionValuation> = l
            .open_venues
            .iter()
            .map(|info| match &snapshot {
                None => unvalued(info, UnvaluedReason::HistoricalWindow),
                Some(Err(reason)) => unvalued(info, *reason),
                Some(Ok(snap)) => value_position(info, snap),
            })
            .collect();
        let view = OpenValuationView {
            version: SOLANA_OPEN_VALUATION_VERSION,
            as_of: window.as_of,
            commitment: VALUATION_COMMITMENT,
            state_slot: run.state_slot,
            positions,
        };
        run.totals.merge(&view.totals());
        l.open_valuation = Some(view);
    }
    run
}

/// The minute (unix seconds) whose SOL price the USD values need, over all
/// wallets; `None` when no position is valued.
#[must_use]
pub fn open_usd_requirement(wallets: &[SolanaWalletStats]) -> Option<i64> {
    wallets
        .iter()
        .filter_map(|w| w.ledger.as_ref()?.open_valuation.as_ref())
        .find_map(OpenValuationView::usd_price_requirement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scout_dex_solana::{
        BONDING_CURVE_ACCOUNT_DISCRIMINATOR, POOL_ACCOUNT_DISCRIMINATOR, PUMP_PROGRAM_ID_BYTES,
    };

    const MINT: SolanaPubkey = [9; 32];

    fn basis(known_lamports: i128, full: bool) -> OpenBasis {
        OpenBasis {
            known_sol_basis: lamports_to_money(known_lamports).unwrap(),
            fully_known_sol: full,
        }
    }

    fn curve(complete: bool, vsol: u64, vtok: u64) -> BondingCurveAccount {
        let mut d = BONDING_CURVE_ACCOUNT_DISCRIMINATOR.to_vec();
        for v in [vtok, vsol, 0, 0, 1_000_000_000_000_000u64] {
            d.extend(v.to_le_bytes());
        }
        d.push(u8::from(complete));
        d.extend([1u8; 32]);
        decode_bonding_curve(&PUMP_PROGRAM_ID_BYTES, &d).unwrap()
    }

    fn pool(base_mint: SolanaPubkey, quote_mint: SolanaPubkey, vq: i128) -> PoolAccount {
        let mut d = POOL_ACCOUNT_DISCRIMINATOR.to_vec();
        d.push(255);
        d.extend(0u16.to_le_bytes());
        d.extend([1u8; 32]);
        d.extend(base_mint);
        d.extend(quote_mint);
        d.extend([4u8; 32]);
        d.extend([5u8; 32]);
        d.extend([6u8; 32]);
        d.extend(0u64.to_le_bytes());
        d.extend([8u8; 32]);
        d.extend([0, 0]);
        d.extend(vq.to_le_bytes());
        decode_pool(&scout_dex_solana::PUMP_AMM_PROGRAM_ID_BYTES, &d).unwrap()
    }

    const CURVE_ADDR: SolanaPubkey = [0xC; 32];
    const POOL_ADDR: SolanaPubkey = [0xA; 32];

    fn curve_fee() -> Option<FeeObservation> {
        Some(FeeObservation::Curve {
            protocol_bps: 95,
            creator_bps: 30,
        })
    }

    fn amm_fee() -> Option<FeeObservation> {
        Some(FeeObservation::Amm {
            lp_bps: 20,
            protocol_bps: 5,
            creator_bps: 95,
        })
    }

    fn info(last: Option<Venue>, with_pool: bool, fee: bool, b: OpenBasis) -> OpenVenueInfo {
        OpenVenueInfo {
            mint: MINT,
            open_amount_raw: 1_000_000,
            last_venue: last,
            curve: Some(VenueAddress {
                address: CURVE_ADDR,
                fee_observed_at: Some(1),
                fee: if fee { curve_fee() } else { None },
            }),
            pool: with_pool.then_some(VenueAddress {
                address: POOL_ADDR,
                fee_observed_at: Some(2),
                fee: if fee { amm_fee() } else { None },
            }),
            basis: b,
            lots: Vec::new(),
        }
    }

    fn snap_curve(c: CurveRead) -> StateSnapshot {
        StateSnapshot {
            account_slot: 77,
            curves: BTreeMap::from([(CURVE_ADDR, c)]),
            ..StateSnapshot::default()
        }
    }

    fn snap_pool(p: PoolRead) -> StateSnapshot {
        StateSnapshot {
            account_slot: 77,
            vault_slot: Some(78),
            pools: BTreeMap::from([(POOL_ADDR, p)]),
            ..StateSnapshot::default()
        }
    }

    fn valued(p: &PositionValuation) -> &ValuedPosition {
        p.valued()
            .unwrap_or_else(|| panic!("not valued: {:?}", p.outcome))
    }

    #[test]
    fn curve_golden_matches_the_formula_and_unrealized_is_net_minus_basis() {
        // Same numbers as swap_math::curve_sell_golden.
        let i = info(
            Some(Venue::BondingCurve),
            false,
            true,
            basis(10_000_000, true),
        );
        let s = snap_curve(CurveRead::Ok(Box::new(curve(
            false,
            30_000_000_000,
            1_000_000_000,
        ))));
        let v = value_position(&i, &s);
        let p = valued(&v);
        assert_eq!(p.label, "realizable_cp_quote");
        assert_eq!(p.venue, Venue::BondingCurve);
        assert_eq!(p.gross_lamports, 29_970_029);
        assert_eq!(p.fee_lamports, 284_716 + 89_911);
        assert_eq!(p.realizable_lamports, 29_970_029 - 284_716 - 89_911);
        assert_eq!(p.marginal_lamports, 30_000_000); // 1e6 * 3e10 / 1e9
        // (30_000_000 - 29_970_029) * 10_000 / 30_000_000 = 9 (floor of 9.99)
        assert_eq!(p.price_impact_bps, Some(9));
        assert_eq!(p.account_slot, 77);
        assert_eq!(p.vault_slot, None);
        assert_eq!(
            p.unrealized_pnl,
            Some(lamports_to_money(i128::from(p.realizable_lamports) - 10_000_000).unwrap())
        );
    }

    #[test]
    fn unrealized_is_unknown_not_zero_without_a_fully_known_sol_basis() {
        let i = info(Some(Venue::BondingCurve), false, true, basis(0, false));
        let s = snap_curve(CurveRead::Ok(Box::new(curve(
            false,
            30_000_000_000,
            1_000_000_000,
        ))));
        let v = value_position(&i, &s);
        assert!(valued(&v).unrealized_pnl.is_none());
    }

    #[test]
    fn thin_market_shows_a_large_price_impact_not_a_paper_profit() {
        // Selling 1_000_000 into a pool of 2_000_000 base / 5_000 quote.
        let i = info(Some(Venue::PumpAmm), true, true, basis(1_000, true));
        let s = snap_pool(PoolRead::Ok {
            pool: Box::new(pool(MINT, WRAPPED_SOL_MINT, 0)),
            base_balance: 2_000_000,
            quote_balance: 5_000,
        });
        let v = value_position(&i, &s);
        let p = valued(&v);
        // raw = floor(1e6*5000/3e6) = 1666; marginal = 1e6*5000/2e6 = 2500.
        assert_eq!((p.gross_lamports, p.marginal_lamports), (1666, 2500));
        assert_eq!(p.price_impact_bps, Some(3336)); // (834*10000)/2500 floor
        assert!(p.price_impact_bps.unwrap() > 3000);
    }

    #[test]
    fn amm_uses_virtual_quote_reserves_and_golden_fees() {
        let i = info(Some(Venue::PumpAmm), true, true, basis(0, true));
        let s = snap_pool(PoolRead::Ok {
            pool: Box::new(pool(MINT, WRAPPED_SOL_MINT, 1_000_000)),
            base_balance: 1_000_000,
            quote_balance: 1_000_000,
        });
        // base_in 1e6 vs 1e6 base: out = 1e6 * 2e6 / 2e6 = 1_000_000 (virtual added).
        let v = value_position(&i, &s);
        let p = valued(&v);
        assert_eq!(p.gross_lamports, 1_000_000);
        // lp 20bps=2000, proto 5bps=500, creator 95bps=9500.
        assert_eq!(p.fee_lamports, 2_000 + 500 + 9_500);
        assert_eq!(p.realizable_lamports, 1_000_000 - 12_000);
        assert_eq!(p.quote_reserve_lamports, 2_000_000);
        assert_eq!(p.vault_slot, Some(78));
        assert!(matches!(p.fee, FeeObservation::Amm { lp_bps: 20, .. }));
    }

    #[test]
    fn unvalued_reasons() {
        let b = basis(0, true);
        let ok_curve = snap_curve(CurveRead::Ok(Box::new(curve(
            false,
            30_000_000_000,
            1_000_000_000,
        ))));
        let reason = |i: &OpenVenueInfo, s: &StateSnapshot| value_position(i, s).unvalued_reason();
        // fee unknown
        assert_eq!(
            reason(&info(Some(Venue::BondingCurve), false, false, b), &ok_curve),
            Some(UnvaluedReason::FeeUnknown)
        );
        // route-only / no venue
        assert_eq!(
            reason(&info(None, false, true, b), &ok_curve),
            Some(UnvaluedReason::NoVenueObserved)
        );
        assert_eq!(
            reason(&info(Some(Venue::Route), false, true, b), &ok_curve),
            Some(UnvaluedReason::NoVenueObserved)
        );
        // migrated: curve complete, no pool known
        let done = snap_curve(CurveRead::Ok(Box::new(curve(true, 1, 1))));
        assert_eq!(
            reason(&info(Some(Venue::BondingCurve), false, true, b), &done),
            Some(UnvaluedReason::MigratedPoolUnknown)
        );
        // missing / invalid accounts
        assert_eq!(
            reason(
                &info(Some(Venue::BondingCurve), false, true, b),
                &snap_curve(CurveRead::Missing)
            ),
            Some(UnvaluedReason::AccountMissing)
        );
        assert_eq!(
            reason(
                &info(Some(Venue::BondingCurve), false, true, b),
                &snap_curve(CurveRead::Invalid)
            ),
            Some(UnvaluedReason::AccountInvalid)
        );
        // not read at all
        assert_eq!(
            reason(
                &info(Some(Venue::BondingCurve), false, true, b),
                &StateSnapshot::default()
            ),
            Some(UnvaluedReason::StateFetchFailed)
        );
        // reversed / other-mint pool and non-SOL quote
        let wrong_base = snap_pool(PoolRead::Ok {
            pool: Box::new(pool(WRAPPED_SOL_MINT, MINT, 0)),
            base_balance: 10,
            quote_balance: 10,
        });
        assert_eq!(
            reason(&info(Some(Venue::PumpAmm), true, true, b), &wrong_base),
            Some(UnvaluedReason::VenueNotSupported)
        );
        let usdc_quote = snap_pool(PoolRead::Ok {
            pool: Box::new(pool(MINT, [3; 32], 0)),
            base_balance: 10,
            quote_balance: 10,
        });
        assert_eq!(
            reason(&info(Some(Venue::PumpAmm), true, true, b), &usdc_quote),
            Some(UnvaluedReason::QuoteNotSol)
        );
        // empty reserves -> math failure
        let empty = snap_pool(PoolRead::Ok {
            pool: Box::new(pool(MINT, WRAPPED_SOL_MINT, 0)),
            base_balance: 0,
            quote_balance: 0,
        });
        assert_eq!(
            reason(&info(Some(Venue::PumpAmm), true, true, b), &empty),
            Some(UnvaluedReason::MathFailure)
        );
        // amount above u64
        let mut huge = info(Some(Venue::BondingCurve), false, true, b);
        huge.open_amount_raw = u128::from(u64::MAX) + 1;
        assert_eq!(
            reason(&huge, &ok_curve),
            Some(UnvaluedReason::AmountTooLarge)
        );
        // a failed read keeps its reason
        let failed = snap_pool(PoolRead::Failed(UnvaluedReason::RequestBudgetExhausted));
        assert_eq!(
            reason(&info(Some(Venue::PumpAmm), true, true, b), &failed),
            Some(UnvaluedReason::RequestBudgetExhausted)
        );
    }

    #[test]
    fn graduated_curve_falls_back_to_the_known_pool() {
        let b = basis(0, true);
        let i = info(Some(Venue::BondingCurve), true, true, b);
        let mut s = snap_pool(PoolRead::Ok {
            pool: Box::new(pool(MINT, WRAPPED_SOL_MINT, 0)),
            base_balance: 1_000_000,
            quote_balance: 2_000_000,
        });
        s.curves
            .insert(CURVE_ADDR, CurveRead::Ok(Box::new(curve(true, 1, 1))));
        let v = value_position(&i, &s);
        assert_eq!(valued(&v).venue, Venue::PumpAmm);
    }

    #[test]
    fn totals_count_reasons_and_known_unrealized_only() {
        let b = basis(1_000, true);
        let ok = value_position(
            &info(Some(Venue::BondingCurve), false, true, b),
            &snap_curve(CurveRead::Ok(Box::new(curve(
                false,
                30_000_000_000,
                1_000_000_000,
            )))),
        );
        let nobasis = value_position(
            &info(Some(Venue::BondingCurve), false, true, basis(0, false)),
            &snap_curve(CurveRead::Ok(Box::new(curve(
                false,
                30_000_000_000,
                1_000_000_000,
            )))),
        );
        let bad = value_position(
            &info(Some(Venue::BondingCurve), false, false, b),
            &snap_curve(CurveRead::Missing),
        );
        let view = OpenValuationView {
            version: SOLANA_OPEN_VALUATION_VERSION,
            as_of: 0,
            commitment: VALUATION_COMMITMENT,
            state_slot: Some(77),
            positions: vec![ok.clone(), nobasis, bad],
        };
        let t = view.totals();
        assert_eq!((t.positions, t.valued, t.unvalued), (3, 2, 1));
        assert_eq!(t.unrealized_known_positions, 1);
        assert_eq!(t.unrealized_unknown_positions, 1);
        assert_eq!(t.unvalued_by_reason.get("account_missing"), Some(&1));
        assert_eq!(
            t.realizable_lamports,
            2 * u128::from(valued(&ok).realizable_lamports)
        );
        assert!(!view.all_valued());
    }
}
