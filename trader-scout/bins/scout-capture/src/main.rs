//! `scout-capture`: dev/P0 tool (not one of the three product CLIs).
//!
//! Fetches raw Helius `getTransactionsForAddress` pages for one Solana
//! address, writes a redacted reproducible fixture (raw `result`
//! objects kept verbatim as `serde_json::Value`), and prints a
//! pump.fun instruction-variant summary so golden fixtures (P0.17) can
//! be selected and trimmed with `--keep-signatures`.
//!
//! Request shape mirrors `HeliusProvider` exactly (it is built by the same
//! `HeliusRequestOptions`): `transactionDetails: "full"`, `sortOrder`,
//! `limit`, optional `paginationToken` and optional `filters`; no
//! `encoding`/`maxSupportedTransactionVersion` is sent (HeliusProvider
//! does not send them either, and they are unmeasured here).
//!
//! Request-shaping flags (all default to the legacy request): `--limit N`
//! (1..=1000), `--status any|succeeded|failed`, `--since/--until` (strict
//! `YYYY-MM-DDTHH:MM:SSZ`, mapped to `filters.blockTime` gte/lt),
//! `--token-accounts none|balance-changed|all`. These are NOT yet
//! live-verified; the summary prints per-page tx counts, failed counts and
//! min/max blockTime so the server-side filters can be checked live.
//!
//! The API key is read only from `SCOUT_HELIUS_API_KEY` and never
//! appears in the output file, stdout or stderr. Test-only hook:
//! `SCOUT_CAPTURE_ENDPOINT` replaces the endpoint URL (used by the
//! offline wiremock tests).
//!
//! Exit codes: 0 ok; 2 argument error; 3 incomplete (request budget
//! exhausted); 4 configuration/infrastructure.
//!
//! `--max-requests N` (N >= 1) bounds the TOTAL HTTP attempts of the
//! run, retries included (absent = unlimited, still counted and printed
//! on stderr as `requests_made`). Exhausting a user-chosen budget is
//! incomplete coverage (exit 3), not infrastructure failure (exit 4):
//! pages fetched before the limit are still summarized and, with
//! `--out`, written with `incomplete` set in the fixture (never
//! presented as a complete capture). With zero pages nothing is
//! written. A terminal `RateLimited` (Retry-After above the transport
//! cap) or `ResponseTooLarge` is exit 4.
//!
//! Bounds: at most 10 sequential pages of `--limit` (default 100)
//! transactions. The response body cap follows
//! `HeliusRequestOptions::derived_max_response_bytes` (16 MiB default,
//! raised above 500 tx/page, never above 64 MiB).
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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use scout_api::ProviderError;
use scout_core::RawSolanaInstruction;
use scout_dex_solana::{PumpInstructionOutcome, classify_pump_instruction, hex8};
use scout_engine::{parse_rfc3339_utc, sanitize_provider_text};
use scout_providers::{
    DEFAULT_PAGE_LIMIT, HeliusRequestOptions, MAX_PAGE_LIMIT, StatusFilter, TokenAccountsFilter,
};
use scout_rpc::{DEFAULT_MAX_RETRY_AFTER, RequestBudgetExhausted, RpcClient, RpcEndpoint};
use serde_json::{Value, json};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_CAPTURE_ENDPOINT";
const METHOD: &str = "getTransactionsForAddress";
const PUMP_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const MAX_PAGES_HARD: u32 = 10;
const TIMEOUT_MS: u64 = 30_000;
const MAX_ATTEMPTS: u32 = 3;
const SAMPLES_PER_VARIANT: usize = 3;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Sort {
    Asc,
    Desc,
}

impl Sort {
    fn as_str(self) -> &'static str {
        match self {
            Sort::Asc => "asc",
            Sort::Desc => "desc",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StatusArg {
    Any,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TokenAccountsArg {
    None,
    BalanceChanged,
    All,
}

/// Capture raw Helius transactions for one address as a golden fixture.
#[derive(Debug, Parser)]
#[command(name = "scout-capture", version)]
struct Args {
    /// Solana address (base58, 32 bytes).
    #[arg(long)]
    address: String,

    /// Sort order of returned transactions.
    #[arg(long, value_enum, default_value = "desc")]
    sort: Sort,

    /// Pages to fetch sequentially (1..=10).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
    max_pages: u32,

    /// Transactions per page (`limit`, 1..=1000). A 1000-tx page is about
    /// 20 MB; the response cap is raised accordingly (max 64 MiB).
    #[arg(long, default_value_t = DEFAULT_PAGE_LIMIT,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_PAGE_LIMIT)))]
    limit: u32,

    /// Server-side `filters.status`. `any` sends no filter.
    #[arg(long, value_enum, default_value = "any")]
    status: StatusArg,

    /// Inclusive window start, `YYYY-MM-DDTHH:MM:SSZ` -> `blockTime.gte`.
    #[arg(long)]
    since: Option<String>,

    /// Exclusive window end, `YYYY-MM-DDTHH:MM:SSZ` -> `blockTime.lt`.
    #[arg(long)]
    until: Option<String>,

    /// Server-side `filters.tokenAccounts`. `none` sends no filter.
    #[arg(long, value_enum, default_value = "none")]
    token_accounts: TokenAccountsArg,

    /// Write the fixture JSON here.
    #[arg(long)]
    out: Option<String>,

    /// Comma-separated signatures; keep only these transactions in the
    /// written file (per page). The summary always covers all fetched
    /// transactions.
    #[arg(long, value_delimiter = ',')]
    keep_signatures: Vec<String>,

    /// Total HTTP request budget for the run (retries included), N >= 1.
    /// Absent = unlimited (requests are still counted and reported).
    /// When exhausted the capture is incomplete and the exit code is 3.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,
}

/// Effective request options from the flags; `Err` is an argument error.
fn request_options(args: &Args) -> Result<HeliusRequestOptions, String> {
    let gte = args
        .since
        .as_deref()
        .map(|t| parse_rfc3339_utc(t).map_err(|e| format!("--since: {e}")))
        .transpose()?;
    let lt = args
        .until
        .as_deref()
        .map(|t| parse_rfc3339_utc(t).map_err(|e| format!("--until: {e}")))
        .transpose()?;
    if let (Some(g), Some(l)) = (gte, lt)
        && g >= l
    {
        return Err("--since must be before --until".to_string());
    }
    Ok(HeliusRequestOptions {
        page_limit: args.limit,
        status: match args.status {
            StatusArg::Any => StatusFilter::Any,
            StatusArg::Succeeded => StatusFilter::Succeeded,
            StatusArg::Failed => StatusFilter::Failed,
        },
        block_time_gte: gte,
        block_time_lt: lt,
        token_accounts: match args.token_accounts {
            TokenAccountsArg::None => TokenAccountsFilter::None,
            TokenAccountsArg::BalanceChanged => TokenAccountsFilter::BalanceChanged,
            TokenAccountsArg::All => TokenAccountsFilter::All,
        },
    })
}

/// Why `run` failed.
enum RunError {
    Provider(ProviderError),
    Other(String),
}

impl From<String> for RunError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let key = std::env::var(HELIUS_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty());
    let Some(key) = key else {
        eprintln!("scout-capture: configuration required: set {HELIUS_KEY_ENV}");
        return ExitCode::from(4);
    };
    match bs58::decode(&args.address).into_vec() {
        Ok(b) if b.len() == 32 => {}
        _ => {
            eprintln!("scout-capture: --address is not a base58 32-byte Solana pubkey");
            return ExitCode::from(2);
        }
    }
    let options = match request_options(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("scout-capture: {message}");
            return ExitCode::from(2);
        }
    };
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("scout-capture: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };
    match rt.block_on(run(&args, &options, &key)) {
        Ok(Completion::Complete) => ExitCode::SUCCESS,
        Ok(Completion::BudgetExhausted) => ExitCode::from(3),
        Err(RunError::Other(message)) => {
            eprintln!("scout-capture: {}", redact(&message, &key));
            ExitCode::from(4)
        }
        Err(RunError::Provider(err)) => {
            let text = match &err {
                ProviderError::RateLimited {
                    retry_after: Some(d),
                } => format!(
                    "rate limited; server asked to retry after {}s (cap {}s)",
                    d.as_secs(),
                    DEFAULT_MAX_RETRY_AFTER.as_secs()
                ),
                ProviderError::RateLimited { retry_after: None } => {
                    "rate limited; server gave no Retry-After".to_string()
                }
                other => other.to_string(),
            };
            eprintln!("scout-capture: {}", redact(&text, &key));
            ExitCode::from(4)
        }
    }
}

enum Completion {
    Complete,
    BudgetExhausted,
}

fn redact(raw: &str, key: &str) -> String {
    let text = sanitize_provider_text(raw);
    if key.is_empty() {
        text
    } else {
        text.replace(key, "<redacted>")
    }
}

async fn run(
    args: &Args,
    options: &HeliusRequestOptions,
    key: &str,
) -> Result<Completion, RunError> {
    let endpoint = match std::env::var(ENDPOINT_OVERRIDE_ENV) {
        Ok(url) if !url.is_empty() => RpcEndpoint::new(url),
        _ => RpcEndpoint::new(format!("https://mainnet.helius-rpc.com/?api-key={key}")),
    };
    let client = RpcClient::new(endpoint, TIMEOUT_MS, MAX_ATTEMPTS)
        .map_err(RunError::Provider)?
        .with_max_total_requests(args.max_requests)
        .with_max_response_bytes(options.derived_max_response_bytes());
    let result = capture(args, options, key, &client).await;
    // Always report measured cost, also on failure.
    eprintln!(
        "scout-capture: requests_made={} max_requests={}",
        client.total_requests_made(),
        args.max_requests
            .map_or_else(|| "unlimited".to_string(), |n| n.to_string())
    );
    result
}

async fn capture(
    args: &Args,
    options: &HeliusRequestOptions,
    key: &str,
    client: &RpcClient,
) -> Result<Completion, RunError> {
    let mut pages: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut budget_limit: Option<u64> = None;
    for _ in 0..args.max_pages.min(MAX_PAGES_HARD) {
        let request = options.request_options_json(args.sort.as_str(), token.as_deref());
        let result: Value = match client.call(METHOD, json!([args.address, request])).await {
            Ok(result) => result,
            Err(ProviderError::Other(inner))
                if inner.downcast_ref::<RequestBudgetExhausted>().is_some() =>
            {
                budget_limit = inner
                    .downcast_ref::<RequestBudgetExhausted>()
                    .map(|e| e.limit);
                break;
            }
            Err(err) => return Err(RunError::Provider(err)),
        };
        token = result
            .get("paginationToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        pages.push(result);
        if token.is_none() {
            break;
        }
    }

    if let Some(limit) = budget_limit {
        eprintln!(
            "scout-capture: request budget exhausted after {} requests (limit {limit}); \
             capture is incomplete ({} page(s) fetched)",
            client.total_requests_made(),
            pages.len()
        );
    }

    if pages.is_empty() && budget_limit.is_some() {
        return Ok(Completion::BudgetExhausted);
    }

    if let Some(path) = &args.out {
        let keep: BTreeSet<&str> = args.keep_signatures.iter().map(String::as_str).collect();
        let written_pages: Vec<Value> = if keep.is_empty() {
            pages.clone()
        } else {
            pages.iter().map(|p| filter_page(p, &keep)).collect()
        };
        let doc = json!({
            "captured_at_utc": scout_app::now_utc_rfc3339(),
            "request": {
                "address": args.address,
                "sort": args.sort.as_str(),
                "max_pages": args.max_pages,
                "method": METHOD,
                "max_requests": args.max_requests,
                "limit": options.page_limit,
                "status": options.status,
                "since_unix": options.block_time_gte,
                "until_unix": options.block_time_lt,
                "token_accounts": options.token_accounts,
                "filters": options.filters_json(),
                "request_options": options.request_options_json(args.sort.as_str(), None),
            },
            "requests_made": client.total_requests_made(),
            "incomplete": budget_limit.map(|limit| format!(
                "request budget exhausted (limit {limit}); pages after the last one were not fetched"
            )),
            "pages": written_pages,
        });
        let mut text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        text.push('\n');
        if text.contains(key) {
            return Err("refusing to write: output would contain the API key"
                .to_string()
                .into());
        }
        std::fs::write(path, text).map_err(|e| format!("could not write {path}: {e}"))?;
    }

    let mut summary = page_stats_summary(&pages);
    summary.push_str(&summarize(&pages));
    // Defense in depth: the summary is built from provider data.
    print!("{}", redact_keep_text(&summary, key));
    Ok(if budget_limit.is_some() {
        Completion::BudgetExhausted
    } else {
        Completion::Complete
    })
}

/// Replace the literal key without truncating (summary lines are
/// structured, not provider error text).
fn redact_keep_text(text: &str, key: &str) -> String {
    text.replace(key, "<redacted>")
}

fn filter_page(page: &Value, keep: &BTreeSet<&str>) -> Value {
    let mut page = page.clone();
    if let Some(data) = page.get_mut("data").and_then(Value::as_array_mut) {
        data.retain(|tx| first_signature(tx).is_some_and(|s| keep.contains(s)));
    }
    page
}

fn first_signature(tx: &Value) -> Option<&str> {
    tx.pointer("/transaction/signatures/0")
        .and_then(Value::as_str)
}

type Key32 = [u8; 32];

fn pubkey(s: &str) -> Option<Key32> {
    bs58::decode(s).into_vec().ok()?.try_into().ok()
}

/// accountKeys, then loadedAddresses.writable, then .readonly.
fn account_keys(tx: &Value) -> Option<Vec<Option<Key32>>> {
    let mut keys = Vec::new();
    let static_keys = tx.pointer("/transaction/message/accountKeys")?.as_array()?;
    for k in static_keys {
        keys.push(k.as_str().and_then(pubkey));
    }
    for part in ["writable", "readonly"] {
        if let Some(list) = tx
            .pointer(&format!("/meta/loadedAddresses/{part}"))
            .and_then(Value::as_array)
        {
            for k in list {
                keys.push(k.as_str().and_then(pubkey));
            }
        }
    }
    Some(keys)
}

struct Row {
    sig: String,
    slot: u64,
    ok: bool,
    where_: &'static str,
    variant: String,
    data_len: usize,
    accounts: usize,
}

fn classify_instruction(
    ins: &Value,
    keys: &[Option<Key32>],
    pump: &Key32,
) -> Option<(String, usize, usize)> {
    let pid_idx = usize::try_from(ins.get("programIdIndex")?.as_u64()?).ok()?;
    let program_id = (*keys.get(pid_idx)?)?;
    if &program_id != pump {
        return None;
    }
    let accounts = ins
        .get("accounts")?
        .as_array()?
        .iter()
        .map(|a| {
            let idx = usize::try_from(a.as_u64()?).ok()?;
            (*keys.get(idx)?).or(Some([0; 32]))
        })
        .collect::<Option<Vec<Key32>>>();
    let account_count = ins.get("accounts")?.as_array()?.len();
    let data = bs58::decode(ins.get("data")?.as_str()?).into_vec().ok()?;
    let data_len = data.len();
    let variant = match accounts {
        None => "unresolved_accounts".to_string(),
        Some(accounts) => {
            let raw = RawSolanaInstruction {
                program_id,
                accounts,
                data,
                instruction_index: 0,
            };
            match classify_pump_instruction(&raw, 0, 0) {
                PumpInstructionOutcome::NotMine => "not_mine".to_string(),
                PumpInstructionOutcome::Trade(t) => t.variant.name().to_string(),
                PumpInstructionOutcome::NonTrade(n) => format!("non_trade:{n}"),
                PumpInstructionOutcome::Malformed { variant, reason } => format!(
                    "malformed:{}:{}",
                    variant.map_or("-", |v| v.name()),
                    reason
                        .split_whitespace()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("_")
                ),
                PumpInstructionOutcome::UnknownDiscriminator { discriminator } => {
                    format!("unknown:{}", hex8(&discriminator))
                }
            }
        }
    };
    Some((variant, data_len, account_count))
}

fn collect_rows(pages: &[Value]) -> (Vec<Row>, usize) {
    let pump = pubkey(PUMP_PROGRAM).unwrap_or([0; 32]);
    let mut rows = Vec::new();
    let mut skipped = 0usize;
    for page in pages {
        let Some(data) = page.get("data").and_then(Value::as_array) else {
            continue;
        };
        for tx in data {
            let (Some(sig), Some(keys)) = (first_signature(tx), account_keys(tx)) else {
                skipped += 1;
                continue;
            };
            let slot = tx.get("slot").and_then(Value::as_u64).unwrap_or(0);
            let ok = tx.pointer("/meta/err").is_some_and(Value::is_null);
            let mut push = |ins: &Value, where_: &'static str| {
                if let Some((variant, data_len, accounts)) = classify_instruction(ins, &keys, &pump)
                {
                    rows.push(Row {
                        sig: sig.to_string(),
                        slot,
                        ok,
                        where_,
                        variant,
                        data_len,
                        accounts,
                    });
                }
            };
            let top = tx
                .pointer("/transaction/message/instructions")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let inner = tx
                .pointer("/meta/innerInstructions")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for (i, ins) in top.iter().enumerate() {
                push(ins, "top");
                for group in inner
                    .iter()
                    .filter(|g| g.get("index").and_then(Value::as_u64) == u64::try_from(i).ok())
                {
                    for cpi in group
                        .get("instructions")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        push(cpi, "inner");
                    }
                }
            }
        }
    }
    (rows, skipped)
}

/// Per-page tx count, failed count and blockTime range, so server-side
/// filters can be verified live from stdout.
fn page_stats_summary(pages: &[Value]) -> String {
    let mut out = String::new();
    let (mut total, mut failed, mut min_t, mut max_t) = (0usize, 0usize, None::<i64>, None::<i64>);
    for (i, page) in pages.iter().enumerate() {
        let data = page
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let page_failed = data
            .iter()
            .filter(|tx| tx.pointer("/meta/err").is_some_and(|e| !e.is_null()))
            .count();
        let times: Vec<i64> = data
            .iter()
            .filter_map(|tx| tx.get("blockTime").and_then(Value::as_i64))
            .collect();
        let (pmin, pmax) = (times.iter().copied().min(), times.iter().copied().max());
        let _ = writeln!(
            out,
            "# page {}: txs={} failed={} min_block_time={} max_block_time={} more={}",
            i + 1,
            data.len(),
            page_failed,
            fmt_opt(pmin),
            fmt_opt(pmax),
            page.get("paginationToken").is_some_and(|t| !t.is_null()),
        );
        total += data.len();
        failed += page_failed;
        min_t = min_t.into_iter().chain(pmin).min();
        max_t = max_t.into_iter().chain(pmax).max();
    }
    let _ = writeln!(
        out,
        "# total: pages={} txs={total} failed={failed} min_block_time={} max_block_time={}\n",
        pages.len(),
        fmt_opt(min_t),
        fmt_opt(max_t),
    );
    out
}

fn fmt_opt(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_string(), |v| v.to_string())
}

fn summarize(pages: &[Value]) -> String {
    let (rows, skipped) = collect_rows(pages);
    let mut out = String::new();
    for r in &rows {
        let _ = writeln!(
            out,
            "sig={}.. slot={} tx_ok={} where={} variant={} data_len={} accounts={}",
            r.sig.chars().take(16).collect::<String>(),
            r.slot,
            r.ok,
            r.where_,
            r.variant,
            r.data_len,
            r.accounts
        );
    }
    let mut agg: BTreeMap<(&str, bool, usize, usize), usize> = BTreeMap::new();
    let mut samples: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in &rows {
        *agg.entry((&r.variant, r.ok, r.data_len, r.accounts))
            .or_insert(0) += 1;
        let list = samples.entry(&r.variant).or_default();
        if r.ok && list.len() < SAMPLES_PER_VARIANT && !list.contains(&r.sig.as_str()) {
            list.push(&r.sig);
        }
    }
    let _ = writeln!(out, "\n# aggregate: count variant tx_ok data_len accounts");
    for ((variant, ok, data_len, accounts), count) in &agg {
        let _ = writeln!(out, "{count}\t{variant}\t{ok}\t{data_len}\t{accounts}");
    }
    let _ = writeln!(out, "\n# successful-tx sample signatures per variant");
    for (variant, sigs) in &samples {
        if sigs.is_empty() {
            let _ = writeln!(out, "{variant}: (none successful)");
        } else {
            let _ = writeln!(out, "{variant}: {}", sigs.join(","));
        }
    }
    if skipped > 0 {
        let _ = writeln!(
            out,
            "\n# skipped {skipped} transaction(s) with unreadable shape"
        );
    }
    out
}
