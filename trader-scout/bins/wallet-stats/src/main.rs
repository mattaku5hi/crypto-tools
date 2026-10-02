//! `wallet-stats`: see docs/CLI.md §5 for the full contract.
//!
//! Solana: when EVERY input wallet is Solana and `SCOUT_HELIUS_API_KEY`
//! is set, a `HeliusProvider` (newest-first scan, bounded page budget per
//! wallet) drives `run_solana_wallet_stats` (pump.fun bonding-curve SOL
//! ledger, ADR-010). One card/row per distinct input wallet, first
//! appearance order; failures and gaps are shown on the card, never
//! dropped. Without a key, or for EVM/mixed input, the run exits 4.
//!
//! `--max-requests N` (N >= 1) bounds the TOTAL HTTP attempts of the run
//! (retries included); absent = unlimited but counted. Run-terminal stops
//! (engine `ScanStop`, typed): budget exhaustion or a terminal rate limit
//! stops scanning; the interrupted wallet is `error` (typed `error_kind`),
//! later wallets are `not_scanned` (`stop_reason`), no further request is
//! made, every input wallet stays in the output.
//!
//! `--period <N>d` / `--since` / `--until` (ADR-011) restrict the run to a UTC
//! window `[since, until)`: the scan stops at the first page older than the
//! window start, pre-window inventory is left-censored.
//!
//! Exit codes (CLI.md §8): 0 complete within declared scope (legitimate
//! N/A, no-activity and unknown-basis cards are NOT failures); 2 usage
//! (incl. an invalid window);
//! 3 incomplete coverage (truncated scan, per-wallet scan failure while
//! other wallets succeeded, decoder gaps, upstream JSONL partial/without
//! footer, request budget exhausted, rate-limit stop after at least one
//! wallet produced data); 4 infrastructure/credentials (also: every wallet
//! failed, rate-limit stop with no data); 130 cancelled; 141 stdout closed.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::io::{self, IsTerminal, Read};
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use scout_api::ProviderError;
use scout_app::{InputFormat, WriteOutcome, write_lines_to_stdout};
use scout_core::{AddressBytes, ChainFamily, SolanaPubkey, WalletKey};
use scout_engine::{
    AnalysisWindow, LedgerDecoders, ScanStop, SolanaWalletStatsReport, pump_amm_decoder,
    pump_bonding_curve_decoder, run_solana_wallet_stats_windowed_venues, sanitize_provider_text,
};
use scout_providers::{HeliusProvider, ScanOrder};
use scout_rpc::DEFAULT_MAX_RETRY_AFTER;
use tokio_util::sync::CancellationToken;

mod output;

use output::{Detail, RunMetaInput, SortMode};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
/// Test-only: send requests to this URL instead of Helius (the API key is
/// never sent to the override).
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_WALLET_STATS_ENDPOINT";
const HELIUS_TIMEOUT_MS: u64 = 30_000;
const HELIUS_MAX_ATTEMPTS: u32 = 3;

/// Print stats for every input wallet, including N/A and no-activity cases.
#[derive(Debug, Parser)]
#[command(name = "wallet-stats", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Input format: `lines` (default) or `jsonl` (records of a previous
    /// trader-scout CLI: buyer_match, wallet_ref, wallet_stats).
    #[arg(long, default_value = "lines", value_parser = ["lines", "jsonl"])]
    input_format: String,

    /// Output format: table or jsonl.
    #[arg(long, default_value = "table", value_parser = ["table", "jsonl"])]
    format: String,

    /// `full` adds per-episode and open-position evidence.
    #[arg(long, default_value = "summary", value_parser = ["summary", "full"])]
    detail: String,

    /// `input` keeps first-appearance order; `realized-net-pnl` orders by
    /// known realized net PnL descending (N/A last). The set never changes.
    #[arg(long, default_value = "input", value_parser = ["input", "realized-net-pnl"])]
    sort: String,

    /// Provider page budget PER WALLET (Helius full mode: 100 transactions
    /// per page). The scan is newest-first: when history needs more pages
    /// the wallet is `incomplete` (older history unseen, opening inventory
    /// unknown) and the run exits 3. This is NOT --max-requests: retries
    /// are not counted against it (use --max-requests for that).
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u32).range(1..=200)
    )]
    max_pages_per_wallet: u32,

    /// Total HTTP request budget for the whole run (all wallets, retries
    /// included), N >= 1. Absent = unlimited (requests are still counted
    /// and reported). When exhausted the run stops: the interrupted wallet
    /// is `error`, the rest `not_scanned`; exit 3.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,

    /// Analysis window start, UTC RFC 3339 `2026-08-01T00:00:00Z` (inclusive;
    /// no offsets). Window `[since, until)`; see ADR-011. Conflicts with --period.
    #[arg(long, conflicts_with = "period")]
    since: Option<String>,
    /// Analysis window end (exclusive), same format; default: run start.
    /// Requires --since or --period.
    #[arg(long)]
    until: Option<String>,
    /// Window of the last N days (`30d`, 1..=365) ending at --until (default
    /// run start). Conflicts with --since.
    #[arg(long, conflicts_with = "since")]
    period: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    // ADR-011: the window is resolved (and `as_of` pinned) exactly once.
    let as_of = unix_now();
    let window = match AnalysisWindow::resolve(
        args.period.as_deref(),
        args.since.as_deref(),
        args.until.as_deref(),
        as_of,
    ) {
        Ok(w) => w,
        Err(err) => {
            eprintln!("wallet-stats: {err}");
            return ExitCode::from(2);
        }
    };

    let input_text = match read_input(args.input.as_deref()) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("wallet-stats: {message}");
            return ExitCode::from(2);
        }
    };

    let (parsed, upstream) = if args.input_format == "jsonl" {
        match scout_app::parse_jsonl_with_upstream(input_text.as_bytes(), None) {
            Ok(r) => r,
            Err(err) => {
                eprintln!("wallet-stats: {err}");
                return ExitCode::from(2);
            }
        }
    } else {
        match scout_app::parse_input(input_text.as_bytes(), InputFormat::Lines, None) {
            Ok(p) => (p, scout_app::UpstreamInfo::default()),
            Err(err) => {
                eprintln!("wallet-stats: {err}");
                return ExitCode::from(2);
            }
        }
    };

    let wallets = match scout_app::resolve_wallet_keys(&parsed) {
        Ok(wallets) => wallets,
        Err(err) => {
            eprintln!("wallet-stats: {err}");
            return ExitCode::from(2);
        }
    };
    if parsed.duplicate_count > 0 {
        eprintln!(
            "wallet-stats: {} duplicate input identit(ies) collapsed",
            parsed.duplicate_count
        );
    }
    let upstream_complete = upstream.is_complete();
    if !upstream_complete {
        eprintln!(
            "wallet-stats: upstream JSONL run is not complete (run_summary status={}); \
             the wallet universe may be partial, the run will exit 3",
            upstream.summary_status.as_deref().unwrap_or("missing")
        );
    }

    let solana: Vec<SolanaPubkey> = wallets
        .iter()
        .filter_map(|w| match (&w.chain.family, &w.address) {
            (ChainFamily::Solana, AddressBytes::Solana(a)) => Some(*a),
            _ => None,
        })
        .collect();
    if solana.len() != wallets.len() {
        let evm = wallets.len() - solana.len();
        eprintln!(
            "wallet-stats: {evm} of {} wallet(s) are EVM; no history provider configured for EVM \
             (SCOUT_EVM_HISTORY_API_KEY): refusing to print a partial list",
            wallets.len()
        );
        return ExitCode::from(4);
    }

    let Some(api_key) = std::env::var(HELIUS_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!(
            "wallet-stats: {} wallet(s) parsed; no history provider configured: \
             configuration required, set {HELIUS_KEY_ENV}",
            wallets.len()
        );
        return ExitCode::from(4);
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("wallet-stats: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };
    run_solana(
        &rt,
        &wallets,
        &solana,
        &args,
        &window,
        &api_key,
        parsed.duplicate_count,
        upstream_complete,
    )
}

fn redact(text: &str, secret: &str) -> String {
    let replaced = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    };
    sanitize_provider_text(&replaced)
}

fn limit_text(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unlimited".to_string(), |n| n.to_string())
}

/// Terminal `RateLimited`: the transport refused to wait out a
/// `Retry-After` above its cap.
fn rate_limited_text(retry_after_secs: Option<u64>) -> String {
    let cap = DEFAULT_MAX_RETRY_AFTER.as_secs();
    match retry_after_secs {
        Some(s) => format!("rate limited; server asked to retry after {s}s (cap {cap}s)"),
        None => "rate limited; server gave no Retry-After".to_string(),
    }
}

/// Run start in unix seconds (pinned once per run as `as_of`).
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn build_provider(
    api_key: &str,
    max_pages: NonZeroU32,
    max_requests: Option<u64>,
    window: &AnalysisWindow,
) -> Result<HeliusProvider, ProviderError> {
    let provider = match std::env::var(ENDPOINT_OVERRIDE_ENV) {
        Ok(url) if !url.is_empty() => HeliusProvider::new_with_endpoint(
            scout_rpc::RpcEndpoint::new(url),
            HELIUS_TIMEOUT_MS,
            HELIUS_MAX_ATTEMPTS,
        )?,
        _ => HeliusProvider::new(api_key, HELIUS_TIMEOUT_MS, HELIUS_MAX_ATTEMPTS)?,
    };
    Ok(provider
        .with_max_pages(max_pages)
        .with_scan_order(ScanOrder::NewestFirst)
        .with_stop_before_block_time(window.bounds().map(|(since, _)| since))
        .with_max_total_requests(max_requests))
}

fn provider_error_exit(err: &ProviderError, secret: &str) -> ExitCode {
    let text = redact(&err.to_string(), secret);
    match err {
        ProviderError::ConfigurationRequired { .. } => eprintln!("wallet-stats: {text}"),
        _ => eprintln!("wallet-stats: provider error: {text}"),
    }
    ExitCode::from(4)
}

#[allow(clippy::too_many_arguments)]
fn run_solana(
    rt: &tokio::runtime::Runtime,
    all: &[WalletKey],
    solana: &[SolanaPubkey],
    args: &Args,
    window: &AnalysisWindow,
    api_key: &str,
    duplicates: usize,
    upstream_complete: bool,
) -> ExitCode {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_wallet) else {
        eprintln!("wallet-stats: --max-pages-per-wallet must be at least 1");
        return ExitCode::from(2);
    };
    // ONE provider for the whole run: the budget and counter are shared
    // by every wallet's scan.
    let provider = match build_provider(api_key, max_pages, args.max_requests, window) {
        Ok(p) => p,
        Err(err) => return provider_error_exit(&err, api_key),
    };
    let decoder = match pump_bonding_curve_decoder() {
        Ok(d) => d,
        Err(err) => {
            eprintln!(
                "wallet-stats: decoder unavailable: {}",
                redact(&err.to_string(), api_key)
            );
            return ExitCode::from(4);
        }
    };
    let amm = pump_amm_decoder();
    let result = rt.block_on(run_solana_wallet_stats_windowed_venues(
        &provider,
        solana,
        &LedgerDecoders {
            curve: &decoder,
            amm: Some(&amm),
        },
        window,
        CancellationToken::new(),
    ));
    let requests_made = provider.total_requests_made();
    let report = match result {
        Ok(r) => r,
        Err(err) => {
            eprintln!(
                "wallet-stats: requests_made={requests_made} max_requests={}",
                limit_text(args.max_requests)
            );
            return provider_error_exit(&err, api_key);
        }
    };

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
    print_diagnostics(&report, api_key, args, window, requests_made);
    let incomplete = report.is_coverage_incomplete() || !upstream_complete;
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
            detail,
            sort,
            input_wallet_count: all.len(),
            input_duplicates: duplicates,
            upstream_complete,
            requests_made,
            max_requests: args.max_requests,
            window: *window,
        };
        match output::jsonl_lines(&meta, &report, incomplete, &|t| redact(t, api_key)) {
            Ok(l) => l,
            Err(message) => {
                eprintln!("wallet-stats: could not render output: {message}");
                return ExitCode::from(4);
            }
        }
    } else {
        output::table_lines(&report, detail, sort, window)
            .into_iter()
            .map(|l| redact(&l, api_key))
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
        return ExitCode::from(4);
    }
    if incomplete {
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

/// Scope and coverage on stderr (stdout stays the single result format).
fn print_diagnostics(
    report: &SolanaWalletStatsReport,
    api_key: &str,
    args: &Args,
    window: &AnalysisWindow,
    requests_made: u64,
) {
    let max_pages = args.max_pages_per_wallet;
    let s = &report.scope;
    eprintln!("wallet-stats: protocol scope (Solana mainnet, SOL-quoted ledger, lamports):");
    eprintln!("  recognized: {}", s.recognized);
    eprintln!("  NOT decoded: {}", s.not_decoded);
    eprintln!(
        "  programs: pump.fun bonding curve {} (IDL {} sha256 {}); PumpSwap AMM {} (IDL {} sha256 {})",
        s.program_id,
        s.idl_commit,
        s.idl_sha256,
        s.amm_program_id.unwrap_or("-"),
        s.amm_idl_commit.unwrap_or("-"),
        s.amm_idl_sha256.unwrap_or("-"),
    );
    eprintln!(
        "  scan: newest-first, max_pages_per_wallet={max_pages} (100 txs/page; retries not counted); \
         {}",
        match window.bounds() {
            Some((since, until)) => format!(
                "window [{}, {}) source={} as_of={}: stops after the first page holding a tx older \
                 than the window start; a wallet that exhausts the budget before it is incomplete; \
                 pre-window inventory is left-censored (ADR-011)",
                scout_app::format_unix_utc(u64::try_from(since).unwrap_or(0)),
                scout_app::format_unix_utc(u64::try_from(until).unwrap_or(0)),
                window.source.label(),
                scout_app::format_unix_utc(u64::try_from(window.as_of).unwrap_or(0))
            ),
            None =>
                "a truncated wallet lacks OLDER history (opening inventory unknown); no time window"
                    .to_string(),
        }
    );
    eprintln!(
        "  requests_made={requests_made} max_requests={} (total HTTP attempts, retries included)",
        limit_text(args.max_requests)
    );
    match report.stop {
        Some(ScanStop::BudgetExhausted { limit }) => eprintln!(
            "wallet-stats: request budget exhausted after {requests_made} requests (limit {limit}); \
             the interrupted wallet is marked error, remaining wallets are not_scanned, results are incomplete"
        ),
        Some(ScanStop::RateLimited { retry_after_secs }) => eprintln!(
            "wallet-stats: {}; remaining wallets were not scanned, results are incomplete",
            rate_limited_text(retry_after_secs)
        ),
        None => {}
    }
    for w in &report.wallets {
        let n = w
            .transactions_scanned
            .map_or_else(|| "n/a".to_string(), |n| n.to_string());
        eprintln!(
            "  wallet {}: status={} txs_scanned={n}",
            bs58::encode(w.wallet).into_string(),
            w.status.label()
        );
    }
    let reasons = report.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!("wallet-stats: status=complete within declared protocol scope");
    } else {
        eprintln!("wallet-stats: status=partial (IncompleteCoverage):");
        for r in reasons {
            eprintln!("  - {}", redact(&r, api_key));
        }
    }
}

fn read_input(input: Option<&str>) -> Result<String, String> {
    match input {
        Some("-") => read_stdin_to_string(),
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))
        }
        None => {
            if io::stdin().is_terminal() {
                Err("no --input given and stdin is a terminal; pass --input <path>, --input -, or pipe data".to_string())
            } else {
                read_stdin_to_string()
            }
        }
    }
}

fn read_stdin_to_string() -> Result<String, String> {
    let mut buf = String::new();
    let mut lock = io::stdin().lock();
    lock.read_to_string(&mut buf)
        .map_err(|e| format!("could not read stdin: {e}"))?;
    Ok(buf)
}
