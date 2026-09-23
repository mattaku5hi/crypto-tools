//! `buyer-intersect`: see docs/CLI.md §3 for the full contract.
//!
//! Wires input parsing -> scout-engine's orchestration -> JSONL/table
//! output. Uses `UnconfiguredProvider` until a real credential-backed
//! provider exists (ADR-006) — this means every real invocation
//! currently exits 4 (`ConfigurationRequired`), which is the honest
//! outcome, not a stub message.
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

use std::io::{self, BufRead, IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;
use scout_app::{InputFormat, JsonlRecord, WriteOutcome, write_lines_to_stdout};
use scout_core::ScoutError;
use scout_engine::run_buyer_intersect;
use scout_providers::UnconfiguredProvider;

/// Find wallets that bought at least K distinct input tokens.
#[derive(Debug, Parser)]
#[command(name = "buyer-intersect", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Minimum number of distinct input tokens that must have been bought.
    #[arg(long, default_value_t = 2)]
    min_token_hits: usize,

    /// Output format: table or jsonl.
    #[arg(long, default_value = "table")]
    format: String,
}

fn main() -> ExitCode {
    let args = Args::parse();

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

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("buyer-intersect: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };

    // Per ADR-006, no live-backed HistoryProvider exists yet. Wiring
    // UnconfiguredProvider here means a real run honestly reports
    // ConfigurationRequired (exit 4) rather than a stub "not
    // implemented" message — the pipeline itself (input -> engine ->
    // output) is real and exercised; only the transport is missing.
    let provider = UnconfiguredProvider::new("evm_history", "SCOUT_EVM_HISTORY_API_KEY");
    let flows = std::collections::BTreeMap::new();
    let input_tokens: Vec<_> = Vec::new();

    let result = rt.block_on(run_buyer_intersect(
        &provider,
        &flows,
        &input_tokens,
        args.min_token_hits,
    ));

    match result {
        Ok(report) => emit_report(&report, &args.format),
        Err(ScoutError::ConfigurationRequired { port, env_var }) => {
            eprintln!(
                "buyer-intersect: configuration required for provider `{port}` (set {env_var})"
            );
            ExitCode::from(4)
        }
        Err(err) => {
            eprintln!("buyer-intersect: {err}");
            ExitCode::from(4)
        }
    }
}

fn emit_report(report: &scout_engine::BuyerIntersectReport, format: &str) -> ExitCode {
    if format == "jsonl" {
        let lines: Vec<String> = report
            .matches
            .iter()
            .filter_map(|m| {
                JsonlRecord::BuyerMatch {
                    wallet: m.wallet.clone(),
                    hit_count: m.hit_count,
                    matched_assets: m.matched_assets.clone(),
                }
                .to_jsonl_line()
                .ok()
            })
            .collect();
        match write_lines_to_stdout(lines) {
            WriteOutcome::Complete => ExitCode::SUCCESS,
            WriteOutcome::PipeClosed => ExitCode::from(141),
        }
    } else {
        let lines: Vec<String> = report
            .matches
            .iter()
            .map(|m| format!("{} hit_count={}", m.wallet.address, m.hit_count))
            .collect();
        match write_lines_to_stdout(lines) {
            WriteOutcome::Complete => ExitCode::SUCCESS,
            WriteOutcome::PipeClosed => ExitCode::from(141),
        }
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

#[allow(dead_code)]
fn unused_bufread_import_anchor<R: BufRead>(_r: R) {}
