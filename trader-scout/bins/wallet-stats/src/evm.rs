//! EVM path of `wallet-stats` (ADR-020 step 2): same cards, same output
//! stack as Solana, fed by `run_evm_wallet_stats`.
//!
//! Sources: RPC from `SCOUT_<CHAIN>_RPC_URL` (Robinhood falls back to the
//! public RPC with a stderr warning), wallet history from Blockscout
//! (`SCOUT_BLOCKSCOUT_API_KEY`, required: a wallet's transactions cannot be
//! listed over plain RPC). The RPC URL and the key are secrets: nothing
//! prints them, every user-visible text is scrubbed.

use std::process::ExitCode;

use alloy_primitives::Address;
use scout_app::{WriteOutcome, write_lines_to_stdout};
use scout_core::{AddressBytes, ChainKey, WalletKey};
use scout_engine::{AnalysisWindow, ScanStop};

use crate::output::{self, Detail, RunMetaInput, SortMode};
use crate::{Args, limit_text, price_wallets};

pub(crate) fn run_evm(
    rt: &tokio::runtime::Runtime,
    all: &[WalletKey],
    chain: &ChainKey,
    args: &Args,
    window: &AnalysisWindow,
    duplicates: usize,
    upstream_complete: bool,
) -> ExitCode {
    let addrs: Vec<Address> = all
        .iter()
        .filter_map(|w| match &w.address {
            AddressBytes::Evm(a) => Some(Address::from(*a)),
            AddressBytes::Solana(_) => None,
        })
        .collect();
    let started = std::time::Instant::now();
    let concurrency = usize::try_from(args.concurrency).unwrap_or(1);
    let notice: scout_app::LimiterNotice =
        std::sync::Arc::new(|m| eprintln!("wallet-stats: warning: {m}"));
    let run = match rt.block_on(scout_app::collect_evm_stats(
        "wallet-stats",
        chain,
        &addrs,
        args.allow_unverified_chain,
        args.max_requests,
        concurrency,
        window,
        !args.no_valuation,
        &args.net,
        &notice,
        |k| std::env::var(k).ok(),
    )) {
        Ok(r) => r,
        Err(scout_app::EvmStatsError::Usage(m)) => {
            eprintln!("{m}");
            return ExitCode::from(2);
        }
        Err(scout_app::EvmStatsError::Config(m)) => {
            eprintln!("{m}");
            return ExitCode::from(4);
        }
        Err(scout_app::EvmStatsError::Budget(m)) => {
            eprintln!("{m}");
            return ExitCode::from(3);
        }
    };
    for w in &run.warnings {
        eprintln!("wallet-stats: warning: {w}");
    }
    eprintln!(
        "wallet-stats: rpc calls by method: {}",
        run.rpc_calls_by_method
            .iter()
            .map(|(m, n)| format!("{m}={n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    for l in &run.rate_limits {
        eprintln!("wallet-stats: rate limit: {l}");
    }
    let requests_made = run.requests_made;
    let valuation = run.valuation.clone();
    eprintln!(
        "{}",
        scout_app::open_valuation_line_evm("wallet-stats", valuation.as_ref())
    );
    if let Some(l) = scout_app::valuation_cost_line_evm(valuation.as_ref()) {
        eprintln!("{l}");
    }
    let secrets = run.secrets.clone();
    let scrub = move |t: &str| {
        let mut o = t.to_string();
        for s in &secrets {
            if s.len() >= 4 {
                o = o.replace(s.as_str(), "<redacted>");
            }
        }
        o
    };
    let mut report = run.report;
    let pricing = price_wallets(rt, &mut report, args);
    let pricing_input = pricing.input();
    eprintln!(
        "{}",
        scout_app::pricing_line("wallet-stats", &pricing_input)
    );
    let mut price_reasons: Vec<String> = Vec::new();
    if valuation.as_ref().is_some_and(|v| v.budget_exhausted) {
        price_reasons.push(format!(
            "request budget exhausted during open-position valuation (max_requests={}): \
             affected positions are unvalued (request_budget_exhausted)",
            limit_text(args.max_requests)
        ));
    }
    if let Some(run) = &pricing.run
        && run.prefetch.pages_skipped_budget > 0
    {
        price_reasons.push(format!(
            "price request budget exhausted ({} page(s) not fetched, max_price_requests={}): \
             affected legs are price_unknown",
            run.prefetch.pages_skipped_budget,
            limit_text(args.max_price_requests)
        ));
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let valuation_planned = valuation
        .as_ref()
        .filter(|v| v.ran && v.state_block.is_some())
        .map(|v| v.planned_calls);
    print_evm_diagnostics(
        &report,
        window,
        requests_made,
        valuation_planned,
        elapsed_ms,
        &scrub,
        args,
    );
    for r in &price_reasons {
        eprintln!("wallet-stats: {r}");
    }
    let incomplete =
        report.is_coverage_incomplete() || !upstream_complete || !price_reasons.is_empty();
    let detail = if args.detail == "full" {
        Detail::Full
    } else {
        Detail::Summary
    };
    let sort = if args.sort == "realized-net-pnl" {
        SortMode::RealizedNetPnl
    } else {
        SortMode::Input
    };
    let captured_at = scout_app::now_utc_rfc3339();
    let lines = if args.format == "jsonl" {
        let compact: String = captured_at
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let run_id = format!("wallet-stats-{compact}");
        let meta = RunMetaInput {
            run_id: &run_id,
            captured_at: &captured_at,
            max_pages_per_wallet: args.max_pages_per_wallet,
            provider_options: scout_providers::HeliusRequestOptions::default(),
            server_window: false,
            concurrency,
            detail,
            sort,
            input_wallet_count: all.len(),
            input_duplicates: duplicates,
            upstream_complete,
            requests_made,
            max_requests: args.max_requests,
            window: *window,
            pricing: scout_app::pricing_meta(&pricing_input),
            price_incomplete_reasons: price_reasons.clone(),
            open_valuation: scout_app::open_valuation_meta_evm(
                valuation.as_ref(),
                window.as_of,
                report.evm.as_ref().map_or("eth", |i| i.chain.native_label),
            ),
        };
        match output::jsonl_lines(&meta, &report, incomplete, &|t| scrub(t)) {
            Ok(l) => l,
            Err(message) => {
                eprintln!("wallet-stats: could not render output: {message}");
                return ExitCode::from(4);
            }
        }
    } else {
        output::table_lines(&report, detail, sort, window)
            .into_iter()
            .map(|l| scrub(&l))
            .collect()
    };
    let outcome = write_lines_to_stdout(lines);
    if report.cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
    }
    match report.stop {
        Some(ScanStop::BudgetExhausted { .. }) => return ExitCode::from(3),
        Some(ScanStop::RateLimited { .. }) => {
            return ExitCode::from(if report.any_data() { 3 } else { 4 });
        }
        None => {}
    }
    if report.all_failed() {
        return ExitCode::from(if report.all_failed_for_budget() { 3 } else { 4 });
    }
    if incomplete {
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

fn print_evm_diagnostics(
    report: &scout_engine::SolanaWalletStatsReport,
    window: &AnalysisWindow,
    requests_made: u64,
    valuation_planned_calls: Option<u64>,
    elapsed_ms: u64,
    scrub: &dyn Fn(&str) -> String,
    args: &Args,
) {
    if let Some(i) = &report.evm {
        eprintln!(
            "wallet-stats: protocol scope ({} chain id {}, {}-quoted ledger, wei + the chain's pinned stable):",
            i.chain.name,
            i.chain.chain_id.unwrap_or(0),
            i.chain.native_symbol
        );
        eprintln!("  venues: {}", i.venues_text());
        let quotes: Vec<String> = i
            .quote_assets
            .iter()
            .map(|q| {
                format!(
                    "{}={} decimals={} ({})",
                    q.symbol, q.address, q.decimals, q.decimals_check
                )
            })
            .collect();
        eprintln!(
            "  quote assets: native {} (WETH merged), {}",
            i.chain.native_symbol,
            quotes.join(", ")
        );
        eprintln!(
            "  history: {} (listing_kind={})",
            i.history_source, i.listing_kind
        );
        for n in &i.coverage_notes {
            eprintln!("  coverage note: {n}");
        }
        eprintln!(
            "  routing: eth_getLogs -> {}; receipts/state -> {}",
            i.logs_source, i.state_source
        );
        eprintln!("  {}", i.native_leg_text());
        eprintln!(
            "  native legs of native-quoted trades by source: {:?}",
            i.native_leg_counts
        );
        eprintln!(
            "  extraction: {} ledger: {}",
            i.extraction_version, i.ledger_version
        );
        match (window.bounds(), i.block_range) {
            (Some((s, u)), r) => eprintln!(
                "  window [{}, {}) source={} blocks={:?}",
                scout_app::format_unix_utc(u64::try_from(s).unwrap_or(0)),
                scout_app::format_unix_utc(u64::try_from(u).unwrap_or(0)),
                window.source.label(),
                r
            ),
            (None, _) => eprintln!("  window: none (full available history)"),
        }
    }
    eprintln!(
        "  concurrency={} requests_made={requests_made}{} max_requests={} elapsed_ms={elapsed_ms} (stderr only)",
        report.concurrency,
        valuation_planned_calls.map_or_else(String::new, |n| format!(
            " (incl. valuation planned_calls={n})"
        )),
        limit_text(args.max_requests)
    );
    for w in &report.wallets {
        let n = w
            .transactions_scanned
            .map_or_else(|| "n/a".to_string(), |n| n.to_string());
        eprintln!(
            "  wallet {}: status={} txs_scanned={n}",
            w.chain.prefixed_address(&w.wallet),
            w.status.label()
        );
    }
    let reasons = report.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!("wallet-stats: status=complete within declared protocol scope");
    } else {
        eprintln!("wallet-stats: status=partial (IncompleteCoverage):");
        for r in reasons {
            eprintln!("  - {}", scrub(&r));
        }
    }
}
