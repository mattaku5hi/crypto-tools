//! Output layer for `wallet-rank` (docs/CLI.md §4, §7).
//!
//! DTOs are built field-by-field from engine reports. Money convention:
//! lamports and SOL are decimal STRINGS (exact, integer arithmetic only);
//! counts are JSON numbers; unknown is `null` with an explicit status,
//! never `0`. Percentages are presentation of exact rationals.

use std::collections::BTreeMap;

use scout_analytics::RatioStatus;
use scout_app::SCHEMA_VERSION;
use scout_core::{MONEY_SCALE, Money};
use scout_engine::{
    AnalysisWindow, ExcludedWallet, OpenExposure, QuoteUnit, QuoteUnitBlock, RankedWallet, Ratio,
    SOLANA_WALLET_LEDGER_SCOPE, SOLANA_WALLET_LEDGER_VERSION, SOLANA_WALLET_RANK_VERSION, ScanStop,
    SolanaProtocolScope, WalletRankObservation, WalletRankReport, format_quote_money,
    format_scaled_decimal, lamports_to_sol_string, money_exact_sol_string, quote_unit_decimals,
    quote_unit_label, quote_units_to_money, rational_to_decimal_string,
};
use serde::Serialize;

const CHAIN: &str = "solana";

fn addr(o: &WalletRankObservation) -> String {
    bs58::encode(o.wallet).into_string()
}

fn money_str(m: Money) -> String {
    format_scaled_decimal(m.scaled_units(), MONEY_SCALE)
}

// ---------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------

fn header(quote: QuoteUnit) -> [String; 10] {
    [
        "rank".to_string(),
        "wallet".to_string(),
        "chain".to_string(),
        format!("realized_net_pnl_{}", quote_unit_label(quote)),
        "realized_cost_roi".to_string(),
        "closed".to_string(),
        "win_rate".to_string(),
        "profit_factor".to_string(),
        "open_exposure".to_string(),
        "quality".to_string(),
    ]
}

/// Exact decimal of `raw` base units of `unit` (SOL 9 dp, USDC/USDT 6 dp).
fn raw_decimal(unit: QuoteUnit, raw: i128) -> Option<String> {
    let money = quote_units_to_money(unit, raw).ok()?;
    format_quote_money(unit, money)
}

/// The block of the ranking unit.
fn unit_block_of(o: &WalletRankObservation) -> Option<&QuoteUnitBlock> {
    o.ledger.as_ref().and_then(|l| l.unit_block(o.quote))
}

fn rfc3339(unix: i64) -> String {
    scout_app::format_unix_utc(u64::try_from(unix).unwrap_or(0))
}

fn roi_cell(o: &WalletRankObservation) -> String {
    o.roi
        .and_then(|r| r.percent_string(2))
        .map_or_else(|| "N/A".to_string(), |p| format!("{p}%"))
}

fn win_rate_cell(o: &WalletRankObservation) -> String {
    match unit_block_of(o).map(|b| &b.win_rate) {
        Some(RatioStatus::Value { value }) => {
            // value is a fraction at MONEY_SCALE (8 digits); truncated for display.
            format!(
                "{}%",
                format_scaled_decimal(value.scaled_units().div_euclid(10_000), 2)
            )
        }
        _ => "N/A".to_string(),
    }
}

fn pf_cell(o: &WalletRankObservation) -> String {
    let pf = if o.quote == QuoteUnit::Lamports {
        o.ledger.as_ref().map(|l| &l.profit_factor)
    } else {
        unit_block_of(o).map(|b| &b.profit_factor)
    };
    match pf {
        Some(RatioStatus::Value { value }) => money_str(*value),
        Some(RatioStatus::NoObservedLosses) => "no_observed_losses".to_string(),
        _ => "N/A".to_string(),
    }
}

fn exposure_cell(o: &WalletRankObservation) -> String {
    match &o.open_exposure {
        OpenExposure::None => "none".to_string(),
        OpenExposure::Unvalued { positions, .. } => format!("unvalued({positions})"),
    }
}

fn row(r: &RankedWallet, profile: &str) -> Vec<String> {
    let o = &r.observation;
    let pnl = o
        .net_pnl_raw
        .and_then(|v| raw_decimal(o.quote, v))
        .unwrap_or_else(|| "N/A".to_string());
    let closed = unit_block_of(o).map_or(0, |b| b.closed_episodes_known);
    vec![
        r.rank.to_string(),
        addr(o),
        CHAIN.to_string(),
        pnl,
        roi_cell(o),
        closed.to_string(),
        win_rate_cell(o),
        pf_cell(o),
        exposure_cell(o),
        format!("{profile}/{}", o.pnl_status.label()),
    ]
}

fn counts_text(counts: &BTreeMap<&'static str, usize>) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }
    counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn primary_counts(report: &WalletRankReport) -> BTreeMap<&'static str, usize> {
    report
        .primary_reason_counts()
        .into_iter()
        .map(|(k, v)| (k.label(), v))
        .collect()
}

fn all_counts(report: &WalletRankReport) -> BTreeMap<&'static str, usize> {
    report
        .all_reason_counts()
        .into_iter()
        .map(|(k, v)| (k.label(), v))
        .collect()
}

/// Header, one row per ranked wallet, then `#` summary lines (stdout is
/// still one format: the table plus its exclusion summary).
pub fn table_lines(
    report: &WalletRankReport,
    partial: bool,
    window: &AnalysisWindow,
) -> Vec<String> {
    let profile = report.policy.profile.label();
    let rows: Vec<Vec<String>> = report.ranked.iter().map(|r| row(r, profile)).collect();
    let header = header(report.policy.quote);
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for cells in &rows {
        for (i, c) in cells.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(c.chars().count());
            }
        }
    }
    let fmt = |cells: &[String]| -> String {
        let mut line = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            let w = widths.get(i).copied().unwrap_or(0);
            line.push_str(&format!("{c:<w$}"));
        }
        line.trim_end().to_string()
    };
    let head: Vec<String> = header.to_vec();
    let mut out = Vec::new();
    if let Some((since, until)) = window.bounds() {
        out.push(format!(
            "# window [{}, {}) source={} as_of={}",
            rfc3339(since),
            rfc3339(until),
            window.source.label(),
            rfc3339(window.as_of)
        ));
    }
    out.push(fmt(&head));
    out.extend(rows.iter().map(|c| fmt(c)));
    let p = &report.policy;
    out.push(format!(
        "# rank_by={} quote={} profile={} top={} input={} eligible={} ranked={} excluded={} status={}",
        p.rank_by.label(),
        quote_unit_label(p.quote),
        p.profile.label(),
        p.top,
        report.input_count,
        report.eligible_count,
        report.ranked.len(),
        report.excluded.len(),
        if partial { "partial" } else { "complete" },
    ));
    out.push(format!(
        "# exclusions by primary reason: {}",
        counts_text(&primary_counts(report))
    ));
    out.push(format!(
        "# exclusions by any reason: {}",
        counts_text(&all_counts(report))
    ));
    if report
        .ranked
        .iter()
        .any(|r| r.observation.open_exposure != OpenExposure::None)
    {
        out.push(
            "# open_exposure=unvalued: open positions have no price source (P5.2) and are not in realized PnL; use --require-no-open to exclude them"
                .to_string(),
        );
    }
    out
}

// ---------------------------------------------------------------------
// JSONL DTOs
// ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct WalletDto {
    pub chain: &'static str,
    pub address: String,
}

#[derive(Debug, Serialize)]
pub struct ThresholdsDto {
    pub min_closed_episodes: u64,
    pub min_active_days: u64,
    pub max_trades_per_day: Option<u64>,
    pub max_mints_per_day: Option<u64>,
    pub exclude_unknown_basis: bool,
    pub require_no_open: bool,
    pub top: usize,
}

#[derive(Debug, Serialize)]
pub struct ProviderOptionsDto {
    #[serde(flatten)]
    pub request: scout_providers::HeliusRequestOptions,
    pub server_window: bool,
    /// `max_pages_per_wallet * page_limit`.
    pub tx_budget_per_wallet: u64,
}

#[derive(Debug, Serialize)]
pub struct ScanDto {
    /// Effective provider request options (not yet live-verified).
    pub provider_options: ProviderOptionsDto,
    pub provider: &'static str,
    pub order: &'static str,
    pub max_pages_per_wallet: u32,
    pub max_requests: Option<u64>,
    pub requests_made: u64,
    pub window: &'static str,
}

/// ADR-011 analysis window of the run (`since`/`until` null without one).
#[derive(Debug, Serialize)]
pub struct WindowDto {
    pub since: Option<String>,
    pub until: Option<String>,
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub as_of: String,
    pub as_of_unix: i64,
    /// `period`, `explicit` or `none`.
    pub source: &'static str,
}

pub fn window_dto(w: &AnalysisWindow) -> WindowDto {
    let bounds = w.bounds();
    WindowDto {
        since: bounds.map(|(s, _)| rfc3339(s)),
        until: bounds.map(|(_, u)| rfc3339(u)),
        since_unix: bounds.map(|(s, _)| s),
        until_unix: bounds.map(|(_, u)| u),
        as_of: rfc3339(w.as_of),
        as_of_unix: w.as_of,
        source: w.source.label(),
    }
}

#[derive(Debug, Serialize)]
pub struct RunMetaRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    pub captured_at: String,
    pub rank_version: &'static str,
    pub ledger_version: &'static str,
    /// Unit of the legacy SOL fields (`realized_trade_pnl`, ...).
    pub quote_unit: &'static str,
    /// ADR-013 §5: unit of the ranking metrics and episode gates.
    pub rank_quote_unit: &'static str,
    /// Allowed quote units and the route-swap rule (ADR-013).
    pub ledger_scope: &'static str,
    pub window: WindowDto,
    pub protocol_scope: &'static str,
    pub not_decoded: &'static str,
    /// Decoded programs with their IDL pins (ADR-012).
    pub programs: Vec<ProgramPinDto>,
    pub scan: ScanDto,
    pub rank_by: &'static str,
    pub profile: &'static str,
    /// Profile defaults with overrides applied.
    pub thresholds: ThresholdsDto,
    pub profile_note: &'static str,
    pub open_exposure_policy: &'static str,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
}

#[derive(Debug, Serialize)]
pub struct MoneyDto {
    pub status: &'static str,
    /// Ranking quote unit of `raw`/`decimal` (`sol`, `usdc`, `usdt`).
    pub unit: &'static str,
    /// Raw base units of `unit` (null = N/A, never zero).
    pub raw: Option<String>,
    /// Exact decimal of `unit` (SOL 9 dp, USDC/USDT 6 dp).
    pub decimal: Option<String>,
    /// Legacy SOL fields: populated only when `unit` is `sol`.
    pub lamports: Option<String>,
    pub sol: Option<String>,
    pub sol_exact: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UnitAmountDto {
    pub raw: String,
    pub decimal: String,
}

#[derive(Debug, Serialize)]
pub struct UnitRoiDto {
    pub numerator_exact: String,
    pub denominator_exact: String,
    pub percent_2dp: Option<String>,
}

/// ADR-013 §4/§5: figures of ONE quote unit (never summed across units).
#[derive(Debug, Serialize)]
pub struct UnitMetricsDto {
    pub unit: &'static str,
    pub decimals: u32,
    pub closed_known: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    /// `null` without a known closed episode in this unit (never zero).
    pub realized_trade_pnl: Option<UnitAmountDto>,
    pub consumed_acquisition_basis: Option<UnitAmountDto>,
    pub realized_cost_roi: Option<UnitRoiDto>,
    pub win_rate: RatioDto,
    pub profit_factor: RatioDto,
}

#[derive(Debug, Serialize)]
pub struct RouteCountsDto {
    pub route_swaps: u64,
    pub route_swaps_sol: u64,
    pub route_swaps_usdc: u64,
    pub route_swaps_usdt: u64,
    pub route_leg_not_wallet_price: u64,
}

#[derive(Debug, Serialize)]
pub struct AmountDto {
    pub lamports: String,
    pub sol: String,
}

#[derive(Debug, Serialize)]
pub struct RatioDto {
    pub status: &'static str,
    pub value: Option<String>,
    pub sample_closed_known: u64,
}

#[derive(Debug, Serialize)]
pub struct RoiDto {
    /// `value` or `undefined` (no known closed episode / zero basis).
    pub status: &'static str,
    /// Exact SOL decimals: Σ realized trade PnL and Σ consumed basis.
    pub numerator_sol_exact: Option<String>,
    pub denominator_sol_exact: Option<String>,
    /// Presentation only: percent, rounded half up to 2 places.
    pub percent_2dp: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RationalDto {
    pub numerator: u64,
    pub denominator: u64,
    pub display_2dp: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActivityDto {
    pub timestamped_trades: u64,
    pub active_utc_days: u64,
    pub distinct_mints_timestamped: u64,
    pub mint_day_pairs: u64,
    pub trades_per_active_day: RationalDto,
    pub mints_per_active_day: RationalDto,
}

#[derive(Debug, Serialize)]
pub struct OpenPositionDto {
    pub mint: String,
    pub open_amount_raw: String,
    pub unknown_basis_amount_raw: String,
}

#[derive(Debug, Serialize)]
pub struct OpenExposureDto {
    /// `none` or `unvalued` (no price source until P5.2).
    pub status: &'static str,
    pub positions: u64,
    pub positions_unknown_basis: u64,
    pub details: Vec<OpenPositionDto>,
}

#[derive(Debug, Serialize)]
pub struct MetricsDto {
    /// Ranking quote unit: `realized_net_pnl`, `realized_cost_roi`,
    /// `closed_known_in_quote` and the gates use this unit.
    pub quote: &'static str,
    pub closed_known_in_quote: u64,
    /// All units side by side.
    pub quote_units: Vec<UnitMetricsDto>,
    pub route: RouteCountsDto,
    pub realized_net_pnl: MoneyDto,
    pub realized_trade_pnl: AmountDto,
    pub consumed_acquisition_basis: AmountDto,
    pub realized_cost_roi: RoiDto,
    /// Over all quote units.
    pub closed_episodes_known: u64,
    pub closed_episodes_unknown: u64,
    /// ADR-011: closed episodes whose inventory predates the window; counted, never valued.
    pub left_censored_episodes: u64,
    pub left_censored_amount_raw: String,
    pub open_episodes: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    pub win_rate: RatioDto,
    pub profit_factor: RatioDto,
    pub failed_trade_fees: AmountDto,
    pub has_unknown_basis_inventory: bool,
    pub has_left_censored_inventory: bool,
    pub open_exposure: OpenExposureDto,
    pub activity: ActivityDto,
}

#[derive(Debug, Serialize)]
pub struct WalletRankRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub rank: usize,
    pub wallet: WalletDto,
    pub rank_by: &'static str,
    pub profile: &'static str,
    pub scan_status: &'static str,
    pub transactions_scanned: Option<u64>,
    pub metrics: MetricsDto,
}

#[derive(Debug, Serialize)]
pub struct WalletExcludedRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub wallet: WalletDto,
    pub primary_reason: &'static str,
    pub reasons: Vec<&'static str>,
    /// For `below_top_n`: the rank it would have had.
    pub eligible_rank: Option<usize>,
    pub scan_status: &'static str,
    pub transactions_scanned: Option<u64>,
    pub error: Option<String>,
    pub incomplete_reasons: Vec<String>,
    /// Observed figures (`null` when the scan failed).
    pub observed: Option<MetricsDto>,
}

#[derive(Debug, Serialize)]
pub struct RunSummaryRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    /// `complete` or `partial` (operational: was the universe fully scanned).
    pub status: &'static str,
    pub cancelled: bool,
    pub rank_by: &'static str,
    pub rank_quote_unit: &'static str,
    pub profile: &'static str,
    pub input_wallets: usize,
    pub eligible: usize,
    pub ranked: usize,
    pub excluded: usize,
    pub excluded_by_primary_reason: BTreeMap<&'static str, usize>,
    pub excluded_by_any_reason: BTreeMap<&'static str, usize>,
    pub requests_made: u64,
    /// Run-terminal stop (budget / rate limit), if any.
    pub stop: Option<StopDto>,
    pub incomplete_reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StopDto {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

fn amount(lamports: i128) -> AmountDto {
    AmountDto {
        lamports: lamports.to_string(),
        sol: lamports_to_sol_string(lamports),
    }
}

fn ratio_dto(r: &RatioStatus<Money>, sample: u64) -> RatioDto {
    match r {
        RatioStatus::Value { value } => RatioDto {
            status: "value",
            value: Some(money_str(*value)),
            sample_closed_known: sample,
        },
        RatioStatus::NoObservedLosses => RatioDto {
            status: "no_observed_losses",
            value: None,
            sample_closed_known: sample,
        },
        RatioStatus::Undefined => RatioDto {
            status: "undefined",
            value: None,
            sample_closed_known: sample,
        },
    }
}

fn rational(n: u64, d: u64) -> RationalDto {
    RationalDto {
        numerator: n,
        denominator: d,
        display_2dp: rational_to_decimal_string(n, d, 2),
    }
}

fn unit_metrics_dto(b: &QuoteUnitBlock) -> UnitMetricsDto {
    let unit = b.unit;
    let known = b.closed_episodes_known > 0;
    let amt = |raw: i128, exact: Money| UnitAmountDto {
        raw: raw.to_string(),
        decimal: format_quote_money(unit, exact).unwrap_or_default(),
    };
    UnitMetricsDto {
        unit: quote_unit_label(unit),
        decimals: quote_unit_decimals(unit).unwrap_or(0),
        closed_known: b.closed_episodes_known,
        wins: b.wins,
        losses: b.losses,
        breakeven: b.breakeven,
        realized_trade_pnl: known
            .then(|| amt(b.realized_trade_pnl_raw, b.realized_trade_pnl_exact)),
        consumed_acquisition_basis: known.then(|| {
            amt(
                b.consumed_acquisition_basis_raw,
                b.consumed_acquisition_basis_exact,
            )
        }),
        realized_cost_roi: b
            .roi_parts()
            .and_then(|(n, d)| Ratio::new(n, d))
            .map(|r| UnitRoiDto {
                numerator_exact: format_quote_money(unit, b.realized_trade_pnl_exact)
                    .unwrap_or_default(),
                denominator_exact: format_quote_money(unit, b.consumed_acquisition_basis_exact)
                    .unwrap_or_default(),
                percent_2dp: r.percent_string(2),
            }),
        win_rate: ratio_dto(&b.win_rate, b.closed_episodes_known),
        profit_factor: ratio_dto(&b.profit_factor, b.closed_episodes_known),
    }
}

fn metrics_dto(o: &WalletRankObservation) -> Option<MetricsDto> {
    let l = o.ledger.as_ref()?;
    let a = &l.activity;
    // Legacy SOL ROI fields keep their SOL meaning whatever the rank unit
    // (the ranking-unit ROI is in `quote_units`).
    let sol_roi = l
        .unit_block(QuoteUnit::Lamports)
        .and_then(|b| b.roi_parts())
        .and_then(|(n, d)| Ratio::new(n, d));
    let roi = match sol_roi {
        Some(r) => RoiDto {
            status: "value",
            numerator_sol_exact: Some(money_exact_sol_string(l.realized_trade_pnl_exact)),
            denominator_sol_exact: Some(money_exact_sol_string(l.consumed_acquisition_basis_exact)),
            percent_2dp: r.percent_string(2),
        },
        None => RoiDto {
            status: "undefined",
            numerator_sol_exact: None,
            denominator_sol_exact: None,
            percent_2dp: None,
        },
    };
    let (open_status, positions, unknown_basis, details) = match &o.open_exposure {
        OpenExposure::None => ("none", 0, 0, Vec::new()),
        OpenExposure::Unvalued {
            positions,
            positions_unknown_basis,
            details,
        } => (
            "unvalued",
            *positions,
            *positions_unknown_basis,
            details
                .iter()
                .map(|p| OpenPositionDto {
                    mint: bs58::encode(p.mint).into_string(),
                    open_amount_raw: p.open_amount_raw.to_string(),
                    unknown_basis_amount_raw: p.unknown_basis_amount_raw.to_string(),
                })
                .collect(),
        ),
    };
    let sol_only = |v: Option<String>| v.filter(|_| o.quote == QuoteUnit::Lamports);
    let t = &l.trades;
    Some(MetricsDto {
        quote: quote_unit_label(o.quote),
        closed_known_in_quote: unit_block_of(o).map_or(0, |b| b.closed_episodes_known),
        quote_units: l.unit_blocks.iter().map(unit_metrics_dto).collect(),
        route: RouteCountsDto {
            route_swaps: t.route_swaps,
            route_swaps_sol: t.route_swaps_by_quote.sol,
            route_swaps_usdc: t.route_swaps_by_quote.usdc,
            route_swaps_usdt: t.route_swaps_by_quote.usdt,
            route_leg_not_wallet_price: t.route_leg_not_wallet_price,
        },
        realized_net_pnl: MoneyDto {
            status: o.pnl_status.label(),
            unit: quote_unit_label(o.quote),
            raw: o.net_pnl_raw.map(|v| v.to_string()),
            decimal: o.net_pnl_raw.and_then(|v| raw_decimal(o.quote, v)),
            lamports: sol_only(o.net_pnl_raw.map(|v| v.to_string())),
            sol: sol_only(o.net_pnl_raw.map(lamports_to_sol_string)),
            sol_exact: sol_only(
                o.net_pnl_raw
                    .map(|_| money_exact_sol_string(l.realized_net_pnl_exact)),
            ),
        },
        realized_trade_pnl: amount(l.realized_trade_pnl_lamports),
        consumed_acquisition_basis: amount(l.consumed_acquisition_basis_lamports),
        realized_cost_roi: roi,
        closed_episodes_known: l.closed_episodes_known,
        closed_episodes_unknown: l.closed_episodes_unknown,
        left_censored_episodes: l.left_censored_episodes,
        left_censored_amount_raw: l.left_censored_amount_raw.to_string(),
        open_episodes: l.open_episodes,
        wins: l.wins,
        losses: l.losses,
        breakeven: l.breakeven,
        win_rate: ratio_dto(&l.win_rate, l.closed_episodes_known),
        profit_factor: ratio_dto(&l.profit_factor, l.closed_episodes_known),
        failed_trade_fees: amount(l.failed_trade_fees_lamports),
        has_unknown_basis_inventory: l.has_unknown_basis_inventory,
        has_left_censored_inventory: l.has_left_censored_inventory,
        open_exposure: OpenExposureDto {
            status: open_status,
            positions,
            positions_unknown_basis: unknown_basis,
            details,
        },
        activity: ActivityDto {
            timestamped_trades: a.timestamped_trades,
            active_utc_days: a.active_utc_days,
            distinct_mints_timestamped: a.distinct_mints_timestamped,
            mint_day_pairs: a.mint_day_pairs,
            trades_per_active_day: rational(a.timestamped_trades, a.active_utc_days),
            mints_per_active_day: rational(a.mint_day_pairs, a.active_utc_days),
        },
    })
}

pub struct RunMetaInput<'a> {
    pub run_id: &'a str,
    pub captured_at: &'a str,
    pub max_pages_per_wallet: u32,
    pub provider_options: scout_providers::HeliusRequestOptions,
    pub server_window: bool,
    pub max_requests: Option<u64>,
    pub requests_made: u64,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
    pub window: AnalysisWindow,
}

#[derive(Debug, Serialize)]
pub struct ProgramPinDto {
    pub name: &'static str,
    pub program_id: &'static str,
    pub idl_commit: &'static str,
    pub idl_sha256: &'static str,
}

fn program_pins(scope: &SolanaProtocolScope) -> Vec<ProgramPinDto> {
    let mut pins = vec![ProgramPinDto {
        name: "pump_bonding_curve",
        program_id: scope.program_id,
        idl_commit: scope.idl_commit,
        idl_sha256: scope.idl_sha256,
    }];
    if let (Some(program_id), Some(idl_commit), Some(idl_sha256)) = (
        scope.amm_program_id,
        scope.amm_idl_commit,
        scope.amm_idl_sha256,
    ) {
        pins.push(ProgramPinDto {
            name: "pump_amm",
            program_id,
            idl_commit,
            idl_sha256,
        });
    }
    pins
}

pub fn run_meta_record(m: &RunMetaInput<'_>, report: &WalletRankReport) -> RunMetaRecord {
    let scope = SolanaProtocolScope::pump_wallet_ledger();
    let p = &report.policy;
    RunMetaRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_meta",
        run_id: m.run_id.to_string(),
        captured_at: m.captured_at.to_string(),
        rank_version: SOLANA_WALLET_RANK_VERSION,
        ledger_version: SOLANA_WALLET_LEDGER_VERSION,
        quote_unit: "lamports",
        rank_quote_unit: quote_unit_label(p.quote),
        ledger_scope: SOLANA_WALLET_LEDGER_SCOPE,
        window: window_dto(&m.window),
        protocol_scope: scope.recognized,
        not_decoded: scope.not_decoded,
        programs: program_pins(&scope),
        scan: ScanDto {
            provider_options: ProviderOptionsDto {
                request: m.provider_options,
                server_window: m.server_window,
                tx_budget_per_wallet: u64::from(m.max_pages_per_wallet)
                    * u64::from(m.provider_options.page_limit),
            },
            provider: "helius",
            order: "newest_first",
            max_pages_per_wallet: m.max_pages_per_wallet,
            max_requests: m.max_requests,
            requests_made: m.requests_made,
            window: if m.window.is_bounded() {
                "newest-first walk until the window start (blockTime < since) or the page budget"
            } else {
                "full available history within the page budget (no time window)"
            },
        },
        rank_by: p.rank_by.label(),
        profile: p.profile.label(),
        thresholds: ThresholdsDto {
            min_closed_episodes: p.min_closed_episodes,
            min_active_days: p.min_active_days,
            max_trades_per_day: p.max_trades_per_day,
            max_mints_per_day: p.max_mints_per_day,
            exclude_unknown_basis: p.exclude_unknown_basis,
            require_no_open: p.require_no_open,
            top: p.top,
        },
        profile_note: "thresholds are research starting policy, not statistical guarantees",
        open_exposure_policy: "open positions are unvalued (no price source, P5.2): included and flagged open_exposure=unvalued unless require_no_open; deviation from config require_resolved_open_exposure=true",
        input_wallet_count: m.input_wallet_count,
        input_duplicates: m.input_duplicates,
        upstream_complete: m.upstream_complete,
    }
}

pub fn rank_record(r: &RankedWallet, report: &WalletRankReport) -> Option<WalletRankRecord> {
    Some(WalletRankRecord {
        schema_version: SCHEMA_VERSION,
        kind: "wallet_rank",
        rank: r.rank,
        wallet: WalletDto {
            chain: CHAIN,
            address: addr(&r.observation),
        },
        rank_by: report.policy.rank_by.label(),
        profile: report.policy.profile.label(),
        scan_status: r.observation.status.label(),
        transactions_scanned: r.observation.transactions_scanned,
        metrics: metrics_dto(&r.observation)?,
    })
}

pub fn excluded_record(
    e: &ExcludedWallet,
    redact: &dyn Fn(&str) -> String,
) -> WalletExcludedRecord {
    let o = &e.observation;
    WalletExcludedRecord {
        schema_version: SCHEMA_VERSION,
        kind: "wallet_excluded",
        wallet: WalletDto {
            chain: CHAIN,
            address: addr(o),
        },
        primary_reason: e.primary_reason().label(),
        reasons: e.reasons.iter().map(|r| r.label()).collect(),
        eligible_rank: e.eligible_rank,
        scan_status: o.status.label(),
        transactions_scanned: o.transactions_scanned,
        error: o.error.as_deref().map(redact),
        incomplete_reasons: o.incomplete_reasons.iter().map(|r| redact(r)).collect(),
        observed: metrics_dto(o),
    }
}

pub struct SummaryInput<'a> {
    pub run_id: &'a str,
    pub partial: bool,
    pub cancelled: bool,
    pub stop: Option<ScanStop>,
    pub requests_made: u64,
    pub incomplete_reasons: Vec<String>,
}

fn stop_dto(stop: ScanStop) -> StopDto {
    match stop {
        ScanStop::BudgetExhausted { limit } => StopDto {
            kind: "budget_exhausted",
            limit: Some(limit),
            retry_after_secs: None,
        },
        ScanStop::RateLimited { retry_after_secs } => StopDto {
            kind: "rate_limited",
            limit: None,
            retry_after_secs,
        },
    }
}

pub fn run_summary_record(report: &WalletRankReport, s: SummaryInput<'_>) -> RunSummaryRecord {
    RunSummaryRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_summary",
        run_id: s.run_id.to_string(),
        status: if s.partial { "partial" } else { "complete" },
        cancelled: s.cancelled,
        rank_by: report.policy.rank_by.label(),
        rank_quote_unit: quote_unit_label(report.policy.quote),
        profile: report.policy.profile.label(),
        input_wallets: report.input_count,
        eligible: report.eligible_count,
        ranked: report.ranked.len(),
        excluded: report.excluded.len(),
        excluded_by_primary_reason: primary_counts(report),
        excluded_by_any_reason: all_counts(report),
        requests_made: s.requests_made,
        stop: s.stop.map(stop_dto),
        incomplete_reasons: s.incomplete_reasons,
    }
}

/// `run_meta`, `wallet_rank`* (rank order), `wallet_excluded`* (input
/// order, below-top wallets included), `run_summary`.
pub fn jsonl_lines(
    meta: &RunMetaInput<'_>,
    report: &WalletRankReport,
    summary: SummaryInput<'_>,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let ser = |r: Result<String, serde_json::Error>| r.map_err(|e| e.to_string());
    let mut lines = Vec::with_capacity(report.ranked.len() + report.excluded.len() + 2);
    lines.push(ser(serde_json::to_string(&run_meta_record(meta, report)))?);
    for r in &report.ranked {
        let rec = rank_record(r, report)
            .ok_or_else(|| "ranked wallet without ledger (internal error)".to_string())?;
        lines.push(ser(serde_json::to_string(&rec))?);
    }
    for e in &report.excluded {
        lines.push(ser(serde_json::to_string(&excluded_record(e, redact)))?);
    }
    lines.push(ser(serde_json::to_string(&run_summary_record(
        report, summary,
    )))?);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scout_engine::{
        RankBy, RankPolicy, RankProfile, SolanaWalletStats, WalletScanStatus,
        build_solana_wallet_ledger, lamports_to_money, pump_bonding_curve_decoder,
        rank_solana_wallets,
    };
    use serde_json::Value;

    fn wallet(b: u8, pnl: i128, basis: i128) -> SolanaWalletStats {
        let decoder = pump_bonding_curve_decoder().unwrap();
        let mut l = build_solana_wallet_ledger(&[b; 32], &[], &decoder).unwrap();
        l.closed_episodes_known = 3;
        l.realized_trade_pnl_exact = lamports_to_money(pnl).unwrap();
        l.realized_trade_pnl_lamports = pnl;
        l.realized_net_pnl_exact = lamports_to_money(pnl).unwrap();
        l.realized_net_pnl_lamports = pnl;
        l.consumed_acquisition_basis_exact = lamports_to_money(basis).unwrap();
        l.consumed_acquisition_basis_lamports = basis;
        {
            let b = &mut l.unit_blocks[0];
            b.closed_episodes_known = 3;
            b.realized_trade_pnl_raw = pnl;
            b.realized_trade_pnl_exact = lamports_to_money(pnl).unwrap();
            b.consumed_acquisition_basis_raw = basis;
            b.consumed_acquisition_basis_exact = lamports_to_money(basis).unwrap();
        }
        l.activity.active_utc_days = 2;
        l.activity.timestamped_trades = 6;
        l.activity.mint_day_pairs = 3;
        SolanaWalletStats {
            wallet: [b; 32],
            status: WalletScanStatus::Ok,
            transactions_scanned: Some(6),
            transactions_in_window: None,
            truncated: false,
            unexpected_payloads: 0,
            error: None,
            ledger: Some(l),
            incomplete_reasons: Vec::new(),
            failure: None,
            not_scanned: None,
        }
    }

    fn report() -> WalletRankReport {
        let mut err = wallet(3, 0, 1);
        err.status = WalletScanStatus::Error;
        err.ledger = None;
        err.error = Some("boom".to_string());
        let wallets = vec![wallet(1, 157_000, 1_020_000), wallet(2, 5, 7), err];
        let mut p = RankPolicy::for_profile(RankProfile::None, RankBy::RealizedNetPnl, 1);
        p.top = 1;
        rank_solana_wallets(&wallets, &p)
    }

    fn meta<'a>() -> RunMetaInput<'a> {
        RunMetaInput {
            run_id: "wallet-rank-t",
            captured_at: "2026-10-02T00:00:00Z",
            max_pages_per_wallet: 10,
            provider_options: scout_providers::HeliusRequestOptions::default(),
            server_window: false,
            max_requests: Some(50),
            requests_made: 4,
            input_wallet_count: 3,
            input_duplicates: 0,
            upstream_complete: true,
            window: AnalysisWindow::none(1_790_000_000),
        }
    }

    #[test]
    fn jsonl_has_meta_ranks_exclusions_and_summary_with_string_money() {
        let rep = report();
        let lines = jsonl_lines(
            &meta(),
            &rep,
            SummaryInput {
                run_id: "wallet-rank-t",
                partial: true,
                cancelled: false,
                stop: None,
                requests_made: 4,
                incomplete_reasons: vec!["wallet x: scan failed".into()],
            },
            &|t| t.to_string(),
        )
        .unwrap();
        let v: Vec<Value> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let kinds: Vec<&str> = v.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            vec![
                "run_meta",
                "wallet_rank",
                "wallet_excluded",
                "wallet_excluded",
                "run_summary"
            ]
        );
        assert_eq!(v[0]["thresholds"]["top"], 1);
        assert_eq!(v[0]["profile"], "none");
        assert_eq!(v[0]["scan"]["requests_made"], 4);
        let m = &v[1]["metrics"];
        assert_eq!(v[1]["rank"], 1);
        assert_eq!(m["realized_net_pnl"]["lamports"], "157000");
        assert_eq!(m["realized_net_pnl"]["sol"], "0.000157000");
        assert_eq!(m["consumed_acquisition_basis"]["lamports"], "1020000");
        assert_eq!(m["realized_cost_roi"]["percent_2dp"], "15.39");
        assert_eq!(m["open_exposure"]["status"], "none");
        // wallet 2 below top; wallet 3 provider error with null observed.
        assert_eq!(v[2]["primary_reason"], "below_top_n");
        assert_eq!(v[2]["eligible_rank"], 2);
        assert_eq!(v[3]["primary_reason"], "provider_error");
        assert!(v[3]["observed"].is_null());
        assert_eq!(v[4]["status"], "partial");
        assert_eq!(v[4]["excluded_by_primary_reason"]["below_top_n"], 1);
        assert_eq!(v[4]["excluded_by_primary_reason"]["provider_error"], 1);
        assert_eq!(v[4]["ranked"], 1);
        assert_eq!(v[4]["eligible"], 2);
    }

    #[test]
    fn table_has_header_rows_and_exclusion_summary() {
        let lines = table_lines(&report(), true, &AnalysisWindow::none(0));
        assert!(lines[0].starts_with("rank"));
        assert!(lines[1].contains("0.000157000"), "{}", lines[1]);
        assert!(lines[1].contains("15.39%"), "{}", lines[1]);
        assert!(lines[1].contains("none/observed"), "{}", lines[1]);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("below_top_n=1") && l.contains("provider_error=1"))
        );
        assert!(lines.iter().any(|l| l.contains("status=partial")));
    }

    #[test]
    fn windowed_table_starts_with_the_window_line() {
        use scout_engine::WindowSource;
        let w = AnalysisWindow {
            since: 1_785_542_400,
            until: 1_788_220_800,
            as_of: 1_790_000_000,
            source: WindowSource::Period,
        };
        let lines = table_lines(&report(), false, &w);
        assert!(lines[0].starts_with("# window [2026-08-01T00:00:00Z, 2026-09-01T00:00:00Z)"));
        assert!(lines[1].starts_with("rank"));
        let mut m = meta();
        m.window = w;
        let v = serde_json::to_value(run_meta_record(&m, &report())).unwrap();
        assert_eq!(v["window"]["source"], "period");
        assert_eq!(v["window"]["as_of_unix"], 1_790_000_000_i64);
        let observed = metrics_dto(&report().ranked[0].observation).unwrap();
        let o = serde_json::to_value(observed).unwrap();
        assert_eq!(o["left_censored_episodes"], 0);
        assert_eq!(o["has_left_censored_inventory"], false);
    }
}
