//! ADR-019 EVM amendment: realizable (on-chain quote) valuation of OPEN
//! positions at the run's as-of head.
//!
//! A position is valued as what the wallet would receive by selling its WHOLE
//! remaining amount into the pool's other asset NOW, asked from the venue
//! itself with an `eth_call` pinned to one block (the head read at the start of
//! the valuation):
//! * Uniswap v3 / Pancake v3 / Slipstream: official `QuoterV2.quoteExactInputSingle`;
//! * Uniswap v4: official `V4Quoter.quoteExactInputSingle` (PoolKey rebuilt from
//!   the pool's `Initialize` log and verified against the pool id);
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
    QuoterFamily, SwapVenue, V2Fee, V4_INITIALIZE_TOPIC0, V4PoolKey, aerodrome_amount_out_calldata,
    decode_amount_out, decode_reserves, decode_v4_initialize, pinned_quoter,
    pinned_slipstream_quoter, price_impact_bps, selector, v2_amount_out, v2_fee_for_factory,
    v2_reserves_calldata, v3_quote_calldata, v4_quote_calldata,
};
use scout_pricing::{PriceLabel, PriceSource, QuoteAsset};
use scout_providers::{EvmRpcClient, EvmSourceError, LogFilter};

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
/// Extra label: a transfer tax was suspected, the quote does not model it.
pub const LABEL_TRANSFER_TAX_NOT_MODELLED: &str = "transfer_tax_not_modelled";

/// Max entries of the per-run `eth_call` answer cache.
const MAX_CACHED_CALLS: usize = 4_096;
/// `eth_getLogs` split limit of one v4 `Initialize` lookup.
const KEY_LOOKUP_MAX_SPLITS: u32 = 32;
/// Default cap on v4 pool-key lookups per run.
pub const DEFAULT_MAX_KEY_LOOKUPS: usize = 32;

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
    /// The quoter / pool call reverted.
    QuoteReverted,
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
    pub valued: u64,
    pub unvalued: u64,
    /// Unit label (`eth`, `usdg`, ...) -> Σ realizable raw.
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
    /// `eth_getLogs` requests of v4 `Initialize` lookups.
    pub log_calls: u64,
    pub cache_hits: u64,
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
}

impl Default for EvmValuationOptions {
    fn default() -> Self {
        Self {
            quoter_overrides: BTreeMap::new(),
            max_key_lookups: DEFAULT_MAX_KEY_LOOKUPS,
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
}

struct Valuer<'a> {
    rpc: &'a EvmRpcClient,
    cfg: &'a EvmExtractionConfig,
    opts: &'a EvmValuationOptions,
    block: u64,
    block_hex: String,
    run: &'a mut EvmOpenValuationRun,
    calls: BTreeMap<(Address, String), Option<Vec<u8>>>,
    idents: BTreeMap<Address, Option<PoolIdent>>,
    keys: BTreeMap<B256, Option<V4PoolKey>>,
    key_lookups: usize,
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
        if let Some(hit) = self.calls.get(&(to, data.clone())) {
            self.run.cache_hits += 1;
            return Ok(hit.clone());
        }
        self.run.eth_calls += 1;
        match self.rpc.eth_call(to, &data, &self.block_hex).await {
            Ok(r) => {
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
        if self.key_lookups >= self.opts.max_key_lookups {
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
        let rpc = self.rpc.with_max_splits(KEY_LOOKUP_MAX_SPLITS);
        let res = rpc.get_logs(&filter, 0, self.block).await;
        let key = match res {
            Ok(r) => {
                self.run.log_calls += u64::from(r.requests);
                r.logs
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
                    .filter(|k| k.pool_id() == pool_id)
            }
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
        let ans = self.call(to, data).await.map_err(QErr::Reason)?;
        let Some(bytes) = ans else {
            return Err(QErr::Reason(R::QuoteReverted));
        };
        decode_amount_out(&bytes, words).ok_or(QErr::Reason(R::QuoteResponseInvalid))
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
            SwapVenue::FourMemeV1 | SwapVenue::FourMemeV2 => Err(R::VenueNotSupported),
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

    async fn value_one(
        &mut self,
        info: &crate::EvmOpenVenueInfo,
        basis: &Basis,
    ) -> EvmPositionValuation {
        let outcome = match self.value_inner(info, basis).await {
            Ok(v) => EvmValuationOutcome::Valued(Box::new(v)),
            Err(reason) => EvmValuationOutcome::Unvalued { reason },
        };
        EvmPositionValuation {
            token: info.token,
            open_amount_raw: info.open_amount_raw,
            outcome,
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
        let amount = U256::from(info.open_amount_raw);
        let out = self
            .quote(&plan, amount)
            .await
            .map_err(|QErr::Reason(r)| r)?;
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
        let (method, quoter_source) = match &plan {
            Plan::V3 { source, .. } => ("quoter_v2", *source),
            Plan::V4 { source, .. } => ("v4_quoter", *source),
            Plan::V2 { .. } => ("v2_reserves_constant_product", "not_applicable"),
            Plan::Aero { .. } => ("aerodrome_get_amount_out", "not_applicable"),
        };
        let (unrealized_pnl, unrealized_status) = basis.unrealized(unit, realizable_raw);
        Ok(EvmValuedPosition {
            venue: last.venue,
            pool: last.pool,
            pool_id: last.pool_id,
            label: LABEL_REALIZABLE_ONCHAIN_QUOTE,
            method,
            quoter,
            quoter_source,
            state_block: self.block,
            quote_unit: unit,
            quote_token,
            realizable_raw,
            probe_amount_raw: (probe_amt > 0).then_some(probe_amt),
            probe_out_raw: probe_out,
            price_impact_bps: impact,
            transfer_tax_not_modelled: info.transfer_tax_seen,
            unrealized_pnl,
            unrealized_status,
            usd: None,
            usd_unpriced_reason: None,
            usd_unrealized: None,
            usd_unrealized_reason: None,
        })
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
            .map(|i| EvmPositionValuation {
                token: i.token,
                open_amount_raw: i.open_amount_raw,
                outcome: EvmValuationOutcome::Unvalued { reason },
            })
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
        });
        for (i, infos, lots) in &jobs {
            let view = match (&mut valuer, &head) {
                (Some(val), Ok(block)) => {
                    let mut positions = Vec::with_capacity(infos.len());
                    for info in infos {
                        let basis = Basis::of(lots.iter().find(|x| x.mint == evm_key(info.token)));
                        positions.push(if val.run.budget_exhausted {
                            EvmPositionValuation {
                                token: info.token,
                                open_amount_raw: info.open_amount_raw,
                                outcome: EvmValuationOutcome::Unvalued {
                                    reason: EvmUnvaluedReason::RequestBudgetExhausted,
                                },
                            }
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
