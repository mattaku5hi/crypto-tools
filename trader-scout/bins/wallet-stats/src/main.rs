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
//! USD (ADR-018): unless `--no-usd`, the ledger's realized figures are also
//! valued in USD (Coinbase Exchange public 1m candles; `--max-price-requests`
//! bounds the price HTTP attempts, counted apart as `requests_made_prices`;
//! `SCOUT_COINBASE_ENDPOINT` overrides the URL for tests). A leg without a
//! price is `price_unknown`, never zero; unpriced legs are NOT a failure
//! (like unknown basis), a spent price budget is (exit 3).
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
    pump_bonding_curve_decoder, run_solana_wallet_stats_concurrent, sanitize_provider_text,
};
use scout_pricing::PriceSource as _;
use scout_providers::{
    HeliusProvider, HeliusRequestOptions, MAX_PAGE_LIMIT, ScanOrder, TokenAccountsFilter,
};
use scout_rpc::DEFAULT_MAX_RETRY_AFTER;
use tokio_util::sync::CancellationToken;

mod evm;
mod output;

use output::{Detail, RunMetaInput, SortMode};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
/// Test-only: send requests to this URL instead of Helius (the API key is
/// never sent to the override).
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_WALLET_STATS_ENDPOINT";
const HELIUS_TIMEOUT_MS: u64 = 30_000;
const HELIUS_MAX_ATTEMPTS: u32 = 3;

// Defaults of the provider request options: live-verified 2026-10-03, see
// docs/p0/measurements/2026-10-03-helius-filters-live.md. The opt-out flags
// reproduce the legacy request. No status filter is ever applied to wallet
// scans: failed transactions still cost fees (ADR-004).
/// `--page-limit` default (`limit` per `getTransactionsForAddress` page).
const DEFAULT_PAGE_LIMIT_ARG: u32 = 500;
/// `--server-window` default: send the window as `filters.blockTime`.
const DEFAULT_SERVER_WINDOW: bool = true;
/// `--token-accounts` default: `none` or `balance-changed`.
const DEFAULT_TOKEN_ACCOUNTS: &str = "balance-changed";

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

    /// Transactions per provider page (`limit`, 1..=1000). The page budget
    /// stays in PAGES: per-wallet transaction budget is
    /// `max-pages-per-wallet * page-limit`. Pages above 500 txs raise the
    /// response-size cap (~20 KB/tx, max 64 MiB). Live-verified 2026-10-03.
    #[arg(
        long,
        default_value_t = DEFAULT_PAGE_LIMIT_ARG,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_PAGE_LIMIT))
    )]
    page_limit: u32,

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

    /// `filters.tokenAccounts`: `none` (only txs referencing the wallet) or
    /// `balance-changed` (also txs changing the balance of a token account
    /// the wallet owns; Helius-recommended). Live-verified 2026-10-03.
    #[arg(
        long,
        default_value = DEFAULT_TOKEN_ACCOUNTS,
        value_parser = ["none", "balance-changed"]
    )]
    token_accounts: String,

    /// Max wallets scanned at once (1..=16, default 4). Each wallet scan
    /// keeps one provider request in flight, so this also caps concurrent
    /// requests. The shared `--max-requests` budget stays exact; results do
    /// not depend on the value (cards merged in input order). `1` =
    /// sequential. Time slicing is not available for wallets.
    #[arg(
        long,
        default_value_t = 4,
        value_parser = clap::value_parser!(u32).range(1..=16)
    )]
    concurrency: u32,

    /// Total HTTP request budget for the whole run (all wallets, retries
    /// included), N >= 1. Absent = unlimited (requests are still counted
    /// and reported). When exhausted the run stops: the interrupted wallet
    /// is `error`, the rest `not_scanned`; exit 3.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,

    #[command(flatten)]
    net: scout_app::EvmNetOptions,

    /// Skip USD pricing (ADR-018). By default the realized figures are also
    /// valued in USD with Coinbase Exchange one-minute candles (SOL-USD,
    /// USDT-USD; USDC at par, labelled), cached per 300-minute page.
    #[arg(long)]
    no_usd: bool,

    /// Total HTTP request budget for PRICE candles (retries included), N >= 1.
    /// Separate from --max-requests (`requests_made_prices` is counted
    /// apart). When exhausted the remaining legs are `price_unknown`
    /// (`request_budget_exhausted`), the run is incomplete (exit 3).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_price_requests: Option<u64>,

    /// Skip the ADR-019 realizable valuation of open positions (no
    /// `getMultipleAccounts` requests). By default, on live runs (window
    /// ending at the run start, or no window) every open position on a
    /// pump.fun curve / PumpSwap pool is valued as a constant-product sell of
    /// its whole amount on live state; historical windows stay unvalued.
    #[arg(long)]
    no_valuation: bool,

    /// EVM only: run a chain whose venue/quote set is not verified yet
    /// (BSC). Without it such a run exits 4. Every trade there is
    /// IdlOnly, so the run is partial.
    #[arg(long)]
    allow_unverified_chain: bool,

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

    // A run is about ONE chain family (never a silent partial list).
    let family = match scout_app::run_family(wallets.iter().map(|w| &w.chain), "wallet-stats") {
        Ok(f) => f,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if let scout_app::RunFamily::Evm(chain) = &family {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(err) => {
                eprintln!("wallet-stats: could not start async runtime: {err}");
                return ExitCode::from(4);
            }
        };
        return evm::run_evm(
            &rt,
            &wallets,
            chain,
            &args,
            &window,
            parsed.duplicate_count,
            upstream_complete,
        );
    }
    let solana: Vec<SolanaPubkey> = wallets
        .iter()
        .filter_map(|w| match (&w.chain.family, &w.address) {
            (ChainFamily::Solana, AddressBytes::Solana(a)) => Some(*a),
            _ => None,
        })
        .collect();

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

/// Outcome of the ADR-018 pricing step (all `None` with `--no-usd`).
struct PricingOutcome {
    policy: Option<scout_pricing::PricePolicy>,
    endpoint_overridden: bool,
    requests_made: u64,
    max_requests: Option<u64>,
    run: Option<scout_engine::UsdPricingRun>,
}

impl PricingOutcome {
    fn input(&self) -> scout_app::PricingMetaInput<'_> {
        scout_app::PricingMetaInput {
            policy: self.policy.as_ref(),
            endpoint_overridden: self.endpoint_overridden,
            requests_made: self.requests_made,
            max_requests: self.max_requests,
            run: self.run.as_ref(),
        }
    }
}

/// ADR-018: value every wallet ledger in USD (unless `--no-usd`): collect
/// the needed minutes of all wallets, prefetch once, apply the views.
pub(crate) fn price_wallets(
    rt: &tokio::runtime::Runtime,
    report: &mut SolanaWalletStatsReport,
    args: &Args,
) -> PricingOutcome {
    let mut out = PricingOutcome {
        policy: None,
        endpoint_overridden: false,
        requests_made: 0,
        max_requests: args.max_price_requests,
        run: None,
    };
    if args.no_usd {
        return out;
    }
    let built = if let Some(evm) = &report.evm {
        if evm.chain.native_label == "bnb" {
            scout_app::build_coinbase_source_evm_bsc(args.max_price_requests)
        } else {
            scout_app::build_coinbase_source_evm(args.max_price_requests)
        }
    } else {
        scout_app::build_coinbase_source(args.max_price_requests)
    };
    match built {
        Ok((source, overridden)) => {
            out.endpoint_overridden = overridden;
            out.policy = Some(source.policy());
            out.run = Some(rt.block_on(scout_engine::apply_usd_pricing(
                &mut report.wallets,
                &source,
            )));
            out.requests_made = source.requests_made();
        }
        Err(message) => eprintln!("wallet-stats: {message}; continuing without USD figures"),
    }
    out
}

pub(crate) fn redact(text: &str, secret: &str) -> String {
    let replaced = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    };
    sanitize_provider_text(&replaced)
}

/// Table lines are built from our own typed values: strip the secret only.
/// (`redact` also caps length at 300 chars, which would cut the wide table.)
fn redact_table_line(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    }
}

pub(crate) fn limit_text(limit: Option<u64>) -> String {
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
pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Effective provider request options for this run (also echoed in
/// `run_meta` and the stderr scope line). Never sets a status filter.
fn provider_options(args: &Args, window: &AnalysisWindow) -> HeliusRequestOptions {
    let (gte, lt) = match (args.server_window, window.bounds()) {
        (true, Some((since, until))) => (Some(since), Some(until)),
        _ => (None, None),
    };
    HeliusRequestOptions {
        page_limit: args.page_limit,
        block_time_gte: gte,
        block_time_lt: lt,
        token_accounts: if args.token_accounts == "balance-changed" {
            TokenAccountsFilter::BalanceChanged
        } else {
            TokenAccountsFilter::None
        },
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
    Ok(provider
        .with_page_limit(options.page_limit)
        .with_block_time_range(options.block_time_gte, options.block_time_lt)
        .with_token_accounts(options.token_accounts)
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
    let options = provider_options(args, window);
    let started = std::time::Instant::now();
    let provider = match build_provider(api_key, max_pages, args.max_requests, window, &options) {
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
    let result = rt.block_on(run_solana_wallet_stats_concurrent(
        &provider,
        solana,
        &LedgerDecoders {
            curve: &decoder,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        },
        window,
        usize::try_from(args.concurrency).unwrap_or(1),
        CancellationToken::new(),
    ));
    let requests_made = provider.total_requests_made();
    let mut report = match result {
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
    // ADR-019: realizable valuation of open positions (live runs only); its
    // account reads share the run's request budget.
    let valuation = if args.no_valuation {
        None
    } else {
        Some(rt.block_on(scout_engine::apply_open_valuation(
            &mut report.wallets,
            &provider,
            window,
        )))
    };
    let requests_made = provider.total_requests_made();
    eprintln!(
        "{}",
        scout_app::open_valuation_line("wallet-stats", valuation.as_ref())
    );
    let pricing = price_wallets(rt, &mut report, args);
    let pricing_input = pricing.input();
    eprintln!(
        "{}",
        scout_app::pricing_line("wallet-stats", &pricing_input)
    );
    let mut price_reasons: Vec<String> = Vec::new();
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
    if valuation.as_ref().is_some_and(|v| v.budget_exhausted) {
        price_reasons.push(format!(
            "request budget exhausted during open-position valuation (max_requests={}): \
             affected open positions are unvalued (request_budget_exhausted)",
            limit_text(args.max_requests)
        ));
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    print_diagnostics(&report, api_key, args, window, requests_made, elapsed_ms);
    for r in &price_reasons {
        eprintln!("wallet-stats: {r}");
    }
    let incomplete =
        report.is_coverage_incomplete() || !upstream_complete || !price_reasons.is_empty();
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
            provider_options: provider.request_options(),
            server_window: args.server_window,
            concurrency: usize::try_from(args.concurrency).unwrap_or(1),
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
            open_valuation: scout_app::open_valuation_meta(valuation.as_ref(), window.as_of),
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
            .map(|l| redact_table_line(&l, api_key))
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
    elapsed_ms: u64,
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
    let effective = provider_options(args, window);
    eprintln!(
        "  provider options: {} server_window={} (tx budget per wallet = max_pages_per_wallet * page_limit = {})",
        effective.describe(),
        args.server_window,
        u64::from(args.max_pages_per_wallet) * u64::from(effective.page_limit)
    );
    eprintln!(
        "  concurrency={} (max wallets scanned at once; slices not used for wallets) \
         elapsed_ms={elapsed_ms} (stderr only, not in JSONL)",
        report.concurrency
    );
    eprintln!(
        "  scan: newest-first, max_pages_per_wallet={max_pages} (page_limit txs/page; retries not counted); \
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
        if let Some(l) = &w.ledger {
            let d = &l.diagnostics;
            if d.malformed_trade_instructions
                + d.orphan_trade_events
                + d.unknown_discriminator_instructions
                + d.jupiter_malformed_events
                + d.jupiter_unknown_events
                + d.dflow_malformed_events
                + d.dflow_unknown_events
                + d.okx_malformed_events
                + d.okx_unknown_events
                + d.okx_swap_with_receiver_not_attributed
                + d.okx_idl_only_order_events
                + d.venue_events.coverage_gaps()
                > 0
            {
                eprintln!(
                    "    coverage gaps: malformed_trade_instructions={} orphan_trade_events={} \
                     unknown_discriminator_instructions={} jupiter_malformed_events={} \
                     jupiter_unknown_events={} dflow_malformed_events={} \
                     dflow_unknown_events={} okx_malformed_events={} okx_unknown_events={} \
                     okx_swap_with_receiver_not_attributed={} okx_idl_only_order_events={} \
                     venue_events={:?}",
                    d.malformed_trade_instructions,
                    d.orphan_trade_events,
                    d.unknown_discriminator_instructions,
                    d.jupiter_malformed_events,
                    d.jupiter_unknown_events,
                    d.dflow_malformed_events,
                    d.dflow_unknown_events,
                    d.okx_malformed_events,
                    d.okx_unknown_events,
                    d.okx_swap_with_receiver_not_attributed,
                    d.okx_idl_only_order_events,
                    d.venue_events
                );
            }
            for e in &l.evidence_samples {
                eprintln!(
                    "    evidence (up to {}): {}",
                    scout_engine::MAX_EVIDENCE_SAMPLES,
                    scout_app::evidence_line(e, &|s| redact(s, api_key))
                );
            }
        }
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
