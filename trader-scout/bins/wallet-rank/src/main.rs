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
//! 2 usage (incl. `--rank-by period-equity-pnl`, `--since/--until/--period`);
//! 3 ranking over a PARTIAL universe (a wallet is incomplete/errored, the
//! request budget ran out, upstream JSONL partial or footerless);
//! 4 infrastructure/credentials (also: every wallet failed);
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

use clap::Parser;
use scout_api::ProviderError;
use scout_app::{InputFormat, WriteOutcome, write_lines_to_stdout};
use scout_core::{AddressBytes, ChainFamily, SolanaPubkey, WalletKey};
use scout_engine::{
    DEFAULT_TOP, RankBy, RankPolicy, RankProfile, SolanaWalletStatsReport, WalletRankReport,
    pump_bonding_curve_decoder, rank_solana_wallets, run_solana_wallet_stats,
    sanitize_provider_text,
};
use scout_providers::{HeliusProvider, ScanOrder};
use tokio_util::sync::CancellationToken;

mod output;

use output::{RunMetaInput, SummaryInput};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
const HELIUS_TIMEOUT_MS: u64 = 30_000;
const HELIUS_MAX_ATTEMPTS: u32 = 3;

/// Rank input wallets by realized trading performance (Solana pump.fun
/// bonding-curve slice, SOL-quoted ledger).
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

    /// Total HTTP attempt budget (retries included) for the whole run.
    /// When spent, remaining wallets are not scanned (`provider_error`,
    /// exit 3, or 4 when every wallet failed).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,

    /// Not supported yet (the provider cannot honour a time window exactly).
    #[arg(long, hide = true)]
    since: Option<String>,
    /// Not supported yet.
    #[arg(long, hide = true)]
    until: Option<String>,
    /// Not supported yet.
    #[arg(long, hide = true)]
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
    Ok(p)
}

fn main() -> ExitCode {
    let args = Args::parse();

    for (name, set) in [
        ("--since", args.since.is_some()),
        ("--until", args.until.is_some()),
        ("--period", args.period.is_some()),
    ] {
        if set {
            eprintln!(
                "wallet-rank: {name} is not supported yet: the history provider cannot \
                 honour a time window exactly (scans are bounded by --max-pages-per-wallet, newest first)"
            );
            return ExitCode::from(2);
        }
    }
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
    api_key: &str,
    duplicates: usize,
    upstream_complete: bool,
) -> ExitCode {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_wallet) else {
        eprintln!("wallet-rank: --max-pages-per-wallet must be at least 1");
        return ExitCode::from(2);
    };
    let provider = match HeliusProvider::new(api_key, HELIUS_TIMEOUT_MS, HELIUS_MAX_ATTEMPTS) {
        Ok(p) => p
            .with_max_pages(max_pages)
            .with_scan_order(ScanOrder::NewestFirst)
            .with_max_total_requests(args.max_requests),
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
    let stats = match rt.block_on(run_solana_wallet_stats(
        &provider,
        solana,
        &decoder,
        CancellationToken::new(),
    )) {
        Ok(r) => r,
        Err(err) => return provider_error_exit(&err, api_key),
    };
    let requests_made = provider.total_requests_made();
    let report = rank_solana_wallets(&stats.wallets, policy);

    let incomplete = stats.is_coverage_incomplete() || !upstream_complete;
    print_diagnostics(&stats, &report, api_key, args, requests_made);
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
            max_requests: args.max_requests,
            requests_made,
            input_wallet_count: all.len(),
            input_duplicates: duplicates,
            upstream_complete,
        };
        let summary = SummaryInput {
            run_id: &run_id,
            partial: incomplete,
            cancelled: stats.cancelled,
            requests_made,
            incomplete_reasons: stats
                .incomplete_reasons()
                .iter()
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
        output::table_lines(&report, incomplete)
            .into_iter()
            .map(|l| redact(&l, api_key))
            .collect()
    };
    let outcome = write_lines_to_stdout(lines);

    if stats.cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
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
    requests_made: u64,
) {
    let s = &stats.scope;
    let p = &report.policy;
    eprintln!("wallet-rank: protocol scope (Solana mainnet, SOL-quoted ledger, lamports):");
    eprintln!("  recognized: {}", s.recognized);
    eprintln!("  NOT decoded: {}", s.not_decoded);
    eprintln!(
        "  scan: newest-first, max_pages_per_wallet={} (100 txs/page; retries not counted), \
         max_requests={}, requests_made={requests_made}; no time window",
        args.max_pages_per_wallet,
        args.max_requests
            .map_or_else(|| "unlimited".to_string(), |n| n.to_string())
    );
    let opt = |v: Option<u64>| v.map_or_else(|| "none".to_string(), |n| n.to_string());
    eprintln!(
        "  policy: rank_by={} profile={} min_closed_episodes={} min_active_days={} \
         max_trades_per_day={} max_mints_per_day={} exclude_unknown_basis={} require_no_open={} top={} \
         (research starting policy, not statistical guarantees)",
        p.rank_by.label(),
        p.profile.label(),
        p.min_closed_episodes,
        p.min_active_days,
        opt(p.max_trades_per_day),
        opt(p.max_mints_per_day),
        p.exclude_unknown_basis,
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
