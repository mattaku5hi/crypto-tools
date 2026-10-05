//! ADR-019 EVM amendment: realizable (on-chain quote) valuation of OPEN
//! positions at the run's as-of head.
//!
//! A position is valued as what the wallet would receive by selling its WHOLE
//! remaining amount into the pool's other asset NOW, asked from the venue
//! itself with an `eth_call` pinned to one block (the head read at the start of
//! the valuation):
//! * Uniswap v3 / Pancake v3 / Slipstream: official `QuoterV2.quoteExactInputSingle`;
//! * Uniswap v4: official `V4Quoter.quoteExactInputSingle` (PoolKey rebuilt from
//!   the official PositionManager's `poolKeys(bytes25)` (one `eth_call`),
//!   accepted only when the key hashes to the full pool id; fallback: the
//!   pool's `Initialize` log through the logs endpoint under a hard per-run
//!   request cap);
//! * Uniswap v2 / Pancake v2: pair `getReserves()` + the exact constant-product
//!   formula with the factory's fee (`scout_dex_evm::v2_fee_for_factory`);
//! * Aerodrome v2: the pool's own `getAmountOut`.
//!
//! The pool is the one the wallet LAST traded the token on (ledger audit
//! trail). A revert is `quote_reverted`, never zero. Quoters are pinned only
//! where a live-verified source names them (`scout_dex_evm::pinned_quoter`, ADR-020 amendment 7); others
//! are `venue_quoter_unpinned`. The quote ignores transfer tax and exit gas;
//! a token with tax-shaped evidence in the run is labelled
//! `transfer_tax_not_modelled`. All calls go through the run's RPC client
//! (request budget, limiter, call counters).

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256, U256};
use scout_core::Money;
use scout_dex_evm::{
    QuoterFamily, QuoterRevert, SwapVenue, V2Fee, V4_INITIALIZE_TOPIC0, V4PoolKey,
    aerodrome_amount_out_calldata, decode_amount_out, decode_liquidity, decode_quoter_revert,
    decode_reserves, decode_v4_initialize, decode_v4_pool_keys, pinned_quoter,
    pinned_slipstream_quoter, pinned_v4_position_manager, pinned_v4_state_view, price_impact_bps,
    selector, v2_amount_out, v2_fee_for_factory, v2_reserves_calldata, v3_liquidity_calldata,
    v3_quote_calldata, v4_liquidity_calldata, v4_pool_keys_calldata, v4_quote_calldata,
};
use scout_pricing::{PriceLabel, PriceSource, QuoteAsset};
use scout_providers::{EthCallOutcome, EvmRpcClient, EvmSourceError, LogFilter};

use crate::analysis_window::AnalysisWindow;
use crate::chain_display::{ChainDisplay, evm_key};
use crate::evm_trade_extraction::{EvmExtractionConfig, QuoteAsset as TradeQuote};
use crate::solana_open_valuation::{OpenUsd, OpenVenueInfo, window_is_live};
use crate::solana_wallet_ledger::{QuoteUnit, evm_unit_of, quote_unit_label, quote_units_to_money};
use crate::solana_wallet_stats::SolanaWalletStats;
use crate::solana_wallet_usd::{Leg, UsdCoverage, convert_quote_to_usd, price_leg, quote_asset_on};

pub const EVM_OPEN_VALUATION_VERSION: &str = "evm-open-valuation/1 (ADR-019 EVM amendment: whole-position exit quote from the venue itself via eth_call pinned to the as-of head: official QuoterV2 (v3/pancake/slipstream), V4Quoter (v4), exact constant-product on getReserves (v2-style, fee by factory), getAmountOut (Aerodrome v2); last-traded pool; revert = quote_reverted, never zero)";

/// The label of a valued EVM position.
pub const LABEL_REALIZABLE_ONCHAIN_QUOTE: &str = "realizable_onchain_quote";
/// `ExitStatus::Exact`: the whole open amount was quoted.
pub const EXIT_STATUS_EXACT: &str = "exact";
/// The last pool has no in-range liquidity (confirmed by a liquidity read):
/// the realizable value is a lower bound of 0.
pub const EXIT_STATUS_ILLIQUID: &str = "illiquid";
/// The last pool has liquidity but the whole amount cannot be filled: the
/// realizable value is the lower bound of the largest fillable amount found.
pub const EXIT_STATUS_PARTIALLY_FILLABLE: &str = "partially_fillable";
/// Reason of an illiquid position.
pub const ILLIQUID_REASON_NO_LIQUIDITY: &str = "no_liquidity_in_last_pool";
/// Caveat of a non-exact exit: only the last-traded pool was asked.
pub const CAVEAT_OTHER_POOLS_NOT_SEARCHED: &str = "other_pools_not_searched";
/// Unrealized status of a lower-bound exit.
pub const UNREALIZED_STATUS_LOWER_BOUND: &str = "lower_bound";
/// Max quote calls of one partial-fill search.
pub const MAX_FILL_SEARCH_CALLS: u64 = 12;
/// Extra label: a transfer tax was suspected, the quote does not model it.
pub const LABEL_TRANSFER_TAX_NOT_MODELLED: &str = "transfer_tax_not_modelled";

/// Max entries of the per-run `eth_call` answer cache.
const MAX_CACHED_CALLS: usize = 4_096;
/// `eth_getLogs` split limit of one v4 `Initialize` lookup.
const KEY_LOOKUP_MAX_SPLITS: u32 = 32;
/// Default cap on v4 pool-key FALLBACK (log) lookups per run.
pub const DEFAULT_MAX_KEY_LOOKUPS: usize = 32;
/// Default hard cap on `eth_getLogs` requests of the v4 pool-key fallback per run.
pub const DEFAULT_MAX_FALLBACK_LOG_REQUESTS: u64 = 64;

/// Why a position is not valued. Never a zero value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvmUnvaluedReason {
    HistoricalWindow,
    NotRun,
    /// No booked swap of the token (transfers only).
    NoVenueObserved,
    /// Launchpad curve, unknown v2 factory fee, or an inconsistent pool.
    VenueNotSupported,
    /// No official quoter is pinned for this chain and venue family.
    VenueQuoterUnpinned,
    /// v4 PoolKey could not be derived/verified.
    PoolKeyUnknown,
    /// The pool did not report token0/token1/fee/tickSpacing/factory.
    PoolIdentityUnknown,
    /// The pool's other asset is neither native/wrapped native nor a pinned quote token.
    QuoteAssetUnsupported,
    /// The quoter / pool call reverted (the raw selector, when any, is
    /// recorded on the position).
    QuoteReverted,
    /// The quoter reverted with `PoolNotInitialized()`.
    PoolNotInitialized,
    /// The answer was not a well-formed ABI word set.
    QuoteResponseInvalid,
    StateFetchFailed,
    RequestBudgetExhausted,
    MathFailure,
}

impl EvmUnvaluedReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::HistoricalWindow => "historical_window",
            Self::NotRun => "not_run",
            Self::NoVenueObserved => "no_venue_observed",
            Self::VenueNotSupported => "venue_not_supported",
            Self::VenueQuoterUnpinned => "venue_quoter_unpinned",
            Self::PoolKeyUnknown => "pool_key_unknown",
            Self::PoolIdentityUnknown => "pool_identity_unknown",
            Self::QuoteAssetUnsupported => "quote_asset_unsupported",
            Self::QuoteReverted => "quote_reverted",
            Self::PoolNotInitialized => "pool_not_initialized",
            Self::QuoteResponseInvalid => "quote_response_invalid",
            Self::StateFetchFailed => "state_fetch_failed",
            Self::RequestBudgetExhausted => "request_budget_exhausted",
            Self::MathFailure => "math_failure",
        }
    }
}

/// A valued open position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmValuedPosition {
    pub venue: SwapVenue,
    pub pool: Address,
    pub pool_id: Option<B256>,
    /// `realizable_onchain_quote`.
    pub label: &'static str,
    /// `quoter_v2`, `v4_quoter`, `v2_reserves_constant_product`, `aerodrome_get_amount_out`.
    pub method: &'static str,
    pub quoter: Option<Address>,
    /// `pinned` or `override` (quoter) / `not_applicable`.
    pub quoter_source: &'static str,
    /// The block every call of the quote was pinned to.
    pub state_block: u64,
    pub quote_unit: QuoteUnit,
    /// `None` = native (WETH/WBNB merged), else the quote token.
    pub quote_token: Option<Address>,
    /// What selling the whole open amount returns, raw units of `quote_unit`.
    pub realizable_raw: u128,
    /// Marginal probe (a thousandth of the position) and its output.
    pub probe_amount_raw: Option<u128>,
    pub probe_out_raw: Option<u128>,
    /// Pool-fee-free price impact of the full sale vs the probe.
    pub price_impact_bps: Option<u64>,
    pub transfer_tax_not_modelled: bool,
    /// `realizable - known remaining basis` when every open lot is known and
    /// in `quote_unit`.
    pub unrealized_pnl: Option<Money>,
    /// `known`, `unknown_basis` or `basis_other_unit`.
    pub unrealized_status: &'static str,
    pub usd: Option<OpenUsd>,
    pub usd_unpriced_reason: Option<String>,
    pub usd_unrealized: Option<Money>,
    pub usd_unrealized_reason: Option<String>,
    /// `exact`, `illiquid` or `partially_fillable`. For the last two
    /// `realizable_raw` is a LOWER BOUND (0 / the largest fillable amount's
    /// output), never an exact value; no probe, no USD, no exact unrealized.
    pub exit_status: &'static str,
    /// `no_liquidity_in_last_pool` for an illiquid position.
    pub illiquid_reason: Option<&'static str>,
    /// `other_pools_not_searched` for a non-exact exit.
    pub fill_caveat: Option<&'static str>,
    /// Largest amount the search found fillable (partially fillable only).
    pub fillable_amount_raw: Option<u128>,
    /// `open amount - fillable amount` (non-exact exits).
    pub unfillable_amount_raw: Option<u128>,
    /// Quote calls spent on the partial-fill search.
    pub fill_search_calls: u64,
    /// `lower-bound value - known remaining basis` of a non-exact exit
    /// (status `lower_bound`); `unrealized_pnl` stays `None`.
    pub unrealized_lower_bound: Option<Money>,
    /// 4-byte selector of the quoter revert that led to a non-exact exit.
    pub revert_selector: Option<[u8; 4]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvmValuationOutcome {
    Valued(Box<EvmValuedPosition>),
    Unvalued { reason: EvmUnvaluedReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmPositionValuation {
    pub token: Address,
    pub open_amount_raw: u128,
    pub outcome: EvmValuationOutcome,
    /// The last-traded venue / pool / v4 pool id of the position, recorded
    /// for unvalued and illiquid positions too so users can inspect them.
    pub venue: Option<SwapVenue>,
    pub pool: Option<Address>,
    pub pool_id: Option<B256>,
    /// Raw 4-byte selector of the quoter revert behind an unvalued
    /// `quote_reverted` / `pool_not_initialized` (or a non-exact exit).
    pub revert_selector: Option<[u8; 4]>,
}

impl EvmPositionValuation {
    #[must_use]
    pub fn valued(&self) -> Option<&EvmValuedPosition> {
        match &self.outcome {
            EvmValuationOutcome::Valued(v) => Some(v),
            EvmValuationOutcome::Unvalued { .. } => None,
        }
    }

    #[must_use]
    pub fn unvalued_reason(&self) -> Option<EvmUnvaluedReason> {
        match &self.outcome {
            EvmValuationOutcome::Unvalued { reason } => Some(*reason),
            EvmValuationOutcome::Valued(_) => None,
        }
    }
}

/// Totals; realizable and unrealized are per quote unit (never summed across units).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvmOpenValuationTotals {
    pub positions: u64,
    /// Positions with an EXACT whole-amount quote.
    pub valued: u64,
    pub unvalued: u64,
    /// Positions whose last pool has no liquidity (lower bound 0).
    pub illiquid: u64,
    /// Positions only partly fillable in the last pool (lower bound).
    pub partially_fillable: u64,
    /// Unit label -> Σ lower-bound realizable raw of non-exact exits.
    pub realizable_lower_bound_raw_by_unit: BTreeMap<&'static str, u128>,
    /// Unit label -> Σ lower-bound unrealized (`Money`-scaled) of non-exact exits.
    pub unrealized_lower_bound_scaled_by_unit: BTreeMap<&'static str, i128>,
    /// Unit label (`eth`, `usdg`, ...) -> Σ realizable raw (exact only).
    pub realizable_raw_by_unit: BTreeMap<&'static str, u128>,
    /// Unit label -> Σ known unrealized (`Money`-scaled).
    pub unrealized_known_scaled_by_unit: BTreeMap<&'static str, i128>,
    pub unrealized_known_positions: u64,
    pub unrealized_unknown_positions: u64,
    pub usd_value_scaled: i128,
    pub usd_priced_positions: u64,
    pub usd_unrealized_known_scaled: i128,
    pub usd_unrealized_known_positions: u64,
    pub usd_unrealized_unknown_positions: u64,
    pub transfer_tax_positions: u64,
    pub unvalued_by_reason: BTreeMap<&'static str, u64>,
}

impl EvmOpenValuationTotals {
    pub fn merge(&mut self, o: &Self) {
        self.positions = self.positions.saturating_add(o.positions);
        self.valued = self.valued.saturating_add(o.valued);
        self.unvalued = self.unvalued.saturating_add(o.unvalued);
        self.illiquid = self.illiquid.saturating_add(o.illiquid);
        self.partially_fillable = self.partially_fillable.saturating_add(o.partially_fillable);
        for (k, v) in &o.realizable_lower_bound_raw_by_unit {
            let e = self
                .realizable_lower_bound_raw_by_unit
                .entry(k)
                .or_insert(0);
            *e = e.saturating_add(*v);
        }
        for (k, v) in &o.unrealized_lower_bound_scaled_by_unit {
            let e = self
                .unrealized_lower_bound_scaled_by_unit
                .entry(k)
                .or_insert(0);
            *e = e.saturating_add(*v);
        }
        for (k, v) in &o.realizable_raw_by_unit {
            let e = self.realizable_raw_by_unit.entry(k).or_insert(0);
            *e = e.saturating_add(*v);
        }
        for (k, v) in &o.unrealized_known_scaled_by_unit {
            let e = self.unrealized_known_scaled_by_unit.entry(k).or_insert(0);
            *e = e.saturating_add(*v);
        }
        self.unrealized_known_positions += o.unrealized_known_positions;
        self.unrealized_unknown_positions += o.unrealized_unknown_positions;
        self.usd_value_scaled = self.usd_value_scaled.saturating_add(o.usd_value_scaled);
        self.usd_priced_positions += o.usd_priced_positions;
        self.usd_unrealized_known_scaled = self
            .usd_unrealized_known_scaled
            .saturating_add(o.usd_unrealized_known_scaled);
        self.usd_unrealized_known_positions += o.usd_unrealized_known_positions;
        self.usd_unrealized_unknown_positions += o.usd_unrealized_unknown_positions;
        self.transfer_tax_positions += o.transfer_tax_positions;
        for (k, v) in &o.unvalued_by_reason {
            *self.unvalued_by_reason.entry(k).or_insert(0) += v;
        }
    }
}

/// The valuation of one wallet's open positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmOpenValuationView {
    pub version: &'static str,
    pub as_of: i64,
    /// Pinned block of the quotes (`None` when nothing was read).
    pub state_block: Option<u64>,
    /// Ascending token order.
    pub positions: Vec<EvmPositionValuation>,
}

impl EvmOpenValuationView {
    #[must_use]
    pub fn totals(&self) -> EvmOpenValuationTotals {
        let mut t = EvmOpenValuationTotals::default();
        for p in &self.positions {
            t.positions += 1;
            match &p.outcome {
                EvmValuationOutcome::Unvalued { reason } => {
                    t.unvalued += 1;
                    *t.unvalued_by_reason.entry(reason.label()).or_insert(0) += 1;
                }
                EvmValuationOutcome::Valued(v) if v.exit_status != EXIT_STATUS_EXACT => {
                    // A lower bound: never mixed into the exact sums.
                    if v.exit_status == EXIT_STATUS_ILLIQUID {
                        t.illiquid += 1;
                    } else {
                        t.partially_fillable += 1;
                    }
                    let u = quote_unit_label(v.quote_unit);
                    let e = t.realizable_lower_bound_raw_by_unit.entry(u).or_insert(0);
                    *e = e.saturating_add(v.realizable_raw);
                    if let Some(m) = v.unrealized_lower_bound {
                        let e = t
                            .unrealized_lower_bound_scaled_by_unit
                            .entry(u)
                            .or_insert(0);
                        *e = e.saturating_add(m.scaled_units());
                    }
                }
                EvmValuationOutcome::Valued(v) => {
                    t.valued += 1;
                    let u = quote_unit_label(v.quote_unit);
                    let e = t.realizable_raw_by_unit.entry(u).or_insert(0);
                    *e = e.saturating_add(v.realizable_raw);
                    if v.transfer_tax_not_modelled {
                        t.transfer_tax_positions += 1;
                    }
                    match v.unrealized_pnl {
                        Some(m) => {
                            t.unrealized_known_positions += 1;
                            let e = t.unrealized_known_scaled_by_unit.entry(u).or_insert(0);
                            *e = e.saturating_add(m.scaled_units());
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

    #[must_use]
    pub fn all_valued(&self) -> bool {
        self.positions.iter().all(|p| p.valued().is_some())
    }

    /// Quote assets (and the as-of minute) the USD values need.
    #[must_use]
    pub fn usd_assets(&self, chain: &ChainDisplay) -> BTreeSet<QuoteAsset> {
        self.positions
            .iter()
            .filter_map(|p| p.valued())
            .filter_map(|v| quote_asset_on(chain, v.quote_unit))
            .collect()
    }

    /// ADR-018: USD value at `as_of` of every valued position (one price per
    /// asset, one half-even rounding per position) and USD unrealized vs the
    /// remaining lots (`lots` = the ledger's `open_venues`). Peg assumptions
    /// are flagged in the label (`+binance_peg`). Never zero for unknown.
    pub fn apply_usd_price(
        &mut self,
        source: &dyn PriceSource,
        chain: &ChainDisplay,
        lots: &[OpenVenueInfo],
    ) {
        for p in &mut self.positions {
            let EvmValuationOutcome::Valued(v) = &mut p.outcome else {
                continue;
            };
            if v.exit_status != EXIT_STATUS_EXACT {
                // A lower bound is not priced as if it were exact.
                v.usd = None;
                v.usd_unpriced_reason = Some("lower_bound_not_priced".to_string());
                v.usd_unrealized = None;
                v.usd_unrealized_reason = Some("lower_bound_not_priced".to_string());
                continue;
            }
            v.usd = None;
            v.usd_unpriced_reason = None;
            v.usd_unrealized = None;
            v.usd_unrealized_reason = None;
            let Some(asset) = quote_asset_on(chain, v.quote_unit) else {
                v.usd_unpriced_reason = Some("unsupported_quote_unit".to_string());
                continue;
            };
            let obs = source.usd_price(asset, self.as_of);
            let Some(price) = obs.value else {
                v.usd_unpriced_reason = Some(match obs.label {
                    PriceLabel::Unknown { reason } => reason.label(),
                    _ => "price_unknown".to_string(),
                });
                continue;
            };
            let money = i128::try_from(v.realizable_raw)
                .ok()
                .and_then(|r| quote_units_to_money(v.quote_unit, r).ok());
            let converted = money.and_then(|m| convert_quote_to_usd(v.quote_unit, m, &price).ok());
            let Some(value) = converted else {
                v.usd_unpriced_reason = Some("conversion_overflow".to_string());
                continue;
            };
            let peg = matches!(
                v.quote_unit,
                QuoteUnit::BinancePegUsdtUnits | QuoteUnit::BinancePegUsdcUnits
            );
            let label = if peg {
                format!("{}+binance_peg", obs.label.label())
            } else {
                obs.label.label()
            };
            v.usd = Some(OpenUsd {
                value,
                price_label: label,
            });
            let Some(info) = lots.iter().find(|i| i.mint == evm_key(p.token)) else {
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
                match price_leg(
                    source,
                    chain,
                    l.unit,
                    l.acquired_price_ts,
                    l.basis,
                    &mut cov,
                ) {
                    Leg::Priced(m) => match basis.checked_add(&m) {
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
                None => match value.checked_sub(&basis) {
                    Ok(m) => v.usd_unrealized = Some(m),
                    Err(_) => v.usd_unrealized_reason = Some("conversion_overflow".to_string()),
                },
            }
        }
    }
}

/// Aggregate of one valuation run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvmOpenValuationRun {
    pub ran: bool,
    pub historical: bool,
    pub as_of: i64,
    /// The pinned block (`None` when nothing was read).
    pub state_block: Option<u64>,
    /// Logical `eth_call`s made (answers served from the cache are not counted).
    pub eth_calls: u64,
    /// `eth_getLogs` requests of v4 `Initialize` fallback lookups.
    pub log_calls: u64,
    pub cache_hits: u64,
    /// Cost plan (ADR-019 amendment): positions planned, calls planned for
    /// the admitted ones, the shared budget left when planning (`None` =
    /// unlimited) and positions refused up front.
    pub planned_positions: u64,
    pub planned_calls: u64,
    pub budget_left: Option<u64>,
    pub refused_positions: u64,
    pub wallets_with_open: u64,
    pub totals: EvmOpenValuationTotals,
    pub budget_exhausted: bool,
    pub fetch_error: Option<String>,
}

/// Options of a valuation run.
#[derive(Debug, Clone)]
pub struct EvmValuationOptions {
    /// SDK/test-only quoter addresses used when no official one is pinned
    /// (reported as `quoter_source = override`).
    pub quoter_overrides: BTreeMap<QuoterFamily, Address>,
    pub max_key_lookups: usize,
    /// Hard cap on `eth_getLogs` requests of the Initialize fallback per run.
    pub max_fallback_log_requests: u64,
}

impl Default for EvmValuationOptions {
    fn default() -> Self {
        Self {
            quoter_overrides: BTreeMap::new(),
            max_key_lookups: DEFAULT_MAX_KEY_LOOKUPS,
            max_fallback_log_requests: DEFAULT_MAX_FALLBACK_LOG_REQUESTS,
        }
    }
}

// ---------------------------------------------------------------------
// I/O
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct PoolIdent {
    token0: Address,
    token1: Address,
    fee: Option<u32>,
    tick_spacing: Option<i32>,
    factory: Option<Address>,
}

/// One concrete way to ask for an exit quote.
enum Plan {
    V3 {
        family: QuoterFamily,
        quoter: Address,
        source: &'static str,
        token_in: Address,
        token_out: Address,
        pool_selector: i32,
    },
    V4 {
        quoter: Address,
        source: &'static str,
        key: V4PoolKey,
        zero_for_one: bool,
    },
    V2 {
        reserve_in: U256,
        reserve_out: U256,
        fee: V2Fee,
    },
    Aero {
        pool: Address,
        token_in: Address,
    },
}

enum QErr {
    Reason(EvmUnvaluedReason),
    /// The quoter reverted; what its data says.
    Reverted(QuoterRevert),
}

/// A cached `eth_call` answer.
#[derive(Clone)]
enum CallOut {
    Returned(Vec<u8>),
    Reverted(Option<Vec<u8>>),
}

struct Valuer<'a> {
    rpc: &'a EvmRpcClient,
    cfg: &'a EvmExtractionConfig,
    opts: &'a EvmValuationOptions,
    block: u64,
    block_hex: String,
    run: &'a mut EvmOpenValuationRun,
    calls: BTreeMap<(Address, String), CallOut>,
    idents: BTreeMap<Address, Option<PoolIdent>>,
    keys: BTreeMap<B256, Option<V4PoolKey>>,
    key_lookups: usize,
    /// Fallback log requests still allowed (run cap, further limited to the
    /// budget slack the plan left: the fallback never burns planned calls).
    fallback_logs_left: u64,
    /// Partial-fill search calls still allowed in the run (`None` =
    /// unlimited); like the fallback, limited to the budget slack of the plan.
    search_calls_left: Option<u64>,
    /// Selector of the quoter revert of the position being valued.
    last_revert: Option<[u8; 4]>,
    /// A call hit the budget limit at run time (beyond the plan, e.g. retries).
    runtime_exhausted: bool,
}

/// What the cost plan has already counted (cache-aware simulation).
#[derive(Default, Clone)]
struct PlanSim {
    calls: BTreeSet<(Address, String)>,
    pools: BTreeSet<Address>,
    keys: BTreeSet<B256>,
    /// (pool or pool id, token, amount) quotes an earlier position already
    /// planned while the identity (so the calldata) was still unknown.
    quotes: BTreeSet<(B256, Address, u128)>,
}

fn sel_data(sig: &str) -> String {
    format!("0x{}", alloy_primitives::hex::encode(selector(sig)))
}

fn word_address(b: &[u8]) -> Option<Address> {
    let (head, tail) = b.get(..32)?.split_at(12);
    if head.iter().any(|x| *x != 0) {
        return None;
    }
    let a = Address::from_slice(tail);
    (a != Address::ZERO).then_some(a)
}

fn fail_reason(e: &EvmSourceError) -> EvmUnvaluedReason {
    if e.is_budget_exhausted() {
        EvmUnvaluedReason::RequestBudgetExhausted
    } else {
        EvmUnvaluedReason::StateFetchFailed
    }
}

impl Valuer<'_> {
    fn note_error(&mut self, e: &EvmSourceError) {
        if e.is_budget_exhausted() {
            self.run.budget_exhausted = true;
            self.runtime_exhausted = true;
        }
        if self.run.fetch_error.is_none() {
            self.run.fetch_error = Some(crate::sanitize_provider_text(&e.to_string()));
        }
    }

    /// One cached `eth_call` at the pinned block. `Ok(None)` = reverted.
    async fn call(
        &mut self,
        to: Address,
        data: String,
    ) -> Result<Option<Vec<u8>>, EvmUnvaluedReason> {
        Ok(match self.call_full(to, data).await? {
            CallOut::Returned(b) => Some(b),
            CallOut::Reverted(_) => None,
        })
    }

    /// Like [`Self::call`], keeping the revert data.
    async fn call_full(&mut self, to: Address, data: String) -> Result<CallOut, EvmUnvaluedReason> {
        if let Some(hit) = self.calls.get(&(to, data.clone())) {
            self.run.cache_hits += 1;
            return Ok(hit.clone());
        }
        self.run.eth_calls += 1;
        match self.rpc.eth_call_detailed(to, &data, &self.block_hex).await {
            Ok(r) => {
                let r = match r {
                    EthCallOutcome::Returned(b) => CallOut::Returned(b),
                    EthCallOutcome::Reverted { data } => CallOut::Reverted(data),
                };
                if self.calls.len() < MAX_CACHED_CALLS {
                    self.calls.insert((to, data), r.clone());
                }
                Ok(r)
            }
            Err(e) => {
                self.note_error(&e);
                Err(fail_reason(&e))
            }
        }
    }

    async fn ident(
        &mut self,
        pool: Address,
        venue: SwapVenue,
    ) -> Result<PoolIdent, EvmUnvaluedReason> {
        if let Some(c) = self.idents.get(&pool) {
            return c.ok_or(EvmUnvaluedReason::PoolIdentityUnknown);
        }
        let r = self.read_ident(pool, venue).await;
        match r {
            Ok(i) => {
                self.idents.insert(pool, Some(i));
                Ok(i)
            }
            Err(EvmUnvaluedReason::PoolIdentityUnknown) => {
                self.idents.insert(pool, None);
                Err(EvmUnvaluedReason::PoolIdentityUnknown)
            }
            Err(e) => Err(e),
        }
    }

    async fn read_ident(
        &mut self,
        pool: Address,
        venue: SwapVenue,
    ) -> Result<PoolIdent, EvmUnvaluedReason> {
        let word = |sel: &str| format!("0x{}", alloy_primitives::hex::encode(selector(sel)));
        let t0 = self.call(pool, word("token0()")).await?;
        let t1 = self.call(pool, word("token1()")).await?;
        let (Some(token0), Some(token1)) = (
            t0.as_deref().and_then(word_address),
            t1.as_deref().and_then(word_address),
        ) else {
            return Err(EvmUnvaluedReason::PoolIdentityUnknown);
        };
        let (mut fee, mut tick_spacing, mut factory) = (None, None, None);
        match venue {
            SwapVenue::UniswapV3 | SwapVenue::PancakeV3 => {
                let r = self.call(pool, word("fee()")).await?;
                fee = r
                    .as_deref()
                    .and_then(|b| b.get(..32))
                    .and_then(|b| u32::try_from(U256::from_be_slice(b)).ok());
                if fee.is_none() {
                    return Err(EvmUnvaluedReason::PoolIdentityUnknown);
                }
            }
            SwapVenue::AerodromeSlipstream => {
                // The pool's factory selects the quoter generation.
                let r = self.call(pool, word("factory()")).await?;
                factory = r.as_deref().and_then(word_address);
                if factory.is_none() {
                    return Err(EvmUnvaluedReason::PoolIdentityUnknown);
                }
                let r = self.call(pool, word("tickSpacing()")).await?;
                // int24 sign-extended; spacings are positive.
                tick_spacing = r
                    .as_deref()
                    .and_then(|b| b.get(..32))
                    .and_then(|b| i32::try_from(U256::from_be_slice(b)).ok());
                if tick_spacing.is_none() {
                    return Err(EvmUnvaluedReason::PoolIdentityUnknown);
                }
            }
            SwapVenue::UniswapV2 => {
                let r = self.call(pool, word("factory()")).await?;
                factory = r.as_deref().and_then(word_address);
                if factory.is_none() {
                    return Err(EvmUnvaluedReason::PoolIdentityUnknown);
                }
            }
            _ => {}
        }
        Ok(PoolIdent {
            token0,
            token1,
            fee,
            tick_spacing,
            factory,
        })
    }

    async fn v4_key(
        &mut self,
        manager: Address,
        pool_id: B256,
    ) -> Result<V4PoolKey, EvmUnvaluedReason> {
        if let Some(k) = self.keys.get(&pool_id) {
            return k.ok_or(EvmUnvaluedReason::PoolKeyUnknown);
        }
        // 1. The official PositionManager: one `eth_call`, accepted only when
        //    the key hashes to the full pool id (a zeroed key = unregistered).
        if let Some(pm) = pinned_v4_position_manager(self.cfg.profile.chain_id) {
            self.run.eth_calls += 1;
            match self
                .rpc
                .eth_call(pm, &v4_pool_keys_calldata(pool_id), &self.block_hex)
                .await
            {
                Ok(Some(bytes)) => {
                    if let Some(k) = decode_v4_pool_keys(&bytes, pool_id) {
                        self.keys.insert(pool_id, Some(k));
                        return Ok(k);
                    }
                }
                Ok(None) => {}
                Err(e) if e.is_budget_exhausted() => {
                    self.note_error(&e);
                    return Err(EvmUnvaluedReason::RequestBudgetExhausted);
                }
                // Any other failure: the log fallback may still find the key.
                Err(_) => {}
            }
        }
        // 2. Fallback: the `Initialize` log through the logs endpoint, under
        //    a hard cap on log requests per run.
        if self.key_lookups >= self.opts.max_key_lookups || self.fallback_logs_left == 0 {
            return Err(EvmUnvaluedReason::PoolKeyUnknown);
        }
        self.key_lookups += 1;
        let filter = LogFilter {
            addresses: vec![manager],
            topics: [
                Some(vec![V4_INITIALIZE_TOPIC0]),
                Some(vec![pool_id]),
                None,
                None,
            ],
        };
        let cap = u32::try_from(self.fallback_logs_left).unwrap_or(u32::MAX);
        let rpc = self
            .rpc
            .with_max_splits(KEY_LOOKUP_MAX_SPLITS)
            .with_max_log_requests(cap);
        let before = rpc
            .calls_by_method()
            .get("eth_getLogs")
            .copied()
            .unwrap_or(0);
        let res = rpc.get_logs(&filter, 0, self.block).await;
        let used = rpc
            .calls_by_method()
            .get("eth_getLogs")
            .copied()
            .unwrap_or(0)
            .saturating_sub(before);
        self.run.log_calls += used;
        self.fallback_logs_left = self.fallback_logs_left.saturating_sub(used);
        let key = match res {
            Ok(r) => r
                .logs
                .iter()
                .filter(|l| l.address == manager)
                .find_map(|l| decode_v4_initialize(l).decoded())
                .map(|i| V4PoolKey {
                    currency0: i.currency0,
                    currency1: i.currency1,
                    fee: i.fee,
                    tick_spacing: i.tick_spacing,
                    hooks: i.hooks,
                })
                // The pool id is the hash of the key: a mismatch is no key.
                .filter(|k| k.pool_id() == pool_id),
            Err(e) => {
                self.note_error(&e);
                if e.is_budget_exhausted() {
                    return Err(EvmUnvaluedReason::RequestBudgetExhausted);
                }
                None
            }
        };
        self.keys.insert(pool_id, key);
        key.ok_or(EvmUnvaluedReason::PoolKeyUnknown)
    }

    fn quote_side(&self, other: Address) -> Option<(QuoteUnit, Option<Address>)> {
        if other == Address::ZERO || other == self.cfg.profile.wrapped_native {
            return Some((QuoteUnit::Wei, None));
        }
        evm_unit_of(self.cfg, TradeQuote::Token(other)).map(|u| (u, Some(other)))
    }

    fn quoter(&self, family: QuoterFamily) -> Result<(Address, &'static str), EvmUnvaluedReason> {
        if let Some(a) = pinned_quoter(self.cfg.profile.chain_id, family) {
            return Ok((a, "pinned"));
        }
        self.opts
            .quoter_overrides
            .get(&family)
            .map(|a| (*a, "override"))
            .ok_or(EvmUnvaluedReason::VenueQuoterUnpinned)
    }

    /// Slipstream: the pinned quoter of the pool's factory generation, else
    /// the family override.
    fn slipstream_quoter(
        &self,
        factory: Option<Address>,
    ) -> Result<(Address, &'static str), EvmUnvaluedReason> {
        if let Some(a) =
            factory.and_then(|f| pinned_slipstream_quoter(self.cfg.profile.chain_id, f))
        {
            return Ok((a, "pinned"));
        }
        self.quoter(QuoterFamily::Slipstream)
    }

    async fn quote(&mut self, plan: &Plan, amount: U256) -> Result<U256, QErr> {
        use EvmUnvaluedReason as R;
        let (to, data, words) = match plan {
            Plan::V3 {
                family,
                quoter,
                token_in,
                token_out,
                pool_selector,
                ..
            } => (
                *quoter,
                v3_quote_calldata(*family, *token_in, *token_out, amount, *pool_selector),
                4,
            ),
            Plan::V4 {
                quoter,
                key,
                zero_for_one,
                ..
            } => {
                let a = u128::try_from(amount).map_err(|_| QErr::Reason(R::MathFailure))?;
                (*quoter, v4_quote_calldata(key, *zero_for_one, a), 2)
            }
            Plan::Aero { pool, token_in } => {
                (*pool, aerodrome_amount_out_calldata(amount, *token_in), 1)
            }
            Plan::V2 {
                reserve_in,
                reserve_out,
                fee,
            } => {
                return v2_amount_out(amount, *reserve_in, *reserve_out, *fee)
                    .ok_or(QErr::Reason(R::MathFailure));
            }
        };
        match self.call_full(to, data).await.map_err(QErr::Reason)? {
            CallOut::Reverted(d) => Err(QErr::Reverted(decode_quoter_revert(
                d.as_deref().unwrap_or_default(),
            ))),
            CallOut::Returned(bytes) => {
                decode_amount_out(&bytes, words).ok_or(QErr::Reason(R::QuoteResponseInvalid))
            }
        }
    }

    /// Build the plan of one position; returns it with the label facts.
    async fn plan(
        &mut self,
        token: Address,
        last: &crate::EvmLastVenue,
    ) -> Result<(Plan, Option<Address>, QuoteUnit, Option<Address>), EvmUnvaluedReason> {
        use EvmUnvaluedReason as R;
        let venue = last.venue;
        match venue {
            SwapVenue::FourMemeV1
            | SwapVenue::FourMemeV2
            | SwapVenue::PonsV2Curve
            | SwapVenue::BagsCurve => Err(R::VenueNotSupported),
            SwapVenue::UniswapV4 => {
                let pool_id = last.pool_id.ok_or(R::PoolKeyUnknown)?;
                let (quoter, source) = self.quoter(QuoterFamily::UniswapV4)?;
                let key = self.v4_key(last.pool, pool_id).await?;
                let (zero_for_one, other) = if key.currency0 == token {
                    (true, key.currency1)
                } else if key.currency1 == token {
                    (false, key.currency0)
                } else {
                    return Err(R::VenueNotSupported);
                };
                let (unit, qt) = self.quote_side(other).ok_or(R::QuoteAssetUnsupported)?;
                Ok((
                    Plan::V4 {
                        quoter,
                        source,
                        key,
                        zero_for_one,
                    },
                    Some(quoter),
                    unit,
                    qt,
                ))
            }
            SwapVenue::UniswapV3 | SwapVenue::PancakeV3 | SwapVenue::AerodromeSlipstream => {
                let family = match venue {
                    SwapVenue::UniswapV3 => QuoterFamily::UniswapV3,
                    SwapVenue::PancakeV3 => QuoterFamily::PancakeV3,
                    _ => QuoterFamily::Slipstream,
                };
                // Slipstream's quoter depends on the pool's factory, so the
                // identity is read first; the others pin one quoter per chain.
                let (quoter, source, id) = if family == QuoterFamily::Slipstream {
                    let id = self.ident(last.pool, venue).await?;
                    let (q, s) = self.slipstream_quoter(id.factory)?;
                    (q, s, id)
                } else {
                    let (q, s) = self.quoter(family)?;
                    (q, s, self.ident(last.pool, venue).await?)
                };
                let other = other_token(&id, token).ok_or(R::VenueNotSupported)?;
                let (unit, qt) = self.quote_side(other).ok_or(R::QuoteAssetUnsupported)?;
                let pool_selector = match family {
                    QuoterFamily::Slipstream => id.tick_spacing,
                    _ => id.fee.and_then(|f| i32::try_from(f).ok()),
                }
                .ok_or(R::PoolIdentityUnknown)?;
                Ok((
                    Plan::V3 {
                        family,
                        quoter,
                        source,
                        token_in: token,
                        token_out: other,
                        pool_selector,
                    },
                    Some(quoter),
                    unit,
                    qt,
                ))
            }
            SwapVenue::UniswapV2 => {
                let id = self.ident(last.pool, venue).await?;
                let other = other_token(&id, token).ok_or(R::VenueNotSupported)?;
                let (unit, qt) = self.quote_side(other).ok_or(R::QuoteAssetUnsupported)?;
                let fee = id
                    .factory
                    .and_then(|f| v2_fee_for_factory(self.cfg.profile.chain_id, f))
                    .ok_or(R::VenueNotSupported)?;
                let res = self
                    .call(last.pool, v2_reserves_calldata())
                    .await?
                    .ok_or(R::QuoteReverted)?;
                let (r0, r1) = decode_reserves(&res).ok_or(R::QuoteResponseInvalid)?;
                let (reserve_in, reserve_out) = if id.token0 == token {
                    (r0, r1)
                } else {
                    (r1, r0)
                };
                Ok((
                    Plan::V2 {
                        reserve_in,
                        reserve_out,
                        fee,
                    },
                    None,
                    unit,
                    qt,
                ))
            }
            SwapVenue::AerodromeV2 => {
                let id = self.ident(last.pool, venue).await?;
                let other = other_token(&id, token).ok_or(R::VenueNotSupported)?;
                let (unit, qt) = self.quote_side(other).ok_or(R::QuoteAssetUnsupported)?;
                Ok((
                    Plan::Aero {
                        pool: last.pool,
                        token_in: token,
                    },
                    None,
                    unit,
                    qt,
                ))
            }
        }
    }

    /// Planned requests of one position given what is already cached and what
    /// earlier admitted positions will fetch. An upper bound of the calls the
    /// valuation makes (a failure on the way costs less); the Initialize
    /// fallback is NOT counted here: it only spends the budget slack.
    fn position_cost(&self, info: &crate::EvmOpenVenueInfo, sim: &mut PlanSim) -> u64 {
        let Some(last) = info.last else { return 0 };
        let token = info.token;
        let amount = U256::from(info.open_amount_raw);
        let probe = info.open_amount_raw.div_euclid(1000);
        // Quotes (full + probe) of a pool whose calldata is not yet known:
        // free when an earlier admitted position planned the same ones.
        let probes = |sim: &mut PlanSim, scope: B256| {
            u64::from(sim.quotes.insert((scope, token, info.open_amount_raw)))
                + u64::from(probe > 0 && sim.quotes.insert((scope, token, probe)))
        };
        let mut c = 0u64;
        let call = |me: &Self, sim: &mut PlanSim, to: Address, data: String| -> u64 {
            if me.calls.contains_key(&(to, data.clone())) || sim.calls.contains(&(to, data.clone()))
            {
                0
            } else {
                sim.calls.insert((to, data));
                1
            }
        };
        // The liquidity read that confirms a dead pool after a revert (counted
        // for every position: the plan is an upper bound). The bounded
        // partial-fill search is NOT counted: it only spends budget slack.
        let liq_v4 = |me: &Self, sim: &mut PlanSim, pool_id: B256| -> u64 {
            pinned_v4_state_view(me.cfg.profile.chain_id)
                .map_or(0, |sv| call(me, sim, sv, v4_liquidity_calldata(pool_id)))
        };
        let liq_v3 = |me: &Self, sim: &mut PlanSim| -> u64 {
            call(me, sim, last.pool, v3_liquidity_calldata())
        };
        match last.venue {
            SwapVenue::FourMemeV1
            | SwapVenue::FourMemeV2
            | SwapVenue::PonsV2Curve
            | SwapVenue::BagsCurve => 0,
            SwapVenue::UniswapV4 => {
                let Some(pool_id) = last.pool_id else {
                    return 0;
                };
                let Ok((quoter, _)) = self.quoter(QuoterFamily::UniswapV4) else {
                    return 0;
                };
                match self.keys.get(&pool_id) {
                    Some(None) => 0,
                    Some(Some(key)) => {
                        let (zfo, other) = if key.currency0 == token {
                            (true, key.currency1)
                        } else if key.currency1 == token {
                            (false, key.currency0)
                        } else {
                            return 0;
                        };
                        if self.quote_side(other).is_none() {
                            return 0;
                        }
                        c += call(
                            self,
                            sim,
                            quoter,
                            v4_quote_calldata(key, zfo, info.open_amount_raw),
                        );
                        if probe > 0 {
                            c += call(self, sim, quoter, v4_quote_calldata(key, zfo, probe));
                        }
                        c + liq_v4(self, sim, pool_id)
                    }
                    None => {
                        if sim.keys.insert(pool_id)
                            && pinned_v4_position_manager(self.cfg.profile.chain_id).is_some()
                        {
                            c += 1;
                        }
                        c + probes(sim, pool_id) + liq_v4(self, sim, pool_id)
                    }
                }
            }
            SwapVenue::UniswapV3 | SwapVenue::PancakeV3 | SwapVenue::AerodromeSlipstream => {
                let family = match last.venue {
                    SwapVenue::UniswapV3 => QuoterFamily::UniswapV3,
                    SwapVenue::PancakeV3 => QuoterFamily::PancakeV3,
                    _ => QuoterFamily::Slipstream,
                };
                let slip = family == QuoterFamily::Slipstream;
                if !slip && self.quoter(family).is_err() {
                    return 0;
                }
                match self.idents.get(&last.pool) {
                    Some(None) => 0,
                    Some(Some(id)) => {
                        let quoter = if slip {
                            self.slipstream_quoter(id.factory)
                        } else {
                            self.quoter(family)
                        };
                        let Ok((quoter, _)) = quoter else { return 0 };
                        let Some(other) = other_token(id, token) else {
                            return 0;
                        };
                        if self.quote_side(other).is_none() {
                            return 0;
                        }
                        let sel = if slip {
                            id.tick_spacing
                        } else {
                            id.fee.and_then(|f| i32::try_from(f).ok())
                        };
                        let Some(sel) = sel else { return 0 };
                        c += call(
                            self,
                            sim,
                            quoter,
                            v3_quote_calldata(family, token, other, amount, sel),
                        );
                        if probe > 0 {
                            c += call(
                                self,
                                sim,
                                quoter,
                                v3_quote_calldata(family, token, other, U256::from(probe), sel),
                            );
                        }
                        c + liq_v3(self, sim)
                    }
                    None => {
                        if sim.pools.insert(last.pool) {
                            let sigs: &[&str] = if slip {
                                &["token0()", "token1()", "factory()", "tickSpacing()"]
                            } else {
                                &["token0()", "token1()", "fee()"]
                            };
                            for sg in sigs {
                                c += call(self, sim, last.pool, sel_data(sg));
                            }
                        }
                        c + probes(sim, last.pool.into_word()) + liq_v3(self, sim)
                    }
                }
            }
            SwapVenue::UniswapV2 | SwapVenue::AerodromeV2 => {
                let v2 = last.venue == SwapVenue::UniswapV2;
                match self.idents.get(&last.pool) {
                    Some(None) => return 0,
                    Some(Some(_)) => {}
                    None => {
                        if sim.pools.insert(last.pool) {
                            let sigs: &[&str] = if v2 {
                                &["token0()", "token1()", "factory()"]
                            } else {
                                &["token0()", "token1()"]
                            };
                            for sg in sigs {
                                c += call(self, sim, last.pool, sel_data(sg));
                            }
                        }
                    }
                }
                if v2 {
                    c + call(self, sim, last.pool, v2_reserves_calldata())
                } else {
                    c += call(
                        self,
                        sim,
                        last.pool,
                        aerodrome_amount_out_calldata(amount, token),
                    );
                    if probe > 0 {
                        c += call(
                            self,
                            sim,
                            last.pool,
                            aerodrome_amount_out_calldata(U256::from(probe), token),
                        );
                    }
                    c
                }
            }
        }
    }

    async fn value_one(
        &mut self,
        info: &crate::EvmOpenVenueInfo,
        basis: &Basis,
    ) -> EvmPositionValuation {
        self.last_revert = None;
        let outcome = match self.value_inner(info, basis).await {
            Ok(v) => EvmValuationOutcome::Valued(Box::new(v)),
            Err(reason) => EvmValuationOutcome::Unvalued { reason },
        };
        let mut p = position(info, outcome);
        p.revert_selector = self.last_revert.take();
        p
    }

    /// The position of a quote that came back reverted: `illiquid` after a
    /// liquidity read of 0, `partially_fillable` after a bounded search when
    /// the pool has liquidity; an unvalued reason otherwise.
    #[allow(clippy::too_many_arguments)]
    async fn on_revert(
        &mut self,
        info: &crate::EvmOpenVenueInfo,
        last: &crate::EvmLastVenue,
        plan: &Plan,
        facts: &Facts,
        basis: &Basis,
        kind: QuoterRevert,
    ) -> Result<EvmValuedPosition, EvmUnvaluedReason> {
        use EvmUnvaluedReason as R;
        self.last_revert = kind.raw_selector();
        // Which liquidity read confirms a dead pool for this revert shape.
        let read = match (plan, kind) {
            (_, QuoterRevert::PoolNotInitialized) => return Err(R::PoolNotInitialized),
            (Plan::V4 { key, .. }, QuoterRevert::NotEnoughLiquidity(_)) => {
                pinned_v4_state_view(self.cfg.profile.chain_id)
                    .map(|sv| (sv, v4_liquidity_calldata(key.pool_id())))
            }
            (Plan::V3 { .. }, QuoterRevert::NoData) => Some((last.pool, v3_liquidity_calldata())),
            _ => None,
        };
        let Some((to, data)) = read else {
            return Err(R::QuoteReverted);
        };
        let liquidity = match self.call_full(to, data).await {
            Ok(CallOut::Returned(b)) => decode_liquidity(&b),
            Ok(CallOut::Reverted(_)) => None,
            Err(r) if r == R::RequestBudgetExhausted => return Err(r),
            Err(_) => None,
        };
        let Some(liquidity) = liquidity else {
            return Err(R::QuoteReverted);
        };
        let amount = info.open_amount_raw;
        let mut v = self.base_position(info, last, plan, facts);
        v.fill_caveat = Some(CAVEAT_OTHER_POOLS_NOT_SEARCHED);
        v.revert_selector = kind.raw_selector();
        if liquidity == 0 {
            v.exit_status = EXIT_STATUS_ILLIQUID;
            v.illiquid_reason = Some(ILLIQUID_REASON_NO_LIQUIDITY);
            v.realizable_raw = 0;
            v.fillable_amount_raw = None;
            v.unfillable_amount_raw = Some(amount);
        } else {
            let (fillable, out, calls) = self.search_fillable(plan, amount).await;
            v.exit_status = EXIT_STATUS_PARTIALLY_FILLABLE;
            v.realizable_raw = out;
            v.fillable_amount_raw = Some(fillable);
            v.unfillable_amount_raw = Some(amount.saturating_sub(fillable));
            v.fill_search_calls = calls;
        }
        let (pnl, status) = basis.unrealized(facts.unit, v.realizable_raw);
        v.unrealized_pnl = None;
        v.unrealized_lower_bound = pnl;
        v.unrealized_status = if pnl.is_some() {
            UNREALIZED_STATUS_LOWER_BOUND
        } else {
            status
        };
        Ok(v)
    }

    /// Bisect the largest fillable amount below `amount` (known unfillable):
    /// at most [`MAX_FILL_SEARCH_CALLS`] quote calls, further limited to the
    /// run's budget slack. Returns `(fillable, output, calls)`; a failed
    /// (non-revert) call ends the search with the best answer so far.
    async fn search_fillable(&mut self, plan: &Plan, amount: u128) -> (u128, u128, u64) {
        let (mut lo, mut lo_out, mut hi) = (0u128, 0u128, amount);
        let mut calls = 0u64;
        while calls < MAX_FILL_SEARCH_CALLS {
            let mid = lo.saturating_add(hi.saturating_sub(lo).div_euclid(2));
            if mid <= lo || mid >= hi {
                break;
            }
            if let Some(left) = self.search_calls_left.as_mut() {
                if *left == 0 {
                    break;
                }
                *left -= 1;
            }
            calls += 1;
            match self.quote(plan, U256::from(mid)).await {
                Ok(o) => match u128::try_from(o) {
                    Ok(o) => (lo, lo_out) = (mid, o),
                    Err(_) => break,
                },
                Err(QErr::Reverted(_)) => hi = mid,
                Err(QErr::Reason(_)) => break,
            }
        }
        (lo, lo_out, calls)
    }

    /// The exact-valued skeleton of a position; callers adjust it.
    fn base_position(
        &self,
        info: &crate::EvmOpenVenueInfo,
        last: &crate::EvmLastVenue,
        plan: &Plan,
        facts: &Facts,
    ) -> EvmValuedPosition {
        let (method, quoter_source) = match plan {
            Plan::V3 { source, .. } => ("quoter_v2", *source),
            Plan::V4 { source, .. } => ("v4_quoter", *source),
            Plan::V2 { .. } => ("v2_reserves_constant_product", "not_applicable"),
            Plan::Aero { .. } => ("aerodrome_get_amount_out", "not_applicable"),
        };
        EvmValuedPosition {
            venue: last.venue,
            pool: last.pool,
            pool_id: last.pool_id,
            label: LABEL_REALIZABLE_ONCHAIN_QUOTE,
            method,
            quoter: facts.quoter,
            quoter_source,
            state_block: self.block,
            quote_unit: facts.unit,
            quote_token: facts.quote_token,
            realizable_raw: 0,
            probe_amount_raw: None,
            probe_out_raw: None,
            price_impact_bps: None,
            transfer_tax_not_modelled: info.transfer_tax_seen,
            unrealized_pnl: None,
            unrealized_status: "unknown_basis",
            usd: None,
            usd_unpriced_reason: None,
            usd_unrealized: None,
            usd_unrealized_reason: None,
            exit_status: EXIT_STATUS_EXACT,
            illiquid_reason: None,
            fill_caveat: None,
            fillable_amount_raw: None,
            unfillable_amount_raw: None,
            fill_search_calls: 0,
            unrealized_lower_bound: None,
            revert_selector: None,
        }
    }

    async fn value_inner(
        &mut self,
        info: &crate::EvmOpenVenueInfo,
        basis: &Basis,
    ) -> Result<EvmValuedPosition, EvmUnvaluedReason> {
        use EvmUnvaluedReason as R;
        let last = info.last.ok_or(R::NoVenueObserved)?;
        let (plan, quoter, unit, quote_token) = self.plan(info.token, &last).await?;
        let facts = Facts {
            quoter,
            unit,
            quote_token,
        };
        let amount = U256::from(info.open_amount_raw);
        let out = match self.quote(&plan, amount).await {
            Ok(o) => o,
            Err(QErr::Reason(r)) => return Err(r),
            Err(QErr::Reverted(kind)) => {
                return self
                    .on_revert(info, &last, &plan, &facts, basis, kind)
                    .await;
            }
        };
        let realizable_raw = u128::try_from(out).map_err(|_| R::MathFailure)?;
        // Marginal probe: a thousandth of the position (best effort).
        let probe_amt = info.open_amount_raw.div_euclid(1000);
        let mut probe_out = None;
        if probe_amt > 0
            && let Ok(o) = self.quote(&plan, U256::from(probe_amt)).await
        {
            probe_out = u128::try_from(o).ok();
        }
        let impact = probe_out
            .and_then(|po| price_impact_bps(amount, out, U256::from(probe_amt), U256::from(po)));
        let (unrealized_pnl, unrealized_status) = basis.unrealized(unit, realizable_raw);
        let mut v = self.base_position(info, &last, &plan, &facts);
        v.realizable_raw = realizable_raw;
        v.probe_amount_raw = (probe_amt > 0).then_some(probe_amt);
        v.probe_out_raw = probe_out;
        v.price_impact_bps = impact;
        v.unrealized_pnl = unrealized_pnl;
        v.unrealized_status = unrealized_status;
        Ok(v)
    }
}

/// Facts of a plan the position record needs.
struct Facts {
    quoter: Option<Address>,
    unit: QuoteUnit,
    quote_token: Option<Address>,
}

/// An unvalued/valued position record carrying the last venue facts.
fn position(info: &crate::EvmOpenVenueInfo, outcome: EvmValuationOutcome) -> EvmPositionValuation {
    EvmPositionValuation {
        token: info.token,
        open_amount_raw: info.open_amount_raw,
        outcome,
        venue: info.last.map(|l| l.venue),
        pool: info.last.map(|l| l.pool),
        pool_id: info.last.and_then(|l| l.pool_id),
        revert_selector: None,
    }
}

fn other_token(id: &PoolIdent, token: Address) -> Option<Address> {
    if id.token0 == token {
        Some(id.token1)
    } else if id.token1 == token {
        Some(id.token0)
    } else {
        None
    }
}

/// Known remaining basis of a position per unit.
struct Basis {
    by_unit: BTreeMap<QuoteUnit, Money>,
    all_known: bool,
}

impl Basis {
    fn of(info: Option<&OpenVenueInfo>) -> Self {
        let mut by_unit: BTreeMap<QuoteUnit, Money> = BTreeMap::new();
        let mut all_known = info.is_some();
        for l in info.map_or(&[][..], |i| &i.lots[..]) {
            if !l.basis_known {
                all_known = false;
                continue;
            }
            let e = by_unit.entry(l.unit).or_insert(Money::ZERO);
            match e.checked_add(&l.basis) {
                Ok(s) => *e = s,
                Err(_) => all_known = false,
            }
        }
        Self { by_unit, all_known }
    }

    fn unrealized(&self, unit: QuoteUnit, realizable_raw: u128) -> (Option<Money>, &'static str) {
        if !self.all_known {
            return (None, "unknown_basis");
        }
        if self.by_unit.keys().any(|u| *u != unit) {
            return (None, "basis_other_unit");
        }
        let basis = self.by_unit.get(&unit).copied().unwrap_or(Money::ZERO);
        let value = i128::try_from(realizable_raw)
            .ok()
            .and_then(|r| quote_units_to_money(unit, r).ok());
        match value.and_then(|v| v.checked_sub(&basis).ok()) {
            Some(m) => (Some(m), "known"),
            None => (None, "unknown_basis"),
        }
    }
}

fn unvalued_view(
    infos: &[crate::EvmOpenVenueInfo],
    as_of: i64,
    block: Option<u64>,
    reason: EvmUnvaluedReason,
) -> EvmOpenValuationView {
    EvmOpenValuationView {
        version: EVM_OPEN_VALUATION_VERSION,
        as_of,
        state_block: block,
        positions: infos
            .iter()
            .map(|i| position(i, EvmValuationOutcome::Unvalued { reason }))
            .collect(),
    }
}

/// ADR-019 EVM amendment: value the open positions of every EVM wallet ledger
/// and store an [`EvmOpenValuationView`] on `ledger.evm`.
///
/// * Historical windows (`until < as_of`): `unvalued { historical_window }`,
///   nothing is read.
/// * Otherwise the head block is read once and pinned; each position costs a
///   few `eth_call`s (pool identity cached, answers cached), all counted by the
///   client's request budget; a spent budget leaves the remaining positions
///   `unvalued { request_budget_exhausted }`.
pub async fn apply_evm_open_valuation(
    wallets: &mut [SolanaWalletStats],
    rpc: &EvmRpcClient,
    cfg: &EvmExtractionConfig,
    window: &AnalysisWindow,
    opts: &EvmValuationOptions,
) -> EvmOpenValuationRun {
    let mut run = EvmOpenValuationRun {
        ran: true,
        as_of: window.as_of,
        ..EvmOpenValuationRun::default()
    };
    let live = window_is_live(window);
    run.historical = !live;
    let any_open = wallets.iter().any(|w| {
        w.ledger
            .as_ref()
            .and_then(|l| l.evm.as_ref())
            .is_some_and(|e| !e.open_venues.is_empty())
    });
    // Pin the head once.
    let mut head: Result<u64, EvmUnvaluedReason> = Err(EvmUnvaluedReason::HistoricalWindow);
    if live && any_open {
        head = match rpc.block_number().await {
            Ok(b) => {
                run.state_block = Some(b);
                Ok(b)
            }
            Err(e) => {
                if e.is_budget_exhausted() {
                    run.budget_exhausted = true;
                }
                run.fetch_error = Some(crate::sanitize_provider_text(&e.to_string()));
                Err(fail_reason(&e))
            }
        };
    }
    let jobs: Vec<(usize, Vec<crate::EvmOpenVenueInfo>, Vec<OpenVenueInfo>)> = wallets
        .iter()
        .enumerate()
        .filter_map(|(i, w)| {
            let l = w.ledger.as_ref()?;
            let e = l.evm.as_ref()?;
            (!e.open_venues.is_empty()).then(|| (i, e.open_venues.clone(), l.open_venues.clone()))
        })
        .collect();
    let wallets_with_open = u64::try_from(jobs.len()).unwrap_or(u64::MAX);
    let mut views: Vec<(usize, EvmOpenValuationView)> = Vec::new();
    {
        let mut valuer = head.as_ref().ok().map(|block| Valuer {
            rpc,
            cfg,
            opts,
            block: *block,
            block_hex: format!("{block:#x}"),
            run: &mut run,
            calls: BTreeMap::new(),
            idents: BTreeMap::new(),
            keys: BTreeMap::new(),
            key_lookups: 0,
            fallback_logs_left: opts.max_fallback_log_requests,
            search_calls_left: None,
            last_revert: None,
            runtime_exhausted: false,
        });
        // Cost plan: admit positions in order while their (cache-aware)
        // planned calls fit the remaining budget; the rest are refused up
        // front, before any call is burnt on them.
        let mut admits: Vec<Vec<bool>> = Vec::new();
        if let Some(val) = valuer.as_mut() {
            let left = rpc.remaining_budget();
            let mut remaining = left;
            let mut sim = PlanSim::default();
            let mut refusing = false;
            let (mut total, mut planned, mut refused) = (0u64, 0u64, 0u64);
            for (_, infos, _) in &jobs {
                let mut row = Vec::with_capacity(infos.len());
                for info in infos {
                    let mut trial = sim.clone();
                    let cost = val.position_cost(info, &mut trial);
                    let fits = cost == 0 || (!refusing && remaining.is_none_or(|r| cost <= r));
                    if fits {
                        sim = trial;
                        total += cost;
                        planned += 1;
                        remaining = remaining.map(|r| r.saturating_sub(cost));
                    } else {
                        refusing = true;
                        refused += 1;
                    }
                    row.push(fits);
                }
                admits.push(row);
            }
            val.run.planned_positions = planned;
            val.run.planned_calls = total;
            val.run.budget_left = left;
            val.run.refused_positions = refused;
            if refused > 0 {
                val.run.budget_exhausted = true;
            }
            // The fallback only spends what the plan left over.
            if let Some(l) = left {
                val.fallback_logs_left = val.fallback_logs_left.min(l.saturating_sub(total));
                val.search_calls_left = Some(l.saturating_sub(total));
            }
        }
        for (ji, (i, infos, lots)) in jobs.iter().enumerate() {
            let view = match (&mut valuer, &head) {
                (Some(val), Ok(block)) => {
                    let mut positions = Vec::with_capacity(infos.len());
                    for (pi, info) in infos.iter().enumerate() {
                        let basis = Basis::of(lots.iter().find(|x| x.mint == evm_key(info.token)));
                        let admitted = admits
                            .get(ji)
                            .and_then(|r| r.get(pi))
                            .copied()
                            .unwrap_or(true);
                        positions.push(if !admitted || val.runtime_exhausted {
                            position(
                                info,
                                EvmValuationOutcome::Unvalued {
                                    reason: EvmUnvaluedReason::RequestBudgetExhausted,
                                },
                            )
                        } else {
                            val.value_one(info, &basis).await
                        });
                    }
                    EvmOpenValuationView {
                        version: EVM_OPEN_VALUATION_VERSION,
                        as_of: window.as_of,
                        state_block: Some(*block),
                        positions,
                    }
                }
                (_, Err(reason)) => unvalued_view(infos, window.as_of, None, *reason),
                (None, Ok(_)) => unvalued_view(
                    infos,
                    window.as_of,
                    None,
                    EvmUnvaluedReason::StateFetchFailed,
                ),
            };
            views.push((*i, view));
        }
    }
    run.wallets_with_open = wallets_with_open;
    for (i, view) in views {
        run.totals.merge(&view.totals());
        if let Some(e) = wallets
            .get_mut(i)
            .and_then(|w| w.ledger.as_mut())
            .and_then(|l| l.evm.as_mut())
        {
            e.open_valuation = Some(view);
        }
    }
    run
}

/// Distinct (asset, minute) pairs the USD values of all valued positions need.
#[must_use]
pub fn evm_open_usd_requirement(wallets: &[SolanaWalletStats]) -> BTreeMap<QuoteAsset, i64> {
    let mut out = BTreeMap::new();
    for w in wallets {
        let Some(l) = &w.ledger else { continue };
        let Some(v) = l.evm.as_ref().and_then(|e| e.open_valuation.as_ref()) else {
            continue;
        };
        for a in v.usd_assets(&l.chain) {
            out.insert(a, v.as_of);
        }
    }
    out
}
