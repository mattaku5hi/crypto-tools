//! ADR-019 EVM amendment DTOs: exit-quote valuation of open EVM positions
//! (`open_positions[]` extras, per-wallet totals by quote unit, `run_meta`).
//! Raw amounts are decimal strings; unknown is `null` with a status.

use std::collections::BTreeMap;

use scout_core::MONEY_SCALE;
use scout_sdk::engine::{
    EVM_OPEN_VALUATION_VERSION, EvmOpenValuationRun, EvmOpenValuationTotals, EvmPositionValuation,
    EvmValuationOutcome, LABEL_REALIZABLE_ONCHAIN_QUOTE, LABEL_TRANSFER_TAX_NOT_MODELLED,
    QuoteUnit, SolanaWalletLedgerReport, format_scaled_decimal, money_to_unit_raw,
    quote_unit_label,
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
}

/// Per-wallet / per-run totals; amounts per quote unit, never summed across units.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EvmOpenValuationTotalsDto {
    pub positions: u64,
    pub valued: u64,
    pub unvalued: u64,
    /// Unit label (`eth`, `usdg`, `usdc`, `usdt_peg`, ...) -> Σ realizable raw.
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
    };
    match &p.outcome {
        EvmValuationOutcome::Unvalued { reason } => {
            d.unvalued_reason = Some(reason.label());
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
        });
        let some = serde_json::to_value(&d).unwrap();
        assert_eq!(some["realizable_native_raw"], "9");
        assert_eq!(some["state_block"], 4096);
        assert_eq!(some["caveat"], "transfer_tax_not_modelled");
        assert_eq!(some["mint"], "0x1");
    }
}
