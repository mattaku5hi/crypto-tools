//! Output layer for `wallet-stats` (docs/CLI.md §5, §7).
//!
//! DTOs are built field-by-field from engine reports; internal types are
//! never serialized. Money convention (CLI.md §7): lamports and SOL are
//! decimal STRINGS (exact, integer arithmetic only); counts are JSON
//! numbers; unknown is `null` with an explicit status, never `0`.

use scout_analytics::RatioStatus;
use scout_app::SCHEMA_VERSION;
use scout_core::MONEY_SCALE;
use scout_core::Money;
use scout_engine::{
    AnalysisWindow, EpisodeOutcome, EpisodeRecord, OpenPosition, QuoteUnit, QuoteUnitBlock, Ratio,
    SOLANA_WALLET_LEDGER_SCOPE, SOLANA_WALLET_LEDGER_VERSION, ScanFailureKind, ScanStop,
    SolanaProtocolScope, SolanaWalletLedgerReport, SolanaWalletStats, SolanaWalletStatsReport,
    format_quote_money, format_scaled_decimal, lamports_to_sol_string, quote_unit_decimals,
    quote_unit_label, rational_to_decimal_string,
};
use serde::Serialize;

pub const SCAN_ORDER: &str = "newest_first";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    Summary,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Input,
    RealizedNetPnl,
}

// ---------------------------------------------------------------------
// Shared views
// ---------------------------------------------------------------------

/// Headline realized net PnL classification.
/// `observed`: coverage complete and no Unknown closed episode.
/// `known_subset`: value covers known closed episodes only (incomplete
/// coverage and/or Unknown closed episodes).
/// `n_a`: nothing known to report (never rendered as zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PnlView {
    pub status: &'static str,
    pub lamports: Option<i128>,
    pub na_reason: Option<&'static str>,
}

pub fn pnl_view(w: &SolanaWalletStats) -> PnlView {
    let Some(l) = &w.ledger else {
        return PnlView {
            status: "n_a",
            lamports: None,
            na_reason: Some(if w.status == scout_engine::WalletScanStatus::NotScanned {
                "not scanned"
            } else {
                "scan failed"
            }),
        };
    };
    // ADR-013: the headline is the SOL block; other units are separate.
    let sol_closed = l
        .unit_block(QuoteUnit::Lamports)
        .map_or(l.closed_episodes_known, |b| b.closed_episodes_known);
    if sol_closed == 0 && l.failed_trade_fees_lamports == 0 {
        return PnlView {
            status: "n_a",
            lamports: None,
            na_reason: Some(
                if w.transactions_in_window.or(w.transactions_scanned) == Some(0) {
                    "no activity"
                } else {
                    "no known closed SOL episodes"
                },
            ),
        };
    }
    let observed = w.coverage_complete() && l.closed_episodes_unknown == 0;
    PnlView {
        status: if observed { "observed" } else { "known_subset" },
        lamports: Some(l.realized_net_pnl_lamports),
        na_reason: None,
    }
}

/// Sort key for `--sort realized-net-pnl` (descending); `None` last.
pub fn sort_key(w: &SolanaWalletStats) -> Option<i128> {
    pnl_view(w).lamports
}

/// Indices into `report.wallets` in display order. Never changes the
/// set; ties and N/A keep input order.
pub fn display_order(report: &SolanaWalletStatsReport, sort: SortMode) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..report.wallets.len()).collect();
    if sort == SortMode::RealizedNetPnl {
        idx.sort_by(|a, b| {
            let (ka, kb) = (
                report.wallets.get(*a).and_then(sort_key),
                report.wallets.get(*b).and_then(sort_key),
            );
            match (ka, kb) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then(a.cmp(b))
        });
    }
    idx
}

fn money_str(m: Money) -> String {
    format_scaled_decimal(m.scaled_units(), MONEY_SCALE)
}

/// Exact SOL of a lamport-scaled `Money` (sub-lamport remainder kept).
fn money_exact_sol(m: Money) -> String {
    format_scaled_decimal(m.scaled_units(), MONEY_SCALE + 9)
}

fn ratio_cell(r: &RatioStatus<Money>, what: &str) -> String {
    match r {
        RatioStatus::Value { value } => money_str(*value),
        RatioStatus::NoObservedLosses => format!("N/A (no observed losses; {what} unbounded)"),
        RatioStatus::Undefined => "N/A (undefined: no closed outcomes)".to_string(),
    }
}

// ---------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------

const HEADER: [&str; 21] = [
    "wallet",
    "status",
    "realized_net_pnl_sol",
    "realized_pnl_usdc",
    "realized_pnl_usdt",
    "route_swaps",
    "closed_known/unknown",
    "left_censored",
    "open",
    "W/L/BE",
    "win_rate",
    "profit_factor",
    "median_hold_s",
    "trades",
    "mints",
    "active_days",
    "trades/day",
    "failed_fees_sol",
    "unknown_basis",
    "unexplained_native_sol",
    "coverage",
];

fn na(reason: &str) -> String {
    format!("N/A ({reason})")
}

fn row(w: &SolanaWalletStats) -> Vec<String> {
    let addr = bs58::encode(w.wallet).into_string();
    let pnl = pnl_view(w);
    let pnl_cell = match (pnl.status, pnl.lamports) {
        ("observed", Some(l)) => lamports_to_sol_string(l),
        ("known_subset", Some(l)) => format!("N/A (known subset: {})", lamports_to_sol_string(l)),
        _ => na(pnl.na_reason.unwrap_or("unknown")),
    };
    let coverage = match w.status {
        scout_engine::WalletScanStatus::Error => "failed".to_string(),
        scout_engine::WalletScanStatus::NotScanned => "not scanned".to_string(),
        scout_engine::WalletScanStatus::Incomplete => {
            if w.truncated {
                "incomplete (truncated)".to_string()
            } else {
                "incomplete".to_string()
            }
        }
        _ => "complete".to_string(),
    };
    let mut cells = vec![addr, w.status.label().to_string(), pnl_cell];
    match &w.ledger {
        None => {
            for _ in 3..HEADER.len() - 1 {
                cells.push(na("scan failed"));
            }
        }
        Some(l) => {
            for unit in [QuoteUnit::UsdcUnits, QuoteUnit::UsdtUnits] {
                cells.push(unit_pnl_cell(w, l, unit));
            }
            cells.push(l.trades.route_swaps.to_string());
            let ratios_ok = w.coverage_complete();
            cells.push(format!(
                "{}/{}",
                l.closed_episodes_known, l.closed_episodes_unknown
            ));
            cells.push(l.left_censored_episodes.to_string());
            cells.push(l.open_episodes.to_string());
            cells.push(format!("{}/{}/{}", l.wins, l.losses, l.breakeven));
            cells.push(if ratios_ok {
                ratio_cell(&l.win_rate, "win rate")
            } else {
                na("incomplete coverage")
            });
            cells.push(if ratios_ok {
                ratio_cell(&l.profit_factor, "PF")
            } else {
                na("incomplete coverage")
            });
            cells.push(
                l.median_holding_seconds
                    .map_or_else(|| na("no timed closed episodes"), |s| s.to_string()),
            );
            cells.push((l.trades.buys + l.trades.sells).to_string());
            cells.push(l.distinct_mints_traded.to_string());
            cells.push(l.activity.active_utc_days.to_string());
            cells.push(
                rational_to_decimal_string(
                    l.activity.timestamped_trades,
                    l.activity.active_utc_days,
                    2,
                )
                .unwrap_or_else(|| na("no timestamped trades")),
            );
            cells.push(lamports_to_sol_string(l.failed_trade_fees_lamports));
            cells.push(
                if l.has_unknown_basis_inventory {
                    "yes"
                } else {
                    "no"
                }
                .to_string(),
            );
            cells.push(lamports_to_sol_string(
                l.diagnostics.unexplained_native_flow_lamports,
            ));
        }
    }
    cells.push(coverage);
    cells
}

/// Realized PnL cell of a non-SOL unit: exact decimal of the known closed
/// episodes (labelled when the figure is a known subset), never 0 for none.
fn unit_pnl_cell(w: &SolanaWalletStats, l: &SolanaWalletLedgerReport, unit: QuoteUnit) -> String {
    let label = quote_unit_label(unit);
    let Some(b) = l.unit_block(unit).filter(|b| b.closed_episodes_known > 0) else {
        return na(&format!("no known closed {label} episodes"));
    };
    let v =
        format_quote_money(unit, b.realized_trade_pnl_exact).unwrap_or_else(|| "N/A".to_string());
    if w.coverage_complete() && l.closed_episodes_unknown == 0 {
        v
    } else {
        format!("N/A (known subset: {v})")
    }
}

fn detail_lines(w: &SolanaWalletStats, out: &mut Vec<String>) {
    if let Some(err) = &w.error {
        out.push(format!("    error: {err}"));
    }
    for r in &w.incomplete_reasons {
        out.push(format!("    gap: {r}"));
    }
    let Some(l) = &w.ledger else { return };
    for ep in &l.episodes {
        out.push(format!("    episode {}", episode_text(ep)));
    }
    for p in &l.open_positions {
        out.push(format!("    open {}", open_text(p)));
    }
}

fn episode_text(ep: &EpisodeRecord) -> String {
    let mint = bs58::encode(ep.mint).into_string();
    let (kind, _) = outcome_parts(&ep.outcome);
    let unit = episode_unit(ep);
    let pnl = match &ep.outcome {
        EpisodeOutcome::ClosedKnown { pnl } => {
            format_quote_money(unit, *pnl).unwrap_or_else(|| "N/A".to_string())
        }
        _ => "N/A".to_string(),
    };
    let reasons: Vec<&str> = ep.unknown_reasons.iter().map(|r| r.label()).collect();
    format!(
        "mint={mint} outcome={kind} pnl_{}={pnl} hold_s={} unknown_reasons=[{}]",
        quote_unit_label(unit),
        ep.holding_seconds
            .map_or_else(|| "N/A".to_string(), |s| s.to_string()),
        reasons.join("; ")
    )
}

fn open_text(p: &OpenPosition) -> String {
    format!(
        "mint={} amount_raw={} unknown_basis_amount_raw={}",
        bs58::encode(p.mint).into_string(),
        p.open_amount_raw,
        p.unknown_basis_amount_raw
    )
}

/// Quote unit of an episode's known figures (SOL when it has none).
fn episode_unit(ep: &EpisodeRecord) -> QuoteUnit {
    ep.quote_unit.unwrap_or(QuoteUnit::Lamports)
}

/// Pnl lamports (truncated from exact money) only for ClosedKnown.
fn outcome_parts(o: &EpisodeOutcome) -> (&'static str, Option<i128>) {
    match o {
        EpisodeOutcome::ClosedKnown { pnl } => (
            "closed_known",
            Some(scout_engine::money_to_lamports_trunc(*pnl)),
        ),
        EpisodeOutcome::ClosedUnknown => ("closed_unknown", None),
        EpisodeOutcome::LeftCensored => ("left_censored", None),
        EpisodeOutcome::Open => ("open", None),
    }
}

/// Header + one aligned row per wallet (display order); `full` adds
/// indented evidence lines under each row.
pub fn table_lines(
    report: &SolanaWalletStatsReport,
    detail: Detail,
    sort: SortMode,
    window: &AnalysisWindow,
) -> Vec<String> {
    let mut rows: Vec<(Vec<String>, &SolanaWalletStats)> = Vec::new();
    for i in display_order(report, sort) {
        if let Some(w) = report.wallets.get(i) {
            rows.push((row(w), w));
        }
    }
    let mut widths: Vec<usize> = HEADER.iter().map(|h| h.chars().count()).collect();
    for (cells, _) in &rows {
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
    let head: Vec<String> = HEADER.iter().map(|s| (*s).to_string()).collect();
    let mut out = Vec::new();
    if let Some(line) = window_line(window) {
        out.push(line);
    }
    out.push(fmt(&head));
    for (cells, w) in &rows {
        out.push(fmt(cells));
        if detail == Detail::Full {
            detail_lines(w, &mut out);
        }
    }
    out
}

fn rfc3339(unix: i64) -> String {
    scout_app::format_unix_utc(u64::try_from(unix).unwrap_or(0))
}

/// `# window ...` line above the table header; `None` without a window.
pub fn window_line(window: &AnalysisWindow) -> Option<String> {
    let (since, until) = window.bounds()?;
    Some(format!(
        "# window [{}, {}) source={} as_of={}",
        rfc3339(since),
        rfc3339(until),
        window.source.label(),
        rfc3339(window.as_of)
    ))
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
pub struct VariantDto {
    pub name: &'static str,
    pub side: &'static str,
    pub verification: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ScopeDto {
    pub chain: &'static str,
    pub program_id: &'static str,
    pub idl_commit: &'static str,
    pub idl_sha256: &'static str,
    /// Second decoded program (PumpSwap AMM, ADR-012) and its IDL pin.
    pub amm_program_id: Option<&'static str>,
    pub amm_idl_commit: Option<&'static str>,
    pub amm_idl_sha256: Option<&'static str>,
    pub qualification_version: &'static str,
    pub recognized: &'static str,
    pub not_decoded: &'static str,
    pub variants: Vec<VariantDto>,
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
    pub scope: ScopeDto,
    pub ledger_version: &'static str,
    /// Unit of the legacy SOL fields; per-unit figures are in `stats.quote_units`.
    pub quote_unit: &'static str,
    /// Allowed quote units and the route-swap rule (ADR-013).
    pub ledger_scope: &'static str,
    pub window: WindowDto,
    pub scan: ScanDto,
    pub detail: &'static str,
    pub sort: &'static str,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
    /// Total HTTP attempts made (retries included).
    pub requests_made: u64,
    pub budget: BudgetDto,
}

/// Run request budget (`--max-requests`; `null` = unlimited).
#[derive(Debug, Serialize)]
pub struct BudgetDto {
    pub max_requests: Option<u64>,
}

/// Typed run-stop reason / failure kind: `budget_exhausted`,
/// `rate_limited` (or `other` for a failure with no stop).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ReasonDto {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

pub fn stop_dto(stop: ScanStop) -> ReasonDto {
    match stop {
        ScanStop::BudgetExhausted { limit } => ReasonDto {
            kind: "budget_exhausted",
            limit: Some(limit),
            retry_after_secs: None,
        },
        ScanStop::RateLimited { retry_after_secs } => ReasonDto {
            kind: "rate_limited",
            limit: None,
            retry_after_secs,
        },
    }
}

fn failure_dto(kind: ScanFailureKind) -> ReasonDto {
    kind.stop().map_or(
        ReasonDto {
            kind: "other",
            limit: None,
            retry_after_secs: None,
        },
        stop_dto,
    )
}

#[derive(Debug, Serialize)]
pub struct MoneyDto {
    /// `observed`, `known_subset` or `n_a`.
    pub status: &'static str,
    pub lamports: Option<String>,
    pub sol: Option<String>,
    /// Exact `Money` (sub-lamport proration remainder kept), SOL.
    pub sol_exact: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AmountDto {
    pub lamports: String,
    pub sol: String,
}

#[derive(Debug, Serialize)]
pub struct RatioDto {
    /// `value`, `no_observed_losses` or `undefined`.
    pub status: &'static str,
    /// Decimal string at 8 fractional digits.
    pub value: Option<String>,
    /// Closed known episodes the ratio is computed over.
    pub sample_closed_known: u64,
}

#[derive(Debug, Serialize)]
pub struct RationalDto {
    pub numerator: u64,
    pub denominator: u64,
    /// Presentation only: rounded half up to 2 places; `null` if denominator is 0.
    pub display_2dp: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TradesDto {
    pub buys: u64,
    pub sells: u64,
    pub priced: u64,
    pub unpaired: u64,
    pub mismatched: u64,
    pub unsupported_quote: u64,
    pub malformed_consideration: u64,
    pub quote_funded_elsewhere: u64,
    pub unreconciled: u64,
    pub fixture_verified_variant: u64,
    pub idl_only_variant: u64,
    /// Per venue, side in token terms (ADR-012 §5).
    pub bonding_curve: VenueSidesDto,
    pub pump_amm: VenueSidesDto,
    /// ADR-013 route swaps booked from wallet deltas (also in `priced`).
    pub route: VenueSidesDto,
    /// Per `(venue, variant)` with its verification status.
    pub variants: Vec<VariantTradesDto>,
}

#[derive(Debug, Serialize)]
pub struct VenueSidesDto {
    pub buys: u64,
    pub sells: u64,
}

#[derive(Debug, Serialize)]
pub struct VariantTradesDto {
    pub venue: &'static str,
    pub variant: &'static str,
    pub verification: &'static str,
    pub trades: u64,
}

#[derive(Debug, Serialize)]
pub struct ActivityDto {
    pub timestamped_trades: u64,
    pub first_trade_timestamp: Option<i64>,
    pub last_trade_timestamp: Option<i64>,
    pub active_span_seconds: Option<i64>,
    pub active_utc_days: u64,
    pub distinct_mints_timestamped: u64,
    pub mint_day_pairs: u64,
    pub trades_per_active_day: RationalDto,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticsDto {
    pub transactions_considered: u64,
    pub duplicate_transactions_ignored: u64,
    pub failed_transactions: u64,
    pub malformed_trade_instructions: u64,
    pub orphan_trade_events: u64,
    pub unexplained_native_flow: AmountDto,
    pub unexplained_native_flow_txs: u64,
    pub out_of_scope_token_movements: u64,
    pub continuity_breaks: u64,
    pub unknown_disposals: u64,
    pub known_disposals: u64,
    pub left_censored_disposals: u64,
    pub router_forward_trades_not_attributed: u64,
    pub quote_funded_elsewhere_trades: u64,
    pub reversed_pool_trades: u64,
}

/// Exact amount in raw base units and as an exact decimal string of its
/// own unit (SOL 9 dp, USDC/USDT 6 dp). Units are never summed.
#[derive(Debug, Serialize)]
pub struct UnitAmountDto {
    pub raw: String,
    pub decimal: String,
}

#[derive(Debug, Serialize)]
pub struct RoiDto {
    /// Exact `numerator / denominator` of the unit (decimal strings).
    pub numerator_exact: String,
    pub denominator_exact: String,
    /// Presentation only: percent rounded half up to 2 places.
    pub percent_2dp: Option<String>,
}

/// ADR-013 §4: figures of the known closed episodes of ONE quote unit.
#[derive(Debug, Serialize)]
pub struct QuoteUnitDto {
    /// `sol`, `usdc` or `usdt`.
    pub unit: &'static str,
    pub decimals: u32,
    /// `observed`, `known_subset` or `n_a` (no known closed episode in this unit).
    pub status: &'static str,
    pub closed_known: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    /// `null` without a known closed episode (never zero).
    pub realized_trade_pnl: Option<UnitAmountDto>,
    pub consumed_acquisition_basis: Option<UnitAmountDto>,
    pub realized_cost_roi: Option<RoiDto>,
    pub win_rate: RatioDto,
    pub profit_factor: RatioDto,
    pub open_episode_known_disposal_pnl: UnitAmountDto,
    pub open_episode_known_disposals: u64,
}

#[derive(Debug, Serialize)]
pub struct QuoteUnitCountsDto {
    pub sol: u64,
    pub usdc: u64,
    pub usdt: u64,
}

/// ADR-013 §1/§2 counters.
#[derive(Debug, Serialize)]
pub struct RouteDto {
    pub route_swaps: u64,
    pub route_swaps_by_quote: QuoteUnitCountsDto,
    pub route_leg_not_wallet_price: u64,
    pub route_rejected_wallet_not_signer: u64,
    pub route_rejected_multi_asset: u64,
    pub route_rejected_not_opposite_signs: u64,
    pub route_rejected_no_quote_leg: u64,
    pub route_rejected_no_verified_leg: u64,
    pub route_rejected_passthrough_nonzero: u64,
}

#[derive(Debug, Serialize)]
pub struct StatsDto {
    pub ledger_version: &'static str,
    /// Unit of the legacy SOL fields below (`realized_*`, `profit_factor`,
    /// `failed_trade_fees`, ...). Per-unit figures are in `quote_units`.
    pub quote_unit: &'static str,
    /// ADR-013: one entry per quote unit (sol, usdc, usdt); never summed.
    pub quote_units: Vec<QuoteUnitDto>,
    pub route: RouteDto,
    pub realized_net_pnl: MoneyDto,
    /// SUM A: realized PnL of known closed episodes (before failed-tx fees).
    pub realized_trade_pnl: AmountDto,
    pub realized_trade_pnl_sol_exact: String,
    pub failed_trade_fees: AmountDto,
    pub failed_trade_fee_txs: u64,
    /// SUM B (not in the headline).
    pub open_episode_known_disposal_pnl: AmountDto,
    pub open_episode_known_disposals: u64,
    /// Over all quote units (each episode in its own unit).
    pub closed_episodes_known: u64,
    pub closed_episodes_unknown: u64,
    /// ADR-011: closed episodes whose inventory predates the window (never valued).
    pub left_censored_episodes: u64,
    /// Raw token units booked as left-censored shortfall (decimal string).
    pub left_censored_amount_raw: String,
    pub open_episodes: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    pub win_rate: RatioDto,
    pub profit_factor: RatioDto,
    pub median_holding_seconds: Option<i64>,
    pub holding_time_samples: u64,
    pub trades: TradesDto,
    pub distinct_mints_traded: u64,
    pub activity: ActivityDto,
    pub has_unknown_basis_inventory: bool,
    pub has_left_censored_inventory: bool,
    pub open_positions_with_unknown_basis: u64,
    pub unknown_basis_lots_created: u64,
    pub diagnostics: DiagnosticsDto,
}

#[derive(Debug, Serialize)]
pub struct CoverageDto {
    pub complete: bool,
    pub truncated: bool,
    /// Older history unseen when `truncated` (newest-first scan).
    pub truncation_meaning: &'static str,
    pub transactions_scanned: Option<u64>,
    /// Transactions inside the window (null without a window or on failure).
    pub transactions_in_window: Option<u64>,
    pub unexpected_payloads: u64,
    pub incomplete_reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EpisodeDto {
    pub mint: String,
    /// `closed_known`, `closed_unknown`, `left_censored` or `open`.
    pub outcome: &'static str,
    /// SOL episodes only (legacy); other units use `pnl_unit`/`pnl_decimal`.
    pub pnl: Option<AmountDto>,
    pub pnl_sol_exact: Option<String>,
    /// ADR-013: unit of the episode's known figures (`sol`/`usdc`/`usdt`).
    pub quote_unit: Option<&'static str>,
    /// Raw base units of `quote_unit` (closed known only).
    pub pnl_raw: Option<String>,
    /// Exact decimal in `quote_unit` (SOL 9 dp, USDC/USDT 6 dp).
    pub pnl_decimal: Option<String>,
    pub opened_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub holding_seconds: Option<i64>,
    pub opened_slot: u64,
    pub opened_transaction_index: u64,
    pub unknown_reasons: Vec<&'static str>,
    pub known_disposals: u64,
    /// SOL episodes only; see `known_disposal_pnl_decimal` for other units.
    pub known_disposal_pnl: Option<AmountDto>,
    pub known_disposal_pnl_decimal: Option<String>,
    pub left_censored_amount_raw: String,
}

#[derive(Debug, Serialize)]
pub struct OpenPositionDto {
    pub mint: String,
    pub open_amount_raw: String,
    pub unknown_basis_amount_raw: String,
    pub opened_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct WalletStatsRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub wallet: WalletDto,
    pub status: &'static str,
    pub error: Option<String>,
    /// Typed cause of an `error` from a provider failure.
    pub error_kind: Option<ReasonDto>,
    /// Why a `not_scanned` wallet was skipped.
    pub stop_reason: Option<ReasonDto>,
    pub coverage: CoverageDto,
    pub stats: Option<StatsDto>,
    /// Only with `--detail full` (and a ledger).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<EpisodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_positions: Option<Vec<OpenPositionDto>>,
}

#[derive(Debug, Serialize)]
pub struct WalletStatusDto {
    pub wallet: WalletDto,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct RunSummaryRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    /// `complete` or `partial` (operational completion).
    pub status: &'static str,
    pub cancelled: bool,
    pub requests_made: u64,
    pub stop: Option<ReasonDto>,
    pub records: usize,
    pub incomplete_reasons: Vec<String>,
    pub wallets: Vec<WalletStatusDto>,
}

fn wallet_dto(w: &SolanaWalletStats) -> WalletDto {
    WalletDto {
        chain: "solana",
        address: bs58::encode(w.wallet).into_string(),
    }
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

fn unit_amount(unit: QuoteUnit, raw: i128, exact: Money) -> UnitAmountDto {
    UnitAmountDto {
        raw: raw.to_string(),
        decimal: format_quote_money(unit, exact).unwrap_or_default(),
    }
}

fn quote_unit_dto(
    w: &SolanaWalletStats,
    l: &SolanaWalletLedgerReport,
    b: &QuoteUnitBlock,
) -> QuoteUnitDto {
    let unit = b.unit;
    let known = b.closed_episodes_known > 0;
    let status = if !known {
        "n_a"
    } else if w.coverage_complete() && l.closed_episodes_unknown == 0 {
        "observed"
    } else {
        "known_subset"
    };
    QuoteUnitDto {
        unit: quote_unit_label(unit),
        decimals: quote_unit_decimals(unit).unwrap_or(0),
        status,
        closed_known: b.closed_episodes_known,
        wins: b.wins,
        losses: b.losses,
        breakeven: b.breakeven,
        realized_trade_pnl: known
            .then(|| unit_amount(unit, b.realized_trade_pnl_raw, b.realized_trade_pnl_exact)),
        consumed_acquisition_basis: known.then(|| {
            unit_amount(
                unit,
                b.consumed_acquisition_basis_raw,
                b.consumed_acquisition_basis_exact,
            )
        }),
        realized_cost_roi: b
            .roi_parts()
            .and_then(|(n, d)| Ratio::new(n, d))
            .map(|r| RoiDto {
                numerator_exact: format_quote_money(unit, b.realized_trade_pnl_exact)
                    .unwrap_or_default(),
                denominator_exact: format_quote_money(unit, b.consumed_acquisition_basis_exact)
                    .unwrap_or_default(),
                percent_2dp: r.percent_string(2),
            }),
        win_rate: ratio_dto(&b.win_rate, b.closed_episodes_known),
        profit_factor: ratio_dto(&b.profit_factor, b.closed_episodes_known),
        open_episode_known_disposal_pnl: UnitAmountDto {
            raw: b.open_episode_known_disposal_pnl_raw.to_string(),
            decimal: format_quote_money(
                unit,
                Money::from_scaled_units(
                    b.open_episode_known_disposal_pnl_raw
                        .saturating_mul(10i128.pow(MONEY_SCALE)),
                ),
            )
            .unwrap_or_default(),
        },
        open_episode_known_disposals: b.open_episode_known_disposals,
    }
}

fn stats_dto(w: &SolanaWalletStats, l: &SolanaWalletLedgerReport) -> StatsDto {
    let pnl = pnl_view(w);
    let d = &l.diagnostics;
    let a = &l.activity;
    let t = &l.trades;
    StatsDto {
        ledger_version: l.ledger_version,
        quote_unit: "lamports",
        quote_units: l
            .unit_blocks
            .iter()
            .map(|b| quote_unit_dto(w, l, b))
            .collect(),
        route: RouteDto {
            route_swaps: t.route_swaps,
            route_swaps_by_quote: QuoteUnitCountsDto {
                sol: t.route_swaps_by_quote.sol,
                usdc: t.route_swaps_by_quote.usdc,
                usdt: t.route_swaps_by_quote.usdt,
            },
            route_leg_not_wallet_price: t.route_leg_not_wallet_price,
            route_rejected_wallet_not_signer: d.route_rejected.wallet_not_signer,
            route_rejected_multi_asset: d.route_rejected.multi_asset,
            route_rejected_not_opposite_signs: d.route_rejected.not_opposite_signs,
            route_rejected_no_quote_leg: d.route_rejected.no_quote_leg,
            route_rejected_no_verified_leg: d.route_rejected.no_verified_leg,
            route_rejected_passthrough_nonzero: d.route_rejected.passthrough_nonzero,
        },
        realized_net_pnl: MoneyDto {
            status: pnl.status,
            lamports: pnl.lamports.map(|v| v.to_string()),
            sol: pnl.lamports.map(lamports_to_sol_string),
            sol_exact: pnl
                .lamports
                .map(|_| money_exact_sol(l.realized_net_pnl_exact)),
        },
        realized_trade_pnl: amount(l.realized_trade_pnl_lamports),
        realized_trade_pnl_sol_exact: money_exact_sol(l.realized_trade_pnl_exact),
        failed_trade_fees: amount(l.failed_trade_fees_lamports),
        failed_trade_fee_txs: l.failed_trade_fee_txs,
        open_episode_known_disposal_pnl: amount(l.open_episode_known_disposal_pnl_lamports),
        open_episode_known_disposals: l.open_episode_known_disposals,
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
        median_holding_seconds: l.median_holding_seconds,
        holding_time_samples: l.holding_time_samples,
        trades: TradesDto {
            buys: t.buys,
            sells: t.sells,
            priced: t.priced,
            unpaired: t.unpaired,
            mismatched: t.mismatched,
            unsupported_quote: t.unsupported_quote,
            malformed_consideration: t.malformed_consideration,
            quote_funded_elsewhere: t.quote_funded_elsewhere,
            unreconciled: t.unreconciled,
            fixture_verified_variant: t.fixture_verified_variant,
            idl_only_variant: t.idl_only_variant,
            bonding_curve: VenueSidesDto {
                buys: t.bonding_curve.buys,
                sells: t.bonding_curve.sells,
            },
            pump_amm: VenueSidesDto {
                buys: t.pump_amm.buys,
                sells: t.pump_amm.sells,
            },
            route: VenueSidesDto {
                buys: t.route.buys,
                sells: t.route.sells,
            },
            variants: l
                .variant_trades
                .iter()
                .map(|v| VariantTradesDto {
                    venue: v.venue.label(),
                    variant: v.variant,
                    verification: v.verification.label(),
                    trades: v.trades,
                })
                .collect(),
        },
        distinct_mints_traded: l.distinct_mints_traded,
        activity: ActivityDto {
            timestamped_trades: a.timestamped_trades,
            first_trade_timestamp: a.first_trade_timestamp,
            last_trade_timestamp: a.last_trade_timestamp,
            active_span_seconds: a.active_span_seconds,
            active_utc_days: a.active_utc_days,
            distinct_mints_timestamped: a.distinct_mints_timestamped,
            mint_day_pairs: a.mint_day_pairs,
            trades_per_active_day: RationalDto {
                numerator: a.timestamped_trades,
                denominator: a.active_utc_days,
                display_2dp: rational_to_decimal_string(a.timestamped_trades, a.active_utc_days, 2),
            },
        },
        has_unknown_basis_inventory: l.has_unknown_basis_inventory,
        has_left_censored_inventory: l.has_left_censored_inventory,
        open_positions_with_unknown_basis: l.open_positions_with_unknown_basis,
        unknown_basis_lots_created: l.unknown_basis_lots_created,
        diagnostics: DiagnosticsDto {
            transactions_considered: d.transactions_considered,
            duplicate_transactions_ignored: d.duplicate_transactions_ignored,
            failed_transactions: d.failed_transactions,
            malformed_trade_instructions: d.malformed_trade_instructions,
            orphan_trade_events: d.orphan_trade_events,
            unexplained_native_flow: amount(d.unexplained_native_flow_lamports),
            unexplained_native_flow_txs: d.unexplained_native_flow_txs,
            out_of_scope_token_movements: d.out_of_scope_token_movements,
            continuity_breaks: d.continuity_breaks,
            unknown_disposals: d.unknown_disposals,
            known_disposals: d.known_disposals,
            left_censored_disposals: d.left_censored_disposals,
            router_forward_trades_not_attributed: d.router_forward_trades_not_attributed,
            quote_funded_elsewhere_trades: d.quote_funded_elsewhere_trades,
            reversed_pool_trades: d.reversed_pool_trades,
        },
    }
}

fn episode_dto(ep: &EpisodeRecord) -> EpisodeDto {
    let (outcome, pnl) = outcome_parts(&ep.outcome);
    let unit = episode_unit(ep);
    let is_sol = unit == QuoteUnit::Lamports;
    let exact = match &ep.outcome {
        EpisodeOutcome::ClosedKnown { pnl } => Some(money_exact_sol(*pnl)),
        _ => None,
    };
    let known = matches!(ep.outcome, EpisodeOutcome::ClosedKnown { .. });
    EpisodeDto {
        mint: bs58::encode(ep.mint).into_string(),
        outcome,
        pnl: pnl.filter(|_| is_sol).map(amount),
        pnl_sol_exact: exact.filter(|_| is_sol),
        quote_unit: known.then(|| quote_unit_label(unit)),
        pnl_raw: pnl.filter(|_| known).map(|v| v.to_string()),
        pnl_decimal: match &ep.outcome {
            EpisodeOutcome::ClosedKnown { pnl } => format_quote_money(unit, *pnl),
            _ => None,
        },
        opened_at: ep.opened_at,
        closed_at: ep.closed_at,
        holding_seconds: ep.holding_seconds,
        opened_slot: ep.opened_location.0,
        opened_transaction_index: ep.opened_location.1,
        unknown_reasons: ep.unknown_reasons.iter().map(|r| r.label()).collect(),
        known_disposals: ep.known_disposals,
        known_disposal_pnl: is_sol
            .then(|| amount(scout_engine::money_to_lamports_trunc(ep.known_disposal_pnl))),
        known_disposal_pnl_decimal: ep
            .quote_unit
            .and_then(|u| format_quote_money(u, ep.known_disposal_pnl)),
        left_censored_amount_raw: ep.left_censored_amount_raw.to_string(),
    }
}

fn open_dto(p: &OpenPosition) -> OpenPositionDto {
    OpenPositionDto {
        mint: bs58::encode(p.mint).into_string(),
        open_amount_raw: p.open_amount_raw.to_string(),
        unknown_basis_amount_raw: p.unknown_basis_amount_raw.to_string(),
        opened_at: p.opened_at,
    }
}

pub fn wallet_record(
    w: &SolanaWalletStats,
    detail: Detail,
    redact: &dyn Fn(&str) -> String,
) -> WalletStatsRecord {
    let full = detail == Detail::Full;
    WalletStatsRecord {
        schema_version: SCHEMA_VERSION,
        kind: "wallet_stats",
        wallet: wallet_dto(w),
        status: w.status.label(),
        error: w.error.as_deref().map(redact),
        error_kind: w.failure.map(failure_dto),
        stop_reason: w.not_scanned.map(stop_dto),
        coverage: CoverageDto {
            complete: w.coverage_complete(),
            truncated: w.truncated,
            truncation_meaning: "older history unseen (newest-first scan, opening inventory unknown)",
            transactions_scanned: w.transactions_scanned,
            transactions_in_window: w.transactions_in_window,
            unexpected_payloads: w.unexpected_payloads,
            incomplete_reasons: w.incomplete_reasons.iter().map(|r| redact(r)).collect(),
        },
        stats: w.ledger.as_ref().map(|l| stats_dto(w, l)),
        episodes: w
            .ledger
            .as_ref()
            .filter(|_| full)
            .map(|l| l.episodes.iter().map(episode_dto).collect()),
        open_positions: w
            .ledger
            .as_ref()
            .filter(|_| full)
            .map(|l| l.open_positions.iter().map(open_dto).collect()),
    }
}

pub struct RunMetaInput<'a> {
    pub run_id: &'a str,
    pub captured_at: &'a str,
    pub max_pages_per_wallet: u32,
    pub provider_options: scout_providers::HeliusRequestOptions,
    pub server_window: bool,
    pub detail: Detail,
    pub sort: SortMode,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
    pub requests_made: u64,
    pub max_requests: Option<u64>,
    pub window: AnalysisWindow,
}

pub fn run_meta_record(m: &RunMetaInput<'_>) -> RunMetaRecord {
    let scope = SolanaProtocolScope::pump_wallet_ledger();
    RunMetaRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_meta",
        run_id: m.run_id.to_string(),
        captured_at: m.captured_at.to_string(),
        scope: ScopeDto {
            chain: "solana",
            program_id: scope.program_id,
            idl_commit: scope.idl_commit,
            idl_sha256: scope.idl_sha256,
            amm_program_id: scope.amm_program_id,
            amm_idl_commit: scope.amm_idl_commit,
            amm_idl_sha256: scope.amm_idl_sha256,
            qualification_version: scope.qualification_version,
            recognized: scope.recognized,
            not_decoded: scope.not_decoded,
            variants: SolanaProtocolScope::variants()
                .into_iter()
                .map(|(name, side, verification)| VariantDto {
                    name,
                    side,
                    verification,
                })
                .collect(),
        },
        ledger_version: SOLANA_WALLET_LEDGER_VERSION,
        quote_unit: "lamports",
        ledger_scope: SOLANA_WALLET_LEDGER_SCOPE,
        window: window_dto(&m.window),
        scan: ScanDto {
            provider_options: ProviderOptionsDto {
                request: m.provider_options,
                server_window: m.server_window,
                tx_budget_per_wallet: u64::from(m.max_pages_per_wallet)
                    * u64::from(m.provider_options.page_limit),
            },
            provider: "helius",
            order: SCAN_ORDER,
            max_pages_per_wallet: m.max_pages_per_wallet,
            window: if m.window.is_bounded() {
                "newest-first walk until the window start (blockTime < since) or the page budget"
            } else {
                "full available history within the page budget (no time window)"
            },
        },
        detail: if m.detail == Detail::Full {
            "full"
        } else {
            "summary"
        },
        sort: if m.sort == SortMode::Input {
            "input"
        } else {
            "realized-net-pnl"
        },
        input_wallet_count: m.input_wallet_count,
        input_duplicates: m.input_duplicates,
        upstream_complete: m.upstream_complete,
        requests_made: m.requests_made,
        budget: BudgetDto {
            max_requests: m.max_requests,
        },
    }
}

pub fn run_summary_record(
    run_id: &str,
    report: &SolanaWalletStatsReport,
    incomplete: bool,
    requests_made: u64,
    redact: &dyn Fn(&str) -> String,
) -> RunSummaryRecord {
    RunSummaryRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_summary",
        run_id: run_id.to_string(),
        status: if incomplete { "partial" } else { "complete" },
        cancelled: report.cancelled,
        requests_made,
        stop: report.stop.map(stop_dto),
        records: report.wallets.len(),
        incomplete_reasons: report
            .incomplete_reasons()
            .iter()
            .map(|r| redact(r))
            .collect(),
        wallets: report
            .wallets
            .iter()
            .map(|w| WalletStatusDto {
                wallet: wallet_dto(w),
                status: w.status.label(),
            })
            .collect(),
    }
}

/// `run_meta`, `wallet_stats`* (display order), `run_summary`.
pub fn jsonl_lines(
    meta: &RunMetaInput<'_>,
    report: &SolanaWalletStatsReport,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let ser = |r: Result<String, serde_json::Error>| r.map_err(|e| e.to_string());
    let mut lines = Vec::with_capacity(report.wallets.len() + 2);
    lines.push(ser(serde_json::to_string(&run_meta_record(meta)))?);
    for i in display_order(report, meta.sort) {
        if let Some(w) = report.wallets.get(i) {
            lines.push(ser(serde_json::to_string(&wallet_record(
                w,
                meta.detail,
                redact,
            )))?);
        }
    }
    lines.push(ser(serde_json::to_string(&run_summary_record(
        meta.run_id,
        report,
        incomplete,
        meta.requests_made,
        redact,
    )))?);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scout_engine::{WalletScanStatus, build_solana_wallet_ledger, pump_bonding_curve_decoder};
    use serde_json::Value;

    fn card(
        b: u8,
        status: WalletScanStatus,
        txs: Option<u64>,
        net: Option<i128>,
    ) -> SolanaWalletStats {
        let decoder = pump_bonding_curve_decoder().unwrap();
        let mut ledger = (status != WalletScanStatus::Error)
            .then(|| build_solana_wallet_ledger(&[b; 32], &[], &decoder).unwrap());
        if let (Some(l), Some(n)) = (ledger.as_mut(), net) {
            l.closed_episodes_known = 1;
            l.unit_blocks[0].closed_episodes_known = 1;
            l.realized_net_pnl_lamports = n;
        }
        SolanaWalletStats {
            wallet: [b; 32],
            status,
            transactions_scanned: txs,
            transactions_in_window: None,
            truncated: status == WalletScanStatus::Incomplete,
            unexpected_payloads: 0,
            error: (status == WalletScanStatus::Error)
                .then(|| "scan failed at https://h/?api-key=SECRET99".to_string()),
            ledger,
            incomplete_reasons: if status == WalletScanStatus::Incomplete {
                vec!["page budget exhausted".to_string()]
            } else {
                vec![]
            },
            failure: None,
            not_scanned: None,
        }
    }

    fn report() -> SolanaWalletStatsReport {
        SolanaWalletStatsReport {
            scope: SolanaProtocolScope::pump_wallet_ledger(),
            wallets: vec![
                card(1, WalletScanStatus::Ok, Some(4), Some(157_000_000)),
                card(2, WalletScanStatus::NoActivity, Some(0), None),
                card(3, WalletScanStatus::Error, None, None),
                card(4, WalletScanStatus::Incomplete, Some(1000), Some(-5)),
                card(5, WalletScanStatus::Ok, Some(2), Some(900_000_000)),
            ],
            cancelled: false,
            stop: None,
        }
    }

    fn redact(t: &str) -> String {
        t.replace("SECRET99", "<redacted>")
    }

    fn meta(sort: SortMode, detail: Detail) -> RunMetaInput<'static> {
        RunMetaInput {
            run_id: "run-1",
            captured_at: "2026-10-02T12:34:56Z",
            max_pages_per_wallet: 10,
            provider_options: scout_providers::HeliusRequestOptions::default(),
            server_window: false,
            detail,
            sort,
            input_wallet_count: 5,
            input_duplicates: 0,
            upstream_complete: true,
            requests_made: 0,
            max_requests: None,
            window: AnalysisWindow::none(1_790_000_000),
        }
    }

    fn parse(lines: &[String]) -> Vec<Value> {
        lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn table_has_one_row_per_wallet_with_na_never_zero() {
        let lines = table_lines(
            &report(),
            Detail::Summary,
            SortMode::Input,
            &AnalysisWindow::none(0),
        );
        assert_eq!(lines.len(), 6);
        let addr = |b: u8| bs58::encode([b; 32]).into_string();
        assert!(lines[1].starts_with(&addr(1)));
        assert!(lines[1].contains("0.157000000"));
        assert!(lines[2].contains("no_activity") && lines[2].contains("N/A (no activity)"));
        assert!(lines[3].contains("error") && lines[3].contains("N/A (scan failed)"));
        assert!(!lines[3].contains("0.000000000"));
        // Incomplete scan: value only as a labelled known subset.
        assert!(lines[4].contains("N/A (known subset: -0.000000005)"));
        assert!(lines[4].contains("incomplete (truncated)"));
    }

    #[test]
    fn sort_orders_by_net_pnl_desc_na_last_and_keeps_the_set() {
        let r = report();
        let order = display_order(&r, SortMode::RealizedNetPnl);
        // 5 (0.9), 1 (0.157), 4 (-5 known subset), then N/A in input order: 2, 3.
        assert_eq!(order, vec![4, 0, 3, 1, 2]);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
        assert_eq!(display_order(&r, SortMode::Input), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn jsonl_shape_money_strings_and_null_for_unknown() {
        let r = report();
        let lines =
            jsonl_lines(&meta(SortMode::Input, Detail::Summary), &r, true, &redact).unwrap();
        let v = parse(&lines);
        let kinds: Vec<&str> = v.iter().map(|x| x["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "run_meta",
                "wallet_stats",
                "wallet_stats",
                "wallet_stats",
                "wallet_stats",
                "wallet_stats",
                "run_summary"
            ]
        );
        assert!(v.iter().all(|x| x["schema_version"] == 1));
        assert_eq!(v[0]["quote_unit"], "lamports");
        assert_eq!(v[0]["scan"]["order"], "newest_first");
        assert!(
            v[0]["ledger_version"]
                .as_str()
                .unwrap()
                .starts_with("solana-wallet-ledger")
        );
        let ok = &v[1];
        assert_eq!(ok["wallet"]["chain"], "solana");
        assert_eq!(
            ok["wallet"]["address"],
            bs58::encode([1u8; 32]).into_string()
        );
        assert_eq!(ok["stats"]["realized_net_pnl"]["lamports"], "157000000");
        assert_eq!(ok["stats"]["realized_net_pnl"]["sol"], "0.157000000");
        assert_eq!(ok["stats"]["realized_net_pnl"]["status"], "observed");
        assert_eq!(ok["coverage"]["complete"], true);
        assert!(ok.get("episodes").is_none());
        let none = &v[2];
        assert_eq!(none["status"], "no_activity");
        assert!(none["stats"]["realized_net_pnl"]["lamports"].is_null());
        assert_eq!(none["stats"]["realized_net_pnl"]["status"], "n_a");
        assert_eq!(none["coverage"]["transactions_scanned"], 0);
        let err = &v[3];
        assert_eq!(err["status"], "error");
        assert!(err["stats"].is_null());
        assert!(err["coverage"]["transactions_scanned"].is_null());
        assert!(!err.to_string().contains("SECRET99"));
        assert_eq!(v[4]["status"], "incomplete");
        assert_eq!(v[4]["stats"]["realized_net_pnl"]["status"], "known_subset");
        assert_eq!(v[4]["coverage"]["complete"], false);
        assert_eq!(v[4]["stats"]["realized_net_pnl"]["sol"], "-0.000000005");
        let sum = &v[6];
        assert_eq!(sum["status"], "partial");
        assert_eq!(sum["records"], 5);
        assert_eq!(sum["wallets"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn full_detail_adds_episodes_and_open_positions_keys() {
        let r = report();
        let lines = jsonl_lines(&meta(SortMode::Input, Detail::Full), &r, false, &redact).unwrap();
        let v = parse(&lines);
        assert!(v[1]["episodes"].is_array());
        assert!(v[1]["open_positions"].is_array());
        // No ledger (error card): keys absent, not empty.
        assert!(v[3].get("episodes").is_none());
    }

    #[test]
    fn ratios_use_status_not_numbers() {
        let r = report();
        let lines =
            jsonl_lines(&meta(SortMode::Input, Detail::Summary), &r, false, &redact).unwrap();
        let v = parse(&lines);
        assert_eq!(v[1]["stats"]["win_rate"]["status"], "undefined");
        assert!(v[1]["stats"]["win_rate"]["value"].is_null());
        assert!(v[1]["stats"]["activity"]["trades_per_active_day"]["display_2dp"].is_null());
    }

    #[test]
    fn windowed_table_starts_with_the_window_line_and_run_meta_echoes_it() {
        use scout_engine::WindowSource;
        let w = AnalysisWindow {
            since: 1_785_542_400,
            until: 1_788_220_800,
            as_of: 1_790_000_000,
            source: WindowSource::Explicit,
        };
        let lines = table_lines(&report(), Detail::Summary, SortMode::Input, &w);
        assert_eq!(lines.len(), 7);
        assert!(lines[0].starts_with("# window [2026-08-01T00:00:00Z, 2026-09-01T00:00:00Z)"));
        assert!(lines[0].contains("source=explicit"));
        assert!(lines[1].starts_with("wallet"));
        assert!(lines[1].contains("left_censored"));
        let mut m = meta(SortMode::Input, Detail::Summary);
        m.window = w;
        let v = serde_json::to_value(run_meta_record(&m)).unwrap();
        assert_eq!(v["window"]["since"], "2026-08-01T00:00:00Z");
        assert_eq!(v["window"]["until_unix"], 1_788_220_800_i64);
        assert_eq!(v["window"]["source"], "explicit");
        let none =
            serde_json::to_value(run_meta_record(&meta(SortMode::Input, Detail::Summary))).unwrap();
        assert!(none["window"]["since"].is_null());
        assert_eq!(none["window"]["source"], "none");
    }
}
