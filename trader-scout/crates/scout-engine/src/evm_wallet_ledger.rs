//! EVM wallet ledger (ADR-020 step 2): the chain-agnostic core of
//! `solana_wallet_ledger` (FIFO lots, episodes, unknown propagation, unit
//! blocks, left-censoring) fed with EVM trades, producing the SAME report
//! struct so `wallet-stats`/`wallet-rank` render EVM results without a
//! parallel output stack.
//!
//! # Design choice (documented, ADR-020 amendment)
//! The Solana `Builder` is generalized, not copied: token amounts are
//! `u128` (18-decimal EVM tokens overflow `u64`), the native unit and the
//! quote-unit set come from a [`ChainDisplay`], and the asset key is a
//! closure. This module is a CHILD of `solana_wallet_ledger` so it reuses the
//! private builder without widening its visibility; no Solana golden changes
//! (the Solana path passes the same values it always did). EVM addresses
//! live in the 32-byte keys left-padded with zeros (`evm_key`).
//!
//! # Booking rules (mirror of ADR-010/013, see the module docs there)
//! * Trades are the extraction's [`EvmAttributedTrade`]s (owner-keyed net
//!   flows of the signer, verified venue evidence, one traded token and one
//!   quote asset), in canonical `(block, tx_index)` order, duplicates once.
//! * Units: native (ETH, WETH merged) -> `Wei` (18 dp, exact);
//!   USDG -> `UsdgUnits` (6 dp, valued at par in USD, never summed with
//!   anything); Base USDC -> `UsdcUnits` (6 dp); BSC Binance-Peg USDT/USDC ->
//!   `BinancePegUsdtUnits`/`BinancePegUsdcUnits` (18 dp, distinct units). A quote token without a unit is `Unknown { UnsupportedQuoteAsset }`.
//! * Gas: the signer pays it ONCE. It is capitalized into the basis of a
//!   native-quoted buy (basis = consideration + fee) and deducted from the
//!   proceeds of a native-quoted sell (net proceeds = gross - fee). For a
//!   USDG-quoted trade the native fee is not mixed into a USDG basis
//!   (ADR-013 section 2); it is recorded as unexplained native flow
//!   (`diagnostics.unexplained_native_flow_lamports` holds WEI here, negative =
//!   outflow). `gasUsedForL1` (Robinhood) is already in `gasUsed`; Base adds
//!   `l1Fee`; a missing Base `l1Fee` makes the trade `Unknown { FeeNotObserved }`.
//! * A native sell whose proceeds no source observed is
//!   `Unknown { NativeLegNotObserved }`: lot consumed, PnL unknown, episode
//!   `ClosedUnknown` - never zero, never priced from the pool event.
//! * Inventory continuity (ADR-010 section 6): any ERC-20 movement of a
//!   TRADED token in a signed transaction that the booked trade does not
//!   explain (transfer out, LP add/remove, router forward) is an
//!   `Unknown` lot / disposal. Tokens the wallet only receives in
//!   transactions it did not sign are invisible to a wallet-signed history:
//!   a later sale beyond the observed inventory books an `Unknown`-basis
//!   shortfall (`InventoryNotObserved`, or `LeftCensored` in a window).
//! * Failed transactions: counted; their fee is NOT attributed to trading
//!   (a reverted transaction carries no log proving it was a swap attempt),
//!   so `failed_trade_fees` stays 0 and no failed-fee journal is built.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256, I256, U256};
use scout_core::RawEvmTransaction;
use scout_dex_evm::{SwapVenue, VenueVerification};

use super::{
    Builder, LedgerOptions, SolanaWalletLedgerError, SolanaWalletLedgerReport, UnknownReason,
    Venue, quote_units_to_money,
};
use crate::chain_display::{ChainDisplay, evm_key};
use crate::evm_trade_extraction::{
    Consideration, EvmAttributedTrade, EvmExtractionConfig, EvmExtractionSummary, EvmFee,
    EvmTxOutcome, NoTradeReason, QuoteAsset, UnknownConsideration, extract_evm_trades,
    wallet_token_deltas,
};
use scout_dex_solana::{TradeSide, VariantVerification};
use scout_ledger::QuoteUnit;

pub const EVM_WALLET_LEDGER_VERSION: &str = "evm-wallet-ledger/1 (ADR-020 step 2: owner-keyed net-flow trades of the signer, wei + USDG (Robinhood) / USDC (Base) quote units, gas capitalized once for native-quoted trades, Unknown native legs, ERC-721 position NFTs are not flows, shared FIFO/episode core with the Solana ledger)";

pub const EVM_WALLET_LEDGER_SCOPE: &str = "quote units: native ETH/BNB (wei, WETH/WBNB merged; 18 dp), USDG on Robinhood / USDC on Base (6 dp raw, par in USD), Binance-Peg USDT/USDC on BSC (bridged/pegged, 18 dp raw, own units `usdt_peg`/`usdc_peg`, USDT via USDT-USD candles, USDC par, peg assumption flagged `binance_peg`); no FX, per-unit PnL never summed; trade = tx signer, a verified venue swap event (FixtureVerified deployments, e.g. Uniswap v4 PoolManager on Robinhood and Base) moving the token, exactly one traded token and one quote asset with opposite signs in the signer's own net flows; gas = fee payer only";

/// One booked EVM trade (audit trail; the EVM analogue of `RouteSwapRecord`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmTradeRecord {
    pub tx_hash: B256,
    pub block_number: u64,
    pub transaction_index: u64,
    pub block_time: u64,
    pub token: Address,
    pub side: TradeSide,
    pub unit: QuoteUnit,
    /// Raw token units of the wallet's net delta.
    pub token_amount: u128,
    /// Raw units of `unit` (paid for a buy, received for a sell); `None` =
    /// unknown (never zero).
    pub quote_amount: Option<u128>,
    /// Gas in wei paid by the wallet; `None` = not observed exactly.
    pub fee_wei: Option<u128>,
    pub venue: &'static str,
    pub venue_kind: SwapVenue,
    pub venue_verification: &'static str,
    /// Swap-event emitter of the trade (pool / PoolManager / launchpad manager).
    pub pool: Address,
    /// Uniswap v4 pool id.
    pub pool_id: Option<B256>,
    /// See `EvmAttributedTrade::token_flow_shortfall`.
    pub token_flow_shortfall: bool,
    /// `not_involved`, `trace`, `explorer_internal`, `alchemy_internal`, `balance_diff`,
    /// `logs_and_value_only` (how the native leg was established).
    pub native_leg: &'static str,
}

/// Ledger-side input of the exit valuation of one open EVM position
/// (ADR-019 EVM amendment): where the wallet last traded the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmOpenVenueInfo {
    pub token: Address,
    pub open_amount_raw: u128,
    /// Venue of the wallet's latest booked trade of the token; `None` =
    /// the position was never traded in a booked swap (transfers only).
    pub last: Option<EvmLastVenue>,
    /// Any booked trade of the token showed a transfer-tax shaped shortfall.
    pub transfer_tax_seen: bool,
}

/// The latest booked venue of a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmLastVenue {
    pub venue: SwapVenue,
    pub pool: Address,
    pub pool_id: Option<B256>,
    pub block_number: u64,
}

/// EVM-only facts of one ledger (the report's `evm` field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmLedgerExtras {
    /// Extraction counters of the wallet's transactions.
    pub extraction: EvmExtractionSummary,
    /// Booked trades in canonical order.
    pub trades: Vec<EvmTradeRecord>,
    /// Native-quoted trades by how their native leg was established.
    pub native_leg_counts: BTreeMap<&'static str, u64>,
    /// Gas (wei) of USDG-quoted trades, not capitalized (see module docs).
    pub uncapitalized_gas_wei: u128,
    /// Swap logs at non-gated emitters in the wallet's transactions (an
    /// unverified venue touched the wallet's history: a COVERAGE GAP).
    pub ungated_swap_logs: u64,
    /// Transactions with a gated swap that were not booked, by reason.
    pub rejected_swap_txs: u64,
    /// One entry per open position (ascending token), the valuation input.
    pub open_venues: Vec<EvmOpenVenueInfo>,
    /// ADR-019 EVM amendment: filled by `apply_evm_open_valuation`.
    pub open_valuation: Option<crate::evm_open_valuation::EvmOpenValuationView>,
}

fn to_u128(x: U256, ctx: &'static str) -> Result<u128, SolanaWalletLedgerError> {
    u128::try_from(x).map_err(|_| SolanaWalletLedgerError::Overflow(ctx))
}

pub(crate) fn unit_of(cfg: &EvmExtractionConfig, quote: QuoteAsset) -> Option<QuoteUnit> {
    match quote {
        QuoteAsset::Native => Some(QuoteUnit::Wei),
        QuoteAsset::Token(a) => {
            cfg.quote_tokens
                .iter()
                .find(|q| q.address == a)
                .and_then(|q| match (cfg.profile.chain_id, q.symbol.as_str()) {
                    // Units are per (chain, asset): BSC's 18-dp Binance-Peg
                    // stables are NOT the 6-dp `UsdcUnits`/`UsdtUnits`.
                    (56, "USDT") => Some(QuoteUnit::BinancePegUsdtUnits),
                    (56, "USDC") => Some(QuoteUnit::BinancePegUsdcUnits),
                    (_, "USDG") => Some(QuoteUnit::UsdgUnits),
                    (8453, "USDC") => Some(QuoteUnit::UsdcUnits),
                    _ => None,
                })
        }
    }
}

fn verification_of(v: VenueVerification) -> VariantVerification {
    match v {
        VenueVerification::FixtureVerified => VariantVerification::FixtureVerified,
        VenueVerification::IdlOnly => VariantVerification::IdlOnly,
    }
}

fn signed_u128(x: u128, ctx: &'static str) -> Result<i128, SolanaWalletLedgerError> {
    i128::try_from(x).map_err(|_| SolanaWalletLedgerError::Overflow(ctx))
}

impl Builder {
    /// Book one extracted EVM trade. Returns the signed explained token
    /// delta (`+` buy, `-` sell) for the continuity check.
    fn apply_evm_trade(
        &mut self,
        cfg: &EvmExtractionConfig,
        t: &EvmAttributedTrade,
        extras: &mut EvmLedgerExtras,
    ) -> Result<i128, SolanaWalletLedgerError> {
        let mint = evm_key(t.token);
        let ts = i64::try_from(t.block_time).ok();
        let loc = (t.block_number, t.transaction_index);
        self.price_ts = ts;
        self.traded.insert(mint);
        if let Some(ts) = ts {
            self.stamped.push((ts, mint));
        }
        let token_amount = to_u128(t.token_amount, "evm token amount")?;
        let signed = signed_u128(token_amount, "evm token amount")?;

        match t.side {
            TradeSide::Buy => {
                self.counts.buys += 1;
                self.counts.route.buys += 1;
            }
            TradeSide::Sell => {
                self.counts.sells += 1;
                self.counts.route.sells += 1;
            }
        }
        let verification = verification_of(t.venue_verification);
        match verification {
            VariantVerification::FixtureVerified => self.counts.fixture_verified_variant += 1,
            VariantVerification::IdlOnly => self.counts.idl_only_variant += 1,
        }
        self.variant_counts
            .entry((Venue::Route, t.venue.label()))
            .or_insert((verification, 0))
            .1 += 1;

        let fee_wei = match t.fee {
            EvmFee::Known(f) => Some(to_u128(f, "evm fee")?),
            EvmFee::Unknown(_) => None,
        };
        let native_leg_label = t.native_leg.label();
        let mut record = EvmTradeRecord {
            tx_hash: t.tx_hash,
            block_number: t.block_number,
            transaction_index: t.transaction_index,
            block_time: t.block_time,
            token: t.token,
            side: t.side,
            unit: QuoteUnit::Wei,
            token_amount,
            quote_amount: None,
            fee_wei,
            venue: t.venue.label(),
            venue_kind: t.venue,
            venue_verification: verification.label(),
            pool: t.venue_emitter,
            pool_id: t.venue_pool_id,
            token_flow_shortfall: t.token_flow_shortfall,
            native_leg: native_leg_label,
        };

        let Some(unit) = unit_of(cfg, t.quote) else {
            // A configured quote token without a ledger unit.
            self.counts.unsupported_quote += 1;
            self.book_unknown(
                mint,
                t.side,
                token_amount,
                QuoteUnit::Wei,
                UnknownReason::UnsupportedQuoteAsset,
                ts,
                loc,
            )?;
            extras.trades.push(record);
            return Ok(match t.side {
                TradeSide::Buy => signed,
                TradeSide::Sell => -signed,
            });
        };
        record.unit = unit;
        if unit == self.chain.native_unit {
            *extras
                .native_leg_counts
                .entry(native_leg_label)
                .or_insert(0) += 1;
        }

        match t.consideration {
            Consideration::Unknown(UnknownConsideration::NativeLegNotObserved) => {
                self.counts.native_leg_not_observed += 1;
                self.book_unknown(
                    mint,
                    t.side,
                    token_amount,
                    unit,
                    UnknownReason::NativeLegNotObserved,
                    ts,
                    loc,
                )?;
            }
            Consideration::Exact(amount) => {
                let amount = to_u128(amount, "evm consideration")?;
                record.quote_amount = Some(amount);
                let native_quoted = unit == self.chain.native_unit;
                let fee_for_basis = if native_quoted { fee_wei } else { None };
                if native_quoted && fee_wei.is_none() {
                    // Exact consideration, fee not exactly known: the
                    // capitalized basis/net proceeds would be wrong.
                    self.counts.fee_not_observed += 1;
                    self.book_unknown(
                        mint,
                        t.side,
                        token_amount,
                        unit,
                        UnknownReason::FeeNotObserved,
                        ts,
                        loc,
                    )?;
                } else {
                    self.counts.priced += 1;
                    self.counts.route_swaps += 1;
                    self.counts.route_swaps_by_quote.bump(unit);
                    let fee = fee_for_basis.unwrap_or(0);
                    match t.side {
                        TradeSide::Buy => {
                            let basis = signed_u128(amount, "evm buy basis")?
                                .checked_add(signed_u128(fee, "evm fee")?)
                                .ok_or(SolanaWalletLedgerError::Overflow("evm buy basis"))?;
                            self.acquire(
                                mint,
                                token_amount,
                                Some(quote_units_to_money(unit, basis)?),
                                unit,
                                None,
                                ts,
                                loc,
                            )?;
                        }
                        TradeSide::Sell => {
                            self.dispose(
                                mint,
                                token_amount,
                                Some(quote_units_to_money(
                                    unit,
                                    signed_u128(amount, "evm proceeds")?,
                                )?),
                                unit,
                                quote_units_to_money(unit, signed_u128(fee, "evm fee")?)?,
                                UnknownReason::ConsiderationUnverified,
                                ts,
                                loc,
                            )?;
                        }
                    }
                    if !native_quoted && let Some(f) = fee_wei {
                        extras.uncapitalized_gas_wei =
                            extras.uncapitalized_gas_wei.saturating_add(f);
                        let f = signed_u128(f, "evm fee")?;
                        self.diag.unexplained_native_flow_lamports = self
                            .diag
                            .unexplained_native_flow_lamports
                            .checked_sub(f)
                            .ok_or(SolanaWalletLedgerError::Overflow("uncapitalized gas"))?;
                        self.diag.unexplained_native_flow_txs += 1;
                    }
                }
            }
        }
        extras.trades.push(record);
        Ok(match t.side {
            TradeSide::Buy => signed,
            TradeSide::Sell => -signed,
        })
    }

    /// Book a trade whose consideration/basis is unknown (`reason`).
    #[allow(clippy::too_many_arguments)]
    fn book_unknown(
        &mut self,
        mint: [u8; 32],
        side: TradeSide,
        token_amount: u128,
        unit: QuoteUnit,
        reason: UnknownReason,
        ts: Option<i64>,
        loc: (u64, u64),
    ) -> Result<(), SolanaWalletLedgerError> {
        match side {
            TradeSide::Buy => self.acquire(mint, token_amount, None, unit, Some(reason), ts, loc),
            TradeSide::Sell => self.dispose(
                mint,
                token_amount,
                None,
                unit,
                scout_core::Money::ZERO,
                reason,
                ts,
                loc,
            ),
        }
    }
}

/// Build the ledger of `wallet` over its transactions `txs` (any order, any
/// duplication; foreign-chain transactions are rejected by the extraction and
/// counted). `options.left_censoring` = the input is the wallet's window.
///
/// # Errors
/// Arithmetic overflow or an internal ledger inconsistency only; malformed
/// data is counted in the report.
pub fn build_evm_wallet_ledger(
    cfg: &EvmExtractionConfig,
    wallet: Address,
    txs: &[RawEvmTransaction],
    options: LedgerOptions,
) -> Result<SolanaWalletLedgerReport, SolanaWalletLedgerError> {
    let display = ChainDisplay::evm(&cfg.profile);
    let mut b = Builder::new(
        evm_key(wallet),
        display,
        options.left_censoring,
        Box::new(move |k| display.asset_key(&k)),
    )?;

    // Canonical order, duplicates once (the extraction does both; the same
    // dedup is applied to the transaction lookup used for continuity).
    let (extractions, summary) = extract_evm_trades(txs, cfg, Some(wallet), None);
    let mut by_hash: BTreeMap<B256, &RawEvmTransaction> = BTreeMap::new();
    let mut ordered: Vec<&RawEvmTransaction> = txs.iter().collect();
    ordered.sort_by_key(|t| (t.block_number, t.transaction_index, t.hash));
    for t in ordered {
        by_hash.entry(t.hash).or_insert(t);
    }
    b.diag.transactions_considered = summary.transactions;
    b.diag.duplicate_transactions_ignored = summary.duplicates_ignored;

    // Pass 1: the traded-token set.
    for e in &extractions {
        if let EvmTxOutcome::Trade(t) = &e.outcome {
            b.traded.insert(evm_key(t.token));
        }
    }

    let mut extras = EvmLedgerExtras {
        extraction: summary,
        trades: Vec::new(),
        native_leg_counts: BTreeMap::new(),
        uncapitalized_gas_wei: 0,
        ungated_swap_logs: 0,
        rejected_swap_txs: 0,
        open_venues: Vec::new(),
        open_valuation: None,
    };
    let wrapped = cfg.profile.wrapped_native;
    let quote_tokens: BTreeSet<Address> = cfg.quote_tokens.iter().map(|q| q.address).collect();

    // Pass 2: canonical order.
    for e in &extractions {
        extras.ungated_swap_logs += u64::from(e.ungated_swap_logs);
        let Some(tx) = by_hash.get(&e.tx_hash).copied() else {
            continue;
        };
        let loc = (tx.block_number, tx.transaction_index);
        let ts = i64::try_from(tx.block_time).ok();
        let mut explained: BTreeMap<Address, i128> = BTreeMap::new();
        match &e.outcome {
            EvmTxOutcome::Failed => {
                b.diag.failed_transactions += 1;
                continue;
            }
            EvmTxOutcome::Trade(t) => {
                let delta = b.apply_evm_trade(cfg, t, &mut extras)?;
                explained.insert(t.token, delta);
            }
            EvmTxOutcome::NoTrade(reason) => {
                if e.gated_swap_logs > 0 {
                    extras.rejected_swap_txs += 1;
                    let rr = &mut b.diag.route_rejected;
                    match reason {
                        NoTradeReason::NotTxSigner
                        | NoTradeReason::LaunchpadAccountNotWallet
                        | NoTradeReason::LaunchpadRecipientNotWallet => {
                            rr.wallet_not_signer += 1;
                        }
                        NoTradeReason::MultiAsset
                        | NoTradeReason::MalformedLog(_)
                        | NoTradeReason::Overflow => rr.multi_asset += 1,
                        NoTradeReason::SameSign => rr.not_opposite_signs += 1,
                        NoTradeReason::NoQuoteLeg => rr.no_quote_leg += 1,
                        NoTradeReason::NoVerifiedSwapEvent
                        | NoTradeReason::NoTradedToken
                        | NoTradeReason::TokenFilterMismatch
                        | NoTradeReason::ChainMismatch => rr.no_verified_leg += 1,
                    }
                }
                if matches!(
                    reason,
                    NoTradeReason::ChainMismatch | NoTradeReason::NotTxSigner
                ) {
                    continue;
                }
            }
        }
        // §6 inventory continuity over the wallet's own token deltas.
        let Some(deltas) = wallet_token_deltas(tx, wallet, wrapped) else {
            return Err(SolanaWalletLedgerError::Overflow("evm wallet token deltas"));
        };
        let mut candidates: BTreeSet<Address> = explained.keys().copied().collect();
        for (token, d) in &deltas {
            if *d == I256::ZERO {
                continue;
            }
            if b.traded.contains(&evm_key(*token)) {
                candidates.insert(*token);
            } else if !quote_tokens.contains(token) {
                b.diag.out_of_scope_token_movements += 1;
            }
        }
        for token in candidates {
            let delta = deltas.get(&token).copied().unwrap_or(I256::ZERO);
            let delta_i = i128::try_from(delta)
                .map_err(|_| SolanaWalletLedgerError::Overflow("evm continuity delta"))?;
            let diff = delta_i
                .checked_sub(explained.get(&token).copied().unwrap_or(0))
                .ok_or(SolanaWalletLedgerError::Overflow("evm continuity diff"))?;
            if diff == 0 {
                continue;
            }
            b.price_ts = ts;
            b.diag.continuity_breaks += 1;
            let mint = evm_key(token);
            let amount = diff.unsigned_abs();
            if diff > 0 {
                b.acquire(
                    mint,
                    amount,
                    None,
                    QuoteUnit::Wei,
                    Some(UnknownReason::UnexplainedInboundTokenMovement),
                    ts,
                    loc,
                )?;
            } else {
                b.dispose(
                    mint,
                    amount,
                    None,
                    QuoteUnit::Wei,
                    scout_core::Money::ZERO,
                    UnknownReason::UnexplainedOutboundTokenMovement,
                    ts,
                    loc,
                )?;
            }
        }
    }

    let mut report = b.finish()?;
    // ADR-019 EVM amendment inputs: the latest booked venue per open token.
    let mut last: BTreeMap<Address, EvmLastVenue> = BTreeMap::new();
    let mut tax: BTreeSet<Address> = BTreeSet::new();
    for r in &extras.trades {
        last.insert(
            r.token,
            EvmLastVenue {
                venue: r.venue_kind,
                pool: r.pool,
                pool_id: r.pool_id,
                block_number: r.block_number,
            },
        );
        if r.token_flow_shortfall {
            tax.insert(r.token);
        }
    }
    extras.open_venues = report
        .open_positions
        .iter()
        .map(|p| {
            let token = crate::chain_display::evm_address_of_key(&p.mint);
            EvmOpenVenueInfo {
                token,
                open_amount_raw: p.open_amount_raw,
                last: last.get(&token).copied(),
                transfer_tax_seen: tax.contains(&token),
            }
        })
        .collect();
    report.ledger_version = EVM_WALLET_LEDGER_VERSION;
    report.evm = Some(extras);
    Ok(report)
}
