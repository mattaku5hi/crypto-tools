//! `wallet-stats`: see docs/CLI.md §5 for the full contract.
//! This is a P1 skeleton: argument parsing only, no scanning yet.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;

/// Print stats for every input wallet, including N/A and no-activity cases.
#[derive(Debug, Parser)]
#[command(name = "wallet-stats", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,
}

fn main() -> ExitCode {
    let _args = Args::parse();
    eprintln!("wallet-stats: scanning not yet implemented (P1 skeleton)");
    ExitCode::from(4)
}
