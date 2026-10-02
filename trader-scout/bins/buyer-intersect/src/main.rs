//! `buyer-intersect`: see docs/CLI.md §3 for the full contract.
//!
//! Wires input parsing -> scout-engine's orchestration -> JSONL/table
//! output.
//!
//! Solana: when EVERY input token is a Solana mint and
//! `SCOUT_HELIUS_API_KEY` is set (non-empty), a `HeliusProvider` drives
//! `run_solana_buyer_intersect` (pump.fun bonding-curve buys only; the
//! scope/lower-bound caveat is printed on stderr). Without the key, or
//! for EVM/mixed input, `UnconfiguredProvider` is used and the run
//! exits 4 (`ConfigurationRequired`) -- the honest outcome, not a stub.
//!
//! Exit codes (ADR-005): 0 complete within declared scope; 2 argument
//! error; 3 incomplete coverage (truncated, failed token scan,
//! malformed or unknown-discriminator bonding-curve instruction,
//! unverified-variant buys, ...) even though matches are
//! still printed; 4 infrastructure/configuration; 130 cancelled; 141
//! output pipe closed.
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
use std::process::ExitCode;

use clap::Parser;
use scout_api::ProviderError;
use scout_app::{InputFormat, JsonlRecord, RunStatus, WriteOutcome, write_lines_to_stdout};
use scout_core::{AddressBytes, AssetKey, ChainFamily};
use scout_engine::{
    PumpTradeVariant, SolanaBuyerIntersectReport, SolanaProtocolScope, run_buyer_intersect,
    run_solana_buyer_intersect, sanitize_provider_text,
};
use scout_providers::{HeliusProvider, UnconfiguredProvider};
use tokio_util::sync::CancellationToken;

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
const HELIUS_TIMEOUT_MS: u64 = 30_000;
const HELIUS_MAX_ATTEMPTS: u32 = 3;

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

    let all_solana = input_tokens.iter().all(|t| {
        matches!(
            t,
            AssetKey::Token(chain, AddressBytes::Solana(_)) if chain.family == ChainFamily::Solana
        )
    });

    if all_solana {
        let api_key = std::env::var(HELIUS_KEY_ENV)
            .ok()
            .filter(|k| !k.trim().is_empty());
        if let Some(api_key) = api_key {
            return run_solana(&rt, &input_tokens, &args, &api_key);
        }
        let provider = UnconfiguredProvider::new("solana_history", HELIUS_KEY_ENV);
        return run_legacy(&rt, &provider, &input_tokens, &args);
    }

    // Per ADR-006, no live-backed EVM HistoryProvider exists yet.
    // Wiring UnconfiguredProvider here means a real run honestly
    // reports ConfigurationRequired (exit 4) rather than a stub "not
    // implemented" message.
    let provider = UnconfiguredProvider::new("evm_history", "SCOUT_EVM_HISTORY_API_KEY");
    run_legacy(&rt, &provider, &input_tokens, &args)
}

fn run_legacy(
    rt: &tokio::runtime::Runtime,
    provider: &UnconfiguredProvider,
    input_tokens: &[AssetKey],
    args: &Args,
) -> ExitCode {
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
        return ExitCode::from(3);
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

/// Map a `ProviderError` to exit 4 with a redacted message. Every
/// current variant is infrastructure/credentials/capability (ADR-005);
/// the wildcard exists because `ProviderError` is `#[non_exhaustive]`.
/// Text is sanitized (API-key query values, control characters) and, if
/// given, the literal key is replaced too: transport errors can embed
/// the request URL.
fn provider_error_exit(err: &ProviderError, secret: Option<&str>) -> ExitCode {
    let mut text = sanitize_provider_text(&err.to_string());
    if let Some(secret) = secret.filter(|s| !s.is_empty()) {
        text = text.replace(secret, "<redacted>");
    }
    match err {
        ProviderError::ConfigurationRequired { .. } => {
            eprintln!("buyer-intersect: {text}");
        }
        _ => eprintln!("buyer-intersect: provider error: {text}"),
    }
    ExitCode::from(4)
}

fn run_solana(
    rt: &tokio::runtime::Runtime,
    input_tokens: &[AssetKey],
    args: &Args,
    api_key: &str,
) -> ExitCode {
    let provider = match HeliusProvider::new(api_key, HELIUS_TIMEOUT_MS, HELIUS_MAX_ATTEMPTS) {
        Ok(provider) => provider,
        Err(err) => return provider_error_exit(&err, Some(api_key)),
    };
    let result = rt.block_on(run_solana_buyer_intersect(
        &provider,
        input_tokens,
        args.min_token_hits,
        CancellationToken::new(),
    ));
    let report = match result {
        Ok(report) => report,
        Err(err) => return provider_error_exit(&err, Some(api_key)),
    };

    print_solana_diagnostics(&report, api_key);
    let incomplete = report.is_coverage_incomplete();
    let outcome = emit_solana_matches(&report, &args.format, incomplete);
    if report.cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
    }
    if incomplete {
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

fn redact(text: &str, secret: &str) -> String {
    sanitize_provider_text(&text.replace(secret, "<redacted>"))
}

/// Scope, coverage and diagnostics block on stderr (stdout stays the
/// single selected result format, CLI.md §2). Unknown is never printed
/// as zero: failed tokens say `scan failed`, not `0 transactions`.
fn print_solana_diagnostics(report: &SolanaBuyerIntersectReport, api_key: &str) {
    let scope = &report.scope;
    eprintln!("buyer-intersect: protocol scope (Solana mainnet):");
    eprintln!("  recognized: {}", scope.recognized);
    eprintln!("  NOT decoded: {}", scope.not_decoded);
    eprintln!(
        "  program={} idl_commit={} idl_file_sha256={} rule={}",
        scope.program_id, scope.idl_commit, scope.idl_sha256, scope.qualification_version
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
        let status = match &token.error {
            Some(error) => format!("scan failed: {}", redact(error, api_key)),
            None if token.truncated => "truncated".to_string(),
            None => "ok".to_string(),
        };
        eprintln!(
            "  token {}: status={} txs_scanned={} qualified_buyers={} decoded_buys={} \
             malformed={} unknown_discriminator={} unverified_variant_buys={} \
             positive_delta_without_instruction={} failed_transactions={}",
            token.asset_label(),
            status,
            token.transactions_scanned,
            token.qualified_buyers,
            token.diagnostics.decoded_buys,
            token.diagnostics.malformed_instructions,
            unknown_or_na(
                token.error.is_some(),
                token.diagnostics.unknown_discriminator_instructions
            ),
            if token.error.is_some() {
                "n/a (scan failed)".to_string()
            } else {
                format_unverified(&token.diagnostics.unverified_variant_buys)
            },
            token.positive_delta_without_instruction,
            token.diagnostics.failed_transactions,
        );
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
    let any_failed = report.per_token.iter().any(|t| t.error.is_some());
    let partial_note = if any_failed || report.cancelled {
        " (partial: some tokens were not fully scanned; counts are lower bounds)"
    } else {
        ""
    };
    eprintln!(
        "  program instructions: known_non_trade={} unknown_discriminator={} \
         unverified_variant_buys={}{partial_note}",
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
    let reasons = report.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!(
            "buyer-intersect: status=complete within declared protocol scope \
             (buyer set is a lower bound for migrated tokens)"
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
    format: &str,
    incomplete: bool,
) -> WriteOutcome {
    let mut lines: Vec<String> = if format == "jsonl" {
        report
            .base
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
            .collect()
    } else {
        report
            .base
            .matches
            .iter()
            .map(|m| format!("{} hit_count={}", m.wallet.address, m.hit_count))
            .collect()
    };
    if format == "jsonl"
        && let Ok(line) = (JsonlRecord::RunSummary {
            run_id: "buyer-intersect-solana".to_string(),
            status: if incomplete {
                RunStatus::Partial
            } else {
                RunStatus::Complete
            },
            records: report.base.matches.len(),
        })
        .to_jsonl_line()
    {
        lines.push(line);
    }
    write_lines_to_stdout(lines)
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
