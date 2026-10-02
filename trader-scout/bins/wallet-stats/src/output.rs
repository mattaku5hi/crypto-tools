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
    EpisodeOutcome, EpisodeRecord, OpenPosition, SOLANA_WALLET_LEDGER_VERSION, SolanaProtocolScope,
    SolanaWalletLedgerReport, SolanaWalletStats, SolanaWalletStatsReport, format_scaled_decimal,
    lamports_to_sol_string, rational_to_decimal_string,
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
            na_reason: Some("scan failed"),
        };
    };
    if l.closed_episodes_known == 0 && l.failed_trade_fees_lamports == 0 {
        return PnlView {
            status: "n_a",
            lamports: None,
            na_reason: Some(if w.transactions_scanned == Some(0) {
                "no activity"
            } else {
                "no known closed episodes"
            }),
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

const HEADER: [&str; 17] = [
    "wallet",
    "status",
    "realized_net_pnl_sol",
    "closed_known/unknown",
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
            let ratios_ok = w.coverage_complete();
            cells.push(format!(
                "{}/{}",
                l.closed_episodes_known, l.closed_episodes_unknown
            ));
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
    let (kind, pnl) = outcome_parts(&ep.outcome);
    let pnl = pnl.map_or_else(|| "N/A".to_string(), lamports_to_sol_string);
    let reasons: Vec<&str> = ep.unknown_reasons.iter().map(|r| r.label()).collect();
    format!(
        "mint={mint} outcome={kind} pnl_sol={pnl} hold_s={} unknown_reasons=[{}]",
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

/// Pnl lamports (truncated from exact money) only for ClosedKnown.
fn outcome_parts(o: &EpisodeOutcome) -> (&'static str, Option<i128>) {
    match o {
        EpisodeOutcome::ClosedKnown { pnl } => (
            "closed_known",
            Some(scout_engine::money_to_lamports_trunc(*pnl)),
        ),
        EpisodeOutcome::ClosedUnknown => ("closed_unknown", None),
        EpisodeOutcome::Open => ("open", None),
    }
}

/// Header + one aligned row per wallet (display order); `full` adds
/// indented evidence lines under each row.
pub fn table_lines(
    report: &SolanaWalletStatsReport,
    detail: Detail,
    sort: SortMode,
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
    let mut out = vec![fmt(&head)];
    for (cells, w) in &rows {
        out.push(fmt(cells));
        if detail == Detail::Full {
            detail_lines(w, &mut out);
        }
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
    pub qualification_version: &'static str,
    pub recognized: &'static str,
    pub not_decoded: &'static str,
    pub variants: Vec<VariantDto>,
}

#[derive(Debug, Serialize)]
pub struct ScanDto {
    pub provider: &'static str,
    pub order: &'static str,
    pub max_pages_per_wallet: u32,
    pub window: &'static str,
}

#[derive(Debug, Serialize)]
pub struct RunMetaRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    pub captured_at: String,
    pub scope: ScopeDto,
    pub ledger_version: &'static str,
    pub quote_unit: &'static str,
    pub scan: ScanDto,
    pub detail: &'static str,
    pub sort: &'static str,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
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
    pub fixture_verified_variant: u64,
    pub idl_only_variant: u64,
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
}

#[derive(Debug, Serialize)]
pub struct StatsDto {
    pub ledger_version: &'static str,
    pub quote_unit: &'static str,
    pub realized_net_pnl: MoneyDto,
    /// SUM A: realized PnL of known closed episodes (before failed-tx fees).
    pub realized_trade_pnl: AmountDto,
    pub realized_trade_pnl_sol_exact: String,
    pub failed_trade_fees: AmountDto,
    pub failed_trade_fee_txs: u64,
    /// SUM B (not in the headline).
    pub open_episode_known_disposal_pnl: AmountDto,
    pub open_episode_known_disposals: u64,
    pub closed_episodes_known: u64,
    pub closed_episodes_unknown: u64,
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
    pub unexpected_payloads: u64,
    pub incomplete_reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EpisodeDto {
    pub mint: String,
    /// `closed_known`, `closed_unknown` or `open`.
    pub outcome: &'static str,
    pub pnl: Option<AmountDto>,
    pub pnl_sol_exact: Option<String>,
    pub opened_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub holding_seconds: Option<i64>,
    pub opened_slot: u64,
    pub opened_transaction_index: u64,
    pub unknown_reasons: Vec<&'static str>,
    pub known_disposals: u64,
    pub known_disposal_pnl: AmountDto,
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

fn stats_dto(w: &SolanaWalletStats, l: &SolanaWalletLedgerReport) -> StatsDto {
    let pnl = pnl_view(w);
    let d = &l.diagnostics;
    let a = &l.activity;
    let t = &l.trades;
    StatsDto {
        ledger_version: l.ledger_version,
        quote_unit: "lamports",
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
            fixture_verified_variant: t.fixture_verified_variant,
            idl_only_variant: t.idl_only_variant,
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
        },
    }
}

fn episode_dto(ep: &EpisodeRecord) -> EpisodeDto {
    let (outcome, pnl) = outcome_parts(&ep.outcome);
    let exact = match &ep.outcome {
        EpisodeOutcome::ClosedKnown { pnl } => Some(money_exact_sol(*pnl)),
        _ => None,
    };
    EpisodeDto {
        mint: bs58::encode(ep.mint).into_string(),
        outcome,
        pnl: pnl.map(amount),
        pnl_sol_exact: exact,
        opened_at: ep.opened_at,
        closed_at: ep.closed_at,
        holding_seconds: ep.holding_seconds,
        opened_slot: ep.opened_location.0,
        opened_transaction_index: ep.opened_location.1,
        unknown_reasons: ep.unknown_reasons.iter().map(|r| r.label()).collect(),
        known_disposals: ep.known_disposals,
        known_disposal_pnl: amount(scout_engine::money_to_lamports_trunc(ep.known_disposal_pnl)),
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
        coverage: CoverageDto {
            complete: w.coverage_complete(),
            truncated: w.truncated,
            truncation_meaning: "older history unseen (newest-first scan, opening inventory unknown)",
            transactions_scanned: w.transactions_scanned,
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
    pub detail: Detail,
    pub sort: SortMode,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
}

pub fn run_meta_record(m: &RunMetaInput<'_>) -> RunMetaRecord {
    let scope = SolanaProtocolScope::pump_bonding_curve();
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
        scan: ScanDto {
            provider: "helius",
            order: SCAN_ORDER,
            max_pages_per_wallet: m.max_pages_per_wallet,
            window: "full available history within the page budget (no time window)",
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
    }
}

pub fn run_summary_record(
    run_id: &str,
    report: &SolanaWalletStatsReport,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> RunSummaryRecord {
    RunSummaryRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_summary",
        run_id: run_id.to_string(),
        status: if incomplete { "partial" } else { "complete" },
        cancelled: report.cancelled,
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
            l.realized_net_pnl_lamports = n;
        }
        SolanaWalletStats {
            wallet: [b; 32],
            status,
            transactions_scanned: txs,
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
        }
    }

    fn report() -> SolanaWalletStatsReport {
        SolanaWalletStatsReport {
            scope: SolanaProtocolScope::pump_bonding_curve(),
            wallets: vec![
                card(1, WalletScanStatus::Ok, Some(4), Some(157_000_000)),
                card(2, WalletScanStatus::NoActivity, Some(0), None),
                card(3, WalletScanStatus::Error, None, None),
                card(4, WalletScanStatus::Incomplete, Some(1000), Some(-5)),
                card(5, WalletScanStatus::Ok, Some(2), Some(900_000_000)),
            ],
            cancelled: false,
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
            detail,
            sort,
            input_wallet_count: 5,
            input_duplicates: 0,
            upstream_complete: true,
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
        let lines = table_lines(&report(), Detail::Summary, SortMode::Input);
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
}
