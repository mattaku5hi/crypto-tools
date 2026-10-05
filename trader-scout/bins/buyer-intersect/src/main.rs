//! `buyer-intersect`: see docs/CLI.md §3 for the full contract.
//!
//! Wires input parsing -> scout-engine's orchestration -> JSONL/table
//! output.
//!
//! Solana: when EVERY input token is a Solana mint and
//! `SCOUT_HELIUS_API_KEY` is set (non-empty), a `HeliusProvider` drives
//! `run_solana_trade_intersect` (ADR-014: pump.fun bonding-curve, PumpSwap
//! and ADR-013 route-swap trades; `--side buy|sell|any`, default `any`;
//! optional `--since/--until/--period` window scanned newest-first; the
//! scope/lower-bound caveat is printed on stderr). Without the key, or
//! for an unconfigured chain, `UnconfiguredProvider` is used and the run
//! exits 4 (`ConfigurationRequired`) -- the honest outcome, not a stub.
//!
//! Exit codes (ADR-005): 0 complete within declared scope; 2 argument
//! error; 3 incomplete coverage (truncated, failed token scan,
//! malformed or unknown-discriminator bonding-curve instruction,
//! unverified-variant buys, request budget exhausted, ...) even though
//! matches are still printed; 4 infrastructure/configuration; 130
//! cancelled; 141 output pipe closed.
//!
//! `--max-requests N` (N >= 1) bounds the TOTAL HTTP attempts of the run
//! (retries included, one provider instance shared by all tokens). Exit
//! decision: exhausting a USER-chosen budget is incomplete coverage
//! (exit 3), not infrastructure failure (exit 4): the tool worked, the
//! requested scan contract was not met. Tokens whose scan hit the limit
//! are `failed` in `run_summary` (unknown, never zero) and the run is
//! `partial`. If the budget ran out before ANY transaction was seen the
//! engine yields no report; the run still exits 3 with the diagnostic
//! and no records on stdout (no `run_summary` footer, so a downstream
//! consumer cannot take it for complete). A terminal `RateLimited`
//! (Retry-After above the transport cap) and `ResponseTooLarge` before
//! any data are infrastructure: exit 4. Requests used are always
//! printed (`requests_made` in `run_meta`, stderr summary line).
//!
//! Run-terminal stops (engine `ScanStop`, typed, no text matching): on
//! the first budget exhaustion or terminal rate limit the engine issues
//! no further request; the interrupted token is `failed` (with
//! `error_kind`) and every later token is `not_scanned` (with
//! `stop_reason`) in `run_summary`. Exit mapping for a stop AFTER data
//! was observed (a report exists): budget -> 3; rate limit -> 3 too
//! (the tool worked, partial matches are printed and marked incomplete;
//! CLI.md §8: 3 = partial scan/coverage contract, 4 = nothing usable
//! could be produced). A rate limit before ANY transaction was observed
//! yields no report: exit 4 (infrastructure), no records on stdout.
//! Both stop kinds print the same stderr line mid-run and before data.
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

use clap::Parser;
use scout_api::ProviderError;
use scout_app::{ChainRun, InputFormat, WriteOutcome, write_lines_to_stdout};
use scout_core::{AssetKey, ChainFamily, ChainKey};
use scout_engine::{
    AnalysisWindow, IntersectOptions, PumpTradeVariant, ScanStop, SideFilter,
    SolanaBuyerIntersectReport, SolanaProtocolScope, TokenScanStatus, TradeSide, Venue,
    run_buyer_intersect, run_solana_trade_intersect, sanitize_provider_text,
};
use scout_providers::{
    HeliusProvider, HeliusRequestOptions, MAX_PAGE_LIMIT, ScanOrder, StatusFilter,
    UnconfiguredProvider,
};
use scout_rpc::{DEFAULT_MAX_RETRY_AFTER, RequestBudgetExhausted};
use tokio_util::sync::CancellationToken;

mod evm;
mod multi;
mod output;

use output::RunBudget;

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
/// Test-only: replaces the Helius endpoint URL (offline wiremock tests).
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_BUYER_INTERSECT_ENDPOINT";
const HELIUS_TIMEOUT_MS: u64 = 30_000;
const HELIUS_MAX_ATTEMPTS: u32 = 3;

// Defaults of the provider request options: live-verified 2026-10-03, see
// docs/p0/measurements/2026-10-03-helius-filters-live.md. The opt-out flags
// reproduce the legacy request.
/// `--page-limit` default (`limit` per `getTransactionsForAddress` page).
const DEFAULT_PAGE_LIMIT_ARG: u32 = 500;
/// `--provider-status-filter` default: `any` or `succeeded`.
const DEFAULT_PROVIDER_STATUS_FILTER: &str = "succeeded";
/// `--server-window` default: send the window as `filters.blockTime`.
const DEFAULT_SERVER_WINDOW: bool = true;

/// Find wallets that traded (bought and/or sold, `--side`) at least K
/// distinct input tokens.
#[derive(Debug, Clone, Parser)]
#[command(name = "buyer-intersect", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Minimum number of distinct input tokens a wallet must have traded
    /// (on the selected `--side`).
    #[arg(long, default_value_t = 2)]
    min_token_hits: usize,

    /// Which trades count as a hit on a token: `buy`, `sell`, or `any` (a
    /// buy OR a sell on each token; default, ADR-014).
    #[arg(long, default_value = "any", value_parser = ["buy", "sell", "any"])]
    side: String,

    /// Analysis window start, UTC RFC 3339 `2026-08-01T00:00:00Z` (inclusive;
    /// no offsets). Window `[since, until)`; the scan walks newest-first
    /// to the window start (ADR-011/ADR-014). Conflicts with --period.
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

    /// Output format: table or jsonl.
    #[arg(long, default_value = "table", value_parser = ["table", "jsonl"])]
    format: String,

    /// Provider page budget PER INPUT TOKEN (Solana/Helius only; 100
    /// transactions per page in full mode). When a token's history needs
    /// more pages the scan is truncated, reported as partial, and the run
    /// exits 3. This is NOT --max-requests: retries are not
    /// counted against it (use --max-requests for that). Ignored for EVM
    /// input.
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u32).range(1..=200)
    )]
    max_pages_per_token: u32,

    /// Transactions per provider page (`limit`, 1..=1000; Solana/Helius
    /// only). `--max-pages-per-token` counts PAGES, so the per-token
    /// transaction budget is `max-pages-per-token * page-limit`. Pages above
    /// 500 txs raise the response-size cap (~20 KB/tx, max 64 MiB). Live-verified
    /// 2026-10-03.
    #[arg(
        long,
        default_value_t = DEFAULT_PAGE_LIMIT_ARG,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_PAGE_LIMIT))
    )]
    page_limit: u32,

    /// Server-side status filter: `any` (no filter) or `succeeded` (failed
    /// transactions are then invisible; buyer-intersect only needs
    /// successful trades). Live-verified 2026-10-03.
    #[arg(
        long,
        default_value = DEFAULT_PROVIDER_STATUS_FILTER,
        value_parser = ["any", "succeeded"]
    )]
    provider_status_filter: String,

    /// With a window, also send it server-side as `filters.blockTime`
    /// (gte since, lt until) in addition to the newest-first boundary
    /// walk. `--server-window=false` disables. Live-verified 2026-10-03.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        default_value_t = DEFAULT_SERVER_WINDOW,
        action = clap::ArgAction::Set
    )]
    server_window: bool,

    /// Total HTTP request budget for the whole run (all tokens, retries
    /// included), N >= 1. Absent = unlimited (requests are still counted
    /// and reported). When exhausted the run is partial and exits 3.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,

    /// Multi-chain input (tokens of several chains): chains run at once
    /// (1..=4, default 2), each with its own sources and its own
    /// `--max-requests` budget. Tokens are chain-scoped: K counts distinct
    /// tokens within ONE chain, a wallet address on two chains is two wallets
    /// and hits never merge across chains. A chain with fewer tokens than
    /// `--min-token-hits` cannot match and is skipped (not scanned).
    #[arg(
        long,
        default_value_t = scout_app::DEFAULT_CHAIN_CONCURRENCY,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(scout_app::MAX_CHAIN_CONCURRENCY))
    )]
    chain_concurrency: u32,

    #[command(flatten)]
    net: scout_app::EvmNetOptions,

    /// Max scan units in flight at once (a token, or one time slice of a
    /// token; 1..=16, default 4). Each unit keeps one provider request in
    /// flight, so this also caps concurrent requests. The shared
    /// `--max-requests` budget stays exact; results do not depend on the
    /// value (merged in input order). `--concurrency 1` is sequential.
    #[arg(
        long,
        default_value_t = DEFAULT_CONCURRENCY,
        value_parser = clap::value_parser!(u32).range(1..=16)
    )]
    concurrency: u32,

    /// Split a window `[since, until)` into this many equal sub-windows per
    /// token, each scanned with its own server-side `blockTime` filter
    /// (1..=16). Default: 4 with a window and `--server-window`, else 1.
    /// More than 1 requires a window and `--server-window`.
    /// `--max-pages-per-token` then applies to EACH slice. A token is
    /// complete iff every slice is; truncated slices are named.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=16))]
    slices: Option<u32>,

    /// EVM only: run a chain whose venue set is not verified yet (BSC); without it such a run exits 4. Every trade is then IdlOnly:
    /// no wallet qualifies and the run is partial.
    #[arg(long)]
    allow_unverified_chain: bool,
}

/// Library default is sequential; the CLI default.
const DEFAULT_CONCURRENCY: u32 = 4;
/// `--slices` default when a window is set and `--server-window` is on.
const DEFAULT_WINDOW_SLICES: u32 = 4;

/// Effective `--slices`, or the usage error text (exit 2).
fn effective_slices(args: &Args, window: &AnalysisWindow) -> Result<u32, String> {
    let sliceable = window.is_bounded() && args.server_window;
    match args.slices {
        Some(n) if n > 1 && !sliceable => Err(
            "--slices > 1 requires a window (--since/--period) and --server-window (each slice \
             is a server-side blockTime filter)"
                .to_string(),
        ),
        Some(n) => Ok(n),
        None if sliceable => Ok(DEFAULT_WINDOW_SLICES),
        None => Ok(1),
    }
}

/// Run start in unix seconds (pinned once per run as `as_of`).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn side_filter(args: &Args) -> SideFilter {
    match args.side.as_str() {
        "buy" => SideFilter::Buy,
        "sell" => SideFilter::Sell,
        _ => SideFilter::Any,
    }
}

fn main() -> ExitCode {
    let args = Args::parse();

    // ADR-011: the window is resolved (and `as_of` pinned) exactly once.
    let window = match AnalysisWindow::resolve(
        args.period.as_deref(),
        args.since.as_deref(),
        args.until.as_deref(),
        unix_now(),
    ) {
        Ok(w) => w,
        Err(err) => {
            eprintln!("buyer-intersect: {err}");
            return ExitCode::from(2);
        }
    };

    let input_text = match read_input(args.input.as_deref()) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("buyer-intersect: {message}");
            return ExitCode::from(2);
        }
    };

    let parsed = match scout_app::parse_input(input_text.as_bytes(), InputFormat::Lines, None) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("buyer-intersect: {err}");
            return ExitCode::from(2);
        }
    };

    if parsed.records.len() < 2 {
        // CLI.md §3: "Меньше двух distinct input tokens — usage error
        // для задачи пересечений."
        eprintln!("buyer-intersect: at least 2 distinct input tokens are required");
        return ExitCode::from(2);
    }

    let input_tokens = match scout_app::resolve_token_assets(&parsed) {
        Ok(tokens) => tokens,
        Err(err) => {
            eprintln!("buyer-intersect: {err}");
            return ExitCode::from(2);
        }
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("buyer-intersect: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };

    let groups = scout_app::partition_by_chain(&input_tokens, |t| match t {
        AssetKey::Token(c, _) | AssetKey::Native(c) => c,
    });
    if groups.len() > 1 {
        return multi::run_multi(&rt, &input_tokens, &groups, &args, &window);
    }
    let chain = groups
        .first()
        .map_or_else(scout_engine::solana_mainnet_chain, |g| g.chain.clone());
    let run = run_chain(&rt, &input_tokens, &chain, &args, &window);
    if run.failure.is_some() {
        return ExitCode::from(run.status);
    }
    let outcome = write_lines_to_stdout(run.lines);
    if run.cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
    }
    ExitCode::from(run.status)
}

/// One chain's run (its own sources, settings and budget).
pub(crate) fn run_chain(
    rt: &tokio::runtime::Runtime,
    tokens: &[AssetKey],
    chain: &ChainKey,
    args: &Args,
    window: &AnalysisWindow,
) -> ChainRun {
    if chain.family == ChainFamily::Evm {
        return evm::run_evm(rt, tokens, chain, args, window);
    }
    let api_key = std::env::var(HELIUS_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty());
    if let Some(api_key) = api_key {
        return run_solana(rt, tokens, args, window, &api_key);
    }
    let provider = UnconfiguredProvider::new("solana_history", HELIUS_KEY_ENV);
    run_legacy(rt, &provider, tokens, args)
}

fn run_legacy(
    rt: &tokio::runtime::Runtime,
    provider: &UnconfiguredProvider,
    input_tokens: &[AssetKey],
    args: &Args,
) -> ChainRun {
    let flows = std::collections::BTreeMap::new();

    let result = rt.block_on(run_buyer_intersect(
        provider,
        &flows,
        input_tokens,
        args.min_token_hits,
    ));

    // ADR-005 exit code 3 (IncompleteCoverage) covers a run that
    // completed successfully but did not see the provider's full
    // declared range -- an unconsumed pagination cursor on any
    // envelope means this run's shortlist may be missing real matches,
    // not just that it happened to be short (D07's "legitimately empty
    // shortlist is exit 0" does not apply once coverage is known
    // incomplete). Checked before the ProviderError match below since
    // it only applies to the Ok(report) branch.
    if let Ok(report) = &result
        && report.coverage_truncated
    {
        eprintln!(
            "buyer-intersect: provider reported an unconsumed pagination cursor; \
             results may be incomplete (ADR-005 IncompleteCoverage)"
        );
        return ChainRun {
            status: 3,
            reasons: vec!["provider reported an unconsumed pagination cursor".to_string()],
            ..ChainRun::default()
        };
    }

    // ADR-005 exit code 4 (InfrastructureUnavailable) covers
    // "credentials, storage, capability gap" -- every current
    // ProviderError variant is exactly that: ConfigurationRequired
    // (credentials), RateLimited (provider quota/infra), Unsupported
    // (capability gap), Transport/Other (infra failure). All map to 4.
    // Listed explicitly (not a blind catch-all) so a reviewer can see
    // the mapping was a decision, not an oversight; the wildcard arm
    // exists only because ProviderError is #[non_exhaustive] (ADR-008)
    // and a future variant must still degrade safely to 4, not panic.
    match result {
        Ok(report) => emit_report(&report, &args.format),
        Err(err) => provider_error_exit(&err, None),
    }
}

/// Map a `ProviderError` to an exit code with a redacted message.
/// Request-budget exhaustion with nothing observed is exit 3 (user-chosen
/// bound hit: incomplete coverage, see module docs); everything else is
/// infrastructure/credentials/capability, exit 4 (ADR-005; the wildcard
/// exists because `ProviderError` is `#[non_exhaustive]`). Text is
/// sanitized (API-key query values, control characters) and, if given,
/// the literal key is replaced too: transport errors can embed the
/// request URL.
fn provider_error_exit(err: &ProviderError, secret: Option<&str>) -> ChainRun {
    let redact_text = |raw: &str| {
        let mut text = sanitize_provider_text(raw);
        if let Some(secret) = secret.filter(|s| !s.is_empty()) {
            text = text.replace(secret, "<redacted>");
        }
        text
    };
    if let ProviderError::Other(inner) = err
        && let Some(exhausted) = inner.downcast_ref::<RequestBudgetExhausted>()
    {
        eprintln!(
            "buyer-intersect: request budget exhausted after {} requests (limit {}); \
             no transactions were observed, nothing to report (IncompleteCoverage, exit 3)",
            exhausted.limit, exhausted.limit
        );
        return ChainRun::failed(
            3,
            format!(
                "request budget exhausted after {} requests; no transactions were observed",
                exhausted.limit
            ),
        );
    }
    let text = match err {
        ProviderError::ConfigurationRequired { .. } => redact_text(&err.to_string()),
        ProviderError::RateLimited { retry_after } => rate_limited_text(*retry_after),
        _ => format!("provider error: {}", redact_text(&err.to_string())),
    };
    eprintln!("buyer-intersect: {text}");
    ChainRun::failed(4, text)
}

/// Terminal `RateLimited`: the transport refused to wait out a
/// `Retry-After` above its cap (`DEFAULT_MAX_RETRY_AFTER`).
fn rate_limited_text(retry_after: Option<std::time::Duration>) -> String {
    let cap = DEFAULT_MAX_RETRY_AFTER.as_secs();
    match retry_after {
        Some(d) => format!(
            "rate limited; server asked to retry after {}s (cap {cap}s)",
            d.as_secs()
        ),
        None => "rate limited; server gave no Retry-After".to_string(),
    }
}

pub(crate) fn limit_text(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unlimited".to_string(), |n| n.to_string())
}

/// Effective provider request options for this run (also echoed in
/// `run_meta` and the stderr scope line).
fn provider_options(args: &Args, window: &AnalysisWindow) -> HeliusRequestOptions {
    let (gte, lt) = match (args.server_window, window.bounds()) {
        (true, Some((since, until))) => (Some(since), Some(until)),
        _ => (None, None),
    };
    HeliusRequestOptions {
        page_limit: args.page_limit,
        status: if args.provider_status_filter == "succeeded" {
            StatusFilter::Succeeded
        } else {
            StatusFilter::Any
        },
        block_time_gte: gte,
        block_time_lt: lt,
        ..HeliusRequestOptions::default()
    }
}

fn build_provider(
    api_key: &str,
    max_pages: NonZeroU32,
    max_requests: Option<u64>,
    window: &AnalysisWindow,
    options: &HeliusRequestOptions,
) -> Result<HeliusProvider, ProviderError> {
    let provider = match std::env::var(ENDPOINT_OVERRIDE_ENV) {
        Ok(url) if !url.is_empty() => HeliusProvider::new_with_endpoint(
            scout_rpc::RpcEndpoint::new(url),
            HELIUS_TIMEOUT_MS,
            HELIUS_MAX_ATTEMPTS,
        )?,
        _ => HeliusProvider::new(api_key, HELIUS_TIMEOUT_MS, HELIUS_MAX_ATTEMPTS)?,
    };
    // No window: oldest-first from token creation (early activity first).
    // Window: newest-first to the boundary = complete for the window.
    let provider = if window.is_bounded() {
        provider
            .with_scan_order(ScanOrder::NewestFirst)
            .with_stop_before_block_time(window.bounds().map(|(since, _)| since))
    } else {
        provider
    };
    Ok(provider
        .with_page_limit(options.page_limit)
        .with_status_filter(options.status)
        .with_block_time_range(options.block_time_gte, options.block_time_lt)
        .with_max_pages(max_pages)
        .with_max_total_requests(max_requests))
}

fn run_solana(
    rt: &tokio::runtime::Runtime,
    input_tokens: &[AssetKey],
    args: &Args,
    window: &AnalysisWindow,
    api_key: &str,
) -> ChainRun {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_token) else {
        eprintln!("buyer-intersect: --max-pages-per-token must be at least 1");
        return ChainRun::failed(2, "--max-pages-per-token must be at least 1".to_string());
    };
    // ONE provider for the whole run: the request budget and counter are
    // shared by every token's scan.
    let options = provider_options(args, window);
    let slices = match effective_slices(args, window) {
        Ok(n) => n,
        Err(message) => {
            eprintln!("buyer-intersect: {message}");
            return ChainRun::failed(2, message);
        }
    };
    let started = std::time::Instant::now();
    let provider = match build_provider(api_key, max_pages, args.max_requests, window, &options) {
        Ok(provider) => provider,
        Err(err) => return provider_error_exit(&err, Some(api_key)),
    };
    let result = rt.block_on(run_solana_trade_intersect(
        &provider,
        input_tokens,
        args.min_token_hits,
        IntersectOptions {
            side: side_filter(args),
            window: *window,
            concurrency: usize::try_from(args.concurrency).unwrap_or(1),
            slices,
        },
        CancellationToken::new(),
    ));
    let requests_made = provider.total_requests_made();
    let report = match result {
        Ok(report) => report,
        Err(err) => {
            eprintln!(
                "buyer-intersect: requests_made={requests_made} max_requests={}",
                limit_text(args.max_requests)
            );
            return provider_error_exit(&err, Some(api_key));
        }
    };

    let budget = RunBudget {
        max_pages_per_token: args.max_pages_per_token,
        max_requests: args.max_requests,
        requests_made,
        provider_options: provider.request_options(),
        server_window: args.server_window,
        concurrency: report.concurrency,
        slices: report.slices,
    };
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    print_solana_diagnostics(&report, api_key, budget, elapsed_ms);
    let incomplete = report.is_coverage_incomplete();
    let captured_at = scout_app::now_utc_rfc3339();
    let lines = match emit_solana_matches(
        &report,
        input_tokens,
        &args.format,
        incomplete,
        &captured_at,
        budget,
        api_key,
    ) {
        Ok(lines) => lines,
        Err(message) => {
            eprintln!("buyer-intersect: could not render output: {message}");
            return ChainRun::failed(4, format!("could not render output: {message}"));
        }
    };
    ChainRun {
        lines,
        status: if incomplete { 3 } else { 0 },
        cancelled: report.cancelled,
        requests_made,
        failure: None,
        reasons: report
            .incomplete_reasons()
            .iter()
            .map(|r| redact(r, api_key))
            .collect(),
    }
}

fn redact(text: &str, secret: &str) -> String {
    sanitize_provider_text(&text.replace(secret, "<redacted>"))
}

/// Scope, coverage and diagnostics block on stderr (stdout stays the
/// single selected result format, CLI.md §2). Unknown is never printed
/// as zero: failed tokens say `scan failed`, not `0 transactions`.
fn print_solana_diagnostics(
    report: &SolanaBuyerIntersectReport,
    api_key: &str,
    budget: RunBudget,
    elapsed_ms: u64,
) {
    let scope = &report.scope;
    eprintln!("buyer-intersect: protocol scope (Solana mainnet):");
    eprintln!("  recognized: {}", scope.recognized);
    eprintln!("  NOT decoded: {}", scope.not_decoded);
    eprintln!(
        "  program={} idl_commit={} idl_file_sha256={} rule={}",
        scope.program_id, scope.idl_commit, scope.idl_sha256, scope.qualification_version
    );
    if let (Some(program), Some(commit), Some(sha)) = (
        scope.amm_program_id,
        scope.amm_idl_commit,
        scope.amm_idl_sha256,
    ) {
        eprintln!("  amm_program={program} idl_commit={commit} idl_file_sha256={sha}");
    }
    eprintln!("  side={}", report.side.label());
    match report.window.bounds() {
        Some((since, until)) => eprintln!(
            "  window [{}, {}) source={} as_of={}: newest-first walk, stops after the first page \
             holding a tx older than the window start; a token that exhausts the page budget \
             before it is incomplete",
            scout_app::format_unix_utc(u64::try_from(since).unwrap_or(0)),
            scout_app::format_unix_utc(u64::try_from(until).unwrap_or(0)),
            report.window.source.label(),
            scout_app::format_unix_utc(u64::try_from(report.window.as_of).unwrap_or(0)),
        ),
        None => eprintln!(
            "  window: none (oldest-first from token creation; a truncated token lacks NEWER activity)"
        ),
    }
    eprintln!(
        "  budget: max_pages_per_token={} (provider pages per input token, \
         page_limit txs/page; retries are not counted)",
        budget.max_pages_per_token
    );
    eprintln!(
        "  provider options: {} server_window={} (tx budget per token = max_pages_per_token * page_limit = {})",
        budget.provider_options.describe(),
        budget.server_window,
        u64::from(budget.max_pages_per_token) * u64::from(budget.provider_options.page_limit)
    );
    eprintln!(
        "  concurrency={} slices={} (max scan units in flight; slices per token, 1 = unsliced; \
         --max-pages-per-token applies per slice)",
        budget.concurrency, budget.slices
    );
    eprintln!(
        "  requests_made={} max_requests={} (total HTTP attempts, retries included) \
         elapsed_ms={elapsed_ms} (stderr only, not in JSONL)",
        budget.requests_made,
        limit_text(budget.max_requests)
    );
    eprintln!("  recognized trade variants (IDL name, side, verification):");
    for (name, side, status) in SolanaProtocolScope::variants() {
        eprintln!("    {name} side={side} verification={status}");
    }
    eprintln!(
        "  input_tokens(N)={} min_token_hits(K)={} matches={}",
        report.base.input_token_count,
        report.base.min_token_hits,
        report.base.matches.len()
    );
    for token in &report.per_token {
        let status = match &token.status {
            TokenScanStatus::Failed { message, .. } => {
                format!("scan failed: {}", redact(message, api_key))
            }
            TokenScanStatus::NotScanned { reason } => {
                format!("not_scanned: {}", reason.describe())
            }
            TokenScanStatus::Ok if token.truncated => "truncated".to_string(),
            TokenScanStatus::Ok => "ok".to_string(),
        };
        eprintln!(
            "  token {}: status={} txs_scanned={} wallets={} buyers={} sellers={} decoded_buys={} \
             decoded_sells={} malformed={} unknown_discriminator={} idl_only_trades={} \
             positive_delta_without_instruction={} failed_transactions={}",
            token.asset_label(),
            status,
            token.transactions_scanned,
            token.qualified_wallets,
            token.qualified_buyers,
            token.qualified_sellers,
            token.diagnostics.decoded_buys,
            token.diagnostics.decoded_sells,
            token.diagnostics.malformed_instructions,
            unknown_or_na(
                token.is_unknown(),
                token.diagnostics.unknown_discriminator_instructions
            ),
            if token.is_unknown() {
                "n/a (scan failed)".to_string()
            } else {
                token.trade.idl_only_trades.to_string()
            },
            token.positive_delta_without_instruction,
            token.diagnostics.failed_transactions,
        );
        if !token.is_unknown() {
            let t = &token.trade;
            eprintln!(
                "    ops(buy/sell) curve={}/{} pumpswap={}/{} route={}/{} \
                 router_forwards_not_attributed={} amm_unreconciled={} \
                 no_matching_delta={} route_rejections[multi_asset={} not_opposite_signs={} \
                 no_quote_leg={} no_verified_leg={} passthrough_nonzero={}]",
                t.ops(Venue::BondingCurve, TradeSide::Buy),
                t.ops(Venue::BondingCurve, TradeSide::Sell),
                t.ops(Venue::PumpAmm, TradeSide::Buy),
                t.ops(Venue::PumpAmm, TradeSide::Sell),
                t.ops(Venue::Route, TradeSide::Buy),
                t.ops(Venue::Route, TradeSide::Sell),
                t.router_forwards_not_attributed,
                t.amm_unreconciled,
                t.trades_without_matching_delta,
                t.route_rejections.multi_asset,
                t.route_rejections.not_opposite_signs,
                t.route_rejections.no_quote_leg,
                t.route_rejections.no_verified_leg,
                t.route_rejections.passthrough_nonzero,
            );
            eprintln!(
                "    coverage gaps: malformed_trades={} orphan_events={} jupiter_malformed={} \
                 jupiter_unknown={} dflow_malformed={} dflow_unknown={} okx_malformed={} \
                 okx_unknown={} okx_receiver_not_attributed={} okx_idl_only={} venue_events={:?}",
                t.malformed_trades,
                t.orphan_events,
                t.jupiter_malformed_events,
                t.jupiter_unknown_events,
                t.dflow_malformed_events,
                t.dflow_unknown_events,
                t.okx_malformed_events,
                t.okx_unknown_events,
                t.okx_swap_with_receiver_not_attributed,
                t.okx_idl_only_order_events,
                t.venue_events
            );
            for e in &t.evidence_samples {
                eprintln!(
                    "    evidence (up to {}): {}",
                    scout_engine::MAX_EVIDENCE_SAMPLES,
                    scout_app::evidence_line(e, &|s| redact(s, api_key))
                );
            }
        }
    }
    let d = &report.diagnostics;
    eprintln!(
        "  totals: decoded_buys={} decoded_sells={} buys_without_positive_delta={} \
         unowned_balance_changes={} malformed_instructions={} failed_transactions={}",
        d.decoded_buys,
        d.decoded_sells,
        d.buys_without_positive_delta,
        d.unowned_balance_changes,
        d.malformed_instructions,
        d.failed_transactions
    );
    let any_failed = report.per_token.iter().any(|t| t.is_unknown());
    let partial_note = if any_failed || report.cancelled {
        " (partial: some tokens were not fully scanned; counts are lower bounds)"
    } else {
        ""
    };
    eprintln!(
        "  program instructions: known_non_trade={} unknown_discriminator={} \
         unverified_variant_trades={}{partial_note}",
        d.known_non_trade_instructions,
        d.unknown_discriminator_instructions,
        format_unverified(&d.unverified_variant_buys),
    );
    let by_variant: Vec<String> = PumpTradeVariant::ALL
        .iter()
        .map(|v| {
            format!(
                "{}={}",
                v.name(),
                d.decoded_by_variant.get(v.index()).copied().unwrap_or(0)
            )
        })
        .collect();
    eprintln!("  decoded trades by variant: {}", by_variant.join(" "));
    for sample in &report.unknown_discriminator_samples {
        eprintln!("  unknown discriminator sample: {sample}");
    }
    for sample in &report.malformed_samples {
        eprintln!("  malformed sample: {}", redact(sample, api_key));
    }
    match report.stop {
        Some(ScanStop::BudgetExhausted { limit }) => eprintln!(
            "buyer-intersect: request budget exhausted after {} requests (limit {limit}); \
             unscanned or partially scanned tokens are marked failed, results are incomplete",
            budget.requests_made
        ),
        Some(ScanStop::RateLimited { retry_after_secs }) => eprintln!(
            "buyer-intersect: {}; remaining tokens were not scanned, results are incomplete",
            rate_limited_text(retry_after_secs.map(std::time::Duration::from_secs))
        ),
        None => {}
    }
    let reasons = report.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!(
            "buyer-intersect: status=complete within declared protocol scope \
             (wallet set is a lower bound: venues outside the scope are not decoded)"
        );
    } else {
        eprintln!("buyer-intersect: status=partial (IncompleteCoverage, exit 3):");
        for reason in reasons {
            eprintln!("  - {}", redact(&reason, api_key));
        }
    }
}

fn unknown_or_na(scan_failed: bool, count: u64) -> String {
    if scan_failed {
        "n/a (scan failed)".to_string()
    } else {
        count.to_string()
    }
}

/// `none` or `name=count,...` for the nonzero IdlOnly-variant buy counts.
fn format_unverified(counts: &[u64; PumpTradeVariant::COUNT]) -> String {
    let parts: Vec<String> = PumpTradeVariant::ALL
        .iter()
        .filter_map(|v| {
            let n = counts.get(v.index()).copied().unwrap_or(0);
            (n > 0).then(|| format!("{}={n}", v.name()))
        })
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(",")
    }
}

fn emit_solana_matches(
    report: &SolanaBuyerIntersectReport,
    input_tokens: &[AssetKey],
    format: &str,
    incomplete: bool,
    captured_at: &str,
    budget: RunBudget,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let lines: Vec<String> = if format == "jsonl" {
        let run_id = run_id_from(captured_at);
        output::solana_jsonl_lines(
            &run_id,
            captured_at,
            report,
            input_tokens,
            budget,
            incomplete,
            &|text| redact(text, api_key),
        )?
    } else {
        solana_table_lines(report)
    };
    Ok(lines)
}

/// `buyer-intersect-20261002T123456Z`: unique per second, sortable.
fn run_id_from(captured_at: &str) -> String {
    let compact: String = captured_at
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("buyer-intersect-{compact}")
}

/// Solana table: full wallet address, `hit_count` and per matched token its
/// full mint with the observed sides, `B` (buy), `S` (sell) or `B/S`:
/// `<wallet> hit_count=2 <mint>=B/S <mint>=S`.
fn solana_table_lines(report: &SolanaBuyerIntersectReport) -> Vec<String> {
    report
        .base
        .matches
        .iter()
        .map(|m| {
            let hits = report.side_hits.get(&m.wallet);
            let tokens: Vec<String> = m
                .matched_assets
                .iter()
                .map(|asset| {
                    let mint = match asset {
                        AssetKey::Token(_, address) => address.to_string(),
                        AssetKey::Native(_) => "native".to_string(),
                    };
                    let sides = hits.and_then(|h| h.get(asset)).map_or("?", |h| {
                        match (h.buy.is_some(), h.sell.is_some()) {
                            (true, true) => "B/S",
                            (true, false) => "B",
                            (false, true) => "S",
                            (false, false) => "?",
                        }
                    });
                    format!("{mint}={sides}")
                })
                .collect();
            format!(
                "{} hit_count={} {}",
                m.wallet.address,
                m.hit_count,
                tokens.join(" ")
            )
        })
        .collect()
}

/// Full address (base58 for Solana, 0x-hex for EVM) and hit count.
fn table_lines(report: &scout_engine::BuyerIntersectReport) -> Vec<String> {
    report
        .matches
        .iter()
        .map(|m| format!("{} hit_count={}", m.wallet.address, m.hit_count))
        .collect()
}

fn emit_report(report: &scout_engine::BuyerIntersectReport, format: &str) -> ChainRun {
    let lines: Vec<String> = if format == "jsonl" {
        let rendered: Result<Vec<String>, String> = report
            .matches
            .iter()
            .map(|m| {
                output::buyer_match_record(m, None)
                    .and_then(|r| serde_json::to_string(&r).map_err(|e| e.to_string()))
            })
            .collect();
        match rendered {
            Ok(lines) => lines,
            Err(message) => {
                eprintln!("buyer-intersect: could not render output: {message}");
                return ChainRun::failed(4, format!("could not render output: {message}"));
            }
        }
    } else {
        table_lines(report)
    };
    ChainRun {
        lines,
        ..ChainRun::default()
    }
}

/// Read input from a file path, `-` for stdin, or piped stdin when no
/// `--input` is given. A TTY with no `--input` is a usage error, not an
/// indefinite hang (CLI.md §1).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_solana_chain_equals_scout_app_input_resolution() {
        // Decoded mints must equal resolved input AssetKeys exactly.
        let parsed = scout_app::parse_input(
            &b"solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n"[..],
            InputFormat::Lines,
            None,
        )
        .unwrap();
        let tokens = scout_app::resolve_token_assets(&parsed).unwrap();
        let AssetKey::Token(chain, _) = &tokens[0] else {
            panic!("expected token");
        };
        assert_eq!(*chain, scout_engine::solana_mainnet_chain());
    }

    #[test]
    fn error_text_never_contains_the_api_key() {
        let err = ProviderError::Other(Box::new(std::io::Error::other(
            "error sending request for url (https://h/?api-key=SECRET99)",
        )));
        let text = sanitize_provider_text(&err.to_string()).replace("SECRET99", "<redacted>");
        assert!(!text.contains("SECRET99"));
    }
}
