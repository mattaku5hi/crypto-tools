//! `buyer-intersect`: see docs/CLI.md §3 for the full contract.
//! This is a P1 skeleton: argument parsing only, no scanning yet.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;

/// Find wallets that bought at least K distinct input tokens.
#[derive(Debug, Parser)]
#[command(name = "buyer-intersect", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Minimum number of distinct input tokens that must have been bought.
    #[arg(long, default_value_t = 2)]
    min_token_hits: u32,
}

fn main() -> ExitCode {
    let _args = Args::parse();
    eprintln!("buyer-intersect: scanning not yet implemented (P1 skeleton)");
    ExitCode::from(4)
}
