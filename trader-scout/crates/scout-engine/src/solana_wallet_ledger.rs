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
//! # Route swaps and quote units (ADR-013)
//! * Event pricing guard (§1): an event trade is priced from its paired
//!   event only when the wallet's non-zero owner-keyed deltas involve no
//!   asset other than the traded token(s) and SOL/wSOL and, for buys, the
//!   wallet's SOL outflow is at least the event cost. Otherwise the event
//!   is one hop of someone else's route and is not the wallet's price.
//! * Route swap (§2): wallet is a signer, a FixtureVerified decoded leg
//!   (pump.fun curve, PumpSwap, or -- ADR-015, full venue runs only -- a
//!   Jupiter v6 `SwapEvent`/`SwapsEvent`, DFlow v4 `SwapEvent` leg, or a FixtureVerified OKX DEX Router
//!   order event naming the wallet as source and destination owner) trades token `T`, the wallet's non-zero deltas are exactly `T` and one
//!   quote asset `Q` (SOL, USDC, USDT) with opposite signs, and every other
//!   leg `user` is a non-signing pass-through netting zero on every mint.
//!   The trade is booked from the wallet's own deltas in `Q`'s unit.
//! * Quote units (§3/§4): every lot carries its unit; one FIFO per
//!   `(wallet, mint)`; a disposal against lots of another unit is
//!   `Unknown { CrossQuoteUnit }`; PnL/basis/ROI/PF are per unit and never
//!   summed across units.
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
    AmmAttribution, AmmTradeEventPairing, BondingCurveBuyDecoder, DflowEventDecoder,
    DflowEventOutcome, JupiterEventDecoder, JupiterEventOutcome, OkxEventDecoder, OkxEventOutcome,
    OkxOrderEventKind, PairedAmmTrade, PumpAmmDecoder, PumpAmmEvent, PumpAmmInstructionOutcome,
    PumpInstructionOutcome, TradeEventPairing, TradeSide, VariantVerification, WRAPPED_SOL_MINT,
    dflow_swap_event_verification, pair_trades_with_events, reconcile_pump_amm_transaction,
};
pub use scout_ledger::QuoteUnit;
use scout_ledger::{BasisStatus, Ledger};
use scout_normalize::{SolanaBalanceAggregationError, solana_owner_net_deltas};

use crate::decode_evidence::{DecodeEvidence, scan_tx_evidence};
use crate::solana_buy_qualification::{
    VariantPolicy, default_variant_policy, solana_mainnet_chain,
};

/// Version tag of the ledger rules, for report metadata (invariant #10).
pub const SOLANA_WALLET_LEDGER_VERSION: &str = "solana-wallet-ledger/10 (OKX DEX Router SwapWithFeesCpiEvent2 FixtureVerified legs + ownership evidence (ADR-017), ADR-016 unknown-episode lower bounds, ADR-010, ADR-004, ADR-011 left-censoring, ADR-012 PumpSwap AMM, ADR-013 route swaps + quote units, ADR-009 PumpSwap 26-byte track_volume trades now priced, ADR-015 Jupiter route legs, ADR-015 amendment DFlow v4 route legs)";

/// Scope text for report metadata (invariant #10): allowed quote units and
/// the route-swap rule of ADR-013.
pub const SOLANA_WALLET_LEDGER_SCOPE: &str = "quote units: SOL (lamports, native+wSOL), USDC (6 dp raw), USDT (6 dp raw); no FX, per-unit PnL never summed; route swap = signer wallet, FixtureVerified decoded leg (pump curve/PumpSwap, or a Jupiter v6 / DFlow Aggregator v4 swap event hop trading the token, ADR-015, or a FixtureVerified OKX DEX Router order event of the signer that trades the token, ADR-017; an order event with a distinct receiver is attributed to nobody), exactly one traded token and one quote asset with opposite signs, other leg users non-signing zero-net pass-through (ADR-013 section 2)";

/// Wrapped SOL, the only non-native quote asset treated as SOL (ADR-010 §3).
pub const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

/// USDC mint (ADR-013 §3).
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// USDT mint (ADR-013 §3).
pub const USDT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";

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
    #[error("internal quote mint constant is invalid")]
    InvalidQuoteMintConstant,
    #[error("quote unit has no Solana base-unit scale: {0:?}")]
    UnsupportedQuoteUnit(QuoteUnit),
}

/// Decimal places of one raw base unit of a Solana quote unit (SOL 9,
/// USDC/USDT 6). `None` for `ReportCurrency`, which has no base unit.
#[must_use]
pub const fn quote_unit_decimals(unit: QuoteUnit) -> Option<u32> {
    match unit {
        QuoteUnit::Lamports => Some(9),
        QuoteUnit::UsdcUnits | QuoteUnit::UsdtUnits => Some(6),
        QuoteUnit::ReportCurrency => None,
    }
}

/// Short label (`sol` / `usdc` / `usdt`) of a Solana quote unit.
#[must_use]
pub const fn quote_unit_label(unit: QuoteUnit) -> &'static str {
    match unit {
        QuoteUnit::Lamports => "sol",
        QuoteUnit::UsdcUnits => "usdc",
        QuoteUnit::UsdtUnits => "usdt",
        QuoteUnit::ReportCurrency => "report_currency",
    }
}

/// The Solana quote units in report order.
pub const SOLANA_QUOTE_UNITS: [QuoteUnit; 3] = [
    QuoteUnit::Lamports,
    QuoteUnit::UsdcUnits,
    QuoteUnit::UsdtUnits,
];

/// P0.14 boundary: raw base units of `unit` -> `Money`
/// (`raw * 10^MONEY_SCALE`, exact, checked). THE only place this workspace
/// converts a Solana quote base unit into `Money`; nothing else in this
/// module scales by hand. The scaling is the same for every unit (the unit
/// is a tag, ADR-013 §3: no FX), so ledgers of different units stay
/// distinguishable only by their tag.
pub fn quote_units_to_money(unit: QuoteUnit, raw: i128) -> Result<Money, SolanaWalletLedgerError> {
    if quote_unit_decimals(unit).is_none() {
        return Err(SolanaWalletLedgerError::UnsupportedQuoteUnit(unit));
    }
    let scaled = raw
        .checked_mul(10i128.pow(MONEY_SCALE))
        .ok_or(SolanaWalletLedgerError::Overflow("quote_units_to_money"))?;
    Ok(Money::from_scaled_units(scaled))
}

/// P0.14 boundary, SOL flavour: lamports -> `Money`. Same boundary as
/// [`quote_units_to_money`].
pub fn lamports_to_money(lamports: i128) -> Result<Money, SolanaWalletLedgerError> {
    quote_units_to_money(QuoteUnit::Lamports, lamports)
}

/// `Money` of a ledger of `unit` -> whole raw base units, truncating toward
/// zero (the exact `Money` stays available next to every raw figure).
#[must_use]
pub fn money_to_quote_units_trunc(money: Money) -> i128 {
    let scale = 10i128.pow(MONEY_SCALE);
    let units = money.scaled_units();
    let magnitude = units.unsigned_abs().div_euclid(scale.unsigned_abs());
    let magnitude = i128::try_from(magnitude).unwrap_or(i128::MAX);
    if units.is_negative() {
        -magnitude
    } else {
        magnitude
    }
}

/// Exact decimal string of `Money` of a ledger of `unit` (SOL 9 dp, USDC /
/// USDT 6 dp). Sub-base-unit proration remainders (< 10^-8 base unit) are
/// truncated toward zero. `None` for `ReportCurrency`.
#[must_use]
pub fn format_quote_money(unit: QuoteUnit, money: Money) -> Option<String> {
    let decimals = quote_unit_decimals(unit)?;
    let raw = money_to_quote_units_trunc(money);
    let neg = raw.is_negative();
    let mag = raw.unsigned_abs();
    let div = 10u128.pow(decimals);
    let whole = mag.div_euclid(div);
    let frac = mag.rem_euclid(div);
    let width = usize::try_from(decimals).unwrap_or(9);
    Some(format!(
        "{}{whole}.{frac:0width$}",
        if neg { "-" } else { "" }
    ))
}

/// `Money` of a lamport ledger -> whole lamports, truncating toward zero.
/// Only partial-lot basis proration (ADR-004 C02) can leave a remainder
/// below one lamport (< 10^-8 lamport per disposal); the exact `Money`
/// stays available in the report next to every lamport figure.
#[must_use]
pub fn money_to_lamports_trunc(money: Money) -> i128 {
    money_to_quote_units_trunc(money)
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
    /// ADR-013 §4: a disposal consumed lots whose basis is in another quote
    /// unit than its proceeds (or one episode realized in two units).
    CrossQuoteUnit,
    /// ADR-013 §1: the paired event is one hop of a route, not the wallet's
    /// price, and the route rule did not apply. Never a partial-hop price.
    RouteLegNotWalletPrice,
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
            Self::CrossQuoteUnit => "basis and proceeds in different quote units",
            Self::RouteLegNotWalletPrice => "event is a route leg, not the wallet's price",
        }
    }
}

/// ADR-016: is the acquisition basis consumed by an episode fully known?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumedBasisStatus {
    /// Every lot consumed by the episode's disposals had a known basis.
    Known,
    /// At least one consumed lot had an unknown basis.
    PartiallyUnknown,
}

impl ConsumedBasisStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::PartiallyUnknown => "partially_unknown",
        }
    }
}

/// ADR-016: worst-case PnL bound of one `ClosedUnknown` episode.
///
/// Proceeds are unknown but never negative, so PnL >= -(consumed known
/// basis). The bound is exact (`Money`), in the single quote unit of the
/// consumed lots. It is [`Unbounded`](Self::Unbounded) when any consumed
/// lot had an unknown basis, or when the consumed lots span more than one
/// quote unit (documented conservative rule: such an episode taints every
/// unit block instead of guessing which unit absorbs the loss).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodePnlBound {
    /// `lower_bound <= 0`, denominated in `unit`.
    Bounded {
        unit: QuoteUnit,
        lower_bound: Money,
    },
    Unbounded,
}

/// ADR-016: a lower bound that may not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LowerBound<T> {
    Bounded(T),
    Unbounded,
}

impl<T> LowerBound<T> {
    #[must_use]
    pub const fn bounded(&self) -> Option<&T> {
        match self {
            Self::Bounded(v) => Some(v),
            Self::Unbounded => None,
        }
    }
}

/// ADR-016: exact `wins / (closed_known + closed_unknown)`; every unknown
/// closed episode counts as a non-win.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinRateLowerBound {
    pub wins: u64,
    pub episodes: u64,
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
    /// Quote unit of the episode's known disposals (`None` when it has none).
    pub quote_unit: Option<QuoteUnit>,
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
    /// ADR-016: whether every lot consumed by the episode's disposals had a
    /// known basis.
    pub consumed_basis_status: ConsumedBasisStatus,
    /// ADR-016: capitalized basis of the KNOWN-basis lots consumed by ALL
    /// disposals of the episode (known and unknown ones), per lot quote
    /// unit (ascending, no duplicates).
    pub consumed_known_basis_by_unit: Vec<(QuoteUnit, Money)>,
    /// ADR-016: worst-case PnL bound; `Some` exactly for `ClosedUnknown`.
    pub unknown_pnl_bound: Option<EpisodePnlBound>,
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
    /// ADR-013 route swap: booked from the wallet's own deltas; the decoded
    /// pump legs are only evidence that the transaction is a swap.
    Route,
}

impl Venue {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BondingCurve => "bonding_curve",
            Self::PumpAmm => "pump_amm",
            Self::Route => "route",
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

/// A counter per Solana quote unit (ADR-013 §3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuoteUnitCounts {
    pub sol: u64,
    pub usdc: u64,
    pub usdt: u64,
}

impl QuoteUnitCounts {
    fn bump(&mut self, unit: QuoteUnit) {
        match unit {
            QuoteUnit::Lamports => self.sol += 1,
            QuoteUnit::UsdcUnits => self.usdc += 1,
            QuoteUnit::UsdtUnits => self.usdt += 1,
            QuoteUnit::ReportCurrency => {}
        }
    }

    #[must_use]
    pub fn get(&self, unit: QuoteUnit) -> u64 {
        match unit {
            QuoteUnit::Lamports => self.sol,
            QuoteUnit::UsdcUnits => self.usdc,
            QuoteUnit::UsdtUnits => self.usdt,
            QuoteUnit::ReportCurrency => 0,
        }
    }
}

/// Why a transaction with decoded swap legs was NOT booked as a route swap
/// (ADR-013 §2), counted instead of dropped. Each such transaction lands in
/// the first failing bucket only; transactions where the wallet moved
/// nothing are not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteRejections {
    /// §2a: the wallet moved value but did not sign.
    pub wallet_not_signer: u64,
    /// §2c: more than one traded token / quote asset, an unsupported quote
    /// asset, or a wallet event trade of another mint.
    pub multi_asset: u64,
    /// §2c: token and quote delta have the same sign.
    pub not_opposite_signs: u64,
    /// §2c: SOL-quoted candidate whose SOL (native+wSOL) delta is zero.
    pub no_quote_leg: u64,
    /// §2b: no FixtureVerified decoded leg trades the wallet's token.
    pub no_verified_leg: u64,
    /// §2d: another leg user signs or does not net zero on every mint.
    pub passthrough_nonzero: u64,
}

/// ADR-015: route swaps by the evidence that proved a swap leg of the
/// token. A swap proven by several sources counts in each (non-exclusive);
/// `jupiter_only` / `dflow_only` are the swaps booked ONLY because of a
/// Jupiter / DFlow leg.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteEvidenceCounts {
    pub curve: u64,
    pub pump_amm: u64,
    pub jupiter: u64,
    pub jupiter_only: u64,
    /// ADR-015 amendment: DFlow Aggregator v4 swap-event legs.
    pub dflow: u64,
    pub dflow_only: u64,
    /// ADR-017: OKX DEX Router order-event legs (owner = wallet or a
    /// zero-net pass-through), and the swaps booked ONLY because of one.
    pub okx: u64,
    pub okx_only: u64,
    /// OKX swaps whose order event names the wallet itself as owner.
    pub okx_owner_is_wallet: u64,
}

/// Evidence sources of one booked route swap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RouteEvidence {
    curve: bool,
    pump_amm: bool,
    jupiter: bool,
    dflow: bool,
    okx: bool,
    okx_owner_is_wallet: bool,
}

impl RouteEvidenceCounts {
    fn bump(&mut self, e: RouteEvidence) {
        self.curve += u64::from(e.curve);
        self.pump_amm += u64::from(e.pump_amm);
        self.jupiter += u64::from(e.jupiter);
        self.dflow += u64::from(e.dflow);
        // `*_only`: booked ONLY because of that aggregator's leg.
        self.okx += u64::from(e.okx);
        self.okx_owner_is_wallet += u64::from(e.okx_owner_is_wallet);
        let other_than = |jup: bool, dfl: bool, okx: bool| {
            !e.curve && !e.pump_amm && e.jupiter == jup && e.dflow == dfl && e.okx == okx
        };
        self.jupiter_only += u64::from(e.jupiter && other_than(true, false, false));
        self.dflow_only += u64::from(e.dflow && other_than(false, true, false));
        self.okx_only += u64::from(e.okx && other_than(false, false, true));
    }
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
    /// ADR-013 route swaps booked from wallet deltas (also counted in `priced`).
    pub route: VenueSideCounts,
    /// ADR-013 §2: route swaps booked, in total and by quote unit.
    pub route_swaps: u64,
    pub route_swaps_by_quote: QuoteUnitCounts,
    /// ADR-015: route swaps by evidence source (curve / pumpswap / jupiter / dflow).
    pub route_swaps_by_evidence: RouteEvidenceCounts,
    /// ADR-013 §1: event trades whose event is not the wallet's price and
    /// that no route rule explained (Unknown consideration).
    pub route_leg_not_wallet_price: u64,
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
    /// ADR-013 §2 route-swap candidates that were not booked, by reason.
    pub route_rejected: RouteRejections,
    /// Instructions/events of a known program (curve, PumpSwap) whose
    /// discriminator is in no table. COVERAGE GAP (samples in
    /// `SolanaWalletLedgerReport::evidence_samples`).
    pub unknown_discriminator_instructions: u64,
    /// ADR-015: Jupiter event-CPIs that did not decode exactly (never
    /// trusted as swap evidence). COVERAGE GAP.
    pub jupiter_malformed_events: u64,
    /// ADR-015: Jupiter event-CPIs with a discriminator outside the known set.
    pub jupiter_unknown_events: u64,
    /// ADR-015 amendment: DFlow event-CPIs that did not decode exactly
    /// (never trusted as swap evidence). COVERAGE GAP.
    pub dflow_malformed_events: u64,
    /// DFlow event-CPIs with a discriminator outside the known set.
    pub dflow_unknown_events: u64,
    /// ADR-017: OKX DEX Router event-CPIs that did not decode exactly
    /// (never trusted as swap evidence). COVERAGE GAP.
    pub okx_malformed_events: u64,
    /// OKX event-CPIs with a discriminator outside the IDL events.
    pub okx_unknown_events: u64,
    /// OKX order events whose source and destination token-account owners
    /// differ (swap with receiver): attributed to neither owner, never leg
    /// evidence. Counted once per event. Informational (a known undercount,
    /// as for ADR-009's router-forward case), not a Partial reason.
    pub okx_swap_with_receiver_not_attributed: u64,
    /// OKX order events of a variant that is not `FixtureVerified`: decoded,
    /// counted, never leg evidence. Informational (not a Partial reason): the
    /// transaction may be explained by another leg; the route-shaped
    /// transactions this leaves unbooked surface in the existing unknown /
    /// continuity diagnostics.
    pub okx_idl_only_order_events: u64,
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
    ///
    /// ADR-013: the legacy figures below (`realized_*`, `consumed_*`,
    /// `open_episode_known_disposal_pnl_lamports`, `profit_factor`) are the
    /// SOL block's values (lamports); other units are in `unit_blocks`.
    /// `closed_episodes_known`, `wins`, `losses`, `breakeven` and
    /// `win_rate` are over ALL units (sign of each episode's PnL in its own
    /// unit).
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
    /// ADR-016: `wins / (closed_known + closed_unknown)` over all units;
    /// `None` when there is no closed known/unknown episode.
    pub win_rate_lower_bound: Option<WinRateLowerBound>,
    /// Any left-censored episode / shortfall (windowed runs).
    pub has_left_censored_inventory: bool,
    pub open_positions_with_unknown_basis: u64,
    pub unknown_basis_lots_created: u64,
    pub activity: ActivityMetrics,
    /// Per active UTC day, ascending (ADR-011 §6 evidence on incomplete scans).
    pub daily_activity: Vec<DailyActivity>,
    pub diagnostics: LedgerDiagnostics,
    /// ADR-013 §4: one block per Solana quote unit (SOL, USDC, USDT, in that
    /// order). Figures of different blocks are never summed.
    pub unit_blocks: Vec<QuoteUnitBlock>,
    /// Booked route swaps in canonical order (ADR-013 audit trail).
    pub route_swap_log: Vec<RouteSwapRecord>,
    /// At most 5 samples (canonical order) of malformed trade instructions,
    /// unknown discriminators and orphan events, so a coverage gap can be
    /// located (signature, program, discriminator, lengths, reason).
    pub evidence_samples: Vec<DecodeEvidence>,
}

/// Audit record of one booked route swap (ADR-013 §2): the wallet's own
/// deltas, exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSwapRecord {
    pub signature: [u8; 64],
    pub slot: u64,
    pub transaction_index: u64,
    pub mint: SolanaPubkey,
    /// Side in token terms.
    pub side: TradeSide,
    pub unit: QuoteUnit,
    /// `|ΔT|` raw token units.
    pub token_amount: u64,
    /// `|ΔQ|` raw units of `unit` (paid for a buy, received for a sell).
    pub quote_amount: u64,
}

/// Realized figures of the known closed episodes of ONE quote unit
/// (ADR-013 §4). `Money` fields are raw-base-unit-scaled (`raw * 10^8`);
/// render with [`format_quote_money`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteUnitBlock {
    pub unit: QuoteUnit,
    pub closed_episodes_known: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    pub realized_trade_pnl_exact: Money,
    pub realized_trade_pnl_raw: i128,
    pub consumed_acquisition_basis_exact: Money,
    pub consumed_acquisition_basis_raw: i128,
    /// PnL of known disposals inside still-open episodes of this unit.
    pub open_episode_known_disposal_pnl_raw: i128,
    pub open_episode_known_disposals: u64,
    pub win_rate: RatioStatus<Money>,
    pub profit_factor: RatioStatus<Money>,
    /// ADR-016: Σ positive / Σ |negative| episode PnL of the known closed
    /// episodes of this unit (the profit-factor parts, exact).
    pub gross_profit_exact: Money,
    pub gross_loss_abs_exact: Money,
    /// ADR-016: Σ worst-case bounds (each <= 0) of the `ClosedUnknown`
    /// episodes bounded in this unit, or `Unbounded` if ANY unknown episode
    /// of the wallet is unbounded (the taint applies to every unit block).
    pub unknown_pnl_bound: LowerBound<Money>,
}

impl QuoteUnitBlock {
    /// ADR-016: `realized_trade_pnl + Σ unknown-episode bounds` (exact).
    #[must_use]
    pub fn realized_pnl_lower_bound(&self) -> LowerBound<Money> {
        match self.unknown_pnl_bound {
            LowerBound::Unbounded => LowerBound::Unbounded,
            LowerBound::Bounded(b) => self
                .realized_trade_pnl_exact
                .checked_add(&b)
                .map_or(LowerBound::Unbounded, LowerBound::Bounded),
        }
    }

    /// ADR-016: raw-unit (truncated) form of [`Self::realized_pnl_lower_bound`].
    #[must_use]
    pub fn realized_pnl_lower_bound_raw(&self) -> LowerBound<i128> {
        match self.realized_pnl_lower_bound() {
            LowerBound::Bounded(m) => LowerBound::Bounded(money_to_quote_units_trunc(m)),
            LowerBound::Unbounded => LowerBound::Unbounded,
        }
    }

    /// ADR-016: consumed basis including the known basis of the bounded
    /// unknown episodes (the lower-bound ROI denominator).
    #[must_use]
    pub fn consumed_basis_with_unknown_exact(&self) -> Option<Money> {
        match self.unknown_pnl_bound {
            LowerBound::Unbounded => None,
            LowerBound::Bounded(b) => self.consumed_acquisition_basis_exact.checked_sub(&b).ok(),
        }
    }

    /// ADR-016: `gross_profit / (gross_loss + Σ |unknown bounds|)` with the
    /// profit-factor semantics (Money scale, floored). `base` is the known
    /// profit factor, returned unchanged when there is nothing to add.
    #[must_use]
    pub fn profit_factor_lower_bound_from(
        &self,
        base: RatioStatus<Money>,
    ) -> LowerBound<RatioStatus<Money>> {
        let b = match self.unknown_pnl_bound {
            LowerBound::Unbounded => return LowerBound::Unbounded,
            LowerBound::Bounded(b) => b,
        };
        if b.is_zero() {
            return LowerBound::Bounded(base);
        }
        let Ok(loss) = self.gross_loss_abs_exact.checked_sub(&b) else {
            return LowerBound::Unbounded;
        };
        let gp = self.gross_profit_exact;
        if loss.is_zero() && gp.is_zero() {
            return LowerBound::Bounded(RatioStatus::Undefined);
        }
        if loss.is_zero() {
            return LowerBound::Bounded(RatioStatus::NoObservedLosses);
        }
        let scale = 10i128.pow(MONEY_SCALE);
        match gp.scaled_units().checked_mul(scale) {
            Some(n) => LowerBound::Bounded(RatioStatus::Value {
                value: Money::from_scaled_units(n.div_euclid(loss.scaled_units())),
            }),
            None => LowerBound::Unbounded,
        }
    }

    /// ADR-016: [`Self::profit_factor_lower_bound_from`] over this block's own
    /// profit factor.
    #[must_use]
    pub fn profit_factor_lower_bound(&self) -> LowerBound<RatioStatus<Money>> {
        self.profit_factor_lower_bound_from(self.profit_factor)
    }

    /// ROI as the exact rational `(realized_pnl, consumed_basis)` in scaled
    /// `Money` units; `None` without known closed episodes or a zero basis.
    #[must_use]
    pub fn roi_parts(&self) -> Option<(i128, i128)> {
        let den = self.consumed_acquisition_basis_exact.scaled_units();
        (self.closed_episodes_known > 0 && den != 0)
            .then_some((self.realized_trade_pnl_exact.scaled_units(), den))
    }

    fn empty(unit: QuoteUnit) -> Self {
        Self {
            unit,
            closed_episodes_known: 0,
            wins: 0,
            losses: 0,
            breakeven: 0,
            realized_trade_pnl_exact: Money::ZERO,
            realized_trade_pnl_raw: 0,
            consumed_acquisition_basis_exact: Money::ZERO,
            consumed_acquisition_basis_raw: 0,
            open_episode_known_disposal_pnl_raw: 0,
            open_episode_known_disposals: 0,
            win_rate: RatioStatus::Undefined,
            profit_factor: RatioStatus::Undefined,
            gross_profit_exact: Money::ZERO,
            gross_loss_abs_exact: Money::ZERO,
            unknown_pnl_bound: LowerBound::Bounded(Money::ZERO),
        }
    }
}

impl SolanaWalletLedgerReport {
    /// The block of `unit`, if it is a Solana quote unit.
    #[must_use]
    pub fn unit_block(&self, unit: QuoteUnit) -> Option<&QuoteUnitBlock> {
        self.unit_blocks.iter().find(|b| b.unit == unit)
    }

    /// ADR-016: `(closed_unknown, closed_known + closed_unknown)`.
    #[must_use]
    pub fn unknown_episode_share_parts(&self) -> (u64, u64) {
        (
            self.closed_episodes_unknown,
            self.closed_episodes_known
                .saturating_add(self.closed_episodes_unknown),
        )
    }
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

type Location = (u64, u64);

#[derive(Debug, Clone, Copy)]
enum Consideration {
    /// Raw units of `unit`: for event pricing (always SOL) the buy cost
    /// (`sol+fee+creator`) or sell proceeds (`sol-fee-creator`); for route
    /// swaps the wallet's own quote delta magnitude.
    Verified {
        unit: QuoteUnit,
        amount: u64,
        token_amount: u64,
    },
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
    /// PumpSwap: how the user's own legs reconcile with the events
    /// (ADR-012 §3); `None` for other venues.
    amm_attribution: Option<AmmAttribution>,
}

/// One decoded swap leg of the transaction (any `user`), evidence for the
/// ADR-013 route rule.
struct LegInfo {
    source: LegSource,
    user: SolanaPubkey,
    /// The traded (non-quote) token of the leg.
    mint: SolanaPubkey,
    fixture_verified: bool,
    instruction_index: u32,
    timestamp: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegSource {
    Curve,
    PumpAmm,
}

/// Aggregator whose swap event produced a leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AggSource {
    Jupiter,
    Dflow,
    /// OKX DEX Router order event (carries the owner, unlike Jupiter/DFlow).
    Okx,
}

/// ADR-015: one aggregator (Jupiter / DFlow) hop. Carries no owner and no
/// consideration.
struct JupLeg {
    source: AggSource,
    /// OKX order events: the single owner named as both source and
    /// destination token-account owner (ownership evidence). `None` for
    /// Jupiter/DFlow hops.
    owner: Option<SolanaPubkey>,
    input_mint: SolanaPubkey,
    output_mint: SolanaPubkey,
    fixture_verified: bool,
    instruction_index: u32,
}

struct EpisodeAcc {
    /// Unit of the known disposals so far (ADR-013 §4).
    unit: Option<QuoteUnit>,
    opened_at: Option<i64>,
    opened_location: Location,
    pnl: Money,
    consumed_basis: Money,
    unknown: BTreeSet<UnknownReason>,
    known_disposals: u64,
    left_censored_raw: u128,
    /// ADR-016: known-basis lots consumed by every disposal, per unit.
    basis_by_unit: BTreeMap<QuoteUnit, Money>,
    /// ADR-016: some disposal consumed an unknown-basis lot.
    consumed_unknown_lot: bool,
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
    quote_mints: QuoteMints,
    route_log: Vec<RouteSwapRecord>,
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
    evidence_samples: Vec<DecodeEvidence>,
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

    #[allow(clippy::too_many_arguments)]
    fn acquire(
        &mut self,
        mint: SolanaPubkey,
        amount: u64,
        basis: Option<Money>,
        unit: QuoteUnit,
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
                unit: None,
                opened_at: ts,
                opened_location: loc,
                pnl: Money::ZERO,
                consumed_basis: Money::ZERO,
                unknown: BTreeSet::new(),
                known_disposals: 0,
                left_censored_raw: 0,
                basis_by_unit: BTreeMap::new(),
                consumed_unknown_lot: false,
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
        state.ledger.acquire_in_unit(
            asset,
            raw(amount),
            basis.unwrap_or(Money::ZERO),
            status,
            unit,
        );
        Ok(())
    }

    /// `gross` is `None` when proceeds are unknown.
    #[allow(clippy::too_many_arguments)]
    fn dispose(
        &mut self,
        mint: SolanaPubkey,
        amount: u64,
        gross: Option<Money>,
        unit: QuoteUnit,
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
            self.acquire(mint, shortfall, None, unit, Some(reason), ts, loc)?;
        }
        let state = self.state(mint);
        let (_, consumes_other_unknown, cross_unit) =
            consumed_unknown_kinds(&state.ledger, need, unit)?;
        // ADR-013 §4: proceeds in one unit against basis in another has no
        // PnL; the disposal is Unknown { CrossQuoteUnit }.
        let (gross, unknown_reason) = if gross.is_some() && cross_unit {
            (None, UnknownReason::CrossQuoteUnit)
        } else {
            (gross, unknown_reason)
        };
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
        for (u, m) in &result.consumed_known_basis_by_unit {
            let acc = ep.basis_by_unit.entry(*u).or_insert(Money::ZERO);
            *acc = money_add(*acc, *m)?;
        }
        if !result.all_basis_known {
            ep.consumed_unknown_lot = true;
        }
        let known_pnl = match (gross, result.realized_trade_pnl) {
            (Some(_), Some(pnl)) => Some(pnl),
            _ => None,
        };
        if let Some(pnl) = known_pnl {
            match ep.unit {
                None => ep.unit = Some(unit),
                // One episode realized in two units has no single PnL.
                Some(u) if u != unit => {
                    ep.unknown.insert(UnknownReason::CrossQuoteUnit);
                }
                Some(_) => {}
            }
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
/// lot, consumes any other Unknown-basis lot, consumes a Known-basis lot
/// denominated in a unit other than `unit`)`.
fn consumed_unknown_kinds(
    ledger: &Ledger,
    need: u128,
    unit: QuoteUnit,
) -> Result<(bool, bool, bool), SolanaWalletLedgerError> {
    let censored_label = UnknownReason::LeftCensored.label();
    let mut left = need;
    let (mut censored, mut other, mut cross) = (false, false, false);
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
        } else if lot.quote_unit != unit {
            cross = true;
        }
    }
    Ok((censored, other, cross))
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
    let consumed_basis_status = if ep.consumed_unknown_lot {
        ConsumedBasisStatus::PartiallyUnknown
    } else {
        ConsumedBasisStatus::Known
    };
    let consumed_known_basis_by_unit: Vec<(QuoteUnit, Money)> =
        ep.basis_by_unit.iter().map(|(u, m)| (*u, *m)).collect();
    let unknown_pnl_bound = (outcome == EpisodeOutcome::ClosedUnknown).then(|| {
        if ep.consumed_unknown_lot || consumed_known_basis_by_unit.len() > 1 {
            return EpisodePnlBound::Unbounded;
        }
        let (unit, basis) = consumed_known_basis_by_unit
            .first()
            .copied()
            .unwrap_or((ep.unit.unwrap_or(QuoteUnit::Lamports), Money::ZERO));
        Money::ZERO
            .checked_sub(&basis)
            .map_or(EpisodePnlBound::Unbounded, |lower_bound| {
                EpisodePnlBound::Bounded { unit, lower_bound }
            })
    });
    Ok(EpisodeRecord {
        mint,
        outcome,
        opened_at: ep.opened_at,
        closed_at,
        holding_seconds,
        opened_location: ep.opened_location,
        quote_unit: ep.unit,
        unknown_reasons: ep.unknown,
        known_disposals: ep.known_disposals,
        known_disposal_pnl: ep.pnl,
        known_disposal_consumed_basis: ep.consumed_basis,
        left_censored_amount_raw: ep.left_censored_raw,
        consumed_basis_status,
        consumed_known_basis_by_unit,
        unknown_pnl_bound,
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
    /// Every decoded swap leg of the transaction, whatever its `user`.
    legs: Vec<LegInfo>,
    /// ADR-013 §2: why a route-swap candidate was not booked.
    route_reject: Option<RouteReject>,
    /// ADR-015: Jupiter and DFlow hops of the transaction (whatever the wallet).
    jup_legs: Vec<JupLeg>,
    /// Evidence sources of the route swap booked in this transaction.
    route_evidence: RouteEvidence,
    /// Orphan events found by the extractors: `(instruction_index, name)`.
    orphan_events: Vec<(u32, &'static str)>,
    /// Bounded coverage-gap samples and counters of this transaction.
    scan: crate::decode_evidence::TxScan,
}

/// Quote-asset mints of ADR-013 §3 (decoded once per build).
struct QuoteMints {
    wsol: SolanaPubkey,
    usdc: SolanaPubkey,
    usdt: SolanaPubkey,
}

impl QuoteMints {
    fn new() -> Result<Self, SolanaWalletLedgerError> {
        let dec = |s: &str| {
            bs58::decode(s)
                .into_vec()
                .ok()
                .and_then(|v| SolanaPubkey::try_from(v).ok())
                .ok_or(SolanaWalletLedgerError::InvalidQuoteMintConstant)
        };
        Ok(Self {
            wsol: wsol_mint()?,
            usdc: dec(USDC_MINT)?,
            usdt: dec(USDT_MINT)?,
        })
    }

    fn stable_unit(&self, mint: &SolanaPubkey) -> Option<QuoteUnit> {
        if *mint == self.usdc {
            Some(QuoteUnit::UsdcUnits)
        } else if *mint == self.usdt {
            Some(QuoteUnit::UsdtUnits)
        } else {
            None
        }
    }
}

/// Why a route-swap candidate was rejected (ADR-013 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteReject {
    NothingMoved,
    NotSigner,
    MultiAsset,
    NotOppositeSigns,
    NoQuoteLeg,
    NoVerifiedLeg,
    PassthroughNonzero,
}

/// An accepted route swap: the wallet's own token and quote deltas.
struct RouteBooking {
    unit: QuoteUnit,
    mint: SolanaPubkey,
    token_delta: i128,
    quote_delta: i128,
    evidence: RouteEvidence,
}

/// Bonding-curve extractor (ADR-010 §2/§3), behaviour unchanged.
fn extract_curve_trades(
    tx: &RawSolanaTransaction,
    wallet: &SolanaPubkey,
    decoder: &BondingCurveBuyDecoder,
    wsol: &SolanaPubkey,
    policy: VariantPolicy,
    work: &mut TxWork<'_>,
) {
    let rep = pair_trades_with_events(decoder, &tx.instructions, tx.slot, tx.transaction_index);
    for p in &rep.trades {
        work.legs.push(LegInfo {
            source: LegSource::Curve,
            user: p.trade.user,
            mint: p.trade.mint,
            fixture_verified: policy(p.trade.variant) == VariantVerification::FixtureVerified,
            instruction_index: p.trade.instruction_index,
            timestamp: match &p.pairing {
                TradeEventPairing::Paired(ev) => Some(ev.timestamp),
                _ => None,
            },
        });
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
                            unit: QuoteUnit::Lamports,
                            amount: lamports,
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
            verification: policy(t.variant),
            mismatched: matches!(p.pairing, TradeEventPairing::Mismatch { .. }),
            unreconciled: false,
            reversed_pool: false,
            amm_attribution: None,
        });
    }
    work.malformed_trades = work
        .malformed_trades
        .saturating_add(u64::try_from(rep.malformed_trades).unwrap_or(u64::MAX));
    work.orphans = work
        .orphans
        .saturating_add(u64::try_from(rep.orphan_events.len()).unwrap_or(u64::MAX));
    work.orphan_events.extend(
        rep.orphan_events
            .iter()
            .map(|e| (e.instruction_index, "TradeEvent")),
    );
}

/// ADR-015: collect Jupiter v6 swap legs (program-id gated, exact layout).
/// Malformed or unknown Jupiter events are counted by the evidence scan and
/// never trusted as legs.
fn extract_jupiter_legs(tx: &RawSolanaTransaction, work: &mut TxWork<'_>) {
    let dec = JupiterEventDecoder::new();
    for ix in &tx.instructions {
        if let JupiterEventOutcome::Swaps {
            kind,
            legs,
            instruction_index,
        } = dec.classify(ix)
        {
            for l in legs {
                work.jup_legs.push(JupLeg {
                    source: AggSource::Jupiter,
                    owner: None,
                    input_mint: l.input_mint,
                    output_mint: l.output_mint,
                    fixture_verified: kind.verification() == VariantVerification::FixtureVerified,
                    instruction_index,
                });
            }
        }
    }
}

/// ADR-015 amendment: collect DFlow Aggregator v4 swap legs (program-id and
/// event-authority gated, exact layout). Malformed or unknown DFlow events
/// are counted by the evidence scan and never trusted as legs.
fn extract_dflow_legs(tx: &RawSolanaTransaction, work: &mut TxWork<'_>) {
    let dec = DflowEventDecoder::new();
    for ix in &tx.instructions {
        if let DflowEventOutcome::Swap {
            leg,
            instruction_index,
        } = dec.classify(ix)
        {
            work.jup_legs.push(JupLeg {
                source: AggSource::Dflow,
                owner: None,
                input_mint: leg.input_mint,
                output_mint: leg.output_mint,
                fixture_verified: dflow_swap_event_verification()
                    == VariantVerification::FixtureVerified,
                instruction_index,
            });
        }
    }
}

/// ADR-017: collect OKX DEX Router order events as swap legs (program
/// id, event-authority and exact-length gated). Only events of a variant the
/// policy rates `FixtureVerified` AND naming ONE owner for both token
/// accounts become legs; the owner is carried as ownership evidence. An
/// order with a distinct receiver is attributed to nobody (it is counted by
/// the evidence scan). Malformed or unknown events are counted there too and
/// never trusted. Per-hop `SwapEvent`s carry no mint and are not legs.
fn extract_okx_legs(tx: &RawSolanaTransaction, policy: OkxOrderPolicy, work: &mut TxWork<'_>) {
    let dec = OkxEventDecoder::new();
    for ix in &tx.instructions {
        if let OkxEventOutcome::Order(e) = dec.classify(ix)
            && e.single_owner()
            && policy(e.kind) == VariantVerification::FixtureVerified
        {
            work.jup_legs.push(JupLeg {
                source: AggSource::Okx,
                owner: Some(e.source_token_account_owner),
                input_mint: e.source_mint,
                output_mint: e.destination_mint,
                fixture_verified: true,
                instruction_index: e.instruction_index,
            });
        }
    }
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
    work.orphan_events
        .extend(rec.pairing.orphan_events.iter().map(|e| match e {
            PumpAmmEvent::Buy(b) => (b.instruction_index, "BuyEvent"),
            PumpAmmEvent::Sell(x) => (x.instruction_index, "SellEvent"),
        }));
    for p in &rec.pairing.trades {
        let r = read_amm_trade(p);
        work.legs.push(LegInfo {
            source: LegSource::PumpAmm,
            user: p.trade.user,
            mint: r.mint,
            fixture_verified: p.trade.verification() == VariantVerification::FixtureVerified,
            instruction_index: p.trade.instruction_index,
            timestamp: match &p.pairing {
                AmmTradeEventPairing::Paired(ev) => Some(amm_event_timestamp(ev)),
                _ => None,
            },
        });
    }
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
                amm_attribution: Some(u.attribution),
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
                                    unit: QuoteUnit::Lamports,
                                    amount: lamports,
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

/// Verification level of an OKX order-event variant (ADR-009 mechanism).
/// Production uses [`default_okx_order_policy`]; tests inject another to
/// exercise the `FixtureVerified` path of a variant that is `IdlOnly`.
pub type OkxOrderPolicy = fn(OkxOrderEventKind) -> VariantVerification;

/// The decoder table's own status for each order-event variant.
#[must_use]
pub fn default_okx_order_policy(kind: OkxOrderEventKind) -> VariantVerification {
    kind.verification()
}

/// The two venue decoders feeding one ledger (ADR-012 §5).
#[derive(Debug, Clone, Copy)]
pub struct LedgerDecoders<'a> {
    pub curve: &'a BondingCurveBuyDecoder,
    /// `None` = bonding-curve-only run (pre-ADR-012 behaviour).
    pub amm: Option<&'a PumpAmmDecoder>,
    /// OKX DEX Router order-event verification (evidence only in full venue runs).
    pub okx_order_policy: OkxOrderPolicy,
}

fn classify_trades<'a>(
    tx: &'a RawSolanaTransaction,
    wallet: &SolanaPubkey,
    decoders: &LedgerDecoders<'_>,
    qm: &QuoteMints,
    policy: VariantPolicy,
    scan: bool,
) -> Result<TxWork<'a>, SolanaWalletLedgerError> {
    let mut work = TxWork {
        tx,
        trades: Vec::new(),
        malformed_trades: 0,
        orphans: 0,
        router_forwards: 0,
        qfe_unbooked: 0,
        forward_mints: BTreeSet::new(),
        legs: Vec::new(),
        route_reject: None,
        jup_legs: Vec::new(),
        route_evidence: RouteEvidence::default(),
        orphan_events: Vec::new(),
        scan: crate::decode_evidence::TxScan::default(),
    };
    extract_curve_trades(tx, wallet, decoders.curve, &qm.wsol, policy, &mut work);
    if let Some(amm) = decoders.amm {
        extract_amm_trades(tx, wallet, amm, &mut work);
        // ADR-015: Jupiter legs are evidence only in full venue runs.
        extract_jupiter_legs(tx, &mut work);
        extract_dflow_legs(tx, &mut work);
        extract_okx_legs(tx, decoders.okx_order_policy, &mut work);
    }
    if scan {
        work.scan = scan_tx_evidence(tx, decoders, decoders.amm.is_some(), &work.orphan_events);
    }
    // Both extractors yield execution order; merge across venues by the
    // flattened instruction index.
    work.trades.sort_by_key(|t| t.instruction_index);
    apply_route_rules(&mut work, wallet, qm)?;
    Ok(work)
}

/// One wallet-attributed trade of a transaction (shared by the ledger's
/// classification and `buyer-intersect`, ADR-014). Side is in TOKEN terms.
#[derive(Debug, Clone)]
pub(crate) struct AttributedTrade {
    pub wallet: SolanaPubkey,
    pub venue: Venue,
    pub variant: &'static str,
    pub side: TradeSide,
    pub mint: SolanaPubkey,
    /// Verification of the decoded variant (curve: through the injected policy).
    pub verification: VariantVerification,
    /// PumpSwap: the wallet's base leg reconciles with the events (ADR-012
    /// §3: `Exact`, `QuoteResidual` or `QuoteFundedElsewhere`). Always true
    /// for the other venues (curve is checked by delta, a route by rule).
    pub reconciled: bool,
    /// Owner-keyed net delta of `mint` for `wallet` in this transaction.
    pub owner_delta: i128,
}

/// Every wallet-attributed trade of one transaction plus the rejections
/// that explain what was NOT attributed.
#[derive(Debug, Clone, Default)]
pub(crate) struct TxAttribution {
    pub trades: Vec<AttributedTrade>,
    /// Decoded trade instructions with broken structure / events without a
    /// trade (both venues), counted once per transaction.
    pub malformed_trades: u64,
    pub orphan_events: u64,
    /// PumpSwap trades whose decoded user is a non-signing zero-net
    /// forwarder (ADR-012 §3): not attributed to anybody.
    pub router_forwards: u64,
    /// ADR-013 §2 rejections of signer candidates (first failing bucket).
    pub route_rejections: Vec<RouteReject>,
    /// Bounded coverage-gap samples of the transaction (ADR-015 forensics).
    pub evidence: Vec<DecodeEvidence>,
    pub unknown_discriminators: u64,
    pub jupiter_malformed: u64,
    pub jupiter_unknown: u64,
    pub dflow_malformed: u64,
    pub dflow_unknown: u64,
    pub okx_malformed: u64,
    pub okx_unknown: u64,
    pub okx_receiver: u64,
    pub okx_idl_only: u64,
}

/// Attribute every trade of `tx` that touches one of `focus_mints` to the
/// wallets that own it, for ALL wallets of the transaction. Candidates are
/// the signers, every decoded leg user and every owner with a non-zero
/// delta of a focus mint; each candidate goes through the SAME per-wallet
/// classification the ledger uses (`classify_trades`: curve and PumpSwap
/// extractors, reversed pools, router-forward guard, ADR-013 route rule), so
/// the two consumers cannot drift apart. A transaction without a decoded leg
/// on a focus mint yields no trade.
pub(crate) fn attribute_transaction_trades(
    tx: &RawSolanaTransaction,
    decoders: &LedgerDecoders<'_>,
    focus_mints: &BTreeSet<SolanaPubkey>,
    policy: VariantPolicy,
) -> Result<TxAttribution, SolanaWalletLedgerError> {
    let mut out = TxAttribution::default();
    if !tx.execution.is_success() {
        return Ok(out);
    }
    let qm = QuoteMints::new()?;
    // Pass 0 with a sentinel wallet that is nobody: yields the decoded legs
    // and the per-transaction decode counters.
    let probe = classify_trades(tx, &[0u8; 32], decoders, &qm, policy, true)?;
    out.malformed_trades = probe.malformed_trades;
    out.orphan_events = probe.orphans;
    out.evidence = probe.scan.evidence.clone();
    out.unknown_discriminators = probe.scan.unknown_discriminators;
    out.jupiter_malformed = probe.scan.jupiter_malformed;
    out.jupiter_unknown = probe.scan.jupiter_unknown;
    out.dflow_malformed = probe.scan.dflow_malformed;
    out.dflow_unknown = probe.scan.dflow_unknown;
    out.okx_malformed = probe.scan.okx_malformed;
    out.okx_unknown = probe.scan.okx_unknown;
    out.okx_receiver = probe.scan.okx_receiver;
    out.okx_idl_only = probe.scan.okx_idl_only;
    let touches_focus = probe.legs.iter().any(|l| focus_mints.contains(&l.mint))
        || probe
            .jup_legs
            .iter()
            .any(|l| focus_mints.contains(&l.input_mint) || focus_mints.contains(&l.output_mint));
    if !touches_focus {
        return Ok(out);
    }
    let deltas = solana_owner_net_deltas(&tx.token_balance_changes)?;
    let mut candidates: BTreeSet<SolanaPubkey> = tx.signers.iter().copied().collect();
    candidates.extend(probe.legs.iter().map(|l| l.user));
    candidates.extend(probe.jup_legs.iter().filter_map(|l| l.owner));
    for ((mint, owner), d) in &deltas.deltas {
        if *d != 0 && focus_mints.contains(mint) {
            candidates.insert(*owner);
        }
    }
    for wallet in &candidates {
        let work = classify_trades(tx, wallet, decoders, &qm, policy, false)?;
        out.router_forwards = out.router_forwards.max(work.router_forwards);
        if let Some(reject) = work.route_reject
            && tx.signers.contains(wallet)
        {
            out.route_rejections.push(reject);
        }
        for t in &work.trades {
            if !focus_mints.contains(&t.mint) {
                continue;
            }
            let reconciled = match t.venue {
                Venue::PumpAmm => matches!(
                    t.amm_attribution,
                    Some(
                        AmmAttribution::Exact
                            | AmmAttribution::QuoteResidual
                            | AmmAttribution::QuoteFundedElsewhere
                    )
                ),
                Venue::BondingCurve | Venue::Route => true,
            };
            out.trades.push(AttributedTrade {
                wallet: *wallet,
                venue: t.venue,
                variant: t.variant,
                side: t.side,
                mint: t.mint,
                verification: t.verification,
                reconciled,
                owner_delta: deltas.deltas.get(&(t.mint, *wallet)).copied().unwrap_or(0),
            });
        }
    }
    Ok(out)
}

/// ADR-013 §1/§2: decide whether the wallet's event trades are its own
/// price, and if not, whether the transaction is a route swap booked from
/// the wallet's own deltas.
fn apply_route_rules(
    work: &mut TxWork<'_>,
    wallet: &SolanaPubkey,
    qm: &QuoteMints,
) -> Result<(), SolanaWalletLedgerError> {
    if work.legs.is_empty() && work.jup_legs.is_empty() {
        return Ok(());
    }
    let tx = work.tx;
    let deltas = solana_owner_net_deltas(&tx.token_balance_changes)?;
    let mut tokens: BTreeMap<SolanaPubkey, i128> = BTreeMap::new();
    for ((mint, owner), d) in &deltas.deltas {
        if owner == wallet && *d != 0 {
            tokens.insert(*mint, *d);
        }
    }
    let wsol_delta = tokens.get(&qm.wsol).copied().unwrap_or(0);
    let mut sol: i128 = tx
        .native_balance_changes
        .iter()
        .filter(|c| c.account == *wallet)
        .map(|c| c.delta())
        .sum();
    if tx.fee_payer == *wallet {
        sol = checked_add_i(sol, i128::from(tx.fee_lamports), "route sol")?;
    }
    sol = checked_add_i(sol, wsol_delta, "route sol")?;

    // §1: event pricing guard.
    let all_verified = work
        .trades
        .iter()
        .all(|t| matches!(t.consideration, Consideration::Verified { .. }));
    let mut guard_failed = false;
    if !work.trades.is_empty() {
        let traded: BTreeSet<SolanaPubkey> = work.trades.iter().map(|t| t.mint).collect();
        let other_asset = tokens.keys().any(|m| *m != qm.wsol && !traded.contains(m));
        let (mut buy_cost, mut sell_proceeds) = (0i128, 0i128);
        for t in &work.trades {
            if let Consideration::Verified { amount, .. } = t.consideration {
                match t.side {
                    TradeSide::Buy => {
                        buy_cost = checked_add_i(buy_cost, i128::from(amount), "guard cost")?;
                    }
                    TradeSide::Sell => {
                        sell_proceeds =
                            checked_add_i(sell_proceeds, i128::from(amount), "guard proceeds")?;
                    }
                }
            }
        }
        // Wallet SOL outflow (net of same-tx sells) must cover the event
        // cost; rent, tips and platform fees only add to it.
        let outflow = sell_proceeds
            .checked_sub(sol)
            .ok_or(SolanaWalletLedgerError::Overflow("guard outflow"))?;
        let cost_ok = buy_cost == 0 || outflow >= buy_cost;
        guard_failed = other_asset || !cost_ok;
        if all_verified && !guard_failed {
            return Ok(());
        }
    }

    match route_candidate(work, wallet, qm, &tokens, wsol_delta, sol, &deltas.deltas) {
        Ok(book) => {
            let token_amount = u64::try_from(book.token_delta.unsigned_abs())
                .map_err(|_| SolanaWalletLedgerError::Overflow("route token amount"))?;
            let amount = u64::try_from(book.quote_delta.unsigned_abs())
                .map_err(|_| SolanaWalletLedgerError::Overflow("route quote amount"))?;
            let mut mine: Vec<&LegInfo> =
                work.legs.iter().filter(|l| l.mint == book.mint).collect();
            mine.sort_by_key(|l| l.instruction_index);
            let jup_first = work
                .jup_legs
                .iter()
                .filter(|l| l.input_mint == book.mint || l.output_mint == book.mint)
                .map(|l| l.instruction_index)
                .min();
            let instruction_index = mine
                .first()
                .map(|l| l.instruction_index)
                .or(jup_first)
                .unwrap_or(0);
            // An aggregator-only route has no event timestamp: use the block time.
            let timestamp = mine
                .iter()
                .find_map(|l| l.timestamp)
                .or_else(|| (mine.is_empty()).then_some(tx.block_time).flatten());
            work.route_evidence = book.evidence;
            work.trades.clear();
            work.router_forwards = 0;
            work.qfe_unbooked = 0;
            work.forward_mints.clear();
            work.trades.push(WalletTrade {
                venue: Venue::Route,
                variant: "route_swap",
                side: if book.token_delta > 0 {
                    TradeSide::Buy
                } else {
                    TradeSide::Sell
                },
                mint: book.mint,
                instruction_index,
                consideration: Consideration::Verified {
                    unit: book.unit,
                    amount,
                    token_amount,
                },
                timestamp,
                verification: VariantVerification::FixtureVerified,
                mismatched: false,
                unreconciled: false,
                reversed_pool: false,
                amm_attribution: None,
            });
        }
        Err(reject) => {
            if reject != RouteReject::NothingMoved {
                work.route_reject = Some(reject);
            }
            if guard_failed {
                // §1: never a partial-hop price.
                for t in &mut work.trades {
                    if matches!(t.consideration, Consideration::Verified { .. }) {
                        t.consideration = Consideration::Unknown {
                            reason: UnknownReason::RouteLegNotWalletPrice,
                            token_amount: None,
                        };
                    }
                }
            }
        }
    }
    Ok(())
}

/// ADR-013 §2 a-d. `tokens` are the wallet's non-zero owner-keyed token
/// deltas (wSOL included), `sol` its SOL delta (native + wSOL, fee added
/// back when it paid).
fn route_candidate(
    work: &TxWork<'_>,
    wallet: &SolanaPubkey,
    qm: &QuoteMints,
    tokens: &BTreeMap<SolanaPubkey, i128>,
    wsol_delta: i128,
    sol: i128,
    deltas: &BTreeMap<(SolanaPubkey, SolanaPubkey), i128>,
) -> Result<RouteBooking, RouteReject> {
    let tx = work.tx;
    let moved: Vec<(SolanaPubkey, i128)> = tokens
        .iter()
        .filter(|(m, _)| **m != qm.wsol)
        .map(|(m, d)| (*m, *d))
        .collect();
    if tokens.is_empty() || moved.is_empty() {
        return Err(RouteReject::NothingMoved);
    }
    // a. the wallet signs.
    if !tx.signers.contains(wallet) {
        return Err(RouteReject::NotSigner);
    }
    // c. exactly one traded token and one quote asset, opposite signs.
    let (mint, token_delta, unit, quote_delta) = match moved.as_slice() {
        [(a, da)] if qm.stable_unit(a).is_none() => {
            if sol == 0 {
                return Err(RouteReject::NoQuoteLeg);
            }
            (*a, *da, QuoteUnit::Lamports, sol)
        }
        [(a, da), (b, db)] if wsol_delta == 0 => match (qm.stable_unit(a), qm.stable_unit(b)) {
            (None, Some(u)) => (*a, *da, u, *db),
            (Some(u), None) => (*b, *db, u, *da),
            _ => return Err(RouteReject::MultiAsset),
        },
        _ => return Err(RouteReject::MultiAsset),
    };
    if work.trades.iter().any(|t| t.mint != mint) {
        return Err(RouteReject::MultiAsset);
    }
    if (token_delta > 0) == (quote_delta > 0) {
        return Err(RouteReject::NotOppositeSigns);
    }
    // b. a decoded, FixtureVerified leg trades the wallet's token: a pump
    // curve / PumpSwap leg, or (ADR-015) a Jupiter hop whose input or output
    // mint is the token. Ownership never comes from a Jupiter leg.
    let verified_leg = |src: LegSource| {
        work.legs
            .iter()
            .any(|l| l.source == src && l.mint == mint && l.fixture_verified)
    };
    let agg_leg = |src: AggSource| {
        work.jup_legs.iter().any(|l| {
            l.source == src && l.fixture_verified && (l.input_mint == mint || l.output_mint == mint)
        })
    };
    // ADR-017: an OKX order event is evidence for the wallet only when
    // it trades the token AND its single owner is the wallet, or a
    // non-signing zero-net pass-through (checked in the owner rule below).
    let okx_trades_token = |owner_is_wallet: Option<bool>| {
        work.jup_legs.iter().any(|l| {
            l.source == AggSource::Okx
                && l.fixture_verified
                && (l.input_mint == mint || l.output_mint == mint)
                && owner_is_wallet.is_none_or(|w| (l.owner == Some(*wallet)) == w)
        })
    };
    let evidence = RouteEvidence {
        curve: verified_leg(LegSource::Curve),
        pump_amm: verified_leg(LegSource::PumpAmm),
        jupiter: agg_leg(AggSource::Jupiter),
        dflow: agg_leg(AggSource::Dflow),
        okx: okx_trades_token(None),
        okx_owner_is_wallet: okx_trades_token(Some(true)),
    };
    if !(evidence.curve || evidence.pump_amm || evidence.jupiter || evidence.dflow || evidence.okx)
    {
        return Err(RouteReject::NoVerifiedLeg);
    }
    // d. every other leg user is a non-signing, zero-net pass-through.
    for l in &work.legs {
        if l.user == *wallet {
            continue;
        }
        if tx.signers.contains(&l.user)
            || deltas
                .iter()
                .any(|((_, owner), d)| *owner == l.user && *d != 0)
        {
            return Err(RouteReject::PassthroughNonzero);
        }
    }
    // d (OKX). The owner an order event names, if not the wallet, is a
    // non-signing zero-net pass-through too; otherwise the order belongs to
    // someone else and is not attributed to this wallet.
    for l in &work.jup_legs {
        let Some(owner) = l.owner else { continue };
        if l.source != AggSource::Okx || owner == *wallet {
            continue;
        }
        if tx.signers.contains(&owner) || deltas.iter().any(|((_, o), d)| *o == owner && *d != 0) {
            return Err(RouteReject::PassthroughNonzero);
        }
    }
    Ok(RouteBooking {
        unit,
        mint,
        token_delta,
        quote_delta,
        evidence,
    })
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
            okx_order_policy: default_okx_order_policy,
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
    let qm = QuoteMints::new()?;
    let mut b = Builder {
        wallet: *wallet,
        left_censoring: options.left_censoring,
        left_censored_total: 0,
        chain_asset: asset_of,
        quote_mints: QuoteMints::new()?,
        route_log: Vec::new(),
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
        evidence_samples: Vec::new(),
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
                let w = classify_trades(tx, wallet, decoders, &qm, default_variant_policy, true)?;
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
        self.diag.unknown_discriminator_instructions += w.scan.unknown_discriminators;
        self.diag.jupiter_malformed_events += w.scan.jupiter_malformed;
        self.diag.jupiter_unknown_events += w.scan.jupiter_unknown;
        self.diag.dflow_malformed_events += w.scan.dflow_malformed;
        self.diag.dflow_unknown_events += w.scan.dflow_unknown;
        self.diag.okx_malformed_events += w.scan.okx_malformed;
        self.diag.okx_unknown_events += w.scan.okx_unknown;
        self.diag.okx_swap_with_receiver_not_attributed += w.scan.okx_receiver;
        self.diag.okx_idl_only_order_events += w.scan.okx_idl_only;
        crate::decode_evidence::merge_evidence(
            &mut self.evidence_samples,
            w.scan.evidence.iter().cloned(),
        );
        self.diag.router_forward_trades_not_attributed += w.router_forwards;
        self.diag.quote_funded_elsewhere_trades += w.qfe_unbooked;
        if let Some(r) = w.route_reject {
            let rr = &mut self.diag.route_rejected;
            match r {
                RouteReject::NothingMoved => {}
                RouteReject::NotSigner => rr.wallet_not_signer += 1,
                RouteReject::MultiAsset => rr.multi_asset += 1,
                RouteReject::NotOppositeSigns => rr.not_opposite_signs += 1,
                RouteReject::NoQuoteLeg => rr.no_quote_leg += 1,
                RouteReject::NoVerifiedLeg => rr.no_verified_leg += 1,
                RouteReject::PassthroughNonzero => rr.passthrough_nonzero += 1,
            }
        }
        let is_payer = tx.fee_payer == self.wallet;
        self.record_atomic_round_trip(w)?;

        // §4 fee allocation over SOL-quoted verified trades only (the network
        // fee is SOL; ADR-013 §2: never mixed into a USDC/USDT basis).
        let verified: Vec<usize> = w
            .trades
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                matches!(
                    t.consideration,
                    Consideration::Verified {
                        unit: QuoteUnit::Lamports,
                        ..
                    }
                )
                .then_some(i)
            })
            .collect();
        let weights: Vec<u64> = verified
            .iter()
            .filter_map(|i| w.trades.get(*i))
            .map(|t| match t.consideration {
                Consideration::Verified { amount, .. } => amount,
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
        // Quote mints moved by this transaction's route swaps (explained,
        // not "out of scope").
        let mut route_quote_mints: BTreeSet<SolanaPubkey> = BTreeSet::new();
        let mut unverified_mints: BTreeMap<SolanaPubkey, UnknownReason> = BTreeMap::new();
        let mut tx_ts: Option<i64> = None;
        let mut buy_cost: i128 = 0;
        let mut sell_proceeds: i128 = 0;

        for (i, t) in w.trades.iter().enumerate() {
            let venue_counts = match t.venue {
                Venue::BondingCurve => &mut self.counts.bonding_curve,
                Venue::PumpAmm => &mut self.counts.pump_amm,
                Venue::Route => &mut self.counts.route,
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
                    unit,
                    amount,
                    token_amount,
                } => {
                    self.counts.priced += 1;
                    if t.venue == Venue::Route {
                        self.counts.route_swaps += 1;
                        self.counts.route_swaps_by_quote.bump(unit);
                        self.counts.route_swaps_by_evidence.bump(w.route_evidence);
                        self.route_log.push(RouteSwapRecord {
                            signature: tx.signature,
                            slot: tx.slot,
                            transaction_index: tx.transaction_index,
                            mint: t.mint,
                            side: t.side,
                            unit,
                            token_amount,
                            quote_amount: amount,
                        });
                        match unit {
                            QuoteUnit::UsdcUnits => {
                                route_quote_mints.insert(self.quote_mints.usdc);
                            }
                            QuoteUnit::UsdtUnits => {
                                route_quote_mints.insert(self.quote_mints.usdt);
                            }
                            QuoteUnit::Lamports | QuoteUnit::ReportCurrency => {}
                        }
                    }
                    let signed = i128::from(token_amount);
                    let is_sol = unit == QuoteUnit::Lamports;
                    match t.side {
                        TradeSide::Buy => {
                            let basis_l = checked_add_i(
                                i128::from(amount),
                                i128::from(fee_share),
                                "buy basis",
                            )?;
                            self.acquire(
                                t.mint,
                                token_amount,
                                Some(quote_units_to_money(unit, basis_l)?),
                                unit,
                                None,
                                t.timestamp,
                                loc,
                            )?;
                            if is_sol {
                                buy_cost = checked_add_i(buy_cost, i128::from(amount), "buy cost")?;
                            }
                            *explained.entry(t.mint).or_insert(0) += signed;
                        }
                        TradeSide::Sell => {
                            self.dispose(
                                t.mint,
                                token_amount,
                                Some(quote_units_to_money(unit, i128::from(amount))?),
                                unit,
                                quote_units_to_money(unit, i128::from(fee_share))?,
                                UnknownReason::ConsiderationUnverified,
                                t.timestamp,
                                loc,
                            )?;
                            if is_sol {
                                sell_proceeds = checked_add_i(
                                    sell_proceeds,
                                    i128::from(amount),
                                    "sell proceeds",
                                )?;
                            }
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
                        UnknownReason::RouteLegNotWalletPrice => {
                            self.counts.route_leg_not_wallet_price += 1;
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
                                        QuoteUnit::Lamports,
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
                                        QuoteUnit::Lamports,
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
                            unverified_mints.insert(t.mint, reason);
                        }
                    }
                }
            }
        }

        // §6 inventory continuity.
        let deltas = solana_owner_net_deltas(&tx.token_balance_changes)?;
        let has_amm_trade = w
            .trades
            .iter()
            .any(|t| matches!(t.venue, Venue::PumpAmm | Venue::Route));
        let mut candidates: BTreeSet<SolanaPubkey> = w.trades.iter().map(|t| t.mint).collect();
        for ((mint, owner), delta) in &deltas.deltas {
            if *owner != self.wallet || *delta == 0 {
                continue;
            }
            if self.traded.contains(mint) {
                candidates.insert(*mint);
            } else if route_quote_mints.contains(mint) {
                // Quote asset of a booked route swap (ADR-013): explained.
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
            let unverified_reason = unverified_mints.get(&mint).copied();
            let unverified = unverified_reason.is_some();
            if !unverified {
                self.diag.continuity_breaks += 1;
            }
            let amount = u64::try_from(diff.unsigned_abs())
                .map_err(|_| SolanaWalletLedgerError::Overflow("continuity amount"))?;
            if diff > 0 {
                let reason =
                    unverified_reason.unwrap_or(UnknownReason::UnexplainedInboundTokenMovement);
                self.acquire(
                    mint,
                    amount,
                    None,
                    QuoteUnit::Lamports,
                    Some(reason),
                    tx_ts,
                    loc,
                )?;
            } else {
                let reason =
                    unverified_reason.unwrap_or(UnknownReason::UnexplainedOutboundTokenMovement);
                self.dispose(
                    mint,
                    amount,
                    None,
                    QuoteUnit::Lamports,
                    Money::ZERO,
                    reason,
                    tx_ts,
                    loc,
                )?;
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
        if has_amm_trade {
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
        let mut blocks: Vec<QuoteUnitBlock> = SOLANA_QUOTE_UNITS
            .iter()
            .map(|u| QuoteUnitBlock::empty(*u))
            .collect();
        let mut unit_cohorts: BTreeMap<QuoteUnit, EpisodeCohort> = BTreeMap::new();
        let mut open_pnls: BTreeMap<QuoteUnit, Money> = BTreeMap::new();
        let mut cohort = EpisodeCohort::default();
        // ADR-016: per-unit worst-case sum of unknown-episode bounds, and
        // the wallet-wide unbounded taint.
        let mut unknown_bounds: BTreeMap<QuoteUnit, Money> = BTreeMap::new();
        let mut any_unbounded = false;
        let mut holds: Vec<i64> = Vec::new();
        for r in &self.records {
            match r.outcome {
                EpisodeOutcome::ClosedKnown { pnl } => {
                    closed_known += 1;
                    let unit = r.quote_unit.unwrap_or(QuoteUnit::Lamports);
                    if let Some(b) = blocks.iter_mut().find(|b| b.unit == unit) {
                        b.closed_episodes_known += 1;
                        b.realized_trade_pnl_exact = money_add(b.realized_trade_pnl_exact, pnl)?;
                        b.consumed_acquisition_basis_exact = money_add(
                            b.consumed_acquisition_basis_exact,
                            r.known_disposal_consumed_basis,
                        )?;
                        if pnl.is_positive() {
                            b.gross_profit_exact = money_add(b.gross_profit_exact, pnl)?;
                            b.wins += 1;
                        } else if pnl.is_negative() {
                            b.gross_loss_abs_exact = b.gross_loss_abs_exact.checked_sub(&pnl)?;
                            b.losses += 1;
                        } else {
                            b.breakeven += 1;
                        }
                    }
                    if pnl.is_positive() {
                        wins += 1;
                    } else if pnl.is_negative() {
                        losses += 1;
                    } else {
                        breakeven += 1;
                    }
                    let ep = Episode {
                        asset: asset_of(r.mint),
                        realized_pnl: pnl,
                        is_closed_within_window: true,
                        opened_before_window: false,
                        has_unresolved_flows: false,
                    };
                    unit_cohorts
                        .entry(unit)
                        .or_default()
                        .episodes
                        .push(ep.clone());
                    cohort.episodes.push(ep);
                    if let Some(h) = r.holding_seconds {
                        holds.push(h);
                    }
                }
                EpisodeOutcome::ClosedUnknown => {
                    closed_unknown += 1;
                    match r.unknown_pnl_bound {
                        Some(EpisodePnlBound::Bounded { unit, lower_bound }) => {
                            let acc = unknown_bounds.entry(unit).or_insert(Money::ZERO);
                            *acc = money_add(*acc, lower_bound)?;
                        }
                        Some(EpisodePnlBound::Unbounded) | None => any_unbounded = true,
                    }
                }
                EpisodeOutcome::LeftCensored => left_censored += 1,
                EpisodeOutcome::Open => {
                    open_eps += 1;
                    // An open episode that mixed units has no single PnL.
                    if !r.unknown_reasons.contains(&UnknownReason::CrossQuoteUnit) {
                        let unit = r.quote_unit.unwrap_or(QuoteUnit::Lamports);
                        let acc = open_pnls.entry(unit).or_insert(Money::ZERO);
                        *acc = money_add(*acc, r.known_disposal_pnl)?;
                        if let Some(b) = blocks.iter_mut().find(|b| b.unit == unit) {
                            b.open_episode_known_disposals += r.known_disposals;
                        }
                    }
                }
            }
        }
        for b in &mut blocks {
            b.realized_trade_pnl_raw = money_to_quote_units_trunc(b.realized_trade_pnl_exact);
            b.consumed_acquisition_basis_raw =
                money_to_quote_units_trunc(b.consumed_acquisition_basis_exact);
            b.open_episode_known_disposal_pnl_raw =
                money_to_quote_units_trunc(open_pnls.get(&b.unit).copied().unwrap_or(Money::ZERO));
            let c = unit_cohorts.remove(&b.unit).unwrap_or_default();
            b.win_rate = win_rate(&c)?;
            b.profit_factor = profit_factor(&c)?;
            b.unknown_pnl_bound = if any_unbounded {
                LowerBound::Unbounded
            } else {
                LowerBound::Bounded(unknown_bounds.get(&b.unit).copied().unwrap_or(Money::ZERO))
            };
        }
        let total_closed = closed_known.saturating_add(closed_unknown);
        let win_rate_lower_bound = (total_closed > 0).then_some(WinRateLowerBound {
            wins,
            episodes: total_closed,
        });
        let win_rate = win_rate(&cohort)?;
        let sol = blocks
            .iter()
            .find(|b| b.unit == QuoteUnit::Lamports)
            .cloned()
            .unwrap_or_else(|| QuoteUnitBlock::empty(QuoteUnit::Lamports));
        let pnl_sum = sol.realized_trade_pnl_exact;
        let basis_sum = sol.consumed_acquisition_basis_exact;
        let profit_factor = sol.profit_factor;
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
            open_episode_known_disposal_pnl_lamports: sol.open_episode_known_disposal_pnl_raw,
            open_episode_known_disposals: sol.open_episode_known_disposals,
            failed_trade_fees_lamports: self.failed_fees,
            failed_trade_fee_txs: self.failed_fee_txs,
            realized_net_pnl_lamports: money_to_lamports_trunc(net),
            realized_net_pnl_exact: net,
            win_rate,
            profit_factor,
            median_holding_seconds,
            holding_time_samples,
            has_unknown_basis_inventory: open_with_unknown > 0 || closed_unknown > 0,
            win_rate_lower_bound,
            has_left_censored_inventory: left_censored > 0 || self.left_censored_total > 0,
            open_positions_with_unknown_basis: open_with_unknown,
            open_positions,
            unknown_basis_lots_created: self.unknown_lots,
            activity,
            daily_activity,
            diagnostics: self.diag,
            unit_blocks: blocks,
            route_swap_log: self.route_log,
            evidence_samples: self.evidence_samples,
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
