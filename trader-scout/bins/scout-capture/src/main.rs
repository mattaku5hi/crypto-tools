//! `scout-capture`: dev/P0 tool (not one of the three product CLIs).
//!
//! Fetches raw Helius `getTransactionsForAddress` pages for one Solana
//! address, writes a redacted reproducible fixture (raw `result`
//! objects kept verbatim as `serde_json::Value`), and prints a
//! pump.fun instruction-variant summary so golden fixtures (P0.17) can
//! be selected and trimmed with `--keep-signatures`.
//!
//! Request shape mirrors `HeliusProvider` exactly: `transactionDetails:
//! "full"`, `sortOrder`, `limit`, optional `paginationToken`; no
//! `encoding`/`maxSupportedTransactionVersion` is sent (HeliusProvider
//! does not send them either, and they are unmeasured here).
//!
//! The API key is read only from `SCOUT_HELIUS_API_KEY` and never
//! appears in the output file, stdout or stderr. Test-only hook:
//! `SCOUT_CAPTURE_ENDPOINT` replaces the endpoint URL (used by the
//! offline wiremock tests).
//!
//! Exit codes: 0 ok; 2 argument error; 4 configuration/infrastructure.
//!
//! Bounds: at most 10 sequential pages of 100 transactions. NOTE:
//! `scout-rpc` does not bound response body size (only the 30 s request
//! timeout), so each page is limited by what Helius returns for 100
//! full transactions.
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
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use scout_core::RawSolanaInstruction;
use scout_dex_solana::{PumpInstructionOutcome, classify_pump_instruction, hex8};
use scout_engine::sanitize_provider_text;
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::{Value, json};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_CAPTURE_ENDPOINT";
const METHOD: &str = "getTransactionsForAddress";
const PUMP_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const PAGE_LIMIT: u32 = 100;
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

    /// Write the fixture JSON here.
    #[arg(long)]
    out: Option<String>,

    /// Comma-separated signatures; keep only these transactions in the
    /// written file (per page). The summary always covers all fetched
    /// transactions.
    #[arg(long, value_delimiter = ',')]
    keep_signatures: Vec<String>,
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
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("scout-capture: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };
    match rt.block_on(run(&args, &key)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("scout-capture: {}", redact(&message, &key));
            ExitCode::from(4)
        }
    }
}

fn redact(raw: &str, key: &str) -> String {
    let text = sanitize_provider_text(raw);
    if key.is_empty() {
        text
    } else {
        text.replace(key, "<redacted>")
    }
}

async fn run(args: &Args, key: &str) -> Result<(), String> {
    let endpoint = match std::env::var(ENDPOINT_OVERRIDE_ENV) {
        Ok(url) if !url.is_empty() => RpcEndpoint::new(url),
        _ => RpcEndpoint::new(format!("https://mainnet.helius-rpc.com/?api-key={key}")),
    };
    let client = RpcClient::new(endpoint, TIMEOUT_MS, MAX_ATTEMPTS).map_err(|e| e.to_string())?;

    let mut pages: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..args.max_pages.min(MAX_PAGES_HARD) {
        let mut options = json!({
            "transactionDetails": "full",
            "sortOrder": args.sort.as_str(),
            "limit": PAGE_LIMIT,
        });
        if let (Some(t), Some(map)) = (token.as_deref(), options.as_object_mut()) {
            map.insert("paginationToken".to_string(), t.into());
        }
        let result: Value = client
            .call(METHOD, json!([args.address, options]))
            .await
            .map_err(|e| e.to_string())?;
        token = result
            .get("paginationToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        pages.push(result);
        if token.is_none() {
            break;
        }
    }

    if let Some(path) = &args.out {
        let keep: BTreeSet<&str> = args.keep_signatures.iter().map(String::as_str).collect();
        let written_pages: Vec<Value> = if keep.is_empty() {
            pages.clone()
        } else {
            pages.iter().map(|p| filter_page(p, &keep)).collect()
        };
        let doc = json!({
            "captured_at_utc": now_utc(),
            "request": {
                "address": args.address,
                "sort": args.sort.as_str(),
                "max_pages": args.max_pages,
                "method": METHOD,
            },
            "pages": written_pages,
        });
        let mut text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        text.push('\n');
        if text.contains(key) {
            return Err("refusing to write: output would contain the API key".to_string());
        }
        std::fs::write(path, text).map_err(|e| format!("could not write {path}: {e}"))?;
    }

    let summary = summarize(&pages);
    // Defense in depth: the summary is built from provider data.
    print!("{}", redact_keep_text(&summary, key));
    Ok(())
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

/// Current UTC time as RFC 3339 (seconds), no external time crate.
#[allow(clippy::integer_division)] // calendar arithmetic is intentionally truncating
fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
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
