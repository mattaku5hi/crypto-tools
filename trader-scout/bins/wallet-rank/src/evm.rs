//! EVM path of `wallet-rank` (ADR-020 step 2): the SAME analysis as
//! `wallet-stats` (`collect_evm_stats`), then the shared rank engine. Secrets
//! (RPC URL, explorer key) never reach any output.

use std::process::ExitCode;

use alloy_primitives::Address;
use scout_app::{WriteOutcome, write_lines_to_stdout};
use scout_core::{AddressBytes, ChainKey, WalletKey};
use scout_engine::{AnalysisWindow, RankPolicy, ScanStop, rank_solana_wallets};

use crate::output::{self, RunMetaInput, SummaryInput};
use crate::{Args, limit_text, price_wallets, rate_limited_text};

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_evm(
    rt: &tokio::runtime::Runtime,
    all: &[WalletKey],
    chain: &ChainKey,
    args: &Args,
    policy: &RankPolicy,
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
    let run = match rt.block_on(scout_app::collect_evm_stats(
        "wallet-rank",
        chain,
        &addrs,
        args.allow_unverified_chain,
        args.max_requests,
        concurrency,
        window,
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
        eprintln!("wallet-rank: warning: {w}");
    }
    let requests_made = run.requests_made;
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
    let mut stats = run.report;
    let pricing = price_wallets(
        rt,
        &mut stats,
        args,
        policy.quote == scout_engine::QuoteUnit::ReportCurrency,
    );
    let pricing_input = pricing.input();
    let pricing_line = scout_app::pricing_line("wallet-rank", &pricing_input);
    let mut price_reasons: Vec<String> = Vec::new();
    if let Some(r) = &pricing.run
        && r.prefetch.pages_skipped_budget > 0
    {
        price_reasons.push(format!(
            "price request budget exhausted ({} page(s) not fetched, max_price_requests={}): \
             affected legs are price_unknown",
            r.prefetch.pages_skipped_budget,
            limit_text(args.max_price_requests)
        ));
    }
    let report = rank_solana_wallets(&stats.wallets, policy);
    let incomplete =
        stats.is_coverage_incomplete() || !upstream_complete || !price_reasons.is_empty();
    if pricing.policy.is_some() {
        eprintln!("{pricing_line}");
    }
    for r in &price_reasons {
        eprintln!("wallet-rank: {r}");
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    print_diagnostics(
        &stats,
        &report,
        window,
        requests_made,
        elapsed_ms,
        &scrub,
        args,
    );

    let captured_at = scout_app::now_utc_rfc3339();
    let lines = if args.format == "jsonl" {
        let compact: String = captured_at
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let run_id = format!("wallet-rank-{compact}");
        let meta = RunMetaInput {
            run_id: &run_id,
            captured_at: &captured_at,
            max_pages_per_wallet: args.max_pages_per_wallet,
            provider_options: scout_providers::HeliusRequestOptions::default(),
            server_window: false,
            concurrency,
            max_requests: args.max_requests,
            requests_made,
            input_wallet_count: all.len(),
            input_duplicates: duplicates,
            upstream_complete,
            window: *window,
            pricing: scout_app::pricing_meta(&pricing_input),
            open_valuation: scout_app::open_valuation_meta(None, window.as_of),
            evm: stats.evm.as_ref(),
        };
        let summary = SummaryInput {
            run_id: &run_id,
            partial: incomplete,
            cancelled: stats.cancelled,
            stop: stats.stop,
            requests_made,
            requests_made_prices: pricing.requests_made,
            incomplete_reasons: stats
                .incomplete_reasons()
                .iter()
                .chain(price_reasons.iter())
                .map(|r| scrub(r))
                .collect(),
        };
        match output::jsonl_lines(&meta, &report, summary, &|t| scrub(t)) {
            Ok(l) => l,
            Err(message) => {
                eprintln!("wallet-rank: could not render output: {message}");
                return ExitCode::from(4);
            }
        }
    } else {
        output::table_lines(
            &report,
            incomplete,
            window,
            pricing.policy.is_some().then_some(pricing_line.as_str()),
        )
        .into_iter()
        .map(|l| scrub(&l))
        .collect()
    };
    let outcome = write_lines_to_stdout(lines);
    if stats.cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
    }
    match stats.stop {
        Some(ScanStop::BudgetExhausted { .. }) => return ExitCode::from(3),
        Some(ScanStop::RateLimited { .. }) => {
            return ExitCode::from(if stats.any_data() { 3 } else { 4 });
        }
        None => {}
    }
    if stats.all_failed() {
        return ExitCode::from(4);
    }
    if incomplete {
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

fn print_diagnostics(
    stats: &scout_engine::SolanaWalletStatsReport,
    report: &scout_engine::WalletRankReport,
    window: &AnalysisWindow,
    requests_made: u64,
    elapsed_ms: u64,
    scrub: &dyn Fn(&str) -> String,
    args: &Args,
) {
    let p = &report.policy;
    if let Some(i) = &stats.evm {
        eprintln!(
            "wallet-rank: protocol scope ({} chain id {}, {}-quoted ledger, wei + USDG):",
            i.chain.name,
            i.chain.chain_id.unwrap_or(0),
            i.chain.native_symbol
        );
        eprintln!("  venues: {}", i.venues_text());
        eprintln!("  history: {}", i.history_source);
        eprintln!("  {}", i.native_leg_text());
        eprintln!(
            "  native legs of native-quoted trades by source: {:?}",
            i.native_leg_counts
        );
        eprintln!(
            "  extraction: {} ledger: {}",
            i.extraction_version, i.ledger_version
        );
    }
    eprintln!(
        "  window: {} concurrency={} requests_made={requests_made} max_requests={} elapsed_ms={elapsed_ms} (stderr only)",
        match window.bounds() {
            Some((s, u)) => format!(
                "[{}, {}) source={}",
                scout_app::format_unix_utc(u64::try_from(s).unwrap_or(0)),
                scout_app::format_unix_utc(u64::try_from(u).unwrap_or(0)),
                window.source.label()
            ),
            None => "none".to_string(),
        },
        stats.concurrency,
        limit_text(args.max_requests)
    );
    match stats.stop {
        Some(ScanStop::BudgetExhausted { limit }) => eprintln!(
            "wallet-rank: request budget exhausted after {requests_made} requests (limit {limit}); \
             remaining wallets are not_scanned, ranking is over an incomplete universe"
        ),
        Some(ScanStop::RateLimited { retry_after_secs }) => eprintln!(
            "wallet-rank: {}; remaining wallets were not scanned, ranking is over an incomplete universe",
            rate_limited_text(retry_after_secs)
        ),
        None => {}
    }
    eprintln!(
        "  policy: rank_by={} quote={} profile={} min_closed_episodes={} min_active_days={} top={}",
        p.rank_by.label(),
        scout_engine::quote_unit_label(p.quote),
        p.profile.label(),
        p.min_closed_episodes,
        p.min_active_days,
        p.top
    );
    eprintln!(
        "wallet-rank: {} input, {} eligible, {} ranked, {} excluded",
        report.input_count,
        report.eligible_count,
        report.ranked.len(),
        report.excluded.len()
    );
    let reasons = stats.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!("wallet-rank: status=complete within declared protocol scope");
    } else {
        eprintln!("wallet-rank: status=partial (ranking over an incomplete universe):");
        for r in reasons {
            eprintln!("  - {}", scrub(&r));
        }
    }
}
