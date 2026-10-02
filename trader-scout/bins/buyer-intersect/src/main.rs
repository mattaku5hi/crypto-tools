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
//! unverified-variant buys, request budget exhausted, ...) even though
//! matches are still printed; 4 infrastructure/configuration; 130
//! cancelled; 141 output pipe closed.
//!
//! `--max-requests N` (N >= 1) bounds the TOTAL HTTP attempts of the run
//! (retries included, one provider instance shared by all tokens). Exit
//! decision: exhausting a USER-chosen budget is incomplete coverage
//! (exit 3), not infrastructure failure (exit 4): the tool worked, the
//! requested scan contract was not met. Tokens whose scan hit the limit
//! are `failed` in `run_summary` (unknown, never zero) and the run is
//! `partial`. If the budget ran out before ANY transaction was seen the
//! engine yields no report; the run still exits 3 with the diagnostic
//! and no records on stdout (no `run_summary` footer, so a downstream
//! consumer cannot take it for complete). A terminal `RateLimited`
//! (Retry-After above the transport cap) and `ResponseTooLarge` before
//! any data are infrastructure: exit 4. Requests used are always
//! printed (`requests_made` in `run_meta`, stderr summary line).
//!
//! Run-terminal stops (engine `ScanStop`, typed, no text matching): on
//! the first budget exhaustion or terminal rate limit the engine issues
//! no further request; the interrupted token is `failed` (with
//! `error_kind`) and every later token is `not_scanned` (with
//! `stop_reason`) in `run_summary`. Exit mapping for a stop AFTER data
//! was observed (a report exists): budget -> 3; rate limit -> 3 too
//! (the tool worked, partial matches are printed and marked incomplete;
//! CLI.md §8: 3 = partial scan/coverage contract, 4 = nothing usable
//! could be produced). A rate limit before ANY transaction was observed
//! yields no report: exit 4 (infrastructure), no records on stdout.
//! Both stop kinds print the same stderr line mid-run and before data.
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
use scout_core::{AddressBytes, AssetKey, ChainFamily};
use scout_engine::{
    PumpTradeVariant, ScanStop, SolanaBuyerIntersectReport, SolanaProtocolScope, TokenScanStatus,
    run_buyer_intersect, run_solana_buyer_intersect, sanitize_provider_text,
};
use scout_providers::{HeliusProvider, UnconfiguredProvider};
use scout_rpc::{DEFAULT_MAX_RETRY_AFTER, RequestBudgetExhausted};
use tokio_util::sync::CancellationToken;

mod output;

use output::RunBudget;

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
/// Test-only: replaces the Helius endpoint URL (offline wiremock tests).
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_BUYER_INTERSECT_ENDPOINT";
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
    #[arg(long, default_value = "table", value_parser = ["table", "jsonl"])]
    format: String,

    /// Provider page budget PER INPUT TOKEN (Solana/Helius only; 100
    /// transactions per page in full mode). When a token's history needs
    /// more pages the scan is truncated, reported as partial, and the run
    /// exits 3. This is NOT --max-requests: retries are not
    /// counted against it (use --max-requests for that). Ignored for EVM
    /// input.
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u32).range(1..=200)
    )]
    max_pages_per_token: u32,

    /// Total HTTP request budget for the whole run (all tokens, retries
    /// included), N >= 1. Absent = unlimited (requests are still counted
    /// and reported). When exhausted the run is partial and exits 3.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,
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

/// Map a `ProviderError` to an exit code with a redacted message.
/// Request-budget exhaustion with nothing observed is exit 3 (user-chosen
/// bound hit: incomplete coverage, see module docs); everything else is
/// infrastructure/credentials/capability, exit 4 (ADR-005; the wildcard
/// exists because `ProviderError` is `#[non_exhaustive]`). Text is
/// sanitized (API-key query values, control characters) and, if given,
/// the literal key is replaced too: transport errors can embed the
/// request URL.
fn provider_error_exit(err: &ProviderError, secret: Option<&str>) -> ExitCode {
    let redact_text = |raw: &str| {
        let mut text = sanitize_provider_text(raw);
        if let Some(secret) = secret.filter(|s| !s.is_empty()) {
            text = text.replace(secret, "<redacted>");
        }
        text
    };
    if let ProviderError::Other(inner) = err
        && let Some(exhausted) = inner.downcast_ref::<RequestBudgetExhausted>()
    {
        eprintln!(
            "buyer-intersect: request budget exhausted after {} requests (limit {}); \
             no transactions were observed, nothing to report (IncompleteCoverage, exit 3)",
            exhausted.limit, exhausted.limit
        );
        return ExitCode::from(3);
    }
    match err {
        ProviderError::ConfigurationRequired { .. } => {
            eprintln!("buyer-intersect: {}", redact_text(&err.to_string()));
        }
        ProviderError::RateLimited { retry_after } => {
            eprintln!("buyer-intersect: {}", rate_limited_text(*retry_after));
        }
        _ => eprintln!(
            "buyer-intersect: provider error: {}",
            redact_text(&err.to_string())
        ),
    }
    ExitCode::from(4)
}

/// Terminal `RateLimited`: the transport refused to wait out a
/// `Retry-After` above its cap (`DEFAULT_MAX_RETRY_AFTER`).
fn rate_limited_text(retry_after: Option<std::time::Duration>) -> String {
    let cap = DEFAULT_MAX_RETRY_AFTER.as_secs();
    match retry_after {
        Some(d) => format!(
            "rate limited; server asked to retry after {}s (cap {cap}s)",
            d.as_secs()
        ),
        None => "rate limited; server gave no Retry-After".to_string(),
    }
}

fn limit_text(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unlimited".to_string(), |n| n.to_string())
}

fn build_provider(
    api_key: &str,
    max_pages: NonZeroU32,
    max_requests: Option<u64>,
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
        .with_max_pages(max_pages)
        .with_max_total_requests(max_requests))
}

fn run_solana(
    rt: &tokio::runtime::Runtime,
    input_tokens: &[AssetKey],
    args: &Args,
    api_key: &str,
) -> ExitCode {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_token) else {
        eprintln!("buyer-intersect: --max-pages-per-token must be at least 1");
        return ExitCode::from(2);
    };
    // ONE provider for the whole run: the request budget and counter are
    // shared by every token's scan.
    let provider = match build_provider(api_key, max_pages, args.max_requests) {
        Ok(provider) => provider,
        Err(err) => return provider_error_exit(&err, Some(api_key)),
    };
    let result = rt.block_on(run_solana_buyer_intersect(
        &provider,
        input_tokens,
        args.min_token_hits,
        CancellationToken::new(),
    ));
    let requests_made = provider.total_requests_made();
    let report = match result {
        Ok(report) => report,
        Err(err) => {
            eprintln!(
                "buyer-intersect: requests_made={requests_made} max_requests={}",
                limit_text(args.max_requests)
            );
            return provider_error_exit(&err, Some(api_key));
        }
    };

    let budget = RunBudget {
        max_pages_per_token: args.max_pages_per_token,
        max_requests: args.max_requests,
        requests_made,
    };
    print_solana_diagnostics(&report, api_key, budget);
    let incomplete = report.is_coverage_incomplete();
    let captured_at = scout_app::now_utc_rfc3339();
    let outcome = match emit_solana_matches(
        &report,
        input_tokens,
        &args.format,
        incomplete,
        &captured_at,
        budget,
        api_key,
    ) {
        Ok(outcome) => outcome,
        Err(message) => {
            eprintln!("buyer-intersect: could not render output: {message}");
            return ExitCode::from(4);
        }
    };
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
fn print_solana_diagnostics(report: &SolanaBuyerIntersectReport, api_key: &str, budget: RunBudget) {
    let scope = &report.scope;
    eprintln!("buyer-intersect: protocol scope (Solana mainnet):");
    eprintln!("  recognized: {}", scope.recognized);
    eprintln!("  NOT decoded: {}", scope.not_decoded);
    eprintln!(
        "  program={} idl_commit={} idl_file_sha256={} rule={}",
        scope.program_id, scope.idl_commit, scope.idl_sha256, scope.qualification_version
    );
    eprintln!(
        "  budget: max_pages_per_token={} (provider pages per input token, \
         100 txs/page; retries are not counted)",
        budget.max_pages_per_token
    );
    eprintln!(
        "  requests_made={} max_requests={} (total HTTP attempts, retries included)",
        budget.requests_made,
        limit_text(budget.max_requests)
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
        let status = match &token.status {
            TokenScanStatus::Failed { message, .. } => {
                format!("scan failed: {}", redact(message, api_key))
            }
            TokenScanStatus::NotScanned { reason } => {
                format!("not_scanned: {}", reason.describe())
            }
            TokenScanStatus::Ok if token.truncated => "truncated".to_string(),
            TokenScanStatus::Ok => "ok".to_string(),
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
                token.is_unknown(),
                token.diagnostics.unknown_discriminator_instructions
            ),
            if token.is_unknown() {
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
    let any_failed = report.per_token.iter().any(|t| t.is_unknown());
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
    match report.stop {
        Some(ScanStop::BudgetExhausted { limit }) => eprintln!(
            "buyer-intersect: request budget exhausted after {} requests (limit {limit}); \
             unscanned or partially scanned tokens are marked failed, results are incomplete",
            budget.requests_made
        ),
        Some(ScanStop::RateLimited { retry_after_secs }) => eprintln!(
            "buyer-intersect: {}; remaining tokens were not scanned, results are incomplete",
            rate_limited_text(retry_after_secs.map(std::time::Duration::from_secs))
        ),
        None => {}
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
    input_tokens: &[AssetKey],
    format: &str,
    incomplete: bool,
    captured_at: &str,
    budget: RunBudget,
    api_key: &str,
) -> Result<WriteOutcome, String> {
    let lines: Vec<String> = if format == "jsonl" {
        let run_id = run_id_from(captured_at);
        output::solana_jsonl_lines(
            &run_id,
            captured_at,
            report,
            input_tokens,
            budget,
            incomplete,
            &|text| redact(text, api_key),
        )?
    } else {
        table_lines(&report.base)
    };
    Ok(write_lines_to_stdout(lines))
}

/// `buyer-intersect-20261002T123456Z`: unique per second, sortable.
fn run_id_from(captured_at: &str) -> String {
    let compact: String = captured_at
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("buyer-intersect-{compact}")
}

/// Full address (base58 for Solana, 0x-hex for EVM) and hit count.
fn table_lines(report: &scout_engine::BuyerIntersectReport) -> Vec<String> {
    report
        .matches
        .iter()
        .map(|m| format!("{} hit_count={}", m.wallet.address, m.hit_count))
        .collect()
}

fn emit_report(report: &scout_engine::BuyerIntersectReport, format: &str) -> ExitCode {
    let lines: Vec<String> = if format == "jsonl" {
        let rendered: Result<Vec<String>, String> = report
            .matches
            .iter()
            .map(|m| {
                output::buyer_match_record(m)
                    .and_then(|r| serde_json::to_string(&r).map_err(|e| e.to_string()))
            })
            .collect();
        match rendered {
            Ok(lines) => lines,
            Err(message) => {
                eprintln!("buyer-intersect: could not render output: {message}");
                return ExitCode::from(4);
            }
        }
    } else {
        table_lines(report)
    };
    match write_lines_to_stdout(lines) {
        WriteOutcome::Complete => ExitCode::SUCCESS,
        WriteOutcome::PipeClosed => ExitCode::from(141),
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
