//! Pure, synchronous per-wallet SOL-quoted ledger for pump.fun
//! bonding-curve trades (ADR-010; fee rules from ADR-004).
//!
//! Input: the raw transactions of ONE wallet in any order, any
//! duplication. Output: [`SolanaWalletLedgerReport`]. No I/O, no clocks,
//! no floats. Every amount is an exact integer: lamports (`i128`/`u64`)
//! or raw token base units (`u64`/`u128`).
//!
//! # Rules implemented (ADR-010)
//! * §1 quote unit: lamports; `Money` holds `lamports * 10^MONEY_SCALE`
//!   (see [`lamports_to_money`], the single P0.14 boundary).
//! * §2 consideration from the paired `TradeEvent` only.
//! * §3 quote gate: v2 non-wSOL quote => PnL `Unknown`.
//! * §4 network fee: fee payer only, split proportionally to
//!   consideration, remainder to the earliest trade, exact sum.
//! * §5 native residual: diagnostic only, never PnL.
//! * §6 inventory continuity: unexplained token movement of a traded
//!   mint => `Unknown` lot / `Unknown`-proceeds disposal.
//! * §7 ordering: `(slot, transaction_index, instruction order)`; the
//!   input order is irrelevant; duplicate signatures are processed once.
//!
//! Additions beyond the ADR text (documented in the report): a sale that
//! exceeds the inventory this history observed first books the shortfall
//! as an `Unknown`-basis lot (`InventoryNotObserved`), never a fabricated
//! buy and never an error; a zero-PnL closed episode is neither a win nor
//! a loss (it is a breakeven, counted in the win-rate denominator by
//! `scout-analytics`).

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::U256;
use scout_analytics::{Episode, EpisodeCohort, RatioStatus, profit_factor, win_rate};
use scout_core::{
    AddressBytes, AssetKey, MONEY_SCALE, Money, RawAmount, RawSolanaTransaction, ScoutError,
    SolanaExecutionStatus, SolanaPubkey,
};
use scout_dex_solana::{
    BondingCurveBuyDecoder, PumpInstructionOutcome, TradeEventPairing, TradeSide,
    VariantVerification, pair_trades_with_events,
};
pub use scout_ledger::QuoteUnit;
use scout_ledger::{BasisStatus, Ledger};
use scout_normalize::{SolanaBalanceAggregationError, solana_owner_net_deltas};

use crate::solana_buy_qualification::solana_mainnet_chain;

/// Version tag of the ledger rules, for report metadata (invariant #10).
pub const SOLANA_WALLET_LEDGER_VERSION: &str = "solana-wallet-ledger/1 (ADR-010, ADR-004)";

/// Wrapped SOL, the only non-native quote asset treated as SOL (ADR-010 §3).
pub const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

/// Seconds in the activity "day" bucket (UTC, `ts.div_euclid(86_400)`).
const SECONDS_PER_DAY: i64 = 86_400;

/// Typed failure of the ledger build. Malformed *data* never lands here
/// (it is counted in the report); only arithmetic overflow and internal
/// inconsistency do.
#[derive(Debug, thiserror::Error)]
pub enum SolanaWalletLedgerError {
    #[error("arithmetic overflow: {0}")]
    Overflow(&'static str),
    #[error("ledger error: {0}")]
    Ledger(#[from] ScoutError),
    #[error("token balance aggregation failed: {0}")]
    Balance(#[from] SolanaBalanceAggregationError),
    #[error("internal wSOL mint constant is invalid")]
    InvalidWsolConstant,
}

/// P0.14 boundary: lamports -> `Money` (`lamports * 10^MONEY_SCALE`,
/// exact, checked). THE only place this workspace converts a Solana base
/// unit into `Money`; nothing else in this module scales by hand.
pub fn lamports_to_money(lamports: i128) -> Result<Money, SolanaWalletLedgerError> {
    let scaled = lamports
        .checked_mul(10i128.pow(MONEY_SCALE))
        .ok_or(SolanaWalletLedgerError::Overflow("lamports_to_money"))?;
    Ok(Money::from_scaled_units(scaled))
}

/// `Money` of a lamport ledger -> whole lamports, truncating toward zero.
/// Only partial-lot basis proration (ADR-004 C02) can leave a remainder
/// below one lamport (< 10^-8 lamport per disposal); the exact `Money`
/// stays available in the report next to every lamport figure.
#[must_use]
pub fn money_to_lamports_trunc(money: Money) -> i128 {
    let scale = 10i128.pow(MONEY_SCALE);
    let units = money.scaled_units();
    // `rem_euclid` of the magnitude keeps truncation toward zero.
    let magnitude = units.unsigned_abs().div_euclid(scale.unsigned_abs());
    let magnitude = i128::try_from(magnitude).unwrap_or(i128::MAX);
    if units.is_negative() {
        -magnitude
    } else {
        magnitude
    }
}

/// Why a figure is `Unknown` (never serialized as zero, invariant #6/#10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnknownReason {
    /// Trade instruction without a consistent paired `TradeEvent`.
    ConsiderationUnverified,
    /// v2 trade whose quote mint is not wSOL.
    UnsupportedQuoteAsset,
    /// Event fees exceed `sol_amount` (sell proceeds underflow).
    MalformedConsideration,
    /// Token inflow not explained by a decoded trade (transfer, other venue).
    UnexplainedInboundTokenMovement,
    /// Token outflow not explained by a decoded trade (router forward, transfer).
    UnexplainedOutboundTokenMovement,
    /// A disposal exceeded the inventory this history observed.
    InventoryNotObserved,
    /// A disposal consumed a lot whose basis was `Unknown`.
    UnknownBasisLotConsumed,
}

impl UnknownReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ConsiderationUnverified => "consideration unverified",
            Self::UnsupportedQuoteAsset => "unsupported quote asset",
            Self::MalformedConsideration => "malformed consideration",
            Self::UnexplainedInboundTokenMovement => "unexplained inbound token movement",
            Self::UnexplainedOutboundTokenMovement => "unexplained outbound token movement",
            Self::InventoryNotObserved => "inventory not observed before disposal",
            Self::UnknownBasisLotConsumed => "consumed unknown-basis lot",
        }
    }
}

/// Outcome of one `(wallet, mint)` inventory episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeOutcome {
    /// Every consumed basis and every proceeds known; exact PnL (lamport-scaled `Money`).
    ClosedKnown { pnl: Money },
    /// Closed, but some basis/proceeds unknown: PnL unknown, never 0.
    ClosedUnknown,
    /// Inventory still open at the end of history. Unvalued (no price source).
    Open,
}

/// Audit record of one episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeRecord {
    pub mint: SolanaPubkey,
    pub outcome: EpisodeOutcome,
    /// Event timestamps (unix seconds); `None` when the opening/closing
    /// transaction carried no verified event timestamp.
    pub opened_at: Option<i64>,
    pub closed_at: Option<i64>,
    /// `closed_at - opened_at` for closed episodes with both timestamps.
    pub holding_seconds: Option<i64>,
    /// Canonical location of the first acquisition: `(slot, transaction_index)`.
    pub opened_location: (u64, u64),
    /// Reasons that made (or, while open, would make) the episode Unknown.
    pub unknown_reasons: BTreeSet<UnknownReason>,
    /// Known disposals inside the episode and their summed PnL.
    pub known_disposals: u64,
    pub known_disposal_pnl: Money,
    /// Σ capitalized basis consumed by the known disposals above
    /// (lamport-scaled `Money`); the ROI denominator for `ClosedKnown`.
    pub known_disposal_consumed_basis: Money,
}

/// One open position at the end of history. Never carries a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPosition {
    pub mint: SolanaPubkey,
    /// Raw token base units still held per this ledger.
    pub open_amount_raw: u128,
    /// Portion of `open_amount_raw` in `Unknown`-basis lots.
    pub unknown_basis_amount_raw: u128,
    pub opened_at: Option<i64>,
}

/// Trade counters. Each decoded trade of the wallet in a successful
/// transaction lands in exactly one of the five consideration buckets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TradeCounts {
    pub buys: u64,
    pub sells: u64,
    /// Paired, SOL-quoted, consideration computed.
    pub priced: u64,
    pub unpaired: u64,
    pub mismatched: u64,
    pub unsupported_quote: u64,
    pub malformed_consideration: u64,
    /// By decoder variant evidence level.
    pub fixture_verified_variant: u64,
    pub idl_only_variant: u64,
}

/// Diagnostics: everything not modelled, counted instead of dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerDiagnostics {
    pub transactions_considered: u64,
    pub duplicate_transactions_ignored: u64,
    pub failed_transactions: u64,
    /// Pump trade instructions that failed to decode (coverage gap; not wallet-attributable).
    pub malformed_trade_instructions: u64,
    /// `TradeEvent`s not claimed by any trade instruction (coverage gap).
    pub orphan_trade_events: u64,
    /// Signed sum of wallet native flow not explained by consideration + fee (ADR-010 §5).
    pub unexplained_native_flow_lamports: i128,
    /// Successful transactions whose residual is non-zero.
    pub unexplained_native_flow_txs: u64,
    /// `(tx, mint)` wallet token movements of mints with no pump trade in the whole history.
    pub out_of_scope_token_movements: u64,
    /// Token movements of traded mints not explained by decoded trades (ADR-010 §6), incl.
    /// disposals beyond observed inventory.
    pub continuity_breaks: u64,
    /// Disposals with `Unknown` PnL / known PnL.
    pub unknown_disposals: u64,
    pub known_disposals: u64,
}

/// Cohort-gate activity metrics (P4.3); exact integers only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityMetrics {
    /// Trades carrying a verified event timestamp (the population below).
    pub timestamped_trades: u64,
    pub first_trade_timestamp: Option<i64>,
    pub last_trade_timestamp: Option<i64>,
    /// `last - first`.
    pub active_span_seconds: Option<i64>,
    /// Distinct UTC days (`ts div 86400`) with at least one timestamped trade.
    pub active_utc_days: u64,
    /// Distinct mints among timestamped trades.
    pub distinct_mints_timestamped: u64,
    /// Sum over active days of distinct mints traded that day. Mints per
    /// active day = `mint_day_pairs / active_utc_days` (rational, not computed here).
    pub mint_day_pairs: u64,
    // Trades per active day = `timestamped_trades / active_utc_days`.
}

/// The per-wallet report. Lamport fields are `i128`; `*_exact` fields are
/// the same value as lamport-scaled `Money` (differ from the truncated
/// integer only by sub-lamport proration remainder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaWalletLedgerReport {
    pub ledger_version: &'static str,
    pub quote_unit: QuoteUnit,
    pub wallet: SolanaPubkey,
    pub trades: TradeCounts,
    /// Distinct mints with at least one decoded wallet trade.
    pub distinct_mints_traded: u64,
    pub episodes: Vec<EpisodeRecord>,
    pub closed_episodes_known: u64,
    pub closed_episodes_unknown: u64,
    pub open_episodes: u64,
    /// Known closed episodes with PnL > 0 / < 0 / == 0 (zero is neither
    /// win nor loss; it stays in the win-rate denominator).
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    /// SUM A: realized PnL of KNOWN CLOSED episodes only (headline).
    pub realized_trade_pnl_lamports: i128,
    pub realized_trade_pnl_exact: Money,
    /// Σ basis of lots consumed by disposals inside KNOWN CLOSED episodes
    /// (the `realized_cost_roi` denominator, ARCHITECTURE §10):
    /// `realized_cost_roi = realized_trade_pnl / consumed_acquisition_basis`.
    /// `_exact` is lamport-scaled `Money` (use it for the exact rational);
    /// the lamport figure truncates a sub-lamport proration remainder.
    pub consumed_acquisition_basis_lamports: i128,
    pub consumed_acquisition_basis_exact: Money,
    /// SUM B (separate, NOT in A): PnL of known disposals that sit inside
    /// still-open episodes, and how many such disposals there are.
    pub open_episode_known_disposal_pnl_lamports: i128,
    pub open_episode_known_disposals: u64,
    /// ADR-004 attributable overhead: fees of failed transactions in which
    /// the wallet paid the fee for its own pump trade instruction.
    pub failed_trade_fees_lamports: i128,
    pub failed_trade_fee_txs: u64,
    /// `realized_trade_pnl - failed_trade_fees` (ADR-004), over known closed episodes.
    pub realized_net_pnl_lamports: i128,
    pub realized_net_pnl_exact: Money,
    /// Ratios at `MONEY_SCALE` over known closed episodes; no NaN/Inf.
    pub win_rate: RatioStatus<Money>,
    pub profit_factor: RatioStatus<Money>,
    /// Median holding time over known closed episodes with both
    /// timestamps (floor of the mean of the two middle values for even n).
    pub median_holding_seconds: Option<i64>,
    pub holding_time_samples: u64,
    pub open_positions: Vec<OpenPosition>,
    /// ARCHITECTURE §10 input: true when any open inventory has unknown
    /// basis or any closed episode is Unknown. Materiality is the caller's policy.
    pub has_unknown_basis_inventory: bool,
    pub open_positions_with_unknown_basis: u64,
    pub unknown_basis_lots_created: u64,
    pub activity: ActivityMetrics,
    pub diagnostics: LedgerDiagnostics,
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

type Location = (u64, u64);

#[derive(Debug, Clone, Copy)]
enum Consideration {
    /// Lamports: buy cost (`sol+fee+creator`) or sell proceeds (`sol-fee-creator`).
    Verified { lamports: u64, token_amount: u64 },
    Unknown {
        reason: UnknownReason,
        token_amount: Option<u64>,
    },
}

struct WalletTrade {
    side: TradeSide,
    mint: SolanaPubkey,
    instruction_index: u32,
    consideration: Consideration,
    timestamp: Option<i64>,
    verification: VariantVerification,
    /// The event disagreed with the instruction (vs. no event at all).
    mismatched: bool,
}

struct EpisodeAcc {
    opened_at: Option<i64>,
    opened_location: Location,
    pnl: Money,
    consumed_basis: Money,
    unknown: BTreeSet<UnknownReason>,
    known_disposals: u64,
}

struct MintState {
    ledger: Ledger,
    episode: Option<EpisodeAcc>,
}

struct Builder {
    wallet: SolanaPubkey,
    chain_asset: fn(SolanaPubkey) -> AssetKey,
    mints: BTreeMap<SolanaPubkey, MintState>,
    records: Vec<EpisodeRecord>,
    diag: LedgerDiagnostics,
    counts: TradeCounts,
    unknown_lots: u64,
    failed_fees: i128,
    failed_fee_txs: u64,
    stamped: Vec<(i64, SolanaPubkey)>,
    traded: BTreeSet<SolanaPubkey>,
}

fn asset_of(mint: SolanaPubkey) -> AssetKey {
    AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(mint))
}

fn raw(amount: u64) -> RawAmount {
    RawAmount::from_u256(U256::from(amount))
}

fn raw_to_u128(amount: RawAmount) -> Result<u128, SolanaWalletLedgerError> {
    u128::try_from(amount.as_u256()).map_err(|_| SolanaWalletLedgerError::Overflow("open amount"))
}

fn checked_add_i(a: i128, b: i128, ctx: &'static str) -> Result<i128, SolanaWalletLedgerError> {
    a.checked_add(b)
        .ok_or(SolanaWalletLedgerError::Overflow(ctx))
}

fn money_add(a: Money, b: Money) -> Result<Money, SolanaWalletLedgerError> {
    Ok(a.checked_add(&b)?)
}

impl MintState {
    fn open_u128(&self) -> Result<u128, SolanaWalletLedgerError> {
        raw_to_u128(self.ledger.open_amount())
    }
}

impl Builder {
    fn state(&mut self, mint: SolanaPubkey) -> &mut MintState {
        self.mints.entry(mint).or_insert_with(|| MintState {
            ledger: Ledger::with_quote_unit(QuoteUnit::Lamports),
            episode: None,
        })
    }

    fn acquire(
        &mut self,
        mint: SolanaPubkey,
        amount: u64,
        basis: Option<Money>,
        reason: Option<UnknownReason>,
        ts: Option<i64>,
        loc: Location,
    ) -> Result<(), SolanaWalletLedgerError> {
        if amount == 0 {
            return Ok(());
        }
        let asset = (self.chain_asset)(mint);
        if reason.is_some() {
            self.unknown_lots += 1;
        }
        let state = self.state(mint);
        if state.episode.is_none() {
            state.episode = Some(EpisodeAcc {
                opened_at: ts,
                opened_location: loc,
                pnl: Money::ZERO,
                consumed_basis: Money::ZERO,
                unknown: BTreeSet::new(),
                known_disposals: 0,
            });
        }
        let status = match reason {
            None => BasisStatus::Known,
            Some(r) => {
                if let Some(ep) = state.episode.as_mut() {
                    ep.unknown.insert(r);
                }
                BasisStatus::Unknown {
                    reason: r.label().to_string(),
                }
            }
        };
        state
            .ledger
            .acquire(asset, raw(amount), basis.unwrap_or(Money::ZERO), status);
        Ok(())
    }

    /// `gross` is `None` when proceeds are unknown.
    #[allow(clippy::too_many_arguments)]
    fn dispose(
        &mut self,
        mint: SolanaPubkey,
        amount: u64,
        gross: Option<Money>,
        sale_fee: Money,
        unknown_reason: UnknownReason,
        ts: Option<i64>,
        loc: Location,
    ) -> Result<(), SolanaWalletLedgerError> {
        if amount == 0 {
            return Ok(());
        }
        let open = self.state(mint).open_u128()?;
        let need = u128::from(amount);
        if open < need {
            let shortfall = u64::try_from(need.saturating_sub(open))
                .map_err(|_| SolanaWalletLedgerError::Overflow("shortfall"))?;
            self.diag.continuity_breaks += 1;
            self.acquire(
                mint,
                shortfall,
                None,
                Some(UnknownReason::InventoryNotObserved),
                ts,
                loc,
            )?;
        }
        let state = self.state(mint);
        let result = state
            .ledger
            .dispose(raw(amount), gross.unwrap_or(Money::ZERO), sale_fee)?;
        let ep = state
            .episode
            .as_mut()
            .ok_or(SolanaWalletLedgerError::Ledger(
                ScoutError::ArithmeticOverflow {
                    context: "dispose without open episode",
                },
            ))?;
        let known_pnl = match (gross, result.realized_trade_pnl) {
            (Some(_), Some(pnl)) => Some(pnl),
            _ => None,
        };
        if let Some(pnl) = known_pnl {
            ep.pnl = money_add(ep.pnl, pnl)?;
            ep.consumed_basis = money_add(ep.consumed_basis, result.consumed_acquisition_basis)?;
            ep.known_disposals += 1;
        } else {
            if gross.is_none() {
                ep.unknown.insert(unknown_reason);
            }
            if !result.all_basis_known {
                ep.unknown.insert(UnknownReason::UnknownBasisLotConsumed);
            }
        }
        let flat = state.open_u128()? == 0;
        let closed = if flat { state.episode.take() } else { None };
        if known_pnl.is_some() {
            self.diag.known_disposals += 1;
        } else {
            self.diag.unknown_disposals += 1;
        }
        if let Some(ep) = closed {
            self.records.push(close_record(mint, ep, ts, loc, false)?);
        }
        Ok(())
    }
}

fn close_record(
    mint: SolanaPubkey,
    ep: EpisodeAcc,
    closed_at: Option<i64>,
    _loc: Location,
    open: bool,
) -> Result<EpisodeRecord, SolanaWalletLedgerError> {
    let outcome = if open {
        EpisodeOutcome::Open
    } else if ep.unknown.is_empty() {
        EpisodeOutcome::ClosedKnown { pnl: ep.pnl }
    } else {
        EpisodeOutcome::ClosedUnknown
    };
    let closed_at = if open { None } else { closed_at };
    let holding_seconds = match (ep.opened_at, closed_at, outcome) {
        (Some(o), Some(c), EpisodeOutcome::ClosedKnown { .. }) if c >= o => c.checked_sub(o),
        _ => None,
    };
    Ok(EpisodeRecord {
        mint,
        outcome,
        opened_at: ep.opened_at,
        closed_at,
        holding_seconds,
        opened_location: ep.opened_location,
        unknown_reasons: ep.unknown,
        known_disposals: ep.known_disposals,
        known_disposal_pnl: ep.pnl,
        known_disposal_consumed_basis: ep.consumed_basis,
    })
}

/// Split `fee` across `weights` proportionally (floor), remainder to the
/// first entry (the earliest trade). The result always sums to `fee`.
/// All-zero weights put the whole fee on the first entry. Empty weights
/// => empty result (nothing to capitalize the fee into).
pub fn allocate_fee_proportionally(
    fee: u64,
    weights: &[u64],
) -> Result<Vec<u64>, SolanaWalletLedgerError> {
    if weights.is_empty() {
        return Ok(Vec::new());
    }
    let total: u128 = weights.iter().map(|w| u128::from(*w)).sum();
    let mut out = Vec::with_capacity(weights.len());
    let mut assigned: u128 = 0;
    for w in weights {
        let share = if total == 0 {
            0
        } else {
            u128::from(fee)
                .checked_mul(u128::from(*w))
                .ok_or(SolanaWalletLedgerError::Overflow("fee allocation"))?
                .div_euclid(total)
        };
        assigned += share;
        out.push(share);
    }
    let remainder = u128::from(fee).saturating_sub(assigned);
    if let Some(first) = out.first_mut() {
        *first += remainder;
    }
    out.into_iter()
        .map(|v| u64::try_from(v).map_err(|_| SolanaWalletLedgerError::Overflow("fee share")))
        .collect()
}

fn wsol_mint() -> Result<SolanaPubkey, SolanaWalletLedgerError> {
    bs58::decode(WSOL_MINT)
        .into_vec()
        .ok()
        .and_then(|v| SolanaPubkey::try_from(v).ok())
        .ok_or(SolanaWalletLedgerError::InvalidWsolConstant)
}

struct TxWork<'a> {
    tx: &'a RawSolanaTransaction,
    trades: Vec<WalletTrade>,
    malformed_trades: u64,
    orphans: u64,
}

fn classify_trades<'a>(
    tx: &'a RawSolanaTransaction,
    wallet: &SolanaPubkey,
    decoder: &BondingCurveBuyDecoder,
    wsol: &SolanaPubkey,
) -> TxWork<'a> {
    let rep = pair_trades_with_events(decoder, &tx.instructions, tx.slot, tx.transaction_index);
    let mut trades = Vec::new();
    for p in &rep.trades {
        if p.trade.user != *wallet {
            continue;
        }
        let t = &p.trade;
        let (consideration, timestamp) = match &p.pairing {
            TradeEventPairing::MissingEvent => (
                Consideration::Unknown {
                    reason: UnknownReason::ConsiderationUnverified,
                    token_amount: None,
                },
                None,
            ),
            TradeEventPairing::Mismatch { .. } => (
                Consideration::Unknown {
                    reason: UnknownReason::ConsiderationUnverified,
                    token_amount: None,
                },
                None,
            ),
            TradeEventPairing::Paired(ev) => {
                let ts = Some(ev.timestamp);
                let sol_quoted = t.quote_mint.is_none_or(|q| q == *wsol);
                let fees = ev.fee.checked_add(ev.creator_fee);
                let c = if !sol_quoted {
                    Consideration::Unknown {
                        reason: UnknownReason::UnsupportedQuoteAsset,
                        token_amount: Some(ev.token_amount),
                    }
                } else {
                    let lamports = match (t.side, fees) {
                        (TradeSide::Buy, Some(f)) => ev.sol_amount.checked_add(f),
                        (TradeSide::Sell, Some(f)) => ev.sol_amount.checked_sub(f),
                        (_, None) => None,
                    };
                    match lamports {
                        Some(lamports) => Consideration::Verified {
                            lamports,
                            token_amount: ev.token_amount,
                        },
                        None => Consideration::Unknown {
                            reason: UnknownReason::MalformedConsideration,
                            token_amount: Some(ev.token_amount),
                        },
                    }
                };
                (c, ts)
            }
        };
        trades.push(WalletTrade {
            side: t.side,
            mint: t.mint,
            instruction_index: t.instruction_index,
            consideration,
            timestamp,
            verification: t.verification(),
            mismatched: matches!(p.pairing, TradeEventPairing::Mismatch { .. }),
        });
    }
    // Pairing already yields execution order; keep it explicit.
    trades.sort_by_key(|t| t.instruction_index);
    TxWork {
        tx,
        trades,
        malformed_trades: u64::try_from(rep.malformed_trades).unwrap_or(u64::MAX),
        orphans: u64::try_from(rep.orphan_events.len()).unwrap_or(u64::MAX),
    }
}

/// Build the report. See the module docs for the rules.
pub fn build_solana_wallet_ledger(
    wallet: &SolanaPubkey,
    txs: &[RawSolanaTransaction],
    decoder: &BondingCurveBuyDecoder,
) -> Result<SolanaWalletLedgerReport, SolanaWalletLedgerError> {
    let wsol = wsol_mint()?;
    let mut b = Builder {
        wallet: *wallet,
        chain_asset: asset_of,
        mints: BTreeMap::new(),
        records: Vec::new(),
        diag: LedgerDiagnostics::default(),
        counts: TradeCounts::default(),
        unknown_lots: 0,
        failed_fees: 0,
        failed_fee_txs: 0,
        stamped: Vec::new(),
        traded: BTreeSet::new(),
    };

    // §7: canonical order, independent of input order; dedup by signature.
    let mut ordered: Vec<&RawSolanaTransaction> = txs.iter().collect();
    ordered.sort_by(|a, c| {
        (a.slot, a.transaction_index, a.signature).cmp(&(c.slot, c.transaction_index, c.signature))
    });
    let mut seen: BTreeSet<[u8; 64]> = BTreeSet::new();
    let mut unique = Vec::with_capacity(ordered.len());
    for tx in ordered {
        if seen.insert(tx.signature) {
            unique.push(tx);
        } else {
            b.diag.duplicate_transactions_ignored += 1;
        }
    }
    b.diag.transactions_considered = u64::try_from(unique.len()).unwrap_or(u64::MAX);

    // Pass 1: decode, failed-tx overhead, and the set of traded mints.
    let mut work: Vec<TxWork<'_>> = Vec::new();
    for tx in unique {
        match &tx.execution {
            SolanaExecutionStatus::Failed { .. } => {
                b.diag.failed_transactions += 1;
                let own_trade_instruction = tx.instructions.iter().any(|ix| {
                    matches!(
                        decoder.classify(ix, tx.slot, tx.transaction_index),
                        PumpInstructionOutcome::Trade(t) if t.user == *wallet
                    )
                });
                if tx.fee_payer == *wallet && own_trade_instruction {
                    b.failed_fees =
                        checked_add_i(b.failed_fees, i128::from(tx.fee_lamports), "failed fees")?;
                    b.failed_fee_txs += 1;
                }
            }
            SolanaExecutionStatus::Succeeded => {
                let w = classify_trades(tx, wallet, decoder, &wsol);
                for t in &w.trades {
                    b.traded.insert(t.mint);
                }
                work.push(w);
            }
        }
    }

    // Pass 2: apply in canonical order.
    for w in &work {
        b.apply_tx(w)?;
    }

    b.finish()
}

impl Builder {
    fn apply_tx(&mut self, w: &TxWork<'_>) -> Result<(), SolanaWalletLedgerError> {
        let tx = w.tx;
        let loc: Location = (tx.slot, tx.transaction_index);
        self.diag.malformed_trade_instructions += w.malformed_trades;
        self.diag.orphan_trade_events += w.orphans;
        let is_payer = tx.fee_payer == self.wallet;

        // §4 fee allocation over verified trades only (consideration known).
        let verified: Vec<usize> = w
            .trades
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                matches!(t.consideration, Consideration::Verified { .. }).then_some(i)
            })
            .collect();
        let weights: Vec<u64> = verified
            .iter()
            .filter_map(|i| w.trades.get(*i))
            .map(|t| match t.consideration {
                Consideration::Verified { lamports, .. } => lamports,
                Consideration::Unknown { .. } => 0,
            })
            .collect();
        let shares = if is_payer {
            allocate_fee_proportionally(tx.fee_lamports, &weights)?
        } else {
            vec![0; weights.len()]
        };
        let share_of: BTreeMap<usize, u64> = verified.iter().copied().zip(shares).collect();

        let mut explained: BTreeMap<SolanaPubkey, i128> = BTreeMap::new();
        let mut unverified_mints: BTreeSet<SolanaPubkey> = BTreeSet::new();
        let mut tx_ts: Option<i64> = None;
        let mut buy_cost: i128 = 0;
        let mut sell_proceeds: i128 = 0;

        for (i, t) in w.trades.iter().enumerate() {
            match t.side {
                TradeSide::Buy => self.counts.buys += 1,
                TradeSide::Sell => self.counts.sells += 1,
            }
            match t.verification {
                VariantVerification::FixtureVerified => self.counts.fixture_verified_variant += 1,
                VariantVerification::IdlOnly => self.counts.idl_only_variant += 1,
            }
            self.traded.insert(t.mint);
            if let Some(ts) = t.timestamp {
                self.stamped.push((ts, t.mint));
                tx_ts = tx_ts.or(Some(ts));
            }
            let fee_share = share_of.get(&i).copied().unwrap_or(0);
            match t.consideration {
                Consideration::Verified {
                    lamports,
                    token_amount,
                } => {
                    self.counts.priced += 1;
                    let signed = i128::from(token_amount);
                    match t.side {
                        TradeSide::Buy => {
                            let basis_l = checked_add_i(
                                i128::from(lamports),
                                i128::from(fee_share),
                                "buy basis",
                            )?;
                            self.acquire(
                                t.mint,
                                token_amount,
                                Some(lamports_to_money(basis_l)?),
                                None,
                                t.timestamp,
                                loc,
                            )?;
                            buy_cost = checked_add_i(buy_cost, i128::from(lamports), "buy cost")?;
                            *explained.entry(t.mint).or_insert(0) += signed;
                        }
                        TradeSide::Sell => {
                            self.dispose(
                                t.mint,
                                token_amount,
                                Some(lamports_to_money(i128::from(lamports))?),
                                lamports_to_money(i128::from(fee_share))?,
                                UnknownReason::ConsiderationUnverified,
                                t.timestamp,
                                loc,
                            )?;
                            sell_proceeds = checked_add_i(
                                sell_proceeds,
                                i128::from(lamports),
                                "sell proceeds",
                            )?;
                            *explained.entry(t.mint).or_insert(0) -= signed;
                        }
                    }
                }
                Consideration::Unknown {
                    reason,
                    token_amount,
                } => {
                    match reason {
                        UnknownReason::UnsupportedQuoteAsset => self.counts.unsupported_quote += 1,
                        UnknownReason::MalformedConsideration => {
                            self.counts.malformed_consideration += 1;
                        }
                        _ => {
                            if t.mismatched {
                                self.counts.mismatched += 1;
                            } else {
                                self.counts.unpaired += 1;
                            }
                        }
                    }
                    match token_amount {
                        Some(amount) => {
                            let signed = i128::from(amount);
                            match t.side {
                                TradeSide::Buy => {
                                    self.acquire(
                                        t.mint,
                                        amount,
                                        None,
                                        Some(reason),
                                        t.timestamp,
                                        loc,
                                    )?;
                                    *explained.entry(t.mint).or_insert(0) += signed;
                                }
                                TradeSide::Sell => {
                                    self.dispose(
                                        t.mint,
                                        amount,
                                        None,
                                        Money::ZERO,
                                        reason,
                                        t.timestamp,
                                        loc,
                                    )?;
                                    *explained.entry(t.mint).or_insert(0) -= signed;
                                }
                            }
                        }
                        None => {
                            unverified_mints.insert(t.mint);
                        }
                    }
                }
            }
        }

        // §6 inventory continuity.
        let deltas = solana_owner_net_deltas(&tx.token_balance_changes)?;
        let mut candidates: BTreeSet<SolanaPubkey> = w.trades.iter().map(|t| t.mint).collect();
        for ((mint, owner), delta) in &deltas.deltas {
            if *owner != self.wallet || *delta == 0 {
                continue;
            }
            if self.traded.contains(mint) {
                candidates.insert(*mint);
            } else {
                self.diag.out_of_scope_token_movements += 1;
            }
        }
        for mint in candidates {
            let delta = deltas
                .deltas
                .get(&(mint, self.wallet))
                .copied()
                .unwrap_or(0);
            let diff = delta
                .checked_sub(explained.get(&mint).copied().unwrap_or(0))
                .ok_or(SolanaWalletLedgerError::Overflow("continuity diff"))?;
            if diff == 0 {
                continue;
            }
            let unverified = unverified_mints.contains(&mint);
            if !unverified {
                self.diag.continuity_breaks += 1;
            }
            let amount = u64::try_from(diff.unsigned_abs())
                .map_err(|_| SolanaWalletLedgerError::Overflow("continuity amount"))?;
            if diff > 0 {
                let reason = if unverified {
                    UnknownReason::ConsiderationUnverified
                } else {
                    UnknownReason::UnexplainedInboundTokenMovement
                };
                self.acquire(mint, amount, None, Some(reason), tx_ts, loc)?;
            } else {
                let reason = if unverified {
                    UnknownReason::ConsiderationUnverified
                } else {
                    UnknownReason::UnexplainedOutboundTokenMovement
                };
                self.dispose(mint, amount, None, Money::ZERO, reason, tx_ts, loc)?;
            }
        }

        // §5 native residual (diagnostic only).
        let wallet_delta: i128 = tx
            .native_balance_changes
            .iter()
            .filter(|c| c.account == self.wallet)
            .map(|c| c.delta())
            .sum();
        let fee = if is_payer {
            i128::from(tx.fee_lamports)
        } else {
            0
        };
        let residual = checked_add_i(
            checked_add_i(wallet_delta, buy_cost, "residual")?,
            checked_add_i(-sell_proceeds, fee, "residual")?,
            "residual",
        )?;
        if residual != 0 {
            self.diag.unexplained_native_flow_lamports = checked_add_i(
                self.diag.unexplained_native_flow_lamports,
                residual,
                "unexplained native flow",
            )?;
            self.diag.unexplained_native_flow_txs += 1;
        }
        Ok(())
    }
}

impl Builder {
    fn finish(mut self) -> Result<SolanaWalletLedgerReport, SolanaWalletLedgerError> {
        // Open episodes and positions at the end of history.
        let mut open_positions = Vec::new();
        let mints: Vec<SolanaPubkey> = self.mints.keys().copied().collect();
        for mint in mints {
            let Some(state) = self.mints.get_mut(&mint) else {
                continue;
            };
            let Some(ep) = state.episode.take() else {
                continue;
            };
            let open_amount_raw = raw_to_u128(state.ledger.open_amount())?;
            let unknown_basis_amount_raw = match state.ledger.open_unknown_basis_amount() {
                Some(a) => raw_to_u128(a)?,
                None => 0,
            };
            open_positions.push(OpenPosition {
                mint,
                open_amount_raw,
                unknown_basis_amount_raw,
                opened_at: ep.opened_at,
            });
            self.records
                .push(close_record(mint, ep, None, (0, 0), true)?);
        }
        self.records.sort_by_key(|r| (r.opened_location, r.mint));

        let mut closed_known = 0u64;
        let mut closed_unknown = 0u64;
        let mut open_eps = 0u64;
        let (mut wins, mut losses, mut breakeven) = (0u64, 0u64, 0u64);
        let mut pnl_sum = Money::ZERO;
        let mut basis_sum = Money::ZERO;
        let mut open_pnl = Money::ZERO;
        let mut open_known_disposals = 0u64;
        let mut cohort = EpisodeCohort::default();
        let mut holds: Vec<i64> = Vec::new();
        for r in &self.records {
            match r.outcome {
                EpisodeOutcome::ClosedKnown { pnl } => {
                    closed_known += 1;
                    pnl_sum = money_add(pnl_sum, pnl)?;
                    basis_sum = money_add(basis_sum, r.known_disposal_consumed_basis)?;
                    if pnl.is_positive() {
                        wins += 1;
                    } else if pnl.is_negative() {
                        losses += 1;
                    } else {
                        breakeven += 1;
                    }
                    cohort.episodes.push(Episode {
                        asset: asset_of(r.mint),
                        realized_pnl: pnl,
                        is_closed_within_window: true,
                        opened_before_window: false,
                        has_unresolved_flows: false,
                    });
                    if let Some(h) = r.holding_seconds {
                        holds.push(h);
                    }
                }
                EpisodeOutcome::ClosedUnknown => closed_unknown += 1,
                EpisodeOutcome::Open => {
                    open_eps += 1;
                    open_pnl = money_add(open_pnl, r.known_disposal_pnl)?;
                    open_known_disposals += r.known_disposals;
                }
            }
        }
        let win_rate = win_rate(&cohort)?;
        let profit_factor = profit_factor(&cohort)?;
        holds.sort_unstable();
        let median_holding_seconds = median(&holds);
        let holding_time_samples = u64::try_from(holds.len()).unwrap_or(u64::MAX);

        let failed = lamports_to_money(self.failed_fees)?;
        let net = pnl_sum.checked_sub(&failed)?;

        let open_with_unknown = u64::try_from(
            open_positions
                .iter()
                .filter(|p| p.unknown_basis_amount_raw > 0)
                .count(),
        )
        .unwrap_or(u64::MAX);

        let activity = activity_metrics(&self.stamped)?;

        Ok(SolanaWalletLedgerReport {
            ledger_version: SOLANA_WALLET_LEDGER_VERSION,
            quote_unit: QuoteUnit::Lamports,
            wallet: self.wallet,
            trades: self.counts,
            distinct_mints_traded: u64::try_from(self.traded.len()).unwrap_or(u64::MAX),
            episodes: self.records,
            closed_episodes_known: closed_known,
            closed_episodes_unknown: closed_unknown,
            open_episodes: open_eps,
            wins,
            losses,
            breakeven,
            realized_trade_pnl_lamports: money_to_lamports_trunc(pnl_sum),
            realized_trade_pnl_exact: pnl_sum,
            consumed_acquisition_basis_lamports: money_to_lamports_trunc(basis_sum),
            consumed_acquisition_basis_exact: basis_sum,
            open_episode_known_disposal_pnl_lamports: money_to_lamports_trunc(open_pnl),
            open_episode_known_disposals: open_known_disposals,
            failed_trade_fees_lamports: self.failed_fees,
            failed_trade_fee_txs: self.failed_fee_txs,
            realized_net_pnl_lamports: money_to_lamports_trunc(net),
            realized_net_pnl_exact: net,
            win_rate,
            profit_factor,
            median_holding_seconds,
            holding_time_samples,
            has_unknown_basis_inventory: open_with_unknown > 0 || closed_unknown > 0,
            open_positions_with_unknown_basis: open_with_unknown,
            open_positions,
            unknown_basis_lots_created: self.unknown_lots,
            activity,
            diagnostics: self.diag,
        })
    }
}

fn median(sorted: &[i64]) -> Option<i64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    let mid = n.div_euclid(2);
    if n.rem_euclid(2) == 1 {
        sorted.get(mid).copied()
    } else {
        let a = i128::from(*sorted.get(mid.checked_sub(1)?)?);
        let b = i128::from(*sorted.get(mid)?);
        i64::try_from((a + b).div_euclid(2)).ok()
    }
}

fn activity_metrics(
    stamped: &[(i64, SolanaPubkey)],
) -> Result<ActivityMetrics, SolanaWalletLedgerError> {
    let mut m = ActivityMetrics {
        timestamped_trades: u64::try_from(stamped.len()).unwrap_or(u64::MAX),
        ..ActivityMetrics::default()
    };
    let first = stamped.iter().map(|(t, _)| *t).min();
    let last = stamped.iter().map(|(t, _)| *t).max();
    m.first_trade_timestamp = first;
    m.last_trade_timestamp = last;
    if let (Some(f), Some(l)) = (first, last) {
        m.active_span_seconds = Some(
            l.checked_sub(f)
                .ok_or(SolanaWalletLedgerError::Overflow("active span"))?,
        );
    }
    let days: BTreeSet<i64> = stamped
        .iter()
        .map(|(t, _)| t.div_euclid(SECONDS_PER_DAY))
        .collect();
    let pairs: BTreeSet<(i64, SolanaPubkey)> = stamped
        .iter()
        .map(|(t, mint)| (t.div_euclid(SECONDS_PER_DAY), *mint))
        .collect();
    let mints: BTreeSet<SolanaPubkey> = stamped.iter().map(|(_, mint)| *mint).collect();
    m.active_utc_days = u64::try_from(days.len()).unwrap_or(u64::MAX);
    m.mint_day_pairs = u64::try_from(pairs.len()).unwrap_or(u64::MAX);
    m.distinct_mints_timestamped = u64::try_from(mints.len()).unwrap_or(u64::MAX);
    Ok(m)
}
