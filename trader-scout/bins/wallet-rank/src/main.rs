//! `wallet-rank`: see docs/CLI.md §4 for the full contract.
//!
//! Solana: when EVERY input wallet is Solana and `SCOUT_HELIUS_API_KEY`
//! is set, a `HeliusProvider` (newest-first scan, bounded page and request
//! budgets) drives `run_solana_wallet_stats` (the SAME analysis
//! `wallet-stats` prints), then `rank_solana_wallets` applies the profile
//! gates and the documented sort. Every input wallet ends up in the ranked
//! list or in the exclusions (with all reasons); nothing is dropped.
//! Without a key, or for EVM/mixed input, the run exits 4.
//!
//! Exit codes (CLI.md §8): 0 the whole wallet universe was scanned within
//! the declared scope (an empty or short shortlist is a normal outcome);
//! 2 usage (incl. `--rank-by period-equity-pnl`, an invalid window:
//! `--period` outside 1d..=365d, non-UTC/non-RFC3339 `--since/--until`, `--period` with
//! `--since`, `--until` without a start, empty window; ADR-011);
//! 3 ranking over a PARTIAL universe (a wallet is incomplete/errored, the
//! request budget ran out, rate-limit stop after at least one wallet
//! produced data, upstream JSONL partial or footerless);
//! 4 infrastructure/credentials (also: every wallet failed, rate-limit
//! stop with no data);
//! 130 cancelled; 141 stdout closed.
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
    AnalysisWindow, DEFAULT_TOP, LedgerDecoders, QuoteUnit, RankBy, RankPolicy, RankProfile,
    ScanStop, SolanaWalletStatsReport, WalletRankReport, pump_amm_decoder,
    pump_bonding_curve_decoder, rank_solana_wallets, run_solana_wallet_stats_windowed_venues,
    sanitize_provider_text,
};
use scout_pricing::PriceSource as _;
use scout_providers::{
    HeliusProvider, HeliusRequestOptions, MAX_PAGE_LIMIT, ScanOrder, TokenAccountsFilter,
};
use scout_rpc::DEFAULT_MAX_RETRY_AFTER;
use tokio_util::sync::CancellationToken;

mod output;

use output::{RunMetaInput, SummaryInput};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
/// Test-only: send requests to this URL instead of Helius (the API key is
/// never sent to the override).
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_WALLET_RANK_ENDPOINT";
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

/// Rank input wallets by realized trading performance (Solana pump.fun
/// bonding-curve + PumpSwap AMM slice, SOL-quoted ledger).
#[derive(Debug, Parser)]
#[command(name = "wallet-rank", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Input format: `lines` (default) or `jsonl` (records of a previous
    /// trader-scout CLI: buyer_match, wallet_ref, wallet_stats, wallet_rank).
    #[arg(long, default_value = "lines", value_parser = ["lines", "jsonl"])]
    input_format: String,

    /// Output format: table or jsonl.
    #[arg(long, default_value = "table", value_parser = ["table", "jsonl"])]
    format: String,

    /// Maximum number of ranked wallets (>= 1); never padded.
    #[arg(
        long,
        default_value_t = u32::try_from(DEFAULT_TOP).unwrap_or(20),
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    top: u32,

    /// Ranking metric. `period-equity-pnl` needs a price source (P5.2) and
    /// is rejected with exit 2.
    #[arg(
        long,
        default_value = "realized-net-pnl",
        value_parser = ["realized-net-pnl", "realized-cost-roi", "profit-factor", "period-equity-pnl"]
    )]
    rank_by: String,

    /// Quote unit of the ranking metrics and the closed-episode gate
    /// (ADR-013): `sol` (lamports, default), `usdc` or `usdt` (6-dp raw
    /// units) -- never mixed or converted -- or `usd` (ADR-018): every
    /// leg valued in USD with Coinbase Exchange 1m candles (USDC at par,
    /// labelled), so SOL- and USDC-quoted wallets compare in one column;
    /// the run then also fetches prices (see --max-price-requests).
    #[arg(long, default_value = "sol", value_parser = ["sol", "usdc", "usdt", "usd"])]
    quote: String,

    /// Total HTTP request budget for PRICE candles under `--quote usd`
    /// (retries included), N >= 1. Counted apart from --max-requests as
    /// `requests_made_prices`. When exhausted the remaining legs are
    /// `price_unknown` and the run is incomplete (exit 3).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_price_requests: Option<u64>,

    /// Gate profile: `quality` (20 closed episodes, 7 active days),
    /// `insider` (5 episodes, 3 days, <= 10 mints and <= 30 trades per
    /// active day) or `none` (no sample/activity gates). Research starting
    /// policy, not statistical guarantees.
    #[arg(long, default_value = "quality", value_parser = ["quality", "insider", "none"])]
    profile: String,

    /// Override: minimum known closed episodes.
    #[arg(long)]
    min_closed_episodes: Option<u64>,

    /// Override: minimum distinct active UTC days.
    #[arg(long)]
    min_active_days: Option<u64>,

    /// Override: ceiling on timestamped trades per active day.
    #[arg(long)]
    max_trades_per_day: Option<u64>,

    /// Override: ceiling on distinct mints per active day.
    #[arg(long)]
    max_mints_per_day: Option<u64>,

    /// ADR-016: maximum share of `closed_unknown` among all closed
    /// known+unknown episodes, integer percent 0..=100 (exact integer
    /// comparison; `quality`/`insider` only). 0 = any unknown episode
    /// excludes (the old strict rule). Default 10.
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u8).range(0..=100)
    )]
    max_unknown_episode_share: u8,

    /// ADR-016: drop wallets whose worst-case PnL is unbounded (tier 2:
    /// an unknown episode consumed an unknown-basis lot or lots of several
    /// quote units) with exclusion `pnl_unbounded`.
    #[arg(long)]
    exclude_unbounded: bool,

    /// Strict variant: exclude wallets with ANY open position (default:
    /// include and flag `open_exposure=unvalued`; no price source yet).
    #[arg(long)]
    require_no_open: bool,

    /// Provider page budget PER WALLET (Helius full mode: 100 transactions
    /// per page). Newest-first: a wallet needing more pages is `incomplete`
    /// (excluded, exit 3). Retries are not counted here.
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

    /// Total HTTP attempt budget (retries included) for the whole run.
    /// When spent, the interrupted wallet is `provider_error`, remaining
    /// wallets are `not_scanned` (both excluded); exit 3.
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

fn policy_from(args: &Args) -> Result<RankPolicy, String> {
    let rank_by = match args.rank_by.as_str() {
        "realized-net-pnl" => RankBy::RealizedNetPnl,
        "realized-cost-roi" => RankBy::RealizedCostRoi,
        "profit-factor" => RankBy::ProfitFactor,
        "period-equity-pnl" => {
            return Err(
                "--rank-by period-equity-pnl is not supported yet: it needs historical \
                 prices and open-position valuation (P5.2)"
                    .to_string(),
            );
        }
        other => return Err(format!("unknown --rank-by {other}")),
    };
    let profile = match args.profile.as_str() {
        "insider" => RankProfile::Insider,
        "none" => RankProfile::None,
        _ => RankProfile::Quality,
    };
    let mut p = RankPolicy::for_profile(
        profile,
        rank_by,
        usize::try_from(args.top).map_err(|_| "--top out of range".to_string())?,
    );
    if let Some(v) = args.min_closed_episodes {
        p.min_closed_episodes = v;
    }
    if let Some(v) = args.min_active_days {
        p.min_active_days = v;
    }
    if let Some(v) = args.max_trades_per_day {
        p.max_trades_per_day = Some(v);
    }
    if let Some(v) = args.max_mints_per_day {
        p.max_mints_per_day = Some(v);
    }
    p.require_no_open = args.require_no_open;
    p.max_unknown_episode_share_percent = args.max_unknown_episode_share;
    p.exclude_unbounded = args.exclude_unbounded;
    p.quote = match args.quote.as_str() {
        "usdc" => QuoteUnit::UsdcUnits,
        "usdt" => QuoteUnit::UsdtUnits,
        "usd" => QuoteUnit::ReportCurrency,
        _ => QuoteUnit::Lamports,
    };
    Ok(p)
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
            eprintln!("wallet-rank: {err}");
            return ExitCode::from(2);
        }
    };
    let policy = match policy_from(&args) {
        Ok(p) => p,
        Err(message) => {
            eprintln!("wallet-rank: {message}");
            return ExitCode::from(2);
        }
    };

    let input_text = match read_input(args.input.as_deref()) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("wallet-rank: {message}");
            return ExitCode::from(2);
        }
    };

    let (parsed, upstream) = if args.input_format == "jsonl" {
        match scout_app::parse_jsonl_with_upstream(input_text.as_bytes(), None) {
            Ok(r) => r,
            Err(err) => {
                eprintln!("wallet-rank: {err}");
                return ExitCode::from(2);
            }
        }
    } else {
        match scout_app::parse_input(input_text.as_bytes(), InputFormat::Lines, None) {
            Ok(p) => (p, scout_app::UpstreamInfo::default()),
            Err(err) => {
                eprintln!("wallet-rank: {err}");
                return ExitCode::from(2);
            }
        }
    };

    let wallets = match scout_app::resolve_wallet_keys(&parsed) {
        Ok(wallets) => wallets,
        Err(err) => {
            eprintln!("wallet-rank: {err}");
            return ExitCode::from(2);
        }
    };
    if parsed.duplicate_count > 0 {
        eprintln!(
            "wallet-rank: {} duplicate input identit(ies) collapsed",
            parsed.duplicate_count
        );
    }
    let upstream_complete = upstream.is_complete();
    if !upstream_complete {
        eprintln!(
            "wallet-rank: upstream JSONL run is not complete (run_summary status={}); \
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
            "wallet-rank: {evm} of {} wallet(s) are EVM; no history provider configured for EVM \
             (SCOUT_EVM_HISTORY_API_KEY): refusing to rank a partial universe",
            wallets.len()
        );
        return ExitCode::from(4);
    }

    let Some(api_key) = std::env::var(HELIUS_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!(
            "wallet-rank: {} wallet(s) parsed, top={}; no history provider configured: \
             configuration required, set {HELIUS_KEY_ENV}",
            wallets.len(),
            args.top
        );
        return ExitCode::from(4);
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("wallet-rank: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };
    run_solana(
        &rt,
        &wallets,
        &solana,
        &args,
        &policy,
        &window,
        &api_key,
        parsed.duplicate_count,
        upstream_complete,
    )
}

/// Outcome of the ADR-018 pricing step (all `None` unless `--quote usd`).
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

/// ADR-018: when `enabled`, collect the needed minutes of all wallets,
/// prefetch once and apply the USD views.
fn price_wallets(
    rt: &tokio::runtime::Runtime,
    stats: &mut SolanaWalletStatsReport,
    args: &Args,
    enabled: bool,
) -> PricingOutcome {
    let mut out = PricingOutcome {
        policy: None,
        endpoint_overridden: false,
        requests_made: 0,
        max_requests: args.max_price_requests,
        run: None,
    };
    if !enabled {
        return out;
    }
    match scout_app::build_coinbase_source(args.max_price_requests) {
        Ok((source, overridden)) => {
            out.endpoint_overridden = overridden;
            out.policy = Some(source.policy());
            out.run =
                Some(rt.block_on(scout_engine::apply_usd_pricing(&mut stats.wallets, &source)));
            out.requests_made = source.requests_made();
        }
        Err(message) => eprintln!("wallet-rank: {message}; USD figures unavailable"),
    }
    out
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

fn limit_text(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unlimited".to_string(), |n| n.to_string())
}

fn redact(text: &str, secret: &str) -> String {
    let replaced = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    };
    sanitize_provider_text(&replaced)
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
        ProviderError::ConfigurationRequired { .. } => eprintln!("wallet-rank: {text}"),
        _ => eprintln!("wallet-rank: provider error: {text}"),
    }
    ExitCode::from(4)
}

#[allow(clippy::too_many_arguments)]
fn run_solana(
    rt: &tokio::runtime::Runtime,
    all: &[WalletKey],
    solana: &[SolanaPubkey],
    args: &Args,
    policy: &RankPolicy,
    window: &AnalysisWindow,
    api_key: &str,
    duplicates: usize,
    upstream_complete: bool,
) -> ExitCode {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_wallet) else {
        eprintln!("wallet-rank: --max-pages-per-wallet must be at least 1");
        return ExitCode::from(2);
    };
    let options = provider_options(args, window);
    let provider = match build_provider(api_key, max_pages, args.max_requests, window, &options) {
        Ok(p) => p,
        Err(err) => return provider_error_exit(&err, api_key),
    };
    let decoder = match pump_bonding_curve_decoder() {
        Ok(d) => d,
        Err(err) => {
            eprintln!(
                "wallet-rank: decoder unavailable: {}",
                redact(&err.to_string(), api_key)
            );
            return ExitCode::from(4);
        }
    };
    let amm = pump_amm_decoder();
    let mut stats = match rt.block_on(run_solana_wallet_stats_windowed_venues(
        &provider,
        solana,
        &LedgerDecoders {
            curve: &decoder,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        },
        window,
        CancellationToken::new(),
    )) {
        Ok(r) => r,
        Err(err) => return provider_error_exit(&err, api_key),
    };
    let requests_made = provider.total_requests_made();
    // ADR-018: prices only for `--quote usd`; other quotes never fetch.
    let pricing = price_wallets(
        rt,
        &mut stats,
        args,
        policy.quote == QuoteUnit::ReportCurrency,
    );
    let pricing_input = pricing.input();
    let pricing_line = scout_app::pricing_line("wallet-rank", &pricing_input);
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
    let report = rank_solana_wallets(&stats.wallets, policy);

    let incomplete =
        stats.is_coverage_incomplete() || !upstream_complete || !price_reasons.is_empty();
    if pricing.policy.is_some() {
        eprintln!("{pricing_line}");
    }
    for r in &price_reasons {
        eprintln!("wallet-rank: {r}");
    }
    print_diagnostics(&stats, &report, api_key, args, window, requests_made);
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
            provider_options: provider.request_options(),
            server_window: args.server_window,
            max_requests: args.max_requests,
            requests_made,
            input_wallet_count: all.len(),
            input_duplicates: duplicates,
            upstream_complete,
            window: *window,
            pricing: scout_app::pricing_meta(&pricing_input),
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
                .map(|r| redact(r, api_key))
                .collect(),
        };
        match output::jsonl_lines(&meta, &report, summary, &|t| redact(t, api_key)) {
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
        .map(|l| redact_table_line(&l, api_key))
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

/// Scope, policy and coverage on stderr (stdout stays the result format).
fn print_diagnostics(
    stats: &SolanaWalletStatsReport,
    report: &WalletRankReport,
    api_key: &str,
    args: &Args,
    window: &AnalysisWindow,
    requests_made: u64,
) {
    let s = &stats.scope;
    let p = &report.policy;
    eprintln!("wallet-rank: protocol scope (Solana mainnet, SOL-quoted ledger, lamports):");
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
        "  scan: newest-first, max_pages_per_wallet={} (page_limit txs/page; retries not counted), \
         max_requests={}, requests_made={requests_made}; {}",
        args.max_pages_per_wallet,
        args.max_requests
            .map_or_else(|| "unlimited".to_string(), |n| n.to_string()),
        match window.bounds() {
            Some((since, until)) => format!(
                "window [{}, {}) source={} as_of={} (stops after the first page older than the \
                 window start; budget exhausted before it = incomplete; pre-window inventory is \
                 left-censored, ADR-011)",
                scout_app::format_unix_utc(u64::try_from(since).unwrap_or(0)),
                scout_app::format_unix_utc(u64::try_from(until).unwrap_or(0)),
                window.source.label(),
                scout_app::format_unix_utc(u64::try_from(window.as_of).unwrap_or(0))
            ),
            None => "no time window".to_string(),
        }
    );
    match stats.stop {
        Some(ScanStop::BudgetExhausted { limit }) => eprintln!(
            "wallet-rank: request budget exhausted after {requests_made} requests (limit {limit}); \
             the interrupted wallet is provider_error, remaining wallets are not_scanned, ranking is over an incomplete universe"
        ),
        Some(ScanStop::RateLimited { retry_after_secs }) => eprintln!(
            "wallet-rank: {}; remaining wallets were not scanned, ranking is over an incomplete universe",
            rate_limited_text(retry_after_secs)
        ),
        None => {}
    }
    let opt = |v: Option<u64>| v.map_or_else(|| "none".to_string(), |n| n.to_string());
    eprintln!(
        "  policy: rank_by={} quote={} profile={} min_closed_episodes={} min_active_days={} \
         max_trades_per_day={} max_mints_per_day={} unknown_share_gate={} max_unknown_episode_share_percent={} exclude_unbounded={} require_no_open={} top={} \
         (research starting policy, not statistical guarantees)",
        p.rank_by.label(),
        scout_engine::quote_unit_label(p.quote),
        p.profile.label(),
        p.min_closed_episodes,
        p.min_active_days,
        opt(p.max_trades_per_day),
        opt(p.max_mints_per_day),
        p.unknown_share_gate,
        p.max_unknown_episode_share_percent,
        p.exclude_unbounded,
        p.require_no_open,
        p.top
    );
    eprintln!(
        "  open positions are unvalued (no price source, P5.2): included and flagged unless --require-no-open"
    );
    eprintln!(
        "wallet-rank: {} input, {} eligible, {} ranked, {} excluded",
        report.input_count,
        report.eligible_count,
        report.ranked.len(),
        report.excluded.len()
    );
    for w in &stats.wallets {
        let Some(l) = &w.ledger else { continue };
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
            > 0
        {
            eprintln!(
                "  wallet {} coverage gaps: malformed_trade_instructions={} orphan_trade_events={} \
                 unknown_discriminator_instructions={} jupiter_malformed_events={} \
                 jupiter_unknown_events={} dflow_malformed_events={} dflow_unknown_events={} \
                 okx_malformed_events={} okx_unknown_events={} \
                 okx_swap_with_receiver_not_attributed={} okx_idl_only_order_events={}",
                bs58::encode(w.wallet).into_string(),
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
                d.okx_idl_only_order_events
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
    let reasons = stats.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!("wallet-rank: status=complete within declared protocol scope");
    } else {
        eprintln!("wallet-rank: status=partial (ranking over an incomplete universe):");
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
