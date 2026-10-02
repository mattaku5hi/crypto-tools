//! Pure, synchronous per-wallet SOL-quoted ledger for pump.fun
//! bonding-curve and PumpSwap AMM trades (ADR-010, ADR-012; fee rules
//! from ADR-004).
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
//! # Two venues (ADR-012)
//! Both decoders feed one venue-agnostic internal `WalletTrade` (venue,
//! variant, verification, traded mint, side in TOKEN terms, token amount,
//! SOL consideration or an `Unknown` reason). The bonding-curve extractor
//! is the ADR-010 one; the PumpSwap extractor takes the consideration from
//! the paired event (ADR-012 §1/§2, normal and reversed pools) and gates
//! attribution on `reconcile_pump_amm_transaction` (§3). Everything after
//! extraction (fee split over ALL the wallet's trades of the tx, FIFO per
//! `(wallet, mint)`, episodes, continuity, left-censoring) is shared, so a
//! curve buy followed by an AMM sell is one episode.
//!
//! # Windowed mode (ADR-011)
//! With [`LedgerOptions::left_censoring`] the input is assumed to be the
//! wallet's transactions inside an analysis window `[since, until)`. A
//! disposal beyond the inventory the window observed is then
//! `LeftCensored` (the inventory predates the window), not
//! `InventoryNotObserved`; the episode is `EpisodeOutcome::LeftCensored`
//! unless an in-window cause already makes it `ClosedUnknown`. Known
//! sums and ratios are over `ClosedKnown` only, as before.
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
    AmmAttribution, AmmTradeEventPairing, BondingCurveBuyDecoder, PairedAmmTrade, PumpAmmDecoder,
    PumpAmmEvent, PumpAmmInstructionOutcome, PumpInstructionOutcome, TradeEventPairing, TradeSide,
    VariantVerification, WRAPPED_SOL_MINT, pair_trades_with_events, reconcile_pump_amm_transaction,
};
pub use scout_ledger::QuoteUnit;
use scout_ledger::{BasisStatus, Ledger};
use scout_normalize::{SolanaBalanceAggregationError, solana_owner_net_deltas};

use crate::solana_buy_qualification::solana_mainnet_chain;

/// Version tag of the ledger rules, for report metadata (invariant #10).
pub const SOLANA_WALLET_LEDGER_VERSION: &str =
    "solana-wallet-ledger/3 (ADR-010, ADR-004, ADR-011 left-censoring, ADR-012 PumpSwap AMM)";

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
    /// Windowed run (ADR-011): a disposal exceeded the in-window inventory
    /// because the inventory predates the window.
    LeftCensored,
    /// ADR-012 §3: the wallet's base-token leg matches the trade but its
    /// quote leg is exactly zero: another account of the transaction paid
    /// (or received) the quote asset. Basis/proceeds unknown, never 0.
    QuoteFundedByAnotherAccount,
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
            Self::LeftCensored => "inventory predates the analysis window (left-censored)",
            Self::QuoteFundedByAnotherAccount => {
                "quote leg settled by another account (funded/received elsewhere)"
            }
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
    /// Windowed run (ADR-011): closed, a consumed lot predates the window
    /// and no in-window cause made it unknown. Counted, never valued.
    LeftCensored,
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
    /// Raw token units booked as left-censored shortfall in this episode.
    pub left_censored_amount_raw: u128,
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

/// Where a trade was executed (ADR-012 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Venue {
    BondingCurve,
    PumpAmm,
}

impl Venue {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BondingCurve => "bonding_curve",
            Self::PumpAmm => "pump_amm",
        }
    }
}

/// Trades of one venue by side IN TOKEN TERMS (`buys` acquire the traded
/// token, `sells` dispose it; for a reversed PumpSwap pool this is the
/// inverse of the instruction side).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VenueSideCounts {
    pub buys: u64,
    pub sells: u64,
}

/// Trades of one `(venue, variant)` with its evidence level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariantTradeCount {
    pub venue: Venue,
    /// IDL instruction name.
    pub variant: &'static str,
    pub verification: VariantVerification,
    pub trades: u64,
}

/// Trade counters. Each decoded trade of the wallet in a successful
/// transaction lands in exactly one of the consideration buckets
/// (`priced`, `unpaired`, `mismatched`, `unsupported_quote`,
/// `malformed_consideration`, `quote_funded_elsewhere`, `unreconciled`).
/// `buys`/`sells` are the totals over both venues, side in token terms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TradeCounts {
    pub buys: u64,
    pub sells: u64,
    pub bonding_curve: VenueSideCounts,
    pub pump_amm: VenueSideCounts,
    /// ADR-012 §3: wallet's quote leg was zero; Unknown basis/proceeds.
    pub quote_funded_elsewhere: u64,
    /// PumpSwap trade whose wallet base leg did not match the paired event
    /// (token movement left to inventory continuity).
    pub unreconciled: u64,
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
    /// `(tx, mint)` wallet token movements of mints never traded on either venue (ADR-012 §5);
    /// wSOL in a transaction with a PumpSwap trade is the quote asset and not counted.
    pub out_of_scope_token_movements: u64,
    /// Token movements of traded mints not explained by decoded trades (ADR-010 §6), incl.
    /// disposals beyond observed inventory.
    pub continuity_breaks: u64,
    /// Disposals with `Unknown` PnL / known PnL.
    pub unknown_disposals: u64,
    pub known_disposals: u64,
    /// Windowed runs: disposals that exceeded the in-window inventory
    /// (booked `LeftCensored`; NOT counted in `continuity_breaks`).
    pub left_censored_disposals: u64,
    /// ADR-012 §3: PumpSwap trades whose decoded `user` moved nothing on
    /// either leg (router/relayer forward) in a transaction the wallet
    /// signed, or whose `user` is the wallet with a zero net movement.
    /// Not attributed; the token transfer is handled by continuity.
    pub router_forward_trades_not_attributed: u64,
    /// ADR-012 §3: PumpSwap trades of the wallet whose quote was funded by
    /// another account (booked Unknown, or unbookable on a reversed pool).
    pub quote_funded_elsewhere_trades: u64,
    /// PumpSwap trades of the wallet on pools whose base mint is wSOL.
    pub reversed_pool_trades: u64,
    /// Cohort signal, NOT PnL: successful transactions the wallet signed in
    /// which at least two distinct swap-venue programs (see
    /// [`SWAP_VENUE_PROGRAM_IDS`]) occur or the wallet has at least two
    /// decoded trade legs, the wallet's owner-keyed net token delta is 0
    /// for every non-wSOL mint, and its SOL (native + wSOL) delta is not 0.
    /// This is the footprint of an atomic arbitrage round trip.
    pub atomic_round_trip_txs: u64,
    /// Signed sum over those transactions of the wallet's native lamport
    /// delta (already net of the network fee when it paid it) plus its
    /// owner-keyed wSOL delta.
    pub atomic_round_trip_sol_lamports: i128,
}

/// Program ids counted as swap venues by the atomic round-trip signal:
/// PumpSwap, pump.fun bonding curve, Meteora DLMM, Raydium CLMM, Raydium
/// CPMM, Orca Whirlpool. Address labels only (observed as pools CPI'd by
/// routers in the 2026-10-02 router-wallet fixtures); this is not a decode
/// or support claim (invariant 16). Aggregators/routers (Jupiter and
/// unidentified programs) are deliberately absent: one route through one
/// pool is not a multi-venue round trip.
pub const SWAP_VENUE_PROGRAM_IDS: [&str; 6] = [
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
    "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
    "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
    "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
    "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
];

/// Ledger build options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerOptions {
    /// ADR-011 windowed mode: shortfall disposals are `LeftCensored`.
    pub left_censoring: bool,
}

/// Activity of one UTC day (`ts div 86400`), timestamped trades only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyActivity {
    pub day: i64,
    pub trades: u64,
    pub distinct_mints: u64,
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
    /// Per `(venue, variant)` trade counts with verification status
    /// (ADR-012 §5), sorted by venue then variant name.
    pub variant_trades: Vec<VariantTradeCount>,
    /// Distinct mints with at least one decoded wallet trade.
    pub distinct_mints_traded: u64,
    pub episodes: Vec<EpisodeRecord>,
    pub closed_episodes_known: u64,
    pub closed_episodes_unknown: u64,
    /// ADR-011: closed episodes whose inventory predates the window.
    pub left_censored_episodes: u64,
    /// Σ raw token units booked as left-censored shortfall.
    pub left_censored_amount_raw: u128,
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
    /// Only in-window causes (ADR-011 §5); left-censoring is excluded.
    pub has_unknown_basis_inventory: bool,
    /// Any left-censored episode / shortfall (windowed runs).
    pub has_left_censored_inventory: bool,
    pub open_positions_with_unknown_basis: u64,
    pub unknown_basis_lots_created: u64,
    pub activity: ActivityMetrics,
    /// Per active UTC day, ascending (ADR-011 §6 evidence on incomplete scans).
    pub daily_activity: Vec<DailyActivity>,
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

/// Venue-agnostic trade of the wallet (ADR-012). Produced by one extractor
/// per venue; consumed by the shared ledger logic.
struct WalletTrade {
    venue: Venue,
    /// IDL instruction name of the decoded variant.
    variant: &'static str,
    /// Side in TOKEN terms: `Buy` acquires `mint`, `Sell` disposes it.
    side: TradeSide,
    /// The traded (non-SOL) token.
    mint: SolanaPubkey,
    /// Flattened instruction order inside the transaction.
    instruction_index: u32,
    consideration: Consideration,
    timestamp: Option<i64>,
    verification: VariantVerification,
    /// The event disagreed with the instruction (vs. no event at all).
    mismatched: bool,
    /// PumpSwap: paired, but the wallet's base leg did not match the event.
    unreconciled: bool,
    /// PumpSwap pool whose base mint is wSOL (ADR-012 §2).
    reversed_pool: bool,
}

struct EpisodeAcc {
    opened_at: Option<i64>,
    opened_location: Location,
    pnl: Money,
    consumed_basis: Money,
    unknown: BTreeSet<UnknownReason>,
    known_disposals: u64,
    left_censored_raw: u128,
}

struct MintState {
    ledger: Ledger,
    episode: Option<EpisodeAcc>,
}

struct Builder {
    wallet: SolanaPubkey,
    left_censoring: bool,
    left_censored_total: u128,
    chain_asset: fn(SolanaPubkey) -> AssetKey,
    mints: BTreeMap<SolanaPubkey, MintState>,
    records: Vec<EpisodeRecord>,
    diag: LedgerDiagnostics,
    counts: TradeCounts,
    variant_counts: BTreeMap<(Venue, &'static str), (VariantVerification, u64)>,
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
        if reason.is_some_and(|r| r != UnknownReason::LeftCensored) {
            self.unknown_lots += 1;
        }
        if reason == Some(UnknownReason::LeftCensored) {
            self.left_censored_total = self.left_censored_total.saturating_add(u128::from(amount));
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
                left_censored_raw: 0,
            });
        }
        let status = match reason {
            None => BasisStatus::Known,
            Some(r) => {
                if let Some(ep) = state.episode.as_mut() {
                    ep.unknown.insert(r);
                    if r == UnknownReason::LeftCensored {
                        ep.left_censored_raw =
                            ep.left_censored_raw.saturating_add(u128::from(amount));
                    }
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
            let reason = if self.left_censoring {
                self.diag.left_censored_disposals += 1;
                UnknownReason::LeftCensored
            } else {
                self.diag.continuity_breaks += 1;
                UnknownReason::InventoryNotObserved
            };
            self.acquire(mint, shortfall, None, Some(reason), ts, loc)?;
        }
        let state = self.state(mint);
        let (_, consumes_other_unknown) = consumed_unknown_kinds(&state.ledger, need)?;
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
            if !result.all_basis_known && consumes_other_unknown {
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

/// FIFO preview of a disposal of `need` units: `(consumes a LeftCensored
/// lot, consumes any other Unknown-basis lot)`.
fn consumed_unknown_kinds(
    ledger: &Ledger,
    need: u128,
) -> Result<(bool, bool), SolanaWalletLedgerError> {
    let censored_label = UnknownReason::LeftCensored.label();
    let mut left = need;
    let (mut censored, mut other) = (false, false);
    for lot in ledger.open_lots() {
        if left == 0 {
            break;
        }
        let take = raw_to_u128(lot.remaining_amount)?.min(left);
        if take == 0 {
            continue;
        }
        left -= take;
        if let BasisStatus::Unknown { reason } = &lot.basis_status {
            if reason == censored_label {
                censored = true;
            } else {
                other = true;
            }
        }
    }
    Ok((censored, other))
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
    } else if ep.unknown.iter().any(|r| *r != UnknownReason::LeftCensored) {
        EpisodeOutcome::ClosedUnknown
    } else if ep.unknown.contains(&UnknownReason::LeftCensored) {
        EpisodeOutcome::LeftCensored
    } else {
        EpisodeOutcome::ClosedKnown { pnl: ep.pnl }
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
        left_censored_amount_raw: ep.left_censored_raw,
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
    /// ADR-012 §3 counters of PumpSwap trades that were NOT booked.
    router_forwards: u64,
    qfe_unbooked: u64,
    /// Mints moved by router-forwarded trades of a tx the wallet signed:
    /// they become continuity candidates (their transfer to the wallet is
    /// an unexplained token movement of a venue mint, not "out of scope").
    forward_mints: BTreeSet<SolanaPubkey>,
}

/// Bonding-curve extractor (ADR-010 §2/§3), behaviour unchanged.
fn extract_curve_trades(
    tx: &RawSolanaTransaction,
    wallet: &SolanaPubkey,
    decoder: &BondingCurveBuyDecoder,
    wsol: &SolanaPubkey,
    work: &mut TxWork<'_>,
) {
    let rep = pair_trades_with_events(decoder, &tx.instructions, tx.slot, tx.transaction_index);
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
        work.trades.push(WalletTrade {
            venue: Venue::BondingCurve,
            variant: t.variant.name(),
            side: t.side,
            mint: t.mint,
            instruction_index: t.instruction_index,
            consideration,
            timestamp,
            verification: t.verification(),
            mismatched: matches!(p.pairing, TradeEventPairing::Mismatch { .. }),
            unreconciled: false,
            reversed_pool: false,
        });
    }
    work.malformed_trades = work
        .malformed_trades
        .saturating_add(u64::try_from(rep.malformed_trades).unwrap_or(u64::MAX));
    work.orphans = work
        .orphans
        .saturating_add(u64::try_from(rep.orphan_events.len()).unwrap_or(u64::MAX));
}

/// Owner-keyed net wSOL token delta of `owner` (all of its wSOL accounts).
fn wsol_token_delta(
    changes: &[scout_core::SolanaTokenBalanceChange],
    owner: &SolanaPubkey,
) -> i128 {
    changes
        .iter()
        .filter(|c| c.owner.as_ref() == Some(owner) && c.mint == WRAPPED_SOL_MINT)
        .map(|c| i128::from(c.post_amount) - i128::from(c.pre_amount.unwrap_or(0)))
        .sum()
}

fn amm_event_timestamp(ev: &PumpAmmEvent) -> i64 {
    match ev {
        PumpAmmEvent::Buy(e) => e.timestamp,
        PumpAmmEvent::Sell(e) => e.timestamp,
    }
}

/// Economic reading of one PumpSwap trade (ADR-012 §2).
struct AmmReading {
    mint: SolanaPubkey,
    /// Side in token terms.
    side: TradeSide,
    reversed: bool,
    wsol_pair: bool,
}

fn read_amm_trade(p: &PairedAmmTrade) -> AmmReading {
    let t = &p.trade;
    let base_wsol = t.base_mint == WRAPPED_SOL_MINT;
    let quote_wsol = t.quote_mint == WRAPPED_SOL_MINT;
    match (base_wsol, quote_wsol) {
        // Normal pool: token = base, instruction side is the token side.
        (false, true) => AmmReading {
            mint: t.base_mint,
            side: t.side,
            reversed: false,
            wsol_pair: true,
        },
        // Reversed pool: token = quote, the instruction side is inverted.
        (true, false) => AmmReading {
            mint: t.quote_mint,
            side: match t.side {
                TradeSide::Buy => TradeSide::Sell,
                TradeSide::Sell => TradeSide::Buy,
            },
            reversed: true,
            wsol_pair: true,
        },
        // Non-SOL pair (or wSOL/wSOL): recorded, PnL unknown.
        _ => AmmReading {
            mint: t.base_mint,
            side: t.side,
            reversed: false,
            wsol_pair: false,
        },
    }
}

/// `(token_amount, lamports)` of a paired trade on a SOL pool, or `None`
/// when the event arithmetic is malformed. Token amount and SOL amount are
/// the signed legs of ADR-012 §1/§2 in raw units.
fn amm_amounts(ev: &PumpAmmEvent, reading: &AmmReading) -> Option<(u64, u64)> {
    let quote = ev.quote_consideration()?;
    let base = ev.base_amount();
    Some(if reading.reversed {
        // base = wSOL leg, quote = token leg.
        (quote, base)
    } else {
        (base, quote)
    })
}

/// PumpSwap extractor (ADR-012 §1-3). Legs come from
/// `reconcile_pump_amm_transaction`; nothing is re-derived here.
fn extract_amm_trades(
    tx: &RawSolanaTransaction,
    wallet: &SolanaPubkey,
    decoder: &PumpAmmDecoder,
    work: &mut TxWork<'_>,
) {
    let rec = reconcile_pump_amm_transaction(decoder, tx);
    work.malformed_trades = work
        .malformed_trades
        .saturating_add(u64::try_from(rec.pairing.malformed_trades).unwrap_or(u64::MAX));
    work.orphans = work
        .orphans
        .saturating_add(u64::try_from(rec.pairing.orphan_events.len()).unwrap_or(u64::MAX));
    let wallet_signed = tx.signers.contains(wallet);
    for u in &rec.users {
        if u.user != *wallet {
            // Router-forward guard: another account executed the trade and
            // moved nothing; the wallet signed the transaction.
            if wallet_signed && !u.user_is_signer && u.attribution == AmmAttribution::NoUserDelta {
                for &i in &u.trade_indices {
                    work.router_forwards = work.router_forwards.saturating_add(1);
                    if let Some(p) = rec.pairing.trades.get(i) {
                        let r = read_amm_trade(p);
                        if r.wsol_pair {
                            work.forward_mints.insert(r.mint);
                        }
                    }
                }
            }
            continue;
        }
        for &i in &u.trade_indices {
            let Some(p) = rec.pairing.trades.get(i) else {
                continue;
            };
            let reading = read_amm_trade(p);
            let base = |consideration, timestamp, mismatched, unreconciled| WalletTrade {
                venue: Venue::PumpAmm,
                variant: p.trade.variant.name(),
                side: reading.side,
                mint: reading.mint,
                instruction_index: p.trade.instruction_index,
                consideration,
                timestamp,
                verification: p.trade.verification(),
                mismatched,
                unreconciled,
                reversed_pool: reading.reversed,
            };
            let unverified = Consideration::Unknown {
                reason: UnknownReason::ConsiderationUnverified,
                token_amount: None,
            };
            let trade = match &p.pairing {
                AmmTradeEventPairing::MissingEvent => base(unverified, None, false, false),
                AmmTradeEventPairing::Mismatch { .. } => base(unverified, None, true, false),
                AmmTradeEventPairing::Paired(ev) => {
                    let ts = Some(amm_event_timestamp(ev));
                    match amm_amounts(ev, &reading) {
                        None => {
                            // Event arithmetic over/underflow: tokens known from
                            // the event's base leg only on a normal pool.
                            let token_amount = (!reading.reversed).then(|| ev.base_amount());
                            base(
                                Consideration::Unknown {
                                    reason: UnknownReason::MalformedConsideration,
                                    token_amount,
                                },
                                ts,
                                false,
                                false,
                            )
                        }
                        Some((token_amount, lamports)) => match u.attribution {
                            AmmAttribution::NoUserDelta => {
                                work.router_forwards = work.router_forwards.saturating_add(1);
                                continue;
                            }
                            AmmAttribution::Unpaired | AmmAttribution::ConsiderationInvalid => {
                                base(unverified, ts, false, false)
                            }
                            AmmAttribution::BaseLegMismatch => base(unverified, ts, false, true),
                            AmmAttribution::Exact | AmmAttribution::QuoteResidual
                                if !reading.wsol_pair =>
                            {
                                base(
                                    Consideration::Unknown {
                                        reason: UnknownReason::UnsupportedQuoteAsset,
                                        token_amount: Some(ev.base_amount()),
                                    },
                                    ts,
                                    false,
                                    false,
                                )
                            }
                            AmmAttribution::Exact | AmmAttribution::QuoteResidual => base(
                                Consideration::Verified {
                                    lamports,
                                    token_amount,
                                },
                                ts,
                                false,
                                false,
                            ),
                            AmmAttribution::QuoteFundedElsewhere => {
                                if reading.reversed {
                                    // The token leg is the zero one: no inventory
                                    // change to book; the SOL paid/received stays
                                    // a native-residual diagnostic.
                                    work.qfe_unbooked = work.qfe_unbooked.saturating_add(1);
                                    continue;
                                }
                                if reading.wsol_pair {
                                    base(
                                        Consideration::Unknown {
                                            reason: UnknownReason::QuoteFundedByAnotherAccount,
                                            token_amount: Some(token_amount),
                                        },
                                        ts,
                                        false,
                                        false,
                                    )
                                } else {
                                    base(
                                        Consideration::Unknown {
                                            reason: UnknownReason::UnsupportedQuoteAsset,
                                            token_amount: Some(ev.base_amount()),
                                        },
                                        ts,
                                        false,
                                        false,
                                    )
                                }
                            }
                        },
                    }
                }
            };
            work.trades.push(trade);
        }
    }
}

/// PumpSwap AMM decoder for Solana mainnet (ADR-012).
#[must_use]
pub fn pump_amm_decoder() -> PumpAmmDecoder {
    PumpAmmDecoder::mainnet()
}

/// The two venue decoders feeding one ledger (ADR-012 §5).
#[derive(Debug, Clone, Copy)]
pub struct LedgerDecoders<'a> {
    pub curve: &'a BondingCurveBuyDecoder,
    /// `None` = bonding-curve-only run (pre-ADR-012 behaviour).
    pub amm: Option<&'a PumpAmmDecoder>,
}

fn classify_trades<'a>(
    tx: &'a RawSolanaTransaction,
    wallet: &SolanaPubkey,
    decoders: &LedgerDecoders<'_>,
    wsol: &SolanaPubkey,
) -> TxWork<'a> {
    let mut work = TxWork {
        tx,
        trades: Vec::new(),
        malformed_trades: 0,
        orphans: 0,
        router_forwards: 0,
        qfe_unbooked: 0,
        forward_mints: BTreeSet::new(),
    };
    extract_curve_trades(tx, wallet, decoders.curve, wsol, &mut work);
    if let Some(amm) = decoders.amm {
        extract_amm_trades(tx, wallet, amm, &mut work);
    }
    // Both extractors yield execution order; merge across venues by the
    // flattened instruction index.
    work.trades.sort_by_key(|t| t.instruction_index);
    work
}

/// Build the report with default options (full history, no window),
/// bonding-curve venue only.
pub fn build_solana_wallet_ledger(
    wallet: &SolanaPubkey,
    txs: &[RawSolanaTransaction],
    decoder: &BondingCurveBuyDecoder,
) -> Result<SolanaWalletLedgerReport, SolanaWalletLedgerError> {
    build_solana_wallet_ledger_with_options(wallet, txs, decoder, LedgerOptions::default())
}

/// Bonding-curve-only build (wrapper of [`build_solana_wallet_ledger_venues`]).
pub fn build_solana_wallet_ledger_with_options(
    wallet: &SolanaPubkey,
    txs: &[RawSolanaTransaction],
    decoder: &BondingCurveBuyDecoder,
    options: LedgerOptions,
) -> Result<SolanaWalletLedgerReport, SolanaWalletLedgerError> {
    build_solana_wallet_ledger_venues(
        wallet,
        txs,
        &LedgerDecoders {
            curve: decoder,
            amm: None,
        },
        options,
    )
}

/// Build the report over both venues. See the module docs for the rules.
pub fn build_solana_wallet_ledger_venues(
    wallet: &SolanaPubkey,
    txs: &[RawSolanaTransaction],
    decoders: &LedgerDecoders<'_>,
    options: LedgerOptions,
) -> Result<SolanaWalletLedgerReport, SolanaWalletLedgerError> {
    let wsol = wsol_mint()?;
    let mut b = Builder {
        wallet: *wallet,
        left_censoring: options.left_censoring,
        left_censored_total: 0,
        chain_asset: asset_of,
        mints: BTreeMap::new(),
        records: Vec::new(),
        diag: LedgerDiagnostics::default(),
        counts: TradeCounts::default(),
        variant_counts: BTreeMap::new(),
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
                        decoders.curve.classify(ix, tx.slot, tx.transaction_index),
                        PumpInstructionOutcome::Trade(t) if t.user == *wallet
                    ) || decoders.amm.is_some_and(|amm| {
                        matches!(
                            amm.classify(ix, tx.slot, tx.transaction_index),
                            PumpAmmInstructionOutcome::Trade(t) if t.user == *wallet
                        )
                    })
                });
                if tx.fee_payer == *wallet && own_trade_instruction {
                    b.failed_fees =
                        checked_add_i(b.failed_fees, i128::from(tx.fee_lamports), "failed fees")?;
                    b.failed_fee_txs += 1;
                }
            }
            SolanaExecutionStatus::Succeeded => {
                let w = classify_trades(tx, wallet, decoders, &wsol);
                for t in &w.trades {
                    b.traded.insert(t.mint);
                }
                b.traded.extend(w.forward_mints.iter().copied());
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
        self.diag.router_forward_trades_not_attributed += w.router_forwards;
        self.diag.quote_funded_elsewhere_trades += w.qfe_unbooked;
        let is_payer = tx.fee_payer == self.wallet;
        self.record_atomic_round_trip(w)?;

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
            let venue_counts = match t.venue {
                Venue::BondingCurve => &mut self.counts.bonding_curve,
                Venue::PumpAmm => &mut self.counts.pump_amm,
            };
            match t.side {
                TradeSide::Buy => {
                    self.counts.buys += 1;
                    venue_counts.buys += 1;
                }
                TradeSide::Sell => {
                    self.counts.sells += 1;
                    venue_counts.sells += 1;
                }
            }
            match t.verification {
                VariantVerification::FixtureVerified => self.counts.fixture_verified_variant += 1,
                VariantVerification::IdlOnly => self.counts.idl_only_variant += 1,
            }
            self.variant_counts
                .entry((t.venue, t.variant))
                .or_insert((t.verification, 0))
                .1 += 1;
            if t.reversed_pool {
                self.diag.reversed_pool_trades += 1;
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
                        UnknownReason::QuoteFundedByAnotherAccount => {
                            self.counts.quote_funded_elsewhere += 1;
                            self.diag.quote_funded_elsewhere_trades += 1;
                        }
                        _ => {
                            if t.unreconciled {
                                self.counts.unreconciled += 1;
                            } else if t.mismatched {
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
        let has_amm_trade = w.trades.iter().any(|t| t.venue == Venue::PumpAmm);
        let mut candidates: BTreeSet<SolanaPubkey> = w.trades.iter().map(|t| t.mint).collect();
        for ((mint, owner), delta) in &deltas.deltas {
            if *owner != self.wallet || *delta == 0 {
                continue;
            }
            if self.traded.contains(mint) {
                candidates.insert(*mint);
            } else if *mint == WRAPPED_SOL_MINT && has_amm_trade {
                // The quote asset of a PumpSwap trade; its flow is part of
                // the native residual above, not an out-of-scope token.
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
        let mut wallet_delta: i128 = tx
            .native_balance_changes
            .iter()
            .filter(|c| c.account == self.wallet)
            .map(|c| c.delta())
            .sum();
        // ADR-012 §3: PumpSwap pays in wSOL; a wSOL account that survives the
        // transaction holds part of the SOL flow as a token delta. Only
        // transactions with a PumpSwap trade of the wallet include it, so
        // the bonding-curve residual stays exactly as before.
        if w.trades.iter().any(|t| t.venue == Venue::PumpAmm) {
            wallet_delta = checked_add_i(
                wallet_delta,
                wsol_token_delta(&tx.token_balance_changes, &self.wallet),
                "wsol delta",
            )?;
        }
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
    /// Cohort signal, see [`LedgerDiagnostics::atomic_round_trip_txs`].
    /// Diagnostic only: never feeds lots, episodes or PnL.
    fn record_atomic_round_trip(&mut self, w: &TxWork<'_>) -> Result<(), SolanaWalletLedgerError> {
        let tx = w.tx;
        if !tx.signers.contains(&self.wallet) {
            return Ok(());
        }
        let venues: BTreeSet<SolanaPubkey> = tx
            .instructions
            .iter()
            .filter(|ix| {
                SWAP_VENUE_PROGRAM_IDS.iter().any(|id| {
                    bs58::decode(id)
                        .into_vec()
                        .is_ok_and(|b| b.as_slice() == ix.program_id.as_slice())
                })
            })
            .map(|ix| ix.program_id)
            .collect();
        if venues.len() < 2 && w.trades.len() < 2 {
            return Ok(());
        }
        let deltas = solana_owner_net_deltas(&tx.token_balance_changes)?;
        let moved_other_mint = deltas.deltas.iter().any(|((mint, owner), delta)| {
            *owner == self.wallet && *mint != WRAPPED_SOL_MINT && *delta != 0
        });
        if moved_other_mint {
            return Ok(());
        }
        let native: i128 = tx
            .native_balance_changes
            .iter()
            .filter(|c| c.account == self.wallet)
            .map(|c| c.delta())
            .sum();
        let sol = checked_add_i(
            native,
            wsol_token_delta(&tx.token_balance_changes, &self.wallet),
            "atomic round trip sol",
        )?;
        if sol != 0 {
            self.diag.atomic_round_trip_txs += 1;
            self.diag.atomic_round_trip_sol_lamports = checked_add_i(
                self.diag.atomic_round_trip_sol_lamports,
                sol,
                "atomic round trip total",
            )?;
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
        let mut left_censored = 0u64;
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
                EpisodeOutcome::LeftCensored => left_censored += 1,
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
        let daily_activity = daily_activity(&self.stamped);

        Ok(SolanaWalletLedgerReport {
            ledger_version: SOLANA_WALLET_LEDGER_VERSION,
            quote_unit: QuoteUnit::Lamports,
            wallet: self.wallet,
            trades: self.counts,
            variant_trades: self
                .variant_counts
                .iter()
                .map(
                    |((venue, variant), (verification, trades))| VariantTradeCount {
                        venue: *venue,
                        variant,
                        verification: *verification,
                        trades: *trades,
                    },
                )
                .collect(),
            distinct_mints_traded: u64::try_from(self.traded.len()).unwrap_or(u64::MAX),
            episodes: self.records,
            closed_episodes_known: closed_known,
            closed_episodes_unknown: closed_unknown,
            left_censored_episodes: left_censored,
            left_censored_amount_raw: self.left_censored_total,
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
            has_left_censored_inventory: left_censored > 0 || self.left_censored_total > 0,
            open_positions_with_unknown_basis: open_with_unknown,
            open_positions,
            unknown_basis_lots_created: self.unknown_lots,
            activity,
            daily_activity,
            diagnostics: self.diag,
        })
    }
}

fn daily_activity(stamped: &[(i64, SolanaPubkey)]) -> Vec<DailyActivity> {
    let mut days: BTreeMap<i64, (u64, BTreeSet<SolanaPubkey>)> = BTreeMap::new();
    for (t, mint) in stamped {
        let entry = days.entry(t.div_euclid(SECONDS_PER_DAY)).or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1.insert(*mint);
    }
    days.into_iter()
        .map(|(day, (trades, mints))| DailyActivity {
            day,
            trades,
            distinct_mints: u64::try_from(mints.len()).unwrap_or(u64::MAX),
        })
        .collect()
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
