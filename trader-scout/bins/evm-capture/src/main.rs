//! `evm-capture`: dev/P0 tool (not one of the three product CLIs).
//!
//! Scans one EVM chain (Robinhood / Base / BSC) for either all `Transfer`
//! logs of a token (`--token`, token-centric via `eth_getLogs`) or the
//! signer transactions of a wallet (`--wallet`, Blockscout `txlist` +
//! `txlistinternal`), over a block range or a `--since/--until` window, and
//! writes the raw JSON-RPC exchanges (method, params, result) as a redacted
//! fixture for golden tests. It prints a summary: transactions, logs,
//! Transfer count, swap-shaped topic counts per emitting address (marked
//! gated/ungated by the venue gate) and what the extraction rule would do.
//!
//! The RPC URL is read from the env var named by `--rpc-url-env` (it may
//! embed a key); neither it nor the Blockscout key (`--blockscout-key-env`,
//! default `SCOUT_BLOCKSCOUT_API_KEY`, wallet mode only) ever reaches stdout,
//! stderr or the fixture. Test-only hook: `SCOUT_EVM_CAPTURE_BLOCKSCOUT_URL`
//! replaces the Blockscout base URL.
//!
//! A chain identity preflight (`eth_chainId` + block-0 hash) runs first; a
//! mismatch aborts with exit 4 and nothing is scanned.
//!
//! Exit codes: 0 ok; 2 argument error; 3 incomplete (request budget
//! exhausted or a source reported incomplete data; the fixture, if written,
//! says `incomplete`); 4 configuration/infrastructure.
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

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use alloy_primitives::{Address, B256};
use clap::{Parser, ValueEnum};
use scout_core::RawEvmTransaction;
use scout_dex_evm::{
    GateOutcome, SwapVenueGate, V2_PAIR_CREATED_TOPIC0, V2_SWAP_EVENT_SIGNATURE,
    V3_POOL_CREATED_TOPIC0, V3_SWAP_TOPIC0, V4_INITIALIZE_TOPIC0, V4_SWAP_TOPIC0,
};
use scout_engine::{EvmExtractionConfig, EvmTxOutcome, extract_evm_trades, parse_rfc3339_utc};
use scout_evm::{EvmChainProfile, TRANSFER_TOPIC0, WETH_DEPOSIT_TOPIC0, WETH_WITHDRAWAL_TOPIC0};
use scout_providers::{
    BlockscoutApiKey, BlockscoutEvmConfig, BlockscoutEvmSource, CallRecorder, EvmHistoryScanner,
    EvmRpcClient, EvmSourceError, ReceiptMode, ScanLimits,
};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::{Value, json};

const BLOCKSCOUT_URL_OVERRIDE_ENV: &str = "SCOUT_EVM_CAPTURE_BLOCKSCOUT_URL";
const TIMEOUT_MS: u64 = 30_000;
const MAX_ATTEMPTS: u32 = 3;
const MAX_FIXTURE_CALLS: usize = 200_000;
const MAX_FIXTURE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ChainArg {
    Robinhood,
    Base,
    Bsc,
}

impl ChainArg {
    fn profile(self) -> EvmChainProfile {
        match self {
            ChainArg::Robinhood => scout_evm::ROBINHOOD,
            ChainArg::Base => scout_evm::BASE,
            ChainArg::Bsc => scout_evm::BSC,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ReceiptsArg {
    /// One `eth_getBlockReceipts` per block.
    Block,
    /// One `eth_getTransactionReceipt` per transaction.
    Tx,
}

/// Capture raw EVM history for a token or a wallet as a golden fixture.
#[derive(Debug, Parser)]
#[command(name = "evm-capture", version)]
struct Args {
    /// Chain; fixes chain id, genesis hash and wrapped-native address.
    #[arg(long, value_enum)]
    chain: ChainArg,

    /// Name of the env var holding the RPC URL.
    #[arg(long)]
    rpc_url_env: String,

    /// Token contract (token-centric scan). Exactly one of --token/--wallet.
    #[arg(long, conflicts_with = "wallet")]
    token: Option<String>,

    /// Wallet address (Blockscout wallet-centric scan).
    #[arg(long)]
    wallet: Option<String>,

    /// Inclusive window start, `YYYY-MM-DDTHH:MM:SSZ`.
    #[arg(long, conflicts_with_all = ["from_block", "to_block"])]
    since: Option<String>,

    /// Exclusive window end, `YYYY-MM-DDTHH:MM:SSZ`.
    #[arg(long, conflicts_with_all = ["from_block", "to_block"])]
    until: Option<String>,

    /// Inclusive first block (alternative to --since/--until).
    #[arg(long)]
    from_block: Option<u64>,

    /// Inclusive last block.
    #[arg(long)]
    to_block: Option<u64>,

    /// Env var with the Blockscout key (wallet mode).
    #[arg(long, default_value = "SCOUT_BLOCKSCOUT_API_KEY")]
    blockscout_key_env: String,

    #[arg(long, value_enum, default_value = "block")]
    receipts: ReceiptsArg,

    /// Max transactions assembled (hard bound).
    #[arg(long, default_value_t = 500)]
    max_txs: usize,

    /// Total HTTP request budget (retries included).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,

    /// Concurrent in-flight RPC requests (1..=64).
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=64))]
    concurrency: u32,

    /// Write the fixture JSON here.
    #[arg(long)]
    out: Option<String>,

    /// Comma-separated tx hashes: keep only calls mentioning these in the
    /// written file (the summary always covers everything fetched).
    #[arg(long, value_delimiter = ',')]
    keep_tx: Vec<String>,
}

enum Target {
    Token(Address),
    Wallet(Address),
}

struct Secrets(Vec<String>);

impl Secrets {
    fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for s in &self.0 {
            if s.len() >= 4 {
                out = out.replace(s.as_str(), "<redacted>");
            }
        }
        out.chars()
            .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
            .take(2_000)
            .collect()
    }
}

/// Secret parts of an RPC URL: the whole URL and everything after the host
/// (API keys live in the path or query).
fn url_secrets(url: &str) -> Vec<String> {
    let mut v = vec![url.to_string()];
    if let Some(rest) = url.split_once("://").map(|x| x.1)
        && let Some(i) = rest.find(['/', '?'])
    {
        let tail = rest.get(i..).unwrap_or("");
        if tail.len() > 1 {
            v.push(tail.to_string());
            v.extend(
                tail.split(['/', '?', '&', '='])
                    .filter(|p| p.len() >= 8)
                    .map(str::to_string),
            );
        }
    }
    v
}

fn parse_address(label: &str, text: &str) -> Result<Address, String> {
    text.parse::<Address>()
        .map_err(|_| format!("{label}: not a valid 20-byte hex address"))
}

fn main() -> ExitCode {
    let args = Args::parse();
    let target = match (&args.token, &args.wallet) {
        (Some(t), None) => parse_address("--token", t).map(Target::Token),
        (None, Some(w)) => parse_address("--wallet", w).map(Target::Wallet),
        _ => Err("exactly one of --token / --wallet is required".to_string()),
    };
    let target = match target {
        Ok(t) => t,
        Err(m) => {
            eprintln!("evm-capture: {m}");
            return ExitCode::from(2);
        }
    };
    let window = match window_args(&args) {
        Ok(w) => w,
        Err(m) => {
            eprintln!("evm-capture: {m}");
            return ExitCode::from(2);
        }
    };
    let Ok(url) = std::env::var(&args.rpc_url_env) else {
        eprintln!(
            "evm-capture: configuration required: env var {} is not set",
            args.rpc_url_env
        );
        return ExitCode::from(4);
    };
    if url.is_empty() {
        eprintln!(
            "evm-capture: configuration required: env var {} is empty",
            args.rpc_url_env
        );
        return ExitCode::from(4);
    }
    let mut secrets = Secrets(url_secrets(&url));
    let bs_key = if matches!(target, Target::Wallet(_)) {
        match std::env::var(&args.blockscout_key_env) {
            Ok(k) if !k.is_empty() => {
                secrets.0.push(k.clone());
                Some(k)
            }
            _ => {
                eprintln!(
                    "evm-capture: configuration required: env var {} is not set (wallet mode needs Blockscout)",
                    args.blockscout_key_env
                );
                return ExitCode::from(4);
            }
        }
    } else {
        None
    };
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("evm-capture: could not start async runtime: {e}");
            return ExitCode::from(4);
        }
    };
    match rt.block_on(run(&args, target, window, &url, bs_key, &secrets)) {
        Ok(Completion::Complete) => ExitCode::SUCCESS,
        Ok(Completion::Incomplete(why)) => {
            eprintln!("evm-capture: INCOMPLETE: {}", secrets.redact(&why));
            ExitCode::from(3)
        }
        Err(e) => {
            eprintln!("evm-capture: {}", secrets.redact(&e));
            ExitCode::from(4)
        }
    }
}

enum Window {
    Blocks(u64, u64),
    Time(Option<u64>, Option<u64>),
}

fn window_args(args: &Args) -> Result<Window, String> {
    let ts = |label: &str, t: &Option<String>| -> Result<Option<u64>, String> {
        t.as_deref()
            .map(|t| {
                parse_rfc3339_utc(t)
                    .map_err(|e| format!("{label}: {e}"))
                    .and_then(|v| u64::try_from(v).map_err(|_| format!("{label}: before 1970")))
            })
            .transpose()
    };
    match (args.from_block, args.to_block, &args.since, &args.until) {
        (Some(a), Some(b), None, None) if a <= b => Ok(Window::Blocks(a, b)),
        (Some(_), Some(_), None, None) => Err("--from-block must be <= --to-block".to_string()),
        (None, None, None, None) => {
            Err("a window is required: --since/--until or --from-block/--to-block".to_string())
        }
        (None, None, _, _) => {
            let (s, u) = (ts("--since", &args.since)?, ts("--until", &args.until)?);
            if let (Some(s), Some(u)) = (s, u)
                && s >= u
            {
                return Err("--since must be before --until".to_string());
            }
            Ok(Window::Time(s, u))
        }
        _ => Err("--from-block and --to-block must be given together".to_string()),
    }
}

enum Completion {
    Complete,
    Incomplete(String),
}

async fn run(
    args: &Args,
    target: Target,
    window: Window,
    url: &str,
    bs_key: Option<String>,
    secrets: &Secrets,
) -> Result<Completion, String> {
    let profile = args.chain.profile();
    let rpc = RpcClient::new(RpcEndpoint::new(url), TIMEOUT_MS, MAX_ATTEMPTS)
        .map_err(|e| e.to_string())?
        .with_max_total_requests(args.max_requests);
    let recorder = Arc::new(CallRecorder::new(MAX_FIXTURE_CALLS, MAX_FIXTURE_BYTES));
    let evm = EvmRpcClient::with_config(
        rpc,
        profile,
        scout_providers::EvmRpcConfig {
            concurrency: usize::try_from(args.concurrency).unwrap_or(8),
            ..scout_providers::EvmRpcConfig::default()
        },
    )
    .with_recorder(recorder.clone());

    let outcome = scan(args, &target, &window, &evm, bs_key, &recorder, &profile).await;
    eprintln!(
        "evm-capture: rpc_requests_made={}",
        evm.total_requests_made()
    );
    let (scanned, completion) = match outcome {
        Ok(v) => v,
        Err(e) if e.is_budget_exhausted() => {
            let (calls, dropped) = recorder.snapshot();
            write_fixture(
                args, &profile, &target, &window, calls, dropped, true, None, secrets,
            )?;
            return Ok(Completion::Incomplete(
                "request budget exhausted before the scan finished".to_string(),
            ));
        }
        Err(e) => return Err(e.to_string()),
    };
    let (calls, dropped) = recorder.snapshot();
    let incomplete = !matches!(completion, Completion::Complete);
    let text = summarize(&scanned, &profile, &target);
    print!("{}", secrets.redact(&text));
    write_fixture(
        args,
        &profile,
        &target,
        &window,
        calls,
        dropped,
        incomplete,
        Some(&scanned),
        secrets,
    )?;
    Ok(completion)
}

struct Scanned {
    window_blocks: Option<(u64, u64)>,
    transactions: Vec<RawEvmTransaction>,
    transfer_logs: Option<usize>,
    log_requests: u32,
    log_splits: u32,
    txlist_complete: Option<bool>,
    internal_complete: Option<bool>,
}

async fn scan(
    args: &Args,
    target: &Target,
    window: &Window,
    evm: &EvmRpcClient,
    bs_key: Option<String>,
    _recorder: &CallRecorder,
    profile: &EvmChainProfile,
) -> Result<(Scanned, Completion), EvmSourceError> {
    let chain = evm.preflight().await?;
    let scanner = EvmHistoryScanner::new(
        evm.clone(),
        chain,
        ScanLimits {
            max_transactions: args.max_txs,
            receipt_mode: match args.receipts {
                ReceiptsArg::Block => ReceiptMode::BlockReceipts,
                ReceiptsArg::Tx => ReceiptMode::PerTransaction,
            },
        },
    );
    let blocks = match window {
        Window::Blocks(a, b) => Some((*a, *b)),
        Window::Time(s, u) => scanner.resolve_window(*s, *u).await?,
    };
    let mut scanned = Scanned {
        window_blocks: blocks,
        transactions: Vec::new(),
        transfer_logs: None,
        log_requests: 0,
        log_splits: 0,
        txlist_complete: None,
        internal_complete: None,
    };
    let Some((from, to)) = blocks else {
        return Ok((scanned, Completion::Complete));
    };
    let mut completion = Completion::Complete;
    match target {
        Target::Token(token) => {
            let out = scanner.scan_token(*token, from, to).await?;
            scanned.transactions = out.transactions;
            scanned.transfer_logs = Some(out.transfer_logs);
            scanned.log_requests = out.log_requests;
            scanned.log_splits = out.log_splits;
        }
        Target::Wallet(wallet) => {
            let key = bs_key.unwrap_or_default();
            let mut cfg = BlockscoutEvmConfig::new(profile.chain_id, BlockscoutApiKey::new(key));
            if let Ok(base) = std::env::var(BLOCKSCOUT_URL_OVERRIDE_ENV)
                && !base.is_empty()
            {
                cfg.base_url = base;
            }
            cfg.max_total_requests = args.max_requests;
            let source = BlockscoutEvmSource::new(cfg)?;
            let out = scanner.scan_wallet(&source, *wallet, from, to).await?;
            scanned.transactions = out.transactions;
            scanned.txlist_complete = Some(out.txlist_complete);
            scanned.internal_complete = Some(out.internal_complete);
            if !out.txlist_complete {
                completion = Completion::Incomplete(
                    "Blockscout txlist hit the page cap; the transaction list is truncated"
                        .to_string(),
                );
            }
        }
    }
    Ok((scanned, completion))
}

fn topic_label(t: &B256) -> Option<&'static str> {
    [
        (TRANSFER_TOPIC0, "erc20_transfer"),
        (WETH_DEPOSIT_TOPIC0, "weth_deposit"),
        (WETH_WITHDRAWAL_TOPIC0, "weth_withdrawal"),
        (V2_SWAP_EVENT_SIGNATURE, "v2_swap"),
        (V3_SWAP_TOPIC0, "v3_swap"),
        (V4_SWAP_TOPIC0, "v4_swap"),
        (V4_INITIALIZE_TOPIC0, "v4_initialize"),
        (V3_POOL_CREATED_TOPIC0, "v3_pool_created"),
        (V2_PAIR_CREATED_TOPIC0, "v2_pair_created"),
    ]
    .into_iter()
    .find(|(topic, _)| topic == t)
    .map(|(_, label)| label)
}

fn summarize(s: &Scanned, profile: &EvmChainProfile, target: &Target) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "chain={} chain_id={} preflight=ok",
        profile.name, profile.chain_id
    );
    match s.window_blocks {
        Some((a, b)) => {
            let _ = writeln!(out, "window_blocks={a}..={b}");
        }
        None => {
            let _ = writeln!(out, "window_blocks=empty (no block in the time window)");
        }
    }
    let logs: usize = s.transactions.iter().map(|t| t.logs.len()).sum();
    let _ = writeln!(out, "transactions={} logs={logs}", s.transactions.len());
    if let Some(n) = s.transfer_logs {
        let _ = writeln!(
            out,
            "token_transfer_logs={n} get_logs_requests={} range_splits={}",
            s.log_requests, s.log_splits
        );
    }
    if let (Some(a), Some(b)) = (s.txlist_complete, s.internal_complete) {
        let _ = writeln!(out, "txlist_complete={a} internal_transfers_complete={b}");
    }
    let gate = SwapVenueGate::new(profile.chain_id);
    let mut counts: BTreeMap<(&'static str, Address), (u64, &'static str)> = BTreeMap::new();
    for l in s.transactions.iter().flat_map(|t| &t.logs) {
        let Some(label) = l.topics.first().and_then(topic_label) else {
            continue;
        };
        if label == "erc20_transfer" {
            continue;
        }
        let gated = match gate.classify(l) {
            GateOutcome::Verified(_) => "gated",
            GateOutcome::UngatedEmitter { .. } => "ungated",
            _ => "-",
        };
        let e = counts.entry((label, l.address)).or_insert((0, gated));
        e.0 += 1;
    }
    let transfers: usize = s
        .transactions
        .iter()
        .flat_map(|t| &t.logs)
        .filter(|l| l.topics.first() == Some(&TRANSFER_TOPIC0))
        .count();
    let _ = writeln!(out, "erc20_transfer_topic_logs_in_txs={transfers}");
    let _ = writeln!(out, "swap/venue topic counts by emitting address:");
    for ((label, addr), (n, gated)) in &counts {
        let _ = writeln!(out, "  {label} {addr:#x} count={n} gate={gated}");
    }
    if counts.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    // What the ADR-020 rule would do with default config (no quote tokens).
    let cfg = EvmExtractionConfig::new(*profile, SwapVenueGate::new(profile.chain_id));
    let token_filter = match target {
        Target::Token(t) => Some(*t),
        Target::Wallet(_) => None,
    };
    let (ex, sum) = extract_evm_trades(&s.transactions, &cfg, None, token_filter);
    let _ = writeln!(
        out,
        "extraction(no stable quote tokens, gate=IdlOnly): trades={} failed={} unknown_consideration={} duplicates={}",
        sum.trades, sum.failed, sum.unknown_consideration, sum.duplicates_ignored
    );
    for (reason, n) in &sum.no_trade {
        let _ = writeln!(out, "  no_trade {reason}={n}");
    }
    let sample = ex
        .iter()
        .filter(|e| matches!(e.outcome, EvmTxOutcome::Trade(_)))
        .take(3)
        .map(|e| format!("{:#x}", e.tx_hash))
        .collect::<Vec<_>>();
    if !sample.is_empty() {
        let _ = writeln!(out, "  sample_trade_txs={}", sample.join(","));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn write_fixture(
    args: &Args,
    profile: &EvmChainProfile,
    target: &Target,
    window: &Window,
    calls: Vec<Value>,
    dropped: u64,
    incomplete: bool,
    scanned: Option<&Scanned>,
    secrets: &Secrets,
) -> Result<(), String> {
    let Some(path) = &args.out else {
        return Ok(());
    };
    let keep: Vec<String> = args
        .keep_tx
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let calls: Vec<Value> = if keep.is_empty() {
        calls
    } else {
        calls
            .into_iter()
            .filter(|c| {
                let text = c.to_string().to_ascii_lowercase();
                // Keep chain-identity/range/lookup calls and anything that
                // mentions a kept hash.
                c["method"] != "eth_getTransactionByHash"
                    && c["method"] != "eth_getTransactionReceipt"
                    || keep.iter().any(|h| text.contains(h))
            })
            .collect()
    };
    let (kind, subject) = match target {
        Target::Token(a) => ("token", format!("{a:#x}")),
        Target::Wallet(a) => ("wallet", format!("{a:#x}")),
    };
    let window_json = match window {
        Window::Blocks(a, b) => json!({"from_block": a, "to_block": b}),
        Window::Time(s, u) => json!({"since": s, "until": u,
            "resolved_blocks": scanned.and_then(|x| x.window_blocks)}),
    };
    let doc = json!({
        "tool": "evm-capture",
        "chain": profile.name,
        "chain_id": profile.chain_id,
        "genesis_hash": format!("{:#x}", profile.genesis_hash),
        "scope": {kind: subject, "window": window_json},
        "incomplete": incomplete,
        "calls_dropped_over_cap": dropped,
        "calls": calls,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    if secrets
        .0
        .iter()
        .any(|s| s.len() >= 4 && text.contains(s.as_str()))
    {
        return Err("refusing to write fixture: it contains a secret value".to_string());
    }
    std::fs::write(path, text).map_err(|e| format!("could not write {path}: {e}"))?;
    eprintln!("evm-capture: wrote {path}");
    Ok(())
}
