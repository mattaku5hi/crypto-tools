//! `wallet-rank`: see docs/CLI.md §4 for the full contract.
//!
//! Uses `UnconfiguredProvider` until a real credential-backed provider
//! exists (ADR-006) — exits 4 honestly rather than a stub message.
#![forbid(unsafe_code)]

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;
use scout_app::InputFormat;

/// Rank input wallets by realized trading performance.
#[derive(Debug, Parser)]
#[command(name = "wallet-rank", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Maximum number of ranked wallets to return.
    #[arg(long, default_value_t = 20)]
    top: u32,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let input_text = match read_input(args.input.as_deref()) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("wallet-rank: {message}");
            return ExitCode::from(2);
        }
    };

    let parsed = match scout_app::parse_input(input_text.as_bytes(), InputFormat::Lines, None) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("wallet-rank: {err}");
            return ExitCode::from(2);
        }
    };

    // Per ADR-006: no live-backed HistoryProvider exists yet. Every
    // wallet requires WalletActivity from a configured provider to rank
    // — with zero configured, the honest outcome is
    // InfrastructureUnavailable (exit 4), not a fabricated top-N.
    eprintln!(
        "wallet-rank: {} wallet(s) parsed, top={}; no history provider configured (set SCOUT_EVM_HISTORY_API_KEY / SCOUT_HELIUS_API_KEY)",
        parsed.records.len(),
        args.top
    );
    ExitCode::from(4)
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
