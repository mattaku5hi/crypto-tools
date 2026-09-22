//! `wallet-rank`: see docs/CLI.md §4 for the full contract.
//! This is a P1 skeleton: argument parsing only, no scanning yet.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;

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
    let _args = Args::parse();
    eprintln!("wallet-rank: scanning not yet implemented (P1 skeleton)");
    ExitCode::from(4)
}
