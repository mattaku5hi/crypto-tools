//! ADR-019 EVM amendment DTOs: exit-quote valuation of open EVM positions
//! (`open_positions[]` extras, per-wallet totals by quote unit, `run_meta`).
//! Raw amounts are decimal strings; unknown is `null` with a status.

use std::collections::BTreeMap;

use scout_core::MONEY_SCALE;
use scout_sdk::engine::{
    EVM_OPEN_VALUATION_VERSION, EXIT_STATUS_EXACT, EXIT_STATUS_ILLIQUID, EvmOpenValuationRun,
    EvmOpenValuationTotals, EvmPositionValuation, EvmValuationOutcome,
    LABEL_REALIZABLE_ONCHAIN_QUOTE, LABEL_TRANSFER_TAX_NOT_MODELLED, QuoteUnit,
    SolanaWalletLedgerReport, format_scaled_decimal, money_to_unit_raw, quote_unit_label,
};
use serde::Serialize;

/// EVM-only fields of one open position record (flattened next to the
/// shared ones).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EvmOpenPositionDto {
    /// `uniswap_v3`, `uniswap_v4`, ... (valued positions).
    pub quote_unit: Option<String>,
    /// `native` or the quote token address.
    pub quote_asset: Option<String>,
    /// Exit quote in wei when the quote asset is the native currency.
    pub realizable_native_raw: Option<String>,
    /// Exit quote in raw units of the quote token otherwise.
    pub realizable_quote_raw: Option<String>,
    /// `quoter_v2`, `v4_quoter`, `v2_reserves_constant_product`, `aerodrome_get_amount_out`.
    pub quote_method: Option<&'static str>,
    pub quoter: Option<String>,
    /// `pinned`, `override` or `not_applicable`.
    pub quoter_source: Option<&'static str>,
    /// The block every call of the quote was pinned to.
    pub state_block: Option<u64>,
    pub pool_id: Option<String>,
    /// Marginal probe (a thousandth of the position) and its output.
    pub probe_amount_raw: Option<String>,
    pub probe_out_raw: Option<String>,
    /// `transfer_tax_not_modelled` when tax-shaped evidence exists.
    pub caveat: Option<&'static str>,
    /// Unrealized PnL in raw units of the quote unit (truncated).
    pub unrealized_pnl_raw: Option<String>,
    /// `known`, `unknown_basis` or `basis_other_unit`.
    pub unrealized_status: Option<&'static str>,
    /// Illiquid / partially fillable exits: the exit quote is a LOWER BOUND in
    /// raw units of `quote_unit` (never in `realizable_*_raw`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realizable_lower_bound_raw: Option<String>,
    /// Open amount the search found unfillable / fillable (token raw units).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unfillable_amount_raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fillable_amount_raw: Option<String>,
    /// `other_pools_not_searched` for a non-exact exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_caveat: Option<&'static str>,
    /// `no_liquidity_in_last_pool` for an illiquid position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illiquid_reason: Option<&'static str>,
    /// `0x` + 8 hex of the quoter revert behind an unvalued / non-exact exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revert_selector: Option<String>,
    /// `lower_bound` for a non-exact exit (`unrealized_pnl_raw` is then a
    /// lower bound).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl_status: Option<&'static str>,
}

/// Per-wallet / per-run totals; amounts per quote unit, never summed across units.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EvmOpenValuationTotalsDto {
    pub positions: u64,
    pub valued: u64,
    pub unvalued: u64,
    /// Positions whose last pool has no liquidity (lower bound 0); `valued`
    /// counts exact quotes only.
    pub illiquid: u64,
    /// Positions only partly fillable in the last pool (lower bound).
    pub partially_fillable: u64,
    /// Unit label -> Σ lower-bound realizable raw of illiquid/partial exits.
    pub realizable_lower_bound_raw_by_unit: BTreeMap<String, String>,
    /// Unit label -> Σ lower-bound unrealized raw (truncated) of those exits.
    pub unrealized_lower_bound_raw_by_unit: BTreeMap<String, String>,
    /// Unit label (`eth`, `usdg`, `usdc`, `usdt_peg`, ...) -> Σ realizable raw (exact only).
    pub realizable_raw_by_unit: BTreeMap<String, String>,
    /// Unit label -> Σ known unrealized raw (truncated).
    pub unrealized_known_raw_by_unit: BTreeMap<String, String>,
    pub unrealized_known_positions: u64,
    pub unrealized_unknown_positions: u64,
    pub value_usd: Option<String>,
    pub usd_priced_positions: u64,
    pub usd_unrealized_known_pnl: Option<String>,
    pub usd_unrealized_known_positions: u64,
    pub usd_unrealized_unknown_positions: u64,
    pub transfer_tax_positions: u64,
    pub unvalued_by_reason: BTreeMap<&'static str, u64>,
}

/// The unit label of the report: native `Wei` carries the chain's own label.
fn unit_name(label: &str, native: &str) -> String {
    if label == "eth" {
        native.to_string()
    } else {
        label.to_string()
    }
}

#[must_use]
pub fn evm_totals_dto(t: &EvmOpenValuationTotals, native: &str) -> EvmOpenValuationTotalsDto {
    let usd = |s: i128| format_scaled_decimal(s, MONEY_SCALE);
    EvmOpenValuationTotalsDto {
        positions: t.positions,
        valued: t.valued,
        unvalued: t.unvalued,
        illiquid: t.illiquid,
        partially_fillable: t.partially_fillable,
        realizable_lower_bound_raw_by_unit: t
            .realizable_lower_bound_raw_by_unit
            .iter()
            .map(|(k, v)| (unit_name(k, native), v.to_string()))
            .collect(),
        unrealized_lower_bound_raw_by_unit: t
            .unrealized_lower_bound_scaled_by_unit
            .iter()
            .map(|(k, v)| {
                (
                    unit_name(k, native),
                    scout_sdk::engine::money_to_quote_units_trunc(
                        scout_core::Money::from_scaled_units(*v),
                    )
                    .to_string(),
                )
            })
            .collect(),
        realizable_raw_by_unit: t
            .realizable_raw_by_unit
            .iter()
            .map(|(k, v)| (unit_name(k, native), v.to_string()))
            .collect(),
        unrealized_known_raw_by_unit: t
            .unrealized_known_scaled_by_unit
            .iter()
            .map(|(k, v)| {
                (
                    unit_name(k, native),
                    scout_sdk::engine::money_to_quote_units_trunc(
                        scout_core::Money::from_scaled_units(*v),
                    )
                    .to_string(),
                )
            })
            .collect(),
        unrealized_known_positions: t.unrealized_known_positions,
        unrealized_unknown_positions: t.unrealized_unknown_positions,
        value_usd: (t.usd_priced_positions > 0).then(|| usd(t.usd_value_scaled)),
        usd_priced_positions: t.usd_priced_positions,
        usd_unrealized_known_pnl: (t.usd_unrealized_known_positions > 0)
            .then(|| usd(t.usd_unrealized_known_scaled)),
        usd_unrealized_known_positions: t.usd_unrealized_known_positions,
        usd_unrealized_unknown_positions: t.usd_unrealized_unknown_positions,
        transfer_tax_positions: t.transfer_tax_positions,
        unvalued_by_reason: t.unvalued_by_reason.clone(),
    }
}

/// `run_meta.open_valuation.evm`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EvmOpenValuationMetaDto {
    pub version: &'static str,
    pub label: &'static str,
    /// The block all quote calls were pinned to.
    pub state_block: Option<u64>,
    /// `eth_call`s made (cache hits excluded); counted in `scan.requests_made`.
    pub eth_calls: u64,
    /// `eth_getLogs` requests of the v4 `Initialize` fallback (hard-capped per run).
    pub log_calls: u64,
    pub cache_hits: u64,
    /// Cost plan: positions admitted, their planned calls, the budget left
    /// when planning (`null` = unlimited) and positions refused up front.
    pub planned_positions: u64,
    pub planned_calls: u64,
    pub budget_left: Option<u64>,
    pub refused_positions: u64,
    pub caveat_label: &'static str,
    pub totals: EvmOpenValuationTotalsDto,
}

#[must_use]
pub fn evm_meta_dto(run: &EvmOpenValuationRun, native: &str) -> EvmOpenValuationMetaDto {
    EvmOpenValuationMetaDto {
        version: EVM_OPEN_VALUATION_VERSION,
        label: LABEL_REALIZABLE_ONCHAIN_QUOTE,
        state_block: run.state_block,
        eth_calls: run.eth_calls,
        log_calls: run.log_calls,
        cache_hits: run.cache_hits,
        planned_positions: run.planned_positions,
        planned_calls: run.planned_calls,
        budget_left: run.budget_left,
        refused_positions: run.refused_positions,
        caveat_label: LABEL_TRANSFER_TAX_NOT_MODELLED,
        totals: evm_totals_dto(&run.totals, native),
    }
}

fn unit_is_native(u: QuoteUnit) -> bool {
    u == QuoteUnit::Wei
}

/// EVM fields and shared-field overrides of one position.
pub(super) fn apply_position(
    d: &mut super::open_valuation::OpenPositionDto,
    p: &EvmPositionValuation,
    native: &str,
) -> EvmOpenPositionDto {
    let mut e = EvmOpenPositionDto {
        quote_unit: None,
        quote_asset: None,
        realizable_native_raw: None,
        realizable_quote_raw: None,
        quote_method: None,
        quoter: None,
        quoter_source: None,
        state_block: None,
        pool_id: None,
        probe_amount_raw: None,
        probe_out_raw: None,
        caveat: None,
        unrealized_pnl_raw: None,
        unrealized_status: None,
        realizable_lower_bound_raw: None,
        unfillable_amount_raw: None,
        fillable_amount_raw: None,
        fill_caveat: None,
        illiquid_reason: None,
        revert_selector: None,
        unrealized_pnl_status: None,
    };
    let selector = |s: [u8; 4]| {
        format!(
            "0x{}",
            s.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    };
    e.revert_selector = p.revert_selector.map(selector);
    match &p.outcome {
        EvmValuationOutcome::Unvalued { reason } => {
            d.unvalued_reason = Some(reason.label());
            d.venue = p.venue.map(|v| v.label());
            d.venue_address = p.pool.map(|a| format!("{a:#x}"));
            e.pool_id = p.pool_id.map(|i| format!("{i:#x}"));
        }
        EvmValuationOutcome::Valued(v) if v.exit_status != EXIT_STATUS_EXACT => {
            // A lower bound, never an exact exit value.
            d.status = if v.exit_status == EXIT_STATUS_ILLIQUID {
                "illiquid"
            } else {
                "partially_fillable"
            };
            d.unvalued_reason = None;
            d.venue = Some(v.venue.label());
            d.venue_address = Some(format!("{:#x}", v.pool));
            d.label = Some(v.label);
            d.account_slot = Some(v.state_block);
            e.quote_unit = Some(unit_name(quote_unit_label(v.quote_unit), native));
            e.quote_asset = Some(
                v.quote_token
                    .map_or_else(|| "native".to_string(), |a| format!("{a:#x}")),
            );
            e.realizable_lower_bound_raw = Some(v.realizable_raw.to_string());
            e.quote_method = Some(v.method);
            e.quoter = v.quoter.map(|a| format!("{a:#x}"));
            e.quoter_source = Some(v.quoter_source);
            e.state_block = Some(v.state_block);
            e.pool_id = v.pool_id.map(|i| format!("{i:#x}"));
            e.unfillable_amount_raw = v.unfillable_amount_raw.map(|x| x.to_string());
            e.fillable_amount_raw = v.fillable_amount_raw.map(|x| x.to_string());
            e.fill_caveat = v.fill_caveat;
            e.illiquid_reason = v.illiquid_reason;
            e.revert_selector = p.revert_selector.or(v.revert_selector).map(selector);
            e.caveat = v
                .transfer_tax_not_modelled
                .then_some(LABEL_TRANSFER_TAX_NOT_MODELLED);
            e.unrealized_pnl_raw = v
                .unrealized_lower_bound
                .map(|m| money_to_unit_raw(v.quote_unit, m).to_string());
            e.unrealized_pnl_status = Some("lower_bound");
            e.unrealized_status = Some(v.unrealized_status);
            d.unrealized_pnl_status = Some("lower_bound");
            d.value_usd = None;
            d.usd_unpriced_reason = Some("lower_bound_not_priced".to_string());
            d.usd_unrealized_status = Some("lower_bound_not_priced".to_string());
        }
        EvmValuationOutcome::Valued(v) => {
            d.status = "valued";
            d.unvalued_reason = None;
            d.venue = Some(v.venue.label());
            d.venue_address = Some(format!("{:#x}", v.pool));
            d.label = Some(v.label);
            d.price_impact_bps = v.price_impact_bps;
            d.account_slot = Some(v.state_block);
            e.quote_unit = Some(unit_name(quote_unit_label(v.quote_unit), native));
            e.quote_asset = Some(
                v.quote_token
                    .map_or_else(|| "native".to_string(), |a| format!("{a:#x}")),
            );
            let raw = v.realizable_raw.to_string();
            if unit_is_native(v.quote_unit) {
                e.realizable_native_raw = Some(raw);
            } else {
                e.realizable_quote_raw = Some(raw);
            }
            e.quote_method = Some(v.method);
            e.quoter = v.quoter.map(|a| format!("{a:#x}"));
            e.quoter_source = Some(v.quoter_source);
            e.state_block = Some(v.state_block);
            e.pool_id = v.pool_id.map(|i| format!("{i:#x}"));
            e.probe_amount_raw = v.probe_amount_raw.map(|x| x.to_string());
            e.probe_out_raw = v.probe_out_raw.map(|x| x.to_string());
            e.caveat = v
                .transfer_tax_not_modelled
                .then_some(LABEL_TRANSFER_TAX_NOT_MODELLED);
            e.unrealized_status = Some(v.unrealized_status);
            d.unrealized_pnl_status = Some(if v.unrealized_pnl.is_some() {
                "known"
            } else {
                "unknown_basis"
            });
            e.unrealized_pnl_raw = v
                .unrealized_pnl
                .map(|m| money_to_unit_raw(v.quote_unit, m).to_string());
            if let Some(u) = &v.usd {
                d.value_usd = Some(format_scaled_decimal(u.value.scaled_units(), MONEY_SCALE));
                d.usd_price_label = Some(u.price_label.clone());
            }
            d.usd_unpriced_reason.clone_from(&v.usd_unpriced_reason);
            match v.usd_unrealized {
                Some(m) => {
                    d.usd_unrealized_pnl =
                        Some(format_scaled_decimal(m.scaled_units(), MONEY_SCALE));
                    d.usd_unrealized_status = Some("known".to_string());
                }
                None => {
                    d.usd_unrealized_status = Some(
                        v.usd_unrealized_reason
                            .clone()
                            .unwrap_or_else(|| "usd_not_priced".to_string()),
                    );
                }
            }
        }
    }
    e
}

/// Totals of an EVM ledger's valuation; `None` without one.
#[must_use]
pub fn ledger_evm_totals_dto(l: &SolanaWalletLedgerReport) -> Option<EvmOpenValuationTotalsDto> {
    l.evm
        .as_ref()
        .and_then(|e| e.open_valuation.as_ref())
        .map(|v| evm_totals_dto(&v.totals(), l.chain.native_label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattened_evm_fields_serialize_and_none_adds_nothing() {
        let mut d = super::super::open_valuation::OpenPositionDto {
            mint: "0x1".into(),
            open_amount_raw: "5".into(),
            unknown_basis_amount_raw: "0".into(),
            basis_known_lamports: None,
            status: "valued",
            unvalued_reason: None,
            venue: Some("uniswap_v3"),
            venue_address: None,
            label: Some("realizable_onchain_quote"),
            account_slot: None,
            vault_slot: None,
            realizable_lamports: None,
            realizable_sol: None,
            gross_lamports: None,
            fee_lamports: None,
            marginal_lamports: None,
            price_impact_bps: Some(12),
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
        let none = serde_json::to_value(&d).unwrap();
        assert!(none.get("realizable_native_raw").is_none());
        d.evm = Some(EvmOpenPositionDto {
            quote_unit: Some("eth".into()),
            quote_asset: Some("native".into()),
            realizable_native_raw: Some("9".into()),
            realizable_quote_raw: None,
            quote_method: Some("quoter_v2"),
            quoter: None,
            quoter_source: Some("pinned"),
            state_block: Some(4096),
            pool_id: None,
            probe_amount_raw: None,
            probe_out_raw: None,
            caveat: Some("transfer_tax_not_modelled"),
            unrealized_pnl_raw: None,
            unrealized_status: Some("unknown_basis"),
            realizable_lower_bound_raw: None,
            unfillable_amount_raw: None,
            fillable_amount_raw: None,
            fill_caveat: None,
            illiquid_reason: None,
            revert_selector: None,
            unrealized_pnl_status: None,
        });
        let some = serde_json::to_value(&d).unwrap();
        assert_eq!(some["realizable_native_raw"], "9");
        assert_eq!(some["state_block"], 4096);
        assert_eq!(some["caveat"], "transfer_tax_not_modelled");
        assert_eq!(some["mint"], "0x1");
    }

    fn base_dto() -> super::super::open_valuation::OpenPositionDto {
        super::super::open_valuation::OpenPositionDto {
            mint: "0x1".into(),
            open_amount_raw: "100".into(),
            unknown_basis_amount_raw: "0".into(),
            basis_known_lamports: None,
            status: "unvalued",
            unvalued_reason: None,
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
        }
    }

    fn valued(status: &'static str) -> scout_sdk::engine::EvmValuedPosition {
        scout_sdk::engine::EvmValuedPosition {
            venue: scout_dex_evm::SwapVenue::UniswapV3,
            pool: alloy_primitives::Address::repeat_byte(0xab),
            pool_id: None,
            label: LABEL_REALIZABLE_ONCHAIN_QUOTE,
            method: "quoter_v2",
            quoter: None,
            quoter_source: "pinned",
            state_block: 77,
            quote_unit: QuoteUnit::Wei,
            quote_token: None,
            realizable_raw: 0,
            probe_amount_raw: None,
            probe_out_raw: None,
            price_impact_bps: None,
            transfer_tax_not_modelled: false,
            unrealized_pnl: None,
            unrealized_status: "unknown_basis",
            usd: None,
            usd_unpriced_reason: None,
            usd_unrealized: None,
            usd_unrealized_reason: None,
            exit_status: status,
            illiquid_reason: None,
            fill_caveat: None,
            fillable_amount_raw: None,
            unfillable_amount_raw: None,
            fill_search_calls: 0,
            unrealized_lower_bound: None,
            revert_selector: None,
        }
    }

    fn position(outcome: EvmValuationOutcome) -> scout_sdk::engine::EvmPositionValuation {
        scout_sdk::engine::EvmPositionValuation {
            token: alloy_primitives::Address::repeat_byte(1),
            open_amount_raw: 100,
            outcome,
            venue: None,
            pool: None,
            pool_id: None,
            revert_selector: None,
        }
    }

    #[test]
    fn illiquid_position_is_a_lower_bound_never_valued() {
        let mut v = valued("illiquid");
        v.illiquid_reason = Some("no_liquidity_in_last_pool");
        v.fill_caveat = Some("other_pools_not_searched");
        v.unfillable_amount_raw = Some(100);
        v.revert_selector = Some([0xde, 0xad, 0xbe, 0x0f]);
        v.unrealized_lower_bound = Some(scout_core::Money::from_scaled_units(-5));
        let mut d = base_dto();
        let e = apply_position(
            &mut d,
            &position(EvmValuationOutcome::Valued(Box::new(v))),
            "eth",
        );
        d.evm = Some(e);
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["status"], "illiquid");
        assert_eq!(j["realizable_lower_bound_raw"], "0");
        assert!(j.get("realizable_native_raw").is_none_or(|x| x.is_null()));
        assert!(j.get("realizable_quote_raw").is_none_or(|x| x.is_null()));
        assert_eq!(j["unfillable_amount_raw"], "100");
        assert_eq!(j["illiquid_reason"], "no_liquidity_in_last_pool");
        assert_eq!(j["fill_caveat"], "other_pools_not_searched");
        assert_eq!(j["revert_selector"], "0xdeadbe0f");
        assert_eq!(j["unrealized_pnl_status"], "lower_bound");
        assert!(j["value_usd"].is_null());
        assert_eq!(j["usd_unpriced_reason"], "lower_bound_not_priced");
    }

    #[test]
    fn partially_fillable_position_reports_both_amounts() {
        let mut v = valued("partially_fillable");
        v.realizable_raw = 42;
        v.fillable_amount_raw = Some(60);
        v.unfillable_amount_raw = Some(40);
        v.fill_caveat = Some("other_pools_not_searched");
        let mut d = base_dto();
        let e = apply_position(
            &mut d,
            &position(EvmValuationOutcome::Valued(Box::new(v))),
            "eth",
        );
        d.evm = Some(e);
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["status"], "partially_fillable");
        assert_eq!(j["realizable_lower_bound_raw"], "42");
        assert_eq!(j["fillable_amount_raw"], "60");
        assert_eq!(j["unfillable_amount_raw"], "40");
        assert!(j.get("realizable_native_raw").is_none_or(|x| x.is_null()));
    }

    #[test]
    fn exact_position_keeps_valued_and_native_raw() {
        let mut v = valued("exact");
        v.realizable_raw = 9;
        let mut d = base_dto();
        let e = apply_position(
            &mut d,
            &position(EvmValuationOutcome::Valued(Box::new(v))),
            "eth",
        );
        d.evm = Some(e);
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["status"], "valued");
        assert_eq!(j["realizable_native_raw"], "9");
        assert!(j.get("realizable_lower_bound_raw").is_none());
    }

    #[test]
    fn unvalued_position_keeps_pool_identity_and_selector() {
        let mut p = position(EvmValuationOutcome::Unvalued {
            reason: scout_sdk::engine::EvmUnvaluedReason::PoolNotInitialized,
        });
        p.venue = Some(scout_dex_evm::SwapVenue::UniswapV4);
        p.pool = Some(alloy_primitives::Address::repeat_byte(0xcd));
        p.pool_id = Some(alloy_primitives::B256::repeat_byte(0x11));
        p.revert_selector = Some([1, 2, 3, 4]);
        let mut d = base_dto();
        let e = apply_position(&mut d, &p, "eth");
        d.evm = Some(e);
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["status"], "unvalued");
        assert_eq!(j["unvalued_reason"], "pool_not_initialized");
        assert_eq!(j["venue"], "uniswap_v4");
        assert!(j["venue_address"].as_str().unwrap().starts_with("0xcdcd"));
        assert!(j["pool_id"].as_str().unwrap().starts_with("0x1111"));
        assert_eq!(j["revert_selector"], "0x01020304");
    }

    #[test]
    fn totals_dto_counts_inexact_exits_apart() {
        let mut t = EvmOpenValuationTotals {
            positions: 5,
            valued: 2,
            illiquid: 2,
            partially_fillable: 1,
            ..EvmOpenValuationTotals::default()
        };
        t.realizable_lower_bound_raw_by_unit.insert("eth", 7);
        t.unrealized_lower_bound_scaled_by_unit
            .insert("eth", -3_000_000);
        let d = evm_totals_dto(&t, "bnb");
        assert_eq!((d.valued, d.illiquid, d.partially_fillable), (2, 2, 1));
        assert_eq!(d.realizable_lower_bound_raw_by_unit["bnb"], "7");
        assert!(d.unrealized_lower_bound_raw_by_unit.contains_key("bnb"));
        assert!(d.realizable_raw_by_unit.is_empty());
    }
}
