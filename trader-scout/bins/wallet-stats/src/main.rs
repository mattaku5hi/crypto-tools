//! `wallet-stats`: see docs/CLI.md §5 for the full contract.
//!
//! Solana: when EVERY input wallet is Solana and `SCOUT_HELIUS_API_KEY`
//! is set, a `HeliusProvider` (newest-first scan, bounded page budget per
//! wallet) drives `run_solana_wallet_stats` (pump.fun bonding-curve SOL
//! ledger, ADR-010). One card/row per distinct input wallet, first
//! appearance order; failures and gaps are shown on the card, never
//! dropped. Without a key, or for EVM/mixed input, the run exits 4.
//!
//! Exit codes (CLI.md §8): 0 complete within declared scope (legitimate
//! N/A, no-activity and unknown-basis cards are NOT failures); 2 usage;
//! 3 incomplete coverage (truncated scan, per-wallet scan failure while
//! other wallets succeeded, decoder gaps, upstream JSONL partial/without
//! footer); 4 infrastructure/credentials (also: every wallet failed);
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
    SolanaWalletStatsReport, pump_bonding_curve_decoder, run_solana_wallet_stats,
    sanitize_provider_text,
};
use scout_providers::{HeliusProvider, ScanOrder};
use tokio_util::sync::CancellationToken;

mod output;

use output::{Detail, RunMetaInput, SortMode};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
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
    /// unknown) and the run exits 3. This is NOT the spec's --max-requests:
    /// retries are not counted against it.
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u32).range(1..=200)
    )]
    max_pages_per_wallet: u32,

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

fn main() -> ExitCode {
    let args = Args::parse();

    for (name, set) in [
        ("--since", args.since.is_some()),
        ("--until", args.until.is_some()),
        ("--period", args.period.is_some()),
    ] {
        if set {
            eprintln!(
                "wallet-stats: {name} is not supported yet: the history provider cannot \
                 honour a time window exactly (scans are bounded by --max-pages-per-wallet, newest first)"
            );
            return ExitCode::from(2);
        }
    }

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
        ProviderError::ConfigurationRequired { .. } => eprintln!("wallet-stats: {text}"),
        _ => eprintln!("wallet-stats: provider error: {text}"),
    }
    ExitCode::from(4)
}

fn run_solana(
    rt: &tokio::runtime::Runtime,
    all: &[WalletKey],
    solana: &[SolanaPubkey],
    args: &Args,
    api_key: &str,
    duplicates: usize,
    upstream_complete: bool,
) -> ExitCode {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_wallet) else {
        eprintln!("wallet-stats: --max-pages-per-wallet must be at least 1");
        return ExitCode::from(2);
    };
    let provider = match HeliusProvider::new(api_key, HELIUS_TIMEOUT_MS, HELIUS_MAX_ATTEMPTS) {
        Ok(p) => p
            .with_max_pages(max_pages)
            .with_scan_order(ScanOrder::NewestFirst),
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
    let report = match rt.block_on(run_solana_wallet_stats(
        &provider,
        solana,
        &decoder,
        CancellationToken::new(),
    )) {
        Ok(r) => r,
        Err(err) => return provider_error_exit(&err, api_key),
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
    print_diagnostics(&report, api_key, args.max_pages_per_wallet);
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
        };
        match output::jsonl_lines(&meta, &report, incomplete, &|t| redact(t, api_key)) {
            Ok(l) => l,
            Err(message) => {
                eprintln!("wallet-stats: could not render output: {message}");
                return ExitCode::from(4);
            }
        }
    } else {
        output::table_lines(&report, detail, sort)
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
    if report.all_failed() {
        return ExitCode::from(4);
    }
    if incomplete {
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

/// Scope and coverage on stderr (stdout stays the single result format).
fn print_diagnostics(report: &SolanaWalletStatsReport, api_key: &str, max_pages: u32) {
    let s = &report.scope;
    eprintln!("wallet-stats: protocol scope (Solana mainnet, SOL-quoted ledger, lamports):");
    eprintln!("  recognized: {}", s.recognized);
    eprintln!("  NOT decoded: {}", s.not_decoded);
    eprintln!(
        "  scan: newest-first, max_pages_per_wallet={max_pages} (100 txs/page; retries not counted); \
         a truncated wallet lacks OLDER history (opening inventory unknown); no time window"
    );
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
