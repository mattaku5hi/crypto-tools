//! ADR-019 open-position valuation DTOs shared by `wallet-stats` and
//! `wallet-rank` (`run_meta.open_valuation`, per-position records, totals).
//! Money convention of the CLIs: lamports and SOL are decimal STRINGS
//! (exact), unknown is `null` with an explicit status, never `0`.

use std::collections::BTreeMap;

use scout_core::{MONEY_SCALE, Money};
use scout_sdk::engine::{
    LABEL_REALIZABLE_CP_QUOTE, OpenPosition, OpenValuationRun, OpenValuationTotals,
    OpenValuationView, PositionValuation, SOLANA_OPEN_VALUATION_VERSION, SolanaWalletLedgerReport,
    VALUATION_COMMITMENT, ValuationOutcome, Venue, format_scaled_decimal, lamports_to_sol_string,
    money_exact_sol_string, money_to_lamports_trunc,
};
use serde::Serialize;

/// Fee bps the valuation used (latest observed event on the curve/pool).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FeeBpsDto {
    /// `null` on a bonding curve (no LP fee).
    pub lp: Option<u64>,
    pub protocol: u64,
    pub creator: u64,
    pub total: u64,
    /// Unix seconds of the event the bps came from.
    pub observed_at_unix: Option<i64>,
}

/// One open position and its (un)valuation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenPositionDto {
    pub mint: String,
    pub open_amount_raw: String,
    pub unknown_basis_amount_raw: String,
    /// Known remaining basis in lamports when EVERY open lot is a
    /// known-basis lamport lot; `null` = unknown (never zero).
    pub basis_known_lamports: Option<String>,
    /// `valued` or `unvalued`.
    pub status: &'static str,
    pub unvalued_reason: Option<&'static str>,
    /// `bonding_curve` or `pump_amm`.
    pub venue: Option<&'static str>,
    pub venue_address: Option<String>,
    /// `realizable_cp_quote`.
    pub label: Option<&'static str>,
    pub account_slot: Option<u64>,
    pub vault_slot: Option<u64>,
    /// Realizable value of the whole open amount, net of fees.
    pub realizable_lamports: Option<String>,
    pub realizable_sol: Option<String>,
    pub gross_lamports: Option<String>,
    pub fee_lamports: Option<String>,
    /// Spot (zero-size) value of the same amount before fees.
    pub marginal_lamports: Option<String>,
    pub price_impact_bps: Option<u64>,
    pub quote_reserve_lamports: Option<String>,
    pub token_reserve_raw: Option<String>,
    pub fee_bps: Option<FeeBpsDto>,
    /// `known` or `unknown_basis` (valued positions only).
    pub unrealized_pnl_status: Option<&'static str>,
    /// `realizable - known remaining basis`, whole lamports (truncated).
    pub unrealized_pnl_lamports: Option<String>,
    pub unrealized_pnl_sol_exact: Option<String>,
    /// USD value at `as_of` (ADR-018); `null` without a price or `--no-usd`.
    pub value_usd: Option<String>,
    pub usd_price_label: Option<String>,
    pub usd_unpriced_reason: Option<String>,
    /// `usd value - Σ usd basis of remaining lots` (8 dp); `null` = unknown.
    pub usd_unrealized_pnl: Option<String>,
    /// `known` or the reason it is unknown (valued positions only).
    pub usd_unrealized_status: Option<String>,
    /// ADR-019 EVM amendment fields (EVM chains only).
    #[serde(flatten)]
    pub evm: Option<super::evm_open_valuation::EvmOpenPositionDto>,
}

/// Totals over a set of open positions.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenValuationTotalsDto {
    pub positions: u64,
    pub valued: u64,
    pub unvalued: u64,
    pub realizable_lamports: String,
    pub realizable_sol: String,
    /// Σ unrealized PnL over the positions whose basis is fully known;
    /// `null` when none is known.
    pub unrealized_known_pnl_lamports: Option<String>,
    pub unrealized_known_positions: u64,
    pub unrealized_unknown_positions: u64,
    /// Σ USD value of the USD-priced positions (`null` when none).
    pub value_usd: Option<String>,
    pub usd_priced_positions: u64,
    /// Σ USD unrealized PnL over the positions where it is known (`null`
    /// when none); the counts say how many are known / unknown.
    pub usd_unrealized_known_pnl: Option<String>,
    pub usd_unrealized_known_positions: u64,
    pub usd_unrealized_unknown_positions: u64,
    pub unvalued_by_reason: BTreeMap<&'static str, u64>,
}

fn usd_str(scaled: i128) -> String {
    format_scaled_decimal(scaled, MONEY_SCALE)
}

#[must_use]
pub fn totals_dto(t: &OpenValuationTotals) -> OpenValuationTotalsDto {
    let realizable = i128::try_from(t.realizable_lamports).unwrap_or(i128::MAX);
    OpenValuationTotalsDto {
        positions: t.positions,
        valued: t.valued,
        unvalued: t.unvalued,
        realizable_lamports: t.realizable_lamports.to_string(),
        realizable_sol: lamports_to_sol_string(realizable),
        unrealized_known_pnl_lamports: (t.unrealized_known_positions > 0).then(|| {
            money_to_lamports_trunc(Money::from_scaled_units(t.unrealized_known_scaled)).to_string()
        }),
        unrealized_known_positions: t.unrealized_known_positions,
        unrealized_unknown_positions: t.unrealized_unknown_positions,
        value_usd: (t.usd_priced_positions > 0).then(|| usd_str(t.usd_value_scaled)),
        usd_priced_positions: t.usd_priced_positions,
        usd_unrealized_known_pnl: (t.usd_unrealized_known_positions > 0)
            .then(|| usd_str(t.usd_unrealized_known_scaled)),
        usd_unrealized_known_positions: t.usd_unrealized_known_positions,
        usd_unrealized_unknown_positions: t.usd_unrealized_unknown_positions,
        unvalued_by_reason: t.unvalued_by_reason.clone(),
    }
}

fn venue_label(v: Venue) -> &'static str {
    v.label()
}

fn position_dto(
    p: &OpenPosition,
    v: Option<&PositionValuation>,
    chain: &scout_sdk::engine::ChainDisplay,
) -> OpenPositionDto {
    let mut d = OpenPositionDto {
        mint: chain.address(&p.mint),
        open_amount_raw: p.open_amount_raw.to_string(),
        unknown_basis_amount_raw: p.unknown_basis_amount_raw.to_string(),
        basis_known_lamports: None,
        status: "unvalued",
        unvalued_reason: Some("not_run"),
        venue: None,
        venue_address: None,
        label: None,
        account_slot: None,
        vault_slot: None,
        realizable_lamports: None,
        realizable_sol: None,
        gross_lamports: None,
        fee_lamports: None,
        marginal_lamports: None,
        price_impact_bps: None,
        quote_reserve_lamports: None,
        token_reserve_raw: None,
        fee_bps: None,
        unrealized_pnl_status: None,
        unrealized_pnl_lamports: None,
        unrealized_pnl_sol_exact: None,
        value_usd: None,
        usd_price_label: None,
        usd_unpriced_reason: None,
        usd_unrealized_pnl: None,
        usd_unrealized_status: None,
        evm: None,
    };
    let Some(v) = v else { return d };
    d.basis_known_lamports = v
        .basis
        .fully_known_sol
        .then(|| money_to_lamports_trunc(v.basis.known_sol_basis).to_string());
    match &v.outcome {
        ValuationOutcome::Unvalued { reason } => {
            d.unvalued_reason = Some(reason.label());
        }
        ValuationOutcome::Valued(x) => {
            use scout_sdk::engine::FeeObservation;
            d.status = "valued";
            d.unvalued_reason = None;
            d.venue = Some(venue_label(x.venue));
            d.venue_address = Some(bs58::encode(x.address).into_string());
            d.label = Some(x.label);
            d.account_slot = Some(x.account_slot);
            d.vault_slot = x.vault_slot;
            d.realizable_lamports = Some(x.realizable_lamports.to_string());
            d.realizable_sol = Some(lamports_to_sol_string(i128::from(x.realizable_lamports)));
            d.gross_lamports = Some(x.gross_lamports.to_string());
            d.fee_lamports = Some(x.fee_lamports.to_string());
            d.marginal_lamports = Some(x.marginal_lamports.to_string());
            d.price_impact_bps = x.price_impact_bps;
            d.quote_reserve_lamports = Some(x.quote_reserve_lamports.to_string());
            d.token_reserve_raw = Some(x.token_reserve_raw.to_string());
            d.fee_bps = Some(match x.fee {
                FeeObservation::Curve {
                    protocol_bps,
                    creator_bps,
                } => FeeBpsDto {
                    lp: None,
                    protocol: protocol_bps,
                    creator: creator_bps,
                    total: x.fee.total_bps(),
                    observed_at_unix: x.fee_observed_at,
                },
                FeeObservation::Amm {
                    lp_bps,
                    protocol_bps,
                    creator_bps,
                } => FeeBpsDto {
                    lp: Some(lp_bps),
                    protocol: protocol_bps,
                    creator: creator_bps,
                    total: x.fee.total_bps(),
                    observed_at_unix: x.fee_observed_at,
                },
            });
            match x.unrealized_pnl {
                Some(m) => {
                    d.unrealized_pnl_status = Some("known");
                    d.unrealized_pnl_lamports = Some(money_to_lamports_trunc(m).to_string());
                    d.unrealized_pnl_sol_exact = Some(money_exact_sol_string(m));
                }
                None => d.unrealized_pnl_status = Some("unknown_basis"),
            }
            if let Some(u) = &x.usd {
                d.value_usd = Some(usd_str(u.value.scaled_units()));
                d.usd_price_label = Some(u.price_label.clone());
            }
            d.usd_unpriced_reason.clone_from(&x.usd_unpriced_reason);
            match x.usd_unrealized {
                Some(m) => {
                    d.usd_unrealized_pnl = Some(usd_str(m.scaled_units()));
                    d.usd_unrealized_status = Some("known".to_string());
                }
                None => {
                    d.usd_unrealized_status = Some(
                        x.usd_unrealized_reason
                            .clone()
                            .unwrap_or_else(|| "usd_not_priced".to_string()),
                    );
                }
            }
        }
    }
    d
}

/// Per-position records of a ledger, in the ledger's open-position order.
/// Without a valuation every position is `unvalued { not_run }`.
#[must_use]
pub fn open_positions_dto(l: &SolanaWalletLedgerReport) -> Vec<OpenPositionDto> {
    if let Some(ev) = &l.evm {
        return l
            .open_positions
            .iter()
            .map(|p| {
                let mut d = position_dto(p, None, &l.chain);
                let pos = ev.open_valuation.as_ref().and_then(|view| {
                    view.positions.iter().find(|x| {
                        let mut k = [0u8; 32];
                        k[12..].copy_from_slice(x.token.as_slice());
                        k == p.mint
                    })
                });
                d.evm = pos.map(|x| {
                    super::evm_open_valuation::apply_position(&mut d, x, l.chain.native_label)
                });
                d
            })
            .collect();
    }
    l.open_positions
        .iter()
        .map(|p| {
            let v = l
                .open_valuation
                .as_ref()
                .and_then(|view| view.positions.iter().find(|x| x.mint == p.mint));
            position_dto(p, v, &l.chain)
        })
        .collect()
}

/// Totals of a ledger's valuation; `None` without one.
#[must_use]
pub fn ledger_totals_dto(l: &SolanaWalletLedgerReport) -> Option<OpenValuationTotalsDto> {
    l.open_valuation
        .as_ref()
        .map(|v: &OpenValuationView| totals_dto(&v.totals()))
}

/// `run_meta.open_valuation`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenValuationMetaDto {
    /// `false` with `--no-valuation`.
    pub enabled: bool,
    pub version: &'static str,
    pub label: &'static str,
    pub commitment: &'static str,
    pub as_of_unix: i64,
    /// The window ends before `as_of`: nothing was read.
    pub historical_window: bool,
    /// `getMultipleAccounts` HTTP calls (chunks); counted in
    /// `scan.requests_made` (shared request budget).
    pub account_calls: u64,
    /// Highest / lowest context slot of the account reads.
    pub state_slot: Option<u64>,
    pub state_slot_min: Option<u64>,
    pub wallets_with_open_positions: u64,
    pub budget_exhausted: bool,
    pub fetch_error: Option<String>,
    pub totals: Option<OpenValuationTotalsDto>,
    /// ADR-019 EVM amendment run facts (EVM runs only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evm: Option<super::evm_open_valuation::EvmOpenValuationMetaDto>,
    /// How the figures are produced (invariant #10).
    pub policy: &'static str,
}

const POLICY_TEXT: &str = "realizable value = constant-product sell of the whole open amount on live curve/pool state at the recorded slot (not spot); fee bps = latest observed event on the same curve/pool in the run (none -> unvalued fee_unknown); unrealized PnL = realizable - known remaining lamport basis (unknown basis -> null); historical windows are not valued; ranking keys read realized figures only";

#[must_use]
pub fn open_valuation_meta(run: Option<&OpenValuationRun>, as_of: i64) -> OpenValuationMetaDto {
    let r = run.filter(|r| r.ran);
    OpenValuationMetaDto {
        enabled: r.is_some(),
        version: SOLANA_OPEN_VALUATION_VERSION,
        label: LABEL_REALIZABLE_CP_QUOTE,
        commitment: VALUATION_COMMITMENT,
        as_of_unix: as_of,
        historical_window: r.is_some_and(|r| r.historical),
        account_calls: r.map_or(0, |r| r.account_calls),
        state_slot: r.and_then(|r| r.state_slot),
        state_slot_min: r.and_then(|r| r.state_slot_min),
        wallets_with_open_positions: r.map_or(0, |r| r.wallets_with_open),
        budget_exhausted: r.is_some_and(|r| r.budget_exhausted),
        fetch_error: r.and_then(|r| r.fetch_error.clone()),
        totals: r.map(|r| totals_dto(&r.totals)),
        evm: None,
        policy: POLICY_TEXT,
    }
}

/// One stderr line summarising the valuation step.
#[must_use]
pub fn open_valuation_line(bin: &str, run: Option<&OpenValuationRun>) -> String {
    let Some(r) = run.filter(|r| r.ran) else {
        return format!(
            "{bin}: open valuation: disabled (--no-valuation); open positions unvalued"
        );
    };
    if r.historical {
        return format!(
            "{bin}: open valuation: historical window (until < as_of): {} open position(s) unvalued (historical_window), nothing read",
            r.totals.positions
        );
    }
    format!(
        "{bin}: open valuation: positions={} valued={} unvalued={} account_calls={} state_slot={}{}{}",
        r.totals.positions,
        r.totals.valued,
        r.totals.unvalued,
        r.account_calls,
        r.state_slot
            .map_or_else(|| "n/a".to_string(), |s| s.to_string()),
        if r.budget_exhausted {
            " request_budget_exhausted=true"
        } else {
            ""
        },
        r.totals
            .unvalued_by_reason
            .iter()
            .map(|(k, v)| format!(" {k}={v}"))
            .collect::<String>()
    )
}

const EVM_POLICY_TEXT: &str = "realizable value = exit quote of the whole open amount from the venue itself at the pinned state block (official QuoterV2 / V4Quoter eth_call, or exact v2 constant-product on getReserves / Aerodrome getAmountOut) into the pool's other asset, on the pool the wallet last traded the token on (not spot; exit gas not deducted); a revert is quote_reverted and an unpinned quoter is venue_quoter_unpinned, never zero; tokens with tax-shaped evidence carry transfer_tax_not_modelled; unrealized PnL = realizable - known remaining basis in the same unit (unknown or other-unit basis -> null); historical windows are not valued; ranking keys read realized figures only";

/// `run_meta.open_valuation` of an EVM run (`state_slot` = the pinned block,
/// `account_calls` = `eth_call` + `eth_getLogs` requests).
#[must_use]
pub fn open_valuation_meta_evm(
    run: Option<&scout_sdk::engine::EvmOpenValuationRun>,
    as_of: i64,
    native_label: &str,
) -> OpenValuationMetaDto {
    let r = run.filter(|r| r.ran);
    OpenValuationMetaDto {
        enabled: r.is_some(),
        version: scout_sdk::engine::EVM_OPEN_VALUATION_VERSION,
        label: scout_sdk::engine::LABEL_REALIZABLE_ONCHAIN_QUOTE,
        commitment: "pinned_block",
        as_of_unix: as_of,
        historical_window: r.is_some_and(|r| r.historical),
        account_calls: r.map_or(0, |r| r.eth_calls.saturating_add(r.log_calls)),
        state_slot: r.and_then(|r| r.state_block),
        state_slot_min: r.and_then(|r| r.state_block),
        wallets_with_open_positions: r.map_or(0, |r| r.wallets_with_open),
        budget_exhausted: r.is_some_and(|r| r.budget_exhausted),
        fetch_error: r.and_then(|r| r.fetch_error.clone()),
        totals: None,
        evm: r.map(|r| super::evm_open_valuation::evm_meta_dto(r, native_label)),
        policy: EVM_POLICY_TEXT,
    }
}

/// One stderr line summarising the EVM valuation step.
#[must_use]
pub fn open_valuation_line_evm(
    bin: &str,
    run: Option<&scout_sdk::engine::EvmOpenValuationRun>,
) -> String {
    let Some(r) = run.filter(|r| r.ran) else {
        return format!(
            "{bin}: open valuation: disabled (--no-valuation); open positions unvalued"
        );
    };
    if r.historical {
        return format!(
            "{bin}: open valuation: historical window (until < as_of): {} open position(s) unvalued (historical_window), nothing read",
            r.totals.positions
        );
    }
    format!(
        "{bin}: open valuation: positions={} valued={} unvalued={} eth_calls={} log_calls={} cache_hits={} state_block={}{}{}",
        r.totals.positions,
        r.totals.valued,
        r.totals.unvalued,
        r.eth_calls,
        r.log_calls,
        r.cache_hits,
        r.state_block
            .map_or_else(|| "n/a".to_string(), |s| s.to_string()),
        if r.budget_exhausted {
            " request_budget_exhausted=true"
        } else {
            ""
        },
        r.totals
            .unvalued_by_reason
            .iter()
            .map(|(k, v)| format!(" {k}={v}"))
            .collect::<String>()
    )
}
