//! Output layer for `wallet-stats` (docs/CLI.md §5, §7).
//!
//! DTOs are built field-by-field from engine reports; internal types are
//! never serialized. Money convention (CLI.md §7): lamports and SOL are
//! decimal STRINGS (exact, integer arithmetic only); counts are JSON
//! numbers; unknown is `null` with an explicit status, never `0`.

use std::collections::BTreeMap;

use scout_analytics::RatioStatus;
use scout_app::SCHEMA_VERSION;
use scout_app::{PriceCoverageDto, PricingMetaDto};
use scout_core::MONEY_SCALE;
use scout_core::Money;
use scout_engine::{
    AnalysisWindow, ChainDisplay, EpisodeOutcome, EpisodePnlBound, EpisodeRecord, LowerBound,
    OpenPosition, QuoteUnit, QuoteUnitBlock, Ratio, SOLANA_DISPLAY, SOLANA_WALLET_LEDGER_SCOPE,
    SOLANA_WALLET_LEDGER_VERSION, ScanFailureKind, ScanStop, SolanaProtocolScope,
    SolanaWalletLedgerReport, SolanaWalletStats, SolanaWalletStatsReport, UsdEpisode,
    UsdLedgerView, UsdNetStatus, UsdOutcome, format_quote_money, format_scaled_decimal,
    lamports_to_sol_string, quote_unit_decimals, quote_unit_label, rational_to_decimal_string,
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
/// `known_subset_unbounded` (ADR-016): as `known_subset`, and an unknown
/// episode has no worst-case bound (see the lower-bound fields).
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
        .unit_block(w.chain.native_unit)
        .map_or(l.closed_episodes_known, |b| b.closed_episodes_known);
    if sol_closed == 0 && l.failed_trade_fees_lamports == 0 {
        return PnlView {
            status: "n_a",
            lamports: None,
            na_reason: Some(
                if w.transactions_in_window.or(w.transactions_scanned) == Some(0) {
                    "no activity"
                } else if w.chain.is_evm() {
                    "no known closed native episodes"
                } else {
                    "no known closed SOL episodes"
                },
            ),
        };
    }
    let observed = w.coverage_complete() && l.closed_episodes_unknown == 0;
    let unbounded = l
        .unit_block(w.chain.native_unit)
        .is_some_and(|b| b.unknown_pnl_bound == LowerBound::Unbounded);
    PnlView {
        status: if observed {
            "observed"
        } else if unbounded {
            "known_subset_unbounded"
        } else {
            "known_subset"
        },
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

/// Decimals of the chain's native unit (SOL 9, ETH 18).
fn native_decimals(c: &ChainDisplay) -> u32 {
    quote_unit_decimals(c.native_unit).unwrap_or(9)
}

/// Exact decimal of raw native base units (lamports / wei).
fn native_str(c: &ChainDisplay, raw: i128) -> String {
    format_scaled_decimal(raw, native_decimals(c))
}

/// Exact native amount of a base-unit-scaled `Money` (sub-unit remainder kept).
fn money_exact_native(c: &ChainDisplay, m: Money) -> String {
    format_scaled_decimal(m.scaled_units(), MONEY_SCALE + native_decimals(c))
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

const HEADER: [&str; 33] = [
    "wallet",
    "status",
    "realized_net_pnl_sol",
    "realized_pnl_usdc",
    "realized_pnl_usdt",
    "realized_pnl_usd",
    "realized_net_pnl_usd",
    "route_swaps",
    "closed_known/unknown",
    "left_censored",
    "open",
    "open_valued",
    "open_realizable_sol",
    "open_unrealized_sol",
    "open_value_usd",
    "open_unrealized_usd",
    "W/L/BE",
    "win_rate",
    "profit_factor",
    "win_rate_lower_bound",
    "pnl_lower_bound_sol",
    "pnl_lower_bound_usd",
    "unknown_share",
    "median_hold_s",
    "trades",
    "mints",
    "active_days",
    "trades/day",
    "failed_fees_sol",
    "unknown_basis",
    "unexplained_native_sol",
    "usd_price_coverage",
    "coverage",
];

/// Table header of a chain: the Solana const unchanged; EVM renames the
/// native column and replaces the USDC/USDT columns by the chain's own
/// non-native quote units.
fn header_for(chain: &ChainDisplay) -> Vec<String> {
    if !chain.is_evm() {
        return HEADER.iter().map(|s| (*s).to_string()).collect();
    }
    let rename = |h: &str| -> String {
        h.split('_')
            .map(|seg| {
                if seg == "sol" {
                    chain.native_label
                } else {
                    seg
                }
            })
            .collect::<Vec<_>>()
            .join("_")
    };
    let mut out: Vec<String> = HEADER.iter().take(3).map(|h| rename(h)).collect();
    for u in chain.quote_units().iter().skip(1) {
        out.push(format!("realized_pnl_{}", quote_unit_label(*u)));
    }
    out.extend(HEADER.iter().skip(5).map(|h| rename(h)));
    out
}

fn na(reason: &str) -> String {
    format!("N/A ({reason})")
}

fn row(w: &SolanaWalletStats) -> Vec<String> {
    let addr = w.chain.address(&w.wallet);
    let pnl = pnl_view(w);
    let n = |l: i128| native_str(&w.chain, l);
    let pnl_cell = match (pnl.status, pnl.lamports) {
        ("observed", Some(l)) => n(l),
        ("known_subset", Some(l)) => format!("N/A (known subset: {})", n(l)),
        ("known_subset_unbounded", Some(l)) => {
            format!("N/A (known subset, unbounded worst case: {})", n(l))
        }
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
            for _ in 3..header_for(&w.chain).len() - 1 {
                cells.push(na("scan failed"));
            }
        }
        Some(l) => {
            for unit in w.chain.quote_units().iter().skip(1) {
                cells.push(unit_pnl_cell(w, l, *unit));
            }
            cells.push(usd_pnl_cell(w, l));
            cells.push(usd_net_cell(l));
            cells.push(l.trades.route_swaps.to_string());
            let ratios_ok = w.coverage_complete();
            cells.push(format!(
                "{}/{}",
                l.closed_episodes_known, l.closed_episodes_unknown
            ));
            cells.push(l.left_censored_episodes.to_string());
            cells.push(l.open_episodes.to_string());
            cells.extend(open_valuation_cells(l));
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
            cells.push(win_rate_lb_cell(l));
            cells.push(pnl_lb_cell(l));
            cells.push(usd_pnl_lb_cell(l));
            cells.push(share_cell(l));
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
            cells.push(native_str(&l.chain, l.failed_trade_fees_lamports));
            cells.push(
                if l.has_unknown_basis_inventory {
                    "yes"
                } else {
                    "no"
                }
                .to_string(),
            );
            cells.push(native_str(
                &l.chain,
                l.diagnostics.unexplained_native_flow_lamports,
            ));
            cells.push(usd_coverage_cell(l));
        }
    }
    cells.push(coverage);
    cells
}

/// ADR-019 cells: `valued/positions`, Σ realizable SOL, Σ known unrealized
/// SOL (each `N/A (reason)` when nothing is known; never zero for unknown).
fn open_valuation_cells(l: &SolanaWalletLedgerReport) -> [String; 5] {
    if l.open_positions.is_empty() {
        return [
            "0/0".to_string(),
            native_str(&l.chain, 0),
            native_str(&l.chain, 0),
            "0.00000000".to_string(),
            "0.00000000".to_string(),
        ];
    }
    if let Some(ev) = l.evm.as_ref().and_then(|e| e.open_valuation.as_ref()) {
        return evm_open_valuation_cells(l, ev);
    }
    let Some(v) = &l.open_valuation else {
        let r = na("valuation not run");
        return [r.clone(), r.clone(), r.clone(), r.clone(), r];
    };
    let t = v.totals();
    let realizable = if t.valued == 0 {
        let reason = t
            .unvalued_by_reason
            .iter()
            .next()
            .map_or("unvalued", |(k, _)| *k);
        na(reason)
    } else if t.unvalued > 0 {
        format!(
            "N/A (partial: {} of {} valued, {} SOL)",
            t.valued,
            t.positions,
            lamports_to_sol_string(i128::try_from(t.realizable_lamports).unwrap_or(i128::MAX))
        )
    } else {
        lamports_to_sol_string(i128::try_from(t.realizable_lamports).unwrap_or(i128::MAX))
    };
    let unrealized = if t.unrealized_known_positions == 0 {
        na("no known-basis valued position")
    } else if t.unrealized_unknown_positions > 0 || t.unvalued > 0 {
        format!(
            "N/A (known subset: {})",
            money_exact_sol(Money::from_scaled_units(t.unrealized_known_scaled))
        )
    } else {
        money_exact_sol(Money::from_scaled_units(t.unrealized_known_scaled))
    };
    let value_usd = if t.usd_priced_positions == 0 {
        na(USD_NOT_PRICED)
    } else if t.usd_priced_positions < t.positions {
        format!(
            "N/A (known subset: {})",
            money_str(Money::from_scaled_units(t.usd_value_scaled))
        )
    } else {
        money_str(Money::from_scaled_units(t.usd_value_scaled))
    };
    let unrealized_usd = if t.usd_unrealized_known_positions == 0 {
        na("no known USD unrealized position")
    } else if t.usd_unrealized_unknown_positions > 0 || t.unvalued > 0 {
        format!(
            "N/A (known subset: {})",
            money_str(Money::from_scaled_units(t.usd_unrealized_known_scaled))
        )
    } else {
        money_str(Money::from_scaled_units(t.usd_unrealized_known_scaled))
    };
    [
        format!("{}/{}", t.valued, t.positions),
        realizable,
        unrealized,
        value_usd,
        unrealized_usd,
    ]
}

/// `1.5 ETH + 20.00 USDG` of raw per-unit amounts (never summed across units).
fn units_text(c: &ChainDisplay, by_unit: &BTreeMap<QuoteUnit, i128>, scale_extra: u32) -> String {
    by_unit
        .iter()
        .map(|(u, raw)| {
            let d = quote_unit_decimals(*u).unwrap_or(18);
            let sym = if *u == QuoteUnit::Wei {
                c.native_symbol.to_string()
            } else {
                quote_unit_label(*u).to_uppercase()
            };
            format!("{} {sym}", format_scaled_decimal(*raw, d + scale_extra))
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// ADR-019 EVM amendment cells (same five columns as the Solana ones).
fn evm_open_valuation_cells(
    l: &SolanaWalletLedgerReport,
    v: &scout_engine::EvmOpenValuationView,
) -> [String; 5] {
    let t = v.totals();
    let mut real: BTreeMap<QuoteUnit, i128> = BTreeMap::new();
    let mut unreal: BTreeMap<QuoteUnit, i128> = BTreeMap::new();
    for p in v.positions.iter().filter_map(|p| p.valued()) {
        *real.entry(p.quote_unit).or_insert(0) = real
            .get(&p.quote_unit)
            .copied()
            .unwrap_or(0)
            .saturating_add(i128::try_from(p.realizable_raw).unwrap_or(i128::MAX));
        if let Some(m) = p.unrealized_pnl {
            let e = unreal.entry(p.quote_unit).or_insert(0);
            *e = e.saturating_add(m.scaled_units());
        }
    }
    let realizable = if t.valued == 0 {
        na(t.unvalued_by_reason
            .iter()
            .next()
            .map_or("unvalued", |(k, _)| *k))
    } else if t.unvalued > 0 {
        format!(
            "N/A (partial: {} of {} valued, {})",
            t.valued,
            t.positions,
            units_text(&l.chain, &real, 0)
        )
    } else {
        units_text(&l.chain, &real, 0)
    };
    let unrealized = if t.unrealized_known_positions == 0 {
        na("no known-basis valued position")
    } else if t.unrealized_unknown_positions > 0 || t.unvalued > 0 {
        format!(
            "N/A (known subset: {})",
            units_text(&l.chain, &unreal, MONEY_SCALE)
        )
    } else {
        units_text(&l.chain, &unreal, MONEY_SCALE)
    };
    let value_usd = if t.usd_priced_positions == 0 {
        na(USD_NOT_PRICED)
    } else if t.usd_priced_positions < t.positions {
        format!(
            "N/A (known subset: {})",
            money_str(Money::from_scaled_units(t.usd_value_scaled))
        )
    } else {
        money_str(Money::from_scaled_units(t.usd_value_scaled))
    };
    let unrealized_usd = if t.usd_unrealized_known_positions == 0 {
        na("no known USD unrealized position")
    } else if t.usd_unrealized_unknown_positions > 0 || t.unvalued > 0 {
        format!(
            "N/A (known subset: {})",
            money_str(Money::from_scaled_units(t.usd_unrealized_known_scaled))
        )
    } else {
        money_str(Money::from_scaled_units(t.usd_unrealized_known_scaled))
    };
    [
        format!("{}/{}", t.valued, t.positions),
        realizable,
        unrealized,
        value_usd,
        unrealized_usd,
    ]
}

/// ADR-016: `wins / (known + unknown)` as an exact fraction and percent.
fn win_rate_lb_cell(l: &SolanaWalletLedgerReport) -> String {
    match l.win_rate_lower_bound {
        None => na("no closed known/unknown episodes"),
        Some(w) => {
            let pct = Ratio::new(i128::from(w.wins), i128::from(w.episodes))
                .and_then(|r| r.percent_string(2))
                .unwrap_or_default();
            format!("{}/{} ({pct}%)", w.wins, w.episodes)
        }
    }
}

/// ADR-016: SOL worst-case net PnL (known + unknown-episode bounds - failed fees).
fn pnl_lb_cell(l: &SolanaWalletLedgerReport) -> String {
    let Some(b) = l.unit_block(l.quote_unit) else {
        return na(&format!("no {} block", l.chain.native_symbol));
    };
    match b.realized_pnl_lower_bound() {
        LowerBound::Unbounded => "unbounded".to_string(),
        LowerBound::Bounded(m) => {
            if b.closed_episodes_known == 0 && m.is_zero() && l.failed_trade_fees_lamports == 0 {
                return na(&format!(
                    "no known closed {} episodes",
                    l.chain.native_symbol
                ));
            }
            match scout_engine::quote_units_to_money(l.quote_unit, l.failed_trade_fees_lamports)
                .ok()
                .and_then(|f| m.checked_sub(&f).ok())
            {
                Some(net) => format!(">= {}", money_exact_native(&l.chain, net)),
                None => "unbounded".to_string(),
            }
        }
    }
}

const USD_NOT_PRICED: &str = "usd not priced";

/// ADR-018 realized USD PnL of the known closed episodes (exact, 8 dp);
/// labelled when it is a known subset, N/A without a known USD episode.
fn usd_pnl_cell(w: &SolanaWalletStats, l: &SolanaWalletLedgerReport) -> String {
    let Some(u) = &l.usd else {
        return na(USD_NOT_PRICED);
    };
    if u.block.closed_episodes_known == 0 {
        return na("no known closed USD episodes");
    }
    let v = money_str(u.block.realized_trade_pnl_exact);
    if usd_observed(w, u) {
        v
    } else {
        format!("N/A (known subset: {v})")
    }
}

/// ADR-004 in USD: realized trade PnL minus priced failed-tx fees.
fn usd_net_cell(l: &SolanaWalletLedgerReport) -> String {
    let Some(u) = &l.usd else {
        return na(USD_NOT_PRICED);
    };
    match (u.net.status, u.net.value) {
        (UsdNetStatus::Known, Some(v)) => money_str(v),
        (UsdNetStatus::KnownSubset, Some(v)) => format!(
            "N/A (known subset: {v}; {} failed fees unpriced)",
            u.failed_fees.unpriced_txs,
            v = money_str(v)
        ),
        _ => na("failed fees unpriced"),
    }
}

/// `observed`: complete coverage, no unknown USD episode, every leg priced.
fn usd_observed(w: &SolanaWalletStats, u: &UsdLedgerView) -> bool {
    w.coverage_complete() && u.closed_episodes_unknown == 0 && u.coverage.unpriced == 0
}

/// ADR-016 in USD: worst-case realized PnL (known + unknown-episode bounds).
fn usd_pnl_lb_cell(l: &SolanaWalletLedgerReport) -> String {
    let Some(u) = &l.usd else {
        return na(USD_NOT_PRICED);
    };
    match u.block.realized_pnl_lower_bound() {
        LowerBound::Unbounded => "unbounded".to_string(),
        LowerBound::Bounded(m) => {
            if u.block.closed_episodes_known == 0 && m.is_zero() {
                na("no known closed USD episodes")
            } else {
                format!(">= {}", money_str(m))
            }
        }
    }
}

/// ADR-018: `priced/legs (pct%)`, plus the USDC-par leg count when non-zero.
fn usd_coverage_cell(l: &SolanaWalletLedgerReport) -> String {
    let Some(u) = &l.usd else {
        return na(USD_NOT_PRICED);
    };
    let c = PriceCoverageDto::from_coverage(&u.coverage);
    let Some(pct) = c.priced_percent_2dp else {
        return "0/0 (no priced legs)".to_string();
    };
    let mut par = if c.usdc_par_legs > 0 {
        format!(" usdc_par={}", c.usdc_par_legs)
    } else {
        String::new()
    };
    if c.binance_peg_legs > 0 {
        par.push_str(&format!(" binance_peg={}", c.binance_peg_legs));
    }
    format!("{}/{} ({pct}%){par}", c.priced, c.legs)
}

/// ADR-016: `closed_unknown / (closed_known + closed_unknown)`.
fn share_cell(l: &SolanaWalletLedgerReport) -> String {
    let (u, t) = l.unknown_episode_share_parts();
    match Ratio::new(i128::from(u), i128::from(t)).and_then(|r| r.percent_string(2)) {
        Some(p) => format!("{u}/{t} ({p}%)"),
        None => na("no closed known/unknown episodes"),
    }
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
    for (i, ep) in l.episodes.iter().enumerate() {
        let usd_ep = l.usd.as_ref().and_then(|u| u.episodes.get(i));
        out.push(format!(
            "    episode {}",
            episode_text(ep, usd_ep, &l.chain)
        ));
    }
    let dtos = scout_app::open_positions_dto(l);
    for (p, d) in l.open_positions.iter().zip(&dtos) {
        out.push(format!(
            "    open {}{}",
            open_text(p, &l.chain),
            open_valuation_text(d)
        ));
    }
}

/// ADR-019 suffix of an open-position line.
fn open_valuation_text(d: &scout_app::OpenPositionDto) -> String {
    let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "N/A".to_string());
    if d.status != "valued" {
        return format!(
            " valuation=unvalued reason={}",
            d.unvalued_reason.unwrap_or("unknown")
        );
    }
    if let Some(e) = &d.evm {
        return format!(
            " valuation={} venue={} method={} quote_unit={} realizable_raw={} price_impact_bps={} \
             unrealized_pnl_raw={} value_usd={} state_block={}{}",
            d.label.unwrap_or("-"),
            d.venue.unwrap_or("-"),
            e.quote_method.unwrap_or("-"),
            e.quote_unit.clone().unwrap_or_else(|| "-".to_string()),
            e.realizable_native_raw
                .clone()
                .or_else(|| e.realizable_quote_raw.clone())
                .unwrap_or_else(|| "N/A".to_string()),
            d.price_impact_bps
                .map_or_else(|| "N/A".to_string(), |b| b.to_string()),
            e.unrealized_pnl_raw.clone().unwrap_or_else(|| format!(
                "N/A ({})",
                e.unrealized_status.unwrap_or("unknown_basis")
            )),
            d.value_usd
                .clone()
                .or_else(|| d.usd_unpriced_reason.as_ref().map(|r| format!("N/A ({r})")))
                .unwrap_or_else(|| "N/A".to_string()),
            e.state_block
                .map_or_else(|| "N/A".to_string(), |b| b.to_string()),
            e.caveat
                .map_or_else(String::new, |c| format!(" caveat={c}")),
        );
    }
    let fee = d.fee_bps.as_ref().map_or_else(String::new, |f| {
        format!(
            " fee_bps={}{}",
            f.total,
            f.observed_at_unix
                .map_or_else(String::new, |t| format!("@{}", rfc3339(t)))
        )
    });
    format!(
        " valuation={} venue={} realizable_lamports={} marginal_lamports={} price_impact_bps={} \
         unrealized_pnl_lamports={} value_usd={} slot={}{fee}",
        d.label.unwrap_or("-"),
        d.venue.unwrap_or("-"),
        opt(&d.realizable_lamports),
        opt(&d.marginal_lamports),
        d.price_impact_bps
            .map_or_else(|| "N/A".to_string(), |b| b.to_string()),
        d.unrealized_pnl_lamports
            .clone()
            .unwrap_or_else(|| "N/A (unknown basis)".to_string()),
        d.value_usd
            .clone()
            .or_else(|| d.usd_unpriced_reason.as_ref().map(|r| format!("N/A ({r})")))
            .unwrap_or_else(|| "N/A".to_string()),
        d.vault_slot
            .or(d.account_slot)
            .map_or_else(|| "N/A".to_string(), |s| s.to_string()),
    )
}

fn episode_text(ep: &EpisodeRecord, usd_ep: Option<&UsdEpisode>, chain: &ChainDisplay) -> String {
    let mint = chain.address(&ep.mint);
    let (kind, _) = outcome_parts(&ep.outcome);
    let unit = episode_unit(ep, chain);
    let pnl = match &ep.outcome {
        EpisodeOutcome::ClosedKnown { pnl } => {
            format_quote_money(unit, *pnl).unwrap_or_else(|| "N/A".to_string())
        }
        _ => "N/A".to_string(),
    };
    let reasons: Vec<&str> = ep.unknown_reasons.iter().map(|r| r.label()).collect();
    let usd = usd_ep.map_or_else(String::new, |u| {
        let v = match u.outcome {
            UsdOutcome::ClosedKnown { pnl, .. } => money_str(pnl),
            _ => "N/A".to_string(),
        };
        let b = match u.unknown_pnl_bound {
            Some(LowerBound::Bounded(m)) => format!(" usd_pnl_lower_bound={}", money_str(m)),
            Some(LowerBound::Unbounded) => " usd_pnl_lower_bound=unbounded".to_string(),
            None => String::new(),
        };
        format!(" usd_outcome={} pnl_usd={v}{b}", u.outcome.label())
    });
    let bound = match ep.unknown_pnl_bound {
        Some(EpisodePnlBound::Bounded { unit, lower_bound }) => format!(
            " pnl_lower_bound_{}={}",
            quote_unit_label(unit),
            format_quote_money(unit, lower_bound).unwrap_or_else(|| "N/A".to_string())
        ),
        Some(EpisodePnlBound::Unbounded) => " pnl_lower_bound=unbounded".to_string(),
        None => String::new(),
    };
    format!(
        "mint={mint} outcome={kind} pnl_{}={pnl} hold_s={} consumed_basis={} unknown_reasons=[{}]{bound}{usd}",
        quote_unit_label(unit),
        ep.holding_seconds
            .map_or_else(|| "N/A".to_string(), |s| s.to_string()),
        ep.consumed_basis_status.label(),
        reasons.join("; ")
    )
}

fn open_text(p: &OpenPosition, chain: &ChainDisplay) -> String {
    format!(
        "mint={} amount_raw={} unknown_basis_amount_raw={}",
        chain.address(&p.mint),
        p.open_amount_raw,
        p.unknown_basis_amount_raw
    )
}

/// Quote unit of an episode's known figures (SOL when it has none).
fn episode_unit(ep: &EpisodeRecord, chain: &ChainDisplay) -> QuoteUnit {
    ep.quote_unit.unwrap_or(chain.native_unit)
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
    let chain = report.wallets.first().map_or(SOLANA_DISPLAY, |w| w.chain);
    let header = header_for(&chain);
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
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
    let head: Vec<String> = header;
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
    /// Max wallets scanned at once (`--concurrency`).
    pub concurrency: usize,
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
    /// HTTP attempts for USD price candles (separate from `requests_made`).
    pub requests_made_prices: u64,
    pub pricing: PricingMetaDto,
    /// ADR-019 valuation of open positions: slot, as_of, policy, totals.
    pub open_valuation: scout_app::OpenValuationMetaDto,
}

/// Run request budget (`--max-requests`; `null` = unlimited).
#[derive(Debug, Serialize)]
pub struct BudgetDto {
    pub max_requests: Option<u64>,
    /// `--max-price-requests` (`null` = unlimited).
    pub max_price_requests: Option<u64>,
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
    /// Instructions/events of a decoded program with an unknown discriminator.
    pub unknown_discriminator_instructions: u64,
    /// ADR-015: Jupiter event-CPIs not decoded exactly / unknown (never evidence).
    pub jupiter_malformed_events: u64,
    pub jupiter_unknown_events: u64,
    pub dflow_malformed_events: u64,
    pub dflow_unknown_events: u64,
    /// ADR-017 draft: OKX DEX Router event coverage (never evidence when malformed/unknown).
    pub okx_malformed_events: u64,
    pub okx_unknown_events: u64,
    /// Order events with a distinct receiver: attributed to nobody.
    pub okx_swap_with_receiver_not_attributed: u64,
    /// Order events of an IdlOnly variant: counted, not leg evidence.
    pub okx_idl_only_order_events: u64,
    /// ADR-013 section 2b: direct-venue swap-event coverage (malformed/unknown
    /// are gaps; idl_only/unresolved informational).
    pub venue_events: scout_app::VenueEventsDto,
    /// At most 5 samples (canonical chain order) of malformed trade
    /// instructions, unknown discriminators and orphan events.
    pub evidence_samples: Vec<scout_app::DecodeEvidenceDto>,
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
    /// ADR-016: worst-case PnL (known + unknown-episode bounds), `unbounded`
    /// if an unknown episode has no bound.
    pub realized_pnl_lower_bound: BoundDto,
    pub profit_factor_lower_bound: RatioBoundDto,
}

/// ADR-016: a lower bound that may not exist (`bounded`, `unbounded`, `n_a`).
#[derive(Debug, Serialize)]
pub struct BoundDto {
    pub status: &'static str,
    pub unit: &'static str,
    pub raw: Option<String>,
    pub decimal: Option<String>,
}

/// ADR-016: ratio lower bound (`value`, `no_observed_losses`, `undefined`, `unbounded`).
#[derive(Debug, Serialize)]
pub struct RatioBoundDto {
    pub status: &'static str,
    pub value: Option<String>,
}

/// ADR-016: exact `wins / (closed_known + closed_unknown)`.
#[derive(Debug, Serialize)]
pub struct WinRateBoundDto {
    pub wins: u64,
    pub episodes: u64,
    pub percent_2dp: Option<String>,
}

/// ADR-016: effective unknown-episode share (exact counts).
#[derive(Debug, Serialize)]
pub struct UnknownShareDto {
    pub closed_unknown: u64,
    pub closed_known_and_unknown: u64,
    pub percent_2dp: Option<String>,
}

/// ADR-016: worst-case PnL of one `closed_unknown` episode.
#[derive(Debug, Serialize)]
pub struct EpisodeBoundDto {
    /// `bounded` or `unbounded`.
    pub status: &'static str,
    pub unit: Option<&'static str>,
    pub raw: Option<String>,
    pub decimal: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QuoteUnitCountsDto {
    /// Native count (SOL; on EVM chains the native/wei count, renamed by the
    /// EVM spelling pass).
    pub sol: u64,
    pub usdc: u64,
    pub usdt: u64,
    /// EVM only (USDG); omitted when zero so Solana output is unchanged.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub usdg: u64,
    /// BSC only (Binance-Peg, 18 dp, ADR-020 amendment 6); omitted when zero.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub usdt_peg: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub usdc_peg: u64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Serialize)]
pub struct RouteEvidenceDto {
    pub curve: u64,
    pub pump_amm: u64,
    pub jupiter: u64,
    /// Booked only because of a Jupiter leg.
    pub jupiter_only: u64,
    pub dflow: u64,
    /// Booked only because of a DFlow leg.
    pub dflow_only: u64,
    /// OKX DEX Router order-event legs (ADR-017 draft).
    pub okx: u64,
    /// Booked only because of an OKX leg.
    pub okx_only: u64,
    /// OKX swaps whose order event names the wallet itself as owner.
    pub okx_owner_is_wallet: u64,
    /// ADR-013 section 2b: direct-venue swap-event legs (non-exclusive).
    pub whirlpool: u64,
    pub dlmm: u64,
    pub raydium_clmm: u64,
    pub raydium_cpmm: u64,
    /// Booked only because of a direct-venue leg.
    pub venue_only: u64,
}

/// ADR-013 §1/§2 counters.
#[derive(Debug, Serialize)]
pub struct RouteDto {
    pub route_swaps: u64,
    pub route_swaps_by_quote: QuoteUnitCountsDto,
    /// ADR-015: route swaps by swap-leg evidence source (non-exclusive).
    pub route_swaps_by_evidence: RouteEvidenceDto,
    pub route_leg_not_wallet_price: u64,
    pub route_rejected_wallet_not_signer: u64,
    pub route_rejected_multi_asset: u64,
    pub route_rejected_not_opposite_signs: u64,
    pub route_rejected_no_quote_leg: u64,
    pub route_rejected_no_verified_leg: u64,
    pub route_rejected_passthrough_nonzero: u64,
}

/// ADR-018: figures of the USD view (`Money` at 8 dp; open positions are
/// never valued here). Unknown is `null`/a status, never zero.
#[derive(Debug, Serialize)]
pub struct UsdStatsDto {
    pub version: &'static str,
    /// `observed`, `known_subset` or `n_a` (no known closed USD episode).
    pub status: &'static str,
    pub closed_known: u64,
    pub closed_unknown: u64,
    pub left_censored: u64,
    pub open_unvalued: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakeven: u64,
    /// Exact USD, decimal string at 8 dp; `raw` is the 1e-8 USD integer.
    pub realized_trade_pnl: Option<UnitAmountDto>,
    pub consumed_acquisition_basis: Option<UnitAmountDto>,
    pub realized_cost_roi: Option<RoiDto>,
    pub win_rate: RatioDto,
    pub profit_factor: RatioDto,
    /// ADR-016 in USD.
    pub realized_pnl_lower_bound: BoundDto,
    pub profit_factor_lower_bound: RatioBoundDto,
    pub win_rate_lower_bound: Option<WinRateBoundDto>,
    pub unknown_episode_share: UnknownShareDto,
    pub price_coverage: PriceCoverageDto,
    /// ADR-004 in USD: failed-tx fees priced at their block time.
    pub failed_trade_fees: UsdFailedFeesDto,
    /// `realized_trade_pnl - failed_trade_fees` (USD).
    pub realized_net_pnl: UsdNetDto,
}

#[derive(Debug, Serialize)]
pub struct UsdFailedFeesDto {
    /// Σ priced fees, 8 dp (`null` when none priced).
    pub priced: Option<String>,
    pub priced_txs: u64,
    pub unpriced_txs: u64,
    pub unpriced_by_reason: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Serialize)]
pub struct UsdNetDto {
    /// `known`, `known_subset` (value = realized - PRICED fees only) or `unknown`.
    pub status: &'static str,
    pub raw: Option<String>,
    pub decimal: Option<String>,
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
    /// ADR-018 USD view; `null` with `--no-usd` or when pricing failed.
    pub usd: Option<UsdStatsDto>,
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
    /// ADR-016 (SOL block): worst-case net PnL, profit factor and the
    /// all-unit win rate with every unknown episode counted as a loss.
    pub realized_net_pnl_lower_bound: BoundDto,
    pub profit_factor_lower_bound: RatioBoundDto,
    pub win_rate_lower_bound: Option<WinRateBoundDto>,
    pub unknown_episode_share: UnknownShareDto,
    pub median_holding_seconds: Option<i64>,
    pub holding_time_samples: u64,
    pub trades: TradesDto,
    pub distinct_mints_traded: u64,
    pub activity: ActivityDto,
    pub has_unknown_basis_inventory: bool,
    pub has_left_censored_inventory: bool,
    /// ADR-019 totals over the open positions (`null` when valuation did
    /// not run or there are no open positions); positions: `--detail full`.
    pub open_valuation: Option<scout_app::OpenValuationTotalsDto>,
    /// ADR-019 EVM amendment totals by quote unit (EVM runs only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evm_open_valuation: Option<scout_app::EvmOpenValuationTotalsDto>,
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
    /// ADR-016: `known` or `partially_unknown`.
    pub consumed_basis_status: &'static str,
    /// ADR-016: `closed_unknown` only; `null` otherwise.
    pub unknown_pnl_lower_bound: Option<EpisodeBoundDto>,
    /// ADR-018 USD view of the episode; omitted without a USD view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd: Option<EpisodeUsdDto>,
}

/// ADR-018: one episode in USD.
#[derive(Debug, Serialize)]
pub struct EpisodeUsdDto {
    /// `closed_known`, `closed_unknown`, `left_censored` or `open`.
    pub outcome: &'static str,
    /// Exact USD (8 dp), `closed_known` only.
    pub pnl_decimal: Option<String>,
    pub consumed_basis_decimal: Option<String>,
    /// Native causes (other than cross-quote) plus unpriced-leg reasons.
    pub unknown_reasons: Vec<String>,
    /// ADR-016 in USD; `closed_unknown` only.
    pub unknown_pnl_lower_bound: Option<EpisodeBoundDto>,
    pub legs_priced: u64,
    pub legs_unpriced: u64,
}

fn episode_usd_dto(u: &UsdEpisode) -> EpisodeUsdDto {
    let (pnl, basis) = match u.outcome {
        UsdOutcome::ClosedKnown {
            pnl,
            consumed_basis,
        } => (Some(money_str(pnl)), Some(money_str(consumed_basis))),
        _ => (None, None),
    };
    let mut reasons: Vec<String> = u
        .native_unknown_reasons
        .iter()
        .map(|r| r.label().to_string())
        .collect();
    reasons.extend(u.price_unknown_reasons.iter().cloned());
    EpisodeUsdDto {
        outcome: u.outcome.label(),
        pnl_decimal: pnl,
        consumed_basis_decimal: basis,
        unknown_reasons: reasons,
        unknown_pnl_lower_bound: u.unknown_pnl_bound.map(|b| match b {
            LowerBound::Bounded(m) => EpisodeBoundDto {
                status: "bounded",
                unit: Some("usd"),
                raw: Some(m.scaled_units().to_string()),
                decimal: Some(money_str(m)),
            },
            LowerBound::Unbounded => EpisodeBoundDto {
                status: "unbounded",
                unit: None,
                raw: None,
                decimal: None,
            },
        }),
        legs_priced: u.legs_priced,
        legs_unpriced: u.legs_unpriced,
    }
}

/// `--detail full` open position: the ledger fields plus the ADR-019
/// valuation (or its unvalued reason).
#[derive(Debug, Serialize)]
pub struct OpenPositionDto {
    #[serde(flatten)]
    pub position: scout_app::OpenPositionDto,
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
    pub requests_made_prices: u64,
    pub stop: Option<ReasonDto>,
    pub records: usize,
    pub incomplete_reasons: Vec<String>,
    pub wallets: Vec<WalletStatusDto>,
}

fn wallet_dto(w: &SolanaWalletStats) -> WalletDto {
    WalletDto {
        chain: w.chain.name,
        address: w.chain.address(&w.wallet),
    }
}

/// Native amount: `lamports`/`sol` keys are the Solana-era names; the EVM
/// spelling pass (`scout_app::evm_spelling`) renames them to `wei`/`eth`.
fn amount(chain: &ChainDisplay, lamports: i128) -> AmountDto {
    AmountDto {
        lamports: lamports.to_string(),
        sol: native_str(chain, lamports),
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
        realized_pnl_lower_bound: match b.realized_pnl_lower_bound_raw() {
            LowerBound::Unbounded => BoundDto {
                status: "unbounded",
                unit: quote_unit_label(unit),
                raw: None,
                decimal: None,
            },
            LowerBound::Bounded(v) => BoundDto {
                status: "bounded",
                unit: quote_unit_label(unit),
                raw: Some(v.to_string()),
                decimal: scout_engine::quote_units_to_money(unit, v)
                    .ok()
                    .and_then(|m| format_quote_money(unit, m)),
            },
        },
        profit_factor_lower_bound: ratio_bound_dto(b.profit_factor_lower_bound()),
    }
}

fn ratio_bound_dto(b: LowerBound<RatioStatus<Money>>) -> RatioBoundDto {
    match b {
        LowerBound::Unbounded => RatioBoundDto {
            status: "unbounded",
            value: None,
        },
        LowerBound::Bounded(RatioStatus::Value { value }) => RatioBoundDto {
            status: "value",
            value: Some(money_str(value)),
        },
        LowerBound::Bounded(RatioStatus::NoObservedLosses) => RatioBoundDto {
            status: "no_observed_losses",
            value: None,
        },
        LowerBound::Bounded(RatioStatus::Undefined) => RatioBoundDto {
            status: "undefined",
            value: None,
        },
    }
}

/// SOL-block worst-case net PnL: lower-bound trade PnL minus failed-tx fees.
fn net_pnl_lower_bound_dto(l: &SolanaWalletLedgerReport) -> BoundDto {
    let na = BoundDto {
        status: "n_a",
        unit: l.chain.native_label,
        raw: None,
        decimal: None,
    };
    let Some(b) = l.unit_block(l.quote_unit) else {
        return na;
    };
    if b.closed_episodes_known == 0 && l.failed_trade_fees_lamports == 0 {
        return na;
    }
    match b.realized_pnl_lower_bound() {
        LowerBound::Unbounded => BoundDto {
            status: "unbounded",
            ..na
        },
        LowerBound::Bounded(m) => {
            match scout_engine::quote_units_to_money(l.quote_unit, l.failed_trade_fees_lamports)
                .ok()
                .and_then(|f| m.checked_sub(&f).ok())
            {
                Some(net) => {
                    let lamports = scout_engine::money_to_lamports_trunc(net);
                    BoundDto {
                        status: "bounded",
                        unit: l.chain.native_label,
                        raw: Some(lamports.to_string()),
                        decimal: Some(native_str(&l.chain, lamports)),
                    }
                }
                None => BoundDto {
                    status: "unbounded",
                    ..na
                },
            }
        }
    }
}

fn usd_stats_dto(w: &SolanaWalletStats, u: &UsdLedgerView) -> UsdStatsDto {
    let b = &u.block;
    let usd = QuoteUnit::ReportCurrency;
    let known = b.closed_episodes_known > 0;
    let amt = |raw: i128, exact: Money| UnitAmountDto {
        raw: raw.to_string(),
        decimal: format_quote_money(usd, exact).unwrap_or_default(),
    };
    let (unknown, total) = u.unknown_episode_share_parts();
    UsdStatsDto {
        version: u.version,
        status: if !known {
            "n_a"
        } else if usd_observed(w, u) {
            "observed"
        } else {
            "known_subset"
        },
        closed_known: b.closed_episodes_known,
        closed_unknown: u.closed_episodes_unknown,
        left_censored: u.left_censored_episodes,
        open_unvalued: u.open_episodes,
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
            .map(|r| RoiDto {
                numerator_exact: money_str(b.realized_trade_pnl_exact),
                denominator_exact: money_str(b.consumed_acquisition_basis_exact),
                percent_2dp: r.percent_string(2),
            }),
        win_rate: ratio_dto(&b.win_rate, b.closed_episodes_known),
        profit_factor: ratio_dto(&b.profit_factor, b.closed_episodes_known),
        realized_pnl_lower_bound: match b.realized_pnl_lower_bound() {
            LowerBound::Unbounded => BoundDto {
                status: "unbounded",
                unit: "usd",
                raw: None,
                decimal: None,
            },
            LowerBound::Bounded(m) => BoundDto {
                status: "bounded",
                unit: "usd",
                raw: Some(m.scaled_units().to_string()),
                decimal: Some(money_str(m)),
            },
        },
        profit_factor_lower_bound: ratio_bound_dto(b.profit_factor_lower_bound()),
        win_rate_lower_bound: u.win_rate_lower_bound.map(|x| WinRateBoundDto {
            wins: x.wins,
            episodes: x.episodes,
            percent_2dp: Ratio::new(i128::from(x.wins), i128::from(x.episodes))
                .and_then(|r| r.percent_string(2)),
        }),
        unknown_episode_share: UnknownShareDto {
            closed_unknown: unknown,
            closed_known_and_unknown: total,
            percent_2dp: Ratio::new(i128::from(unknown), i128::from(total))
                .and_then(|r| r.percent_string(2)),
        },
        price_coverage: PriceCoverageDto::from_coverage(&u.coverage),
        failed_trade_fees: UsdFailedFeesDto {
            priced: (u.failed_fees.priced_txs > 0).then(|| money_str(u.failed_fees.priced_usd)),
            priced_txs: u.failed_fees.priced_txs,
            unpriced_txs: u.failed_fees.unpriced_txs,
            unpriced_by_reason: u.failed_fees.unpriced_by_reason.clone(),
        },
        realized_net_pnl: UsdNetDto {
            status: u.net.status.label(),
            raw: u.net.value.map(|m| m.scaled_units().to_string()),
            decimal: u.net.value.map(money_str),
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
        quote_unit: if l.chain.is_evm() { "wei" } else { "lamports" },
        quote_units: l
            .unit_blocks
            .iter()
            .map(|b| quote_unit_dto(w, l, b))
            .collect(),
        route: RouteDto {
            route_swaps: t.route_swaps,
            route_swaps_by_quote: QuoteUnitCountsDto {
                sol: t.route_swaps_by_quote.sol + t.route_swaps_by_quote.wei,
                usdc: t.route_swaps_by_quote.usdc,
                usdt: t.route_swaps_by_quote.usdt,
                usdg: t.route_swaps_by_quote.usdg,
                usdt_peg: t.route_swaps_by_quote.usdt_peg,
                usdc_peg: t.route_swaps_by_quote.usdc_peg,
            },
            route_swaps_by_evidence: RouteEvidenceDto {
                curve: t.route_swaps_by_evidence.curve,
                pump_amm: t.route_swaps_by_evidence.pump_amm,
                jupiter: t.route_swaps_by_evidence.jupiter,
                jupiter_only: t.route_swaps_by_evidence.jupiter_only,
                dflow: t.route_swaps_by_evidence.dflow,
                dflow_only: t.route_swaps_by_evidence.dflow_only,
                okx: t.route_swaps_by_evidence.okx,
                okx_only: t.route_swaps_by_evidence.okx_only,
                okx_owner_is_wallet: t.route_swaps_by_evidence.okx_owner_is_wallet,
                whirlpool: t.route_swaps_by_evidence.whirlpool,
                dlmm: t.route_swaps_by_evidence.dlmm,
                raydium_clmm: t.route_swaps_by_evidence.raydium_clmm,
                raydium_cpmm: t.route_swaps_by_evidence.raydium_cpmm,
                venue_only: t.route_swaps_by_evidence.venue_only,
            },
            route_leg_not_wallet_price: t.route_leg_not_wallet_price,
            route_rejected_wallet_not_signer: d.route_rejected.wallet_not_signer,
            route_rejected_multi_asset: d.route_rejected.multi_asset,
            route_rejected_not_opposite_signs: d.route_rejected.not_opposite_signs,
            route_rejected_no_quote_leg: d.route_rejected.no_quote_leg,
            route_rejected_no_verified_leg: d.route_rejected.no_verified_leg,
            route_rejected_passthrough_nonzero: d.route_rejected.passthrough_nonzero,
        },
        usd: l.usd.as_ref().map(|u| usd_stats_dto(w, u)),
        realized_net_pnl: MoneyDto {
            status: pnl.status,
            lamports: pnl.lamports.map(|v| v.to_string()),
            sol: pnl.lamports.map(|v| native_str(&l.chain, v)),
            sol_exact: pnl
                .lamports
                .map(|_| money_exact_native(&l.chain, l.realized_net_pnl_exact)),
        },
        realized_trade_pnl: amount(&l.chain, l.realized_trade_pnl_lamports),
        realized_trade_pnl_sol_exact: money_exact_native(&l.chain, l.realized_trade_pnl_exact),
        failed_trade_fees: amount(&l.chain, l.failed_trade_fees_lamports),
        failed_trade_fee_txs: l.failed_trade_fee_txs,
        open_episode_known_disposal_pnl: amount(
            &l.chain,
            l.open_episode_known_disposal_pnl_lamports,
        ),
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
        realized_net_pnl_lower_bound: net_pnl_lower_bound_dto(l),
        profit_factor_lower_bound: ratio_bound_dto(
            l.unit_block(l.quote_unit)
                .map_or(LowerBound::Bounded(RatioStatus::Undefined), |b| {
                    b.profit_factor_lower_bound_from(l.profit_factor)
                }),
        ),
        win_rate_lower_bound: l.win_rate_lower_bound.map(|w| WinRateBoundDto {
            wins: w.wins,
            episodes: w.episodes,
            percent_2dp: Ratio::new(i128::from(w.wins), i128::from(w.episodes))
                .and_then(|r| r.percent_string(2)),
        }),
        unknown_episode_share: {
            let (u, t) = l.unknown_episode_share_parts();
            UnknownShareDto {
                closed_unknown: u,
                closed_known_and_unknown: t,
                percent_2dp: Ratio::new(i128::from(u), i128::from(t))
                    .and_then(|r| r.percent_string(2)),
            }
        },
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
        open_valuation: scout_app::ledger_totals_dto(l),
        evm_open_valuation: scout_app::ledger_evm_totals_dto(l),
        open_positions_with_unknown_basis: l.open_positions_with_unknown_basis,
        unknown_basis_lots_created: l.unknown_basis_lots_created,
        diagnostics: DiagnosticsDto {
            transactions_considered: d.transactions_considered,
            duplicate_transactions_ignored: d.duplicate_transactions_ignored,
            failed_transactions: d.failed_transactions,
            malformed_trade_instructions: d.malformed_trade_instructions,
            orphan_trade_events: d.orphan_trade_events,
            unexplained_native_flow: amount(&l.chain, d.unexplained_native_flow_lamports),
            unexplained_native_flow_txs: d.unexplained_native_flow_txs,
            out_of_scope_token_movements: d.out_of_scope_token_movements,
            continuity_breaks: d.continuity_breaks,
            unknown_disposals: d.unknown_disposals,
            known_disposals: d.known_disposals,
            left_censored_disposals: d.left_censored_disposals,
            router_forward_trades_not_attributed: d.router_forward_trades_not_attributed,
            quote_funded_elsewhere_trades: d.quote_funded_elsewhere_trades,
            reversed_pool_trades: d.reversed_pool_trades,
            unknown_discriminator_instructions: d.unknown_discriminator_instructions,
            jupiter_malformed_events: d.jupiter_malformed_events,
            jupiter_unknown_events: d.jupiter_unknown_events,
            dflow_malformed_events: d.dflow_malformed_events,
            dflow_unknown_events: d.dflow_unknown_events,
            okx_malformed_events: d.okx_malformed_events,
            okx_unknown_events: d.okx_unknown_events,
            okx_swap_with_receiver_not_attributed: d.okx_swap_with_receiver_not_attributed,
            okx_idl_only_order_events: d.okx_idl_only_order_events,
            venue_events: (&d.venue_events).into(),
            evidence_samples: scout_app::evidence_dtos(&l.evidence_samples, &str::to_owned),
        },
    }
}

fn episode_dto(ep: &EpisodeRecord, usd: Option<&UsdEpisode>, chain: &ChainDisplay) -> EpisodeDto {
    let (outcome, pnl) = outcome_parts(&ep.outcome);
    let unit = episode_unit(ep, chain);
    let is_sol = unit == chain.native_unit;
    let exact = match &ep.outcome {
        EpisodeOutcome::ClosedKnown { pnl } => Some(money_exact_native(chain, *pnl)),
        _ => None,
    };
    let known = matches!(ep.outcome, EpisodeOutcome::ClosedKnown { .. });
    EpisodeDto {
        mint: chain.address(&ep.mint),
        outcome,
        pnl: pnl.filter(|_| is_sol).map(|v| amount(chain, v)),
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
        known_disposal_pnl: is_sol.then(|| {
            amount(
                chain,
                scout_engine::money_to_lamports_trunc(ep.known_disposal_pnl),
            )
        }),
        known_disposal_pnl_decimal: ep
            .quote_unit
            .and_then(|u| format_quote_money(u, ep.known_disposal_pnl)),
        left_censored_amount_raw: ep.left_censored_amount_raw.to_string(),
        consumed_basis_status: ep.consumed_basis_status.label(),
        unknown_pnl_lower_bound: ep.unknown_pnl_bound.map(|b| match b {
            EpisodePnlBound::Bounded { unit, lower_bound } => EpisodeBoundDto {
                status: "bounded",
                unit: Some(quote_unit_label(unit)),
                raw: Some(scout_engine::money_to_quote_units_trunc(lower_bound).to_string()),
                decimal: format_quote_money(unit, lower_bound),
            },
            EpisodePnlBound::Unbounded => EpisodeBoundDto {
                status: "unbounded",
                unit: None,
                raw: None,
                decimal: None,
            },
        }),
        usd: usd.map(episode_usd_dto),
    }
}

fn open_dtos(l: &SolanaWalletLedgerReport) -> Vec<OpenPositionDto> {
    l.open_positions
        .iter()
        .zip(scout_app::open_positions_dto(l))
        .map(|(p, position)| OpenPositionDto {
            position,
            opened_at: p.opened_at,
        })
        .collect()
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
        episodes: w.ledger.as_ref().filter(|_| full).map(|l| {
            l.episodes
                .iter()
                .enumerate()
                .map(|(i, ep)| {
                    episode_dto(ep, l.usd.as_ref().and_then(|u| u.episodes.get(i)), &l.chain)
                })
                .collect()
        }),
        open_positions: w.ledger.as_ref().filter(|_| full).map(open_dtos),
    }
}

pub struct RunMetaInput<'a> {
    pub run_id: &'a str,
    pub captured_at: &'a str,
    pub max_pages_per_wallet: u32,
    pub provider_options: scout_providers::HeliusRequestOptions,
    pub server_window: bool,
    /// `--concurrency`: max wallets scanned at once.
    pub concurrency: usize,
    pub detail: Detail,
    pub sort: SortMode,
    pub input_wallet_count: usize,
    pub input_duplicates: usize,
    pub upstream_complete: bool,
    pub requests_made: u64,
    pub max_requests: Option<u64>,
    pub window: AnalysisWindow,
    /// ADR-018 price source / policy / coverage of the run.
    pub pricing: PricingMetaDto,
    /// Why the run is incomplete because of pricing (spent price budget).
    pub price_incomplete_reasons: Vec<String>,
    /// ADR-019 valuation run facts (disabled with `--no-valuation`).
    pub open_valuation: scout_app::OpenValuationMetaDto,
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
            concurrency: m.concurrency,
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
            max_price_requests: m.pricing.max_price_requests,
        },
        requests_made_prices: m.pricing.requests_made_prices,
        pricing: m.pricing.clone(),
        open_valuation: m.open_valuation.clone(),
    }
}

pub fn run_summary_record(
    run_id: &str,
    report: &SolanaWalletStatsReport,
    incomplete: bool,
    requests_made: u64,
    price: (u64, &[String]),
    redact: &dyn Fn(&str) -> String,
) -> RunSummaryRecord {
    RunSummaryRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_summary",
        run_id: run_id.to_string(),
        status: if incomplete { "partial" } else { "complete" },
        cancelled: report.cancelled,
        requests_made,
        requests_made_prices: price.0,
        stop: report.stop.map(stop_dto),
        records: report.wallets.len(),
        incomplete_reasons: report
            .incomplete_reasons()
            .iter()
            .chain(price.1.iter())
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

/// EVM `run_meta` facts: scope (chain, venue evidence, quote assets, native-leg
/// sources), ledger version/scope and scan description replace the Solana
/// ones (ADR-020 step 2).
fn patch_evm_run_meta(
    v: &mut serde_json::Value,
    info: &scout_engine::EvmRunInfo,
    window: &AnalysisWindow,
) {
    use serde_json::json;
    v["scope"] = json!({
        "chain": info.chain.name,
        "chain_id": info.chain.chain_id,
        "extraction_version": info.extraction_version,
        "recognized": "swap events of FixtureVerified venue deployments only (see venues); the trade is the transaction signer's own owner-keyed net flow: exactly one traded token and one quote asset (native/wrapped-native merged, the chain's pinned stable: USDG on Robinhood, USDC on Base) with opposite signs",
        "not_decoded": "every venue not listed as FixtureVerified (swap-shaped logs there are counted as coverage gaps), smart-wallet/AA ownership, wallets that do not sign the transaction",
        "venues": info.venues.iter().map(|x| json!({
            "venue": x.venue, "anchor": x.anchor, "role": x.role,
            "verification": x.verification, "active_from_block": x.active_from_block,
        })).collect::<Vec<_>>(),
        "quote_assets": info.quote_assets.iter().map(|q| json!({
            "symbol": q.symbol, "address": q.address, "decimals": q.decimals,
            "decimals_check": q.decimals_check,
        })).collect::<Vec<_>>(),
        "native_leg": {
            "policy": info.native_leg_text(),
            "trace": info.trace,
            "archive_state": info.archive_state,
            "trades_by_source": info.native_leg_counts,
        },
    });
    v["ledger_version"] = json!(info.ledger_version);
    v["ledger_scope"] = json!(info.ledger_scope);
    v["quote_unit"] = json!("lamports");
    v["scan"] = json!({
        "provider": "evm_rpc",
        "history_source": info.history_source,
        "listing_kind": info.listing_kind,
        "coverage_notes": info.coverage_notes,
        "logs_source": info.logs_source,
        "state_source": info.state_source,
        "rate_limits": info.rate_limits,
        "block_range": info.block_range.map(|(a, b)| json!([a, b])),
        "window": if window.is_bounded() {
            "block range resolved from the window by block timestamps"
        } else {
            "full available history (no time window)"
        },
    });
}

/// `run_meta`, `wallet_stats`* (display order), `run_summary`.
pub fn jsonl_lines(
    meta: &RunMetaInput<'_>,
    report: &SolanaWalletStatsReport,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let Some(evm) = &report.evm else {
        // Solana: the DTOs serialize directly (field order unchanged).
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
            (
                meta.pricing.requests_made_prices,
                &meta.price_incomplete_reasons,
            ),
            redact,
        )))?);
        return Ok(lines);
    };
    let to_value = |r: Result<serde_json::Value, serde_json::Error>| r.map_err(|e| e.to_string());
    let mut records: Vec<serde_json::Value> = Vec::with_capacity(report.wallets.len() + 2);
    let mut run_meta = to_value(serde_json::to_value(run_meta_record(meta)))?;
    patch_evm_run_meta(&mut run_meta, evm, &meta.window);
    records.push(run_meta);
    for i in display_order(report, meta.sort) {
        if let Some(w) = report.wallets.get(i) {
            records.push(to_value(serde_json::to_value(wallet_record(
                w,
                meta.detail,
                redact,
            )))?);
        }
    }
    records.push(to_value(serde_json::to_value(run_summary_record(
        meta.run_id,
        report,
        incomplete,
        meta.requests_made,
        (
            meta.pricing.requests_made_prices,
            &meta.price_incomplete_reasons,
        ),
        redact,
    )))?);
    for r in &mut records {
        scout_app::evm_spelling(r, evm.chain.native_label);
    }
    records
        .iter()
        .map(|r| serde_json::to_string(r).map_err(|e| e.to_string()))
        .collect()
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
            chain: scout_engine::SOLANA_DISPLAY,
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
            evm: None,
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
            concurrency: 1,
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
            concurrency: 4,
            detail,
            sort,
            input_wallet_count: 5,
            input_duplicates: 0,
            upstream_complete: true,
            requests_made: 0,
            max_requests: None,
            window: AnalysisWindow::none(1_790_000_000),
            pricing: scout_app::pricing_meta(&scout_app::PricingMetaInput {
                policy: None,
                endpoint_overridden: false,
                requests_made: 0,
                max_requests: None,
                run: None,
            }),
            price_incomplete_reasons: Vec::new(),
            open_valuation: scout_app::open_valuation_meta(None, 0),
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
