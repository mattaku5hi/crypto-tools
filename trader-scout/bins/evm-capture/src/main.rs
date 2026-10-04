//! `evm-capture`: dev/P0 tool (not one of the three product CLIs).
//!
//! Scans one EVM chain (Robinhood / Base / BSC) for either all `Transfer`
//! logs of a token (`--token`, token-centric via `eth_getLogs`), the
//! signer transactions of a wallet (`--wallet`, Blockscout `txlist` +
//! `txlistinternal`), or the venue `Swap` events themselves (`--swaps
//! uniswap-v2|uniswap-v3|uniswap-v4|aerodrome-v2|pancake-v3|fourmeme|all`, see below), over a block range or a
//! `--since/--until` window, and
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
//! Swap-topic mode (`--swaps`): `eth_getLogs` by the venue's `Swap` topic0
//! (v2/v3/Pancake v3: NO address filter, any emitter; v4: the PoolManager;
//! `fourmeme`: the pinned TokenManager V1/V2 addresses with their
//! `TokenPurchase`/`TokenSale` topics), the newest
//! `--max-txs` distinct transactions are kept (receipts + transactions
//! fetched), and for every distinct v2/v3/Pancake v3 emitter in them (at most
//! `--max-pools`) `factory()`, `token0()`, `token1()`, `fee()` (v3) and the
//! factory's `getPool`/`getPair` answer are read with `eth_call` at the head
//! block and written to the fixture as `pool_metadata` (the calls themselves
//! are in `calls` too). The summary prints per-emitter counts, the factory
//! each pool reports and the gate's admission verdict.
//!
//! Network politeness is the CLIs' (`scout_app::make_limiter`): a shared
//! token-bucket limiter in front of every HTTP attempt (`--rpc-rps`, default
//! 10/s keyed and 5/s for the known keyless endpoints, or `--rpc-cu-per-sec`
//! for compute-unit providers such as Alchemy, about 300), AIMD halving on a
//! 429 without `Retry-After`, up to 8 attempts per request with exponential
//! backoff / `Retry-After`, and the optional `SCOUT_<CHAIN>_LOGS_RPC_URL`
//! endpoint for `eth_getLogs` only (receipts and `eth_call` stay on the main
//! RPC). A 429 backs off and continues; only an exhausted `--max-requests`
//! budget or a provider that keeps refusing after every attempt (or asks for
//! a wait over 60 s) stops the capture, which is then written as
//! `incomplete` (exit 3). A `rate limit:` line per endpoint is printed at the
//! end on stderr.
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
use futures::stream::{self, StreamExt};
use scout_app::{
    EvmNetOptions, LimiterNotice, ROBINHOOD_PUBLIC_RPC, logs_rpc_env_name, make_limiter,
    rate_limit_line,
};
use scout_core::RawEvmTransaction;
use scout_dex_evm::{
    AERODROME_V2_SWAP_TOPIC0, AnchorRole, FOURMEME_V1_PURCHASE_TOPIC0, FOURMEME_V1_SALE_TOPIC0,
    FOURMEME_V2_PURCHASE_TOPIC0, FOURMEME_V2_SALE_TOPIC0, GateOutcome, PANCAKE_V3_SWAP_TOPIC0,
    SwapVenue, SwapVenueGate, V2_PAIR_CREATED_TOPIC0, V2_SWAP_EVENT_SIGNATURE,
    V3_POOL_CREATED_TOPIC0, V3_SWAP_TOPIC0, V4_INITIALIZE_TOPIC0, V4_SWAP_TOPIC0,
    VENUE_DEPLOYMENTS,
};
use scout_engine::{
    EvmExtractionConfig, EvmTxOutcome, admit_recorded, extract_evm_trades, parse_rfc3339_utc,
    pool_kind, pool_venue,
};
use scout_evm::{EvmChainProfile, TRANSFER_TOPIC0, WETH_DEPOSIT_TOPIC0, WETH_WITHDRAWAL_TOPIC0};
use scout_providers::{
    BlockscoutApiKey, BlockscoutEvmConfig, BlockscoutEvmSource, CallRecorder, EvmHistoryScanner,
    EvmRpcClient, EvmSourceError, LogFilter, PoolKind, PoolOnchainMetadata, ReceiptMode,
    ScanLimits,
};
use scout_rpc::{RateLimiter, RpcClient, RpcEndpoint};
use serde_json::{Value, json};

const BLOCKSCOUT_URL_OVERRIDE_ENV: &str = "SCOUT_EVM_CAPTURE_BLOCKSCOUT_URL";
const TIMEOUT_MS: u64 = 30_000;
/// Attempts per request: a 429 backs off (exponential, `Retry-After`) and
/// retries this many times before the call fails (bounded, invariant #13).
const MAX_ATTEMPTS: u32 = 8;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SwapsArg {
    UniswapV2,
    UniswapV3,
    UniswapV4,
    /// Aerodrome v2 (Solidly-style `Swap` topic, any emitter). Slipstream
    /// shares the Uniswap v3 topic and is captured by `uniswap-v3`/`all`.
    AerodromeV2,
    /// PancakeSwap v3 (its own `Swap` topic with two protocol-fee words, any
    /// emitter; pools are admitted via the PoolDeployer CREATE2 / factory).
    PancakeV3,
    /// four.meme TokenManager V1/V2 `TokenPurchase`/`TokenSale` (logs by the
    /// pinned manager addresses; BSC only).
    #[value(name = "fourmeme")]
    FourMeme,
    /// v2 + v3 + Aerodrome v2 + Pancake v3 (any emitter), v4 (PoolManager)
    /// and the four.meme managers where the chain pins them.
    All,
}

impl SwapsArg {
    fn label(self) -> &'static str {
        match self {
            SwapsArg::UniswapV2 => "uniswap-v2",
            SwapsArg::UniswapV3 => "uniswap-v3",
            SwapsArg::UniswapV4 => "uniswap-v4",
            SwapsArg::AerodromeV2 => "aerodrome-v2",
            SwapsArg::PancakeV3 => "pancake-v3",
            SwapsArg::FourMeme => "fourmeme",
            SwapsArg::All => "all",
        }
    }
}

/// Capture raw EVM history for a token, a wallet or a venue's swaps as a
/// golden fixture.
#[derive(Debug, Parser)]
#[command(name = "evm-capture", version)]
struct Args {
    /// Chain; fixes chain id, genesis hash and wrapped-native address.
    #[arg(long, value_enum)]
    chain: ChainArg,

    /// Name of the env var holding the RPC URL.
    #[arg(long)]
    rpc_url_env: String,

    /// Token contract (token-centric scan). Exactly one of
    /// --token/--wallet/--swaps.
    #[arg(long, conflicts_with_all = ["wallet", "swaps"])]
    token: Option<String>,

    /// Wallet address (Blockscout wallet-centric scan).
    #[arg(long, conflicts_with = "swaps")]
    wallet: Option<String>,

    /// Swap-topic scan of a venue family (v2/v3: any emitter; v4: the
    /// PoolManager); records pool metadata of every v2/v3 emitter.
    #[arg(long, value_enum)]
    swaps: Option<SwapsArg>,

    /// Swap mode: max distinct v2/v3 emitters whose `factory()`/`token0()`/
    /// `token1()`/`fee()`/`getPool` are read (hard bound).
    #[arg(long, default_value_t = 200)]
    max_pools: usize,

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

    #[command(flatten)]
    net: EvmNetOptions,

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
    Swaps(SwapsArg),
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
    let target = match (&args.token, &args.wallet, args.swaps) {
        (Some(t), None, None) => parse_address("--token", t).map(Target::Token),
        (None, Some(w), None) => parse_address("--wallet", w).map(Target::Wallet),
        (None, None, Some(v)) => Ok(Target::Swaps(v)),
        _ => Err("exactly one of --token / --wallet / --swaps is required".to_string()),
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
    let logs_url = logs_rpc_env_name(args.chain.profile().name)
        .and_then(|var| std::env::var(var).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if let Some(l) = &logs_url {
        secrets.0.extend(url_secrets(l));
    }
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
    match rt.block_on(run(
        &args,
        target,
        window,
        Endpoints {
            main: &url,
            logs: logs_url.as_deref(),
        },
        bs_key,
        &secrets,
    )) {
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

/// The RPC URLs of the run (secrets; never printed).
struct Endpoints<'a> {
    main: &'a str,
    logs: Option<&'a str>,
}

/// Known keyless endpoints: capped at the public rate like the CLIs' own
/// keyless fallback.
fn is_public_rpc(url: &str) -> bool {
    [
        ROBINHOOD_PUBLIC_RPC,
        "https://mainnet.base.org",
        "https://base-rpc.publicnode.com",
        "https://bsc-rpc.publicnode.com",
        "https://bsc-dataseed.bnbchain.org",
    ]
    .iter()
    .any(|p| url.trim_end_matches('/') == *p)
}

/// One endpoint's client with the CLIs' limiter in front of it.
fn limited_client(
    url: &str,
    max_requests: Option<u64>,
    net: &EvmNetOptions,
    label: &str,
    notice: &LimiterNotice,
    limiters: &mut Vec<(String, Arc<RateLimiter>)>,
) -> Result<RpcClient, String> {
    let (limiter, cost) = make_limiter(net, is_public_rpc(url), label, notice);
    limiters.push((label.to_string(), Arc::clone(&limiter)));
    Ok(
        RpcClient::new(RpcEndpoint::new(url), TIMEOUT_MS, MAX_ATTEMPTS)
            .map_err(|e| e.to_string())?
            .with_max_total_requests(max_requests)
            .with_rate_limiter(limiter, cost),
    )
}

async fn run(
    args: &Args,
    target: Target,
    window: Window,
    endpoints: Endpoints<'_>,
    bs_key: Option<String>,
    secrets: &Secrets,
) -> Result<Completion, String> {
    let profile = args.chain.profile();
    let redacted: LimiterNotice = {
        let s = Secrets(secrets.0.clone());
        Arc::new(move |m: String| eprintln!("evm-capture: {}", s.redact(&m)))
    };
    let mut limiters: Vec<(String, Arc<RateLimiter>)> = Vec::new();
    let rpc = limited_client(
        endpoints.main,
        args.max_requests,
        &args.net,
        "rpc",
        &redacted,
        &mut limiters,
    )?;
    let recorder = Arc::new(CallRecorder::new(MAX_FIXTURE_CALLS, MAX_FIXTURE_BYTES));
    let mut evm = EvmRpcClient::with_config(
        rpc,
        profile,
        scout_providers::EvmRpcConfig {
            concurrency: usize::try_from(args.concurrency).unwrap_or(8),
            ..scout_providers::EvmRpcConfig::default()
        },
    )
    .with_recorder(recorder.clone());
    if let Some(logs) = endpoints.logs {
        let logs_rpc = limited_client(
            logs,
            args.max_requests,
            &args.net,
            "logs rpc",
            &redacted,
            &mut limiters,
        )?;
        evm = evm.with_logs_endpoint(logs_rpc);
    }
    let result = run_scan(
        args, target, window, &evm, bs_key, secrets, &recorder, &profile,
    )
    .await;
    for (label, l) in &limiters {
        eprintln!(
            "evm-capture: rate limit: {}",
            secrets.redact(&rate_limit_line(label, &l.stats()))
        );
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_scan(
    args: &Args,
    target: Target,
    window: Window,
    evm: &EvmRpcClient,
    bs_key: Option<String>,
    secrets: &Secrets,
    recorder: &Arc<CallRecorder>,
    profile: &EvmChainProfile,
) -> Result<Completion, String> {
    let outcome = scan(args, &target, &window, evm, bs_key, recorder, profile).await;
    eprintln!(
        "evm-capture: rpc_requests_made={}",
        evm.total_requests_made()
    );
    let (scanned, completion) = match outcome {
        Ok(v) => v,
        Err(e) if e.is_budget_exhausted() || e.is_rate_limited() => {
            let (calls, dropped) = recorder.snapshot();
            write_fixture(
                args, profile, &target, &window, calls, dropped, true, None, secrets,
            )?;
            return Ok(Completion::Incomplete(if e.is_rate_limited() {
                format!(
                    "the provider kept rate limiting (429) after {MAX_ATTEMPTS} attempts or \
                     asked for a wait over the cap: {e}"
                )
            } else {
                "request budget exhausted before the scan finished".to_string()
            }));
        }
        Err(e) => return Err(e.to_string()),
    };
    let (calls, dropped) = recorder.snapshot();
    let incomplete = !matches!(completion, Completion::Complete);
    let text = summarize(&scanned, profile, &target);
    print!("{}", secrets.redact(&text));
    write_fixture(
        args,
        profile,
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
    /// Swap mode only.
    swap: Option<SwapScanFacts>,
    /// Swap mode: what the v2/v3 emitters report (fixture `pool_metadata`).
    pool_metadata: Vec<PoolOnchainMetadata>,
    /// Block tag the metadata calls used (the head block, hex).
    pool_metadata_block: Option<String>,
}

struct SwapScanFacts {
    swap_logs: usize,
    txs_before_cap: usize,
    /// v2/v3 emitters not looked up because of `--max-pools`.
    pools_skipped: usize,
}

/// `eth_getLogs` filters of a swap scan: v2/v3 share one address-less filter
/// (OR on topic0), v4 is the chain's PoolManager.
fn swap_filters(swaps: SwapsArg, chain_id: u64) -> Result<Vec<LogFilter>, EvmSourceError> {
    let mut topics = Vec::new();
    if matches!(swaps, SwapsArg::UniswapV2 | SwapsArg::All) {
        topics.push(V2_SWAP_EVENT_SIGNATURE);
    }
    if matches!(swaps, SwapsArg::UniswapV3 | SwapsArg::All) {
        topics.push(V3_SWAP_TOPIC0);
    }
    if matches!(swaps, SwapsArg::AerodromeV2 | SwapsArg::All) {
        topics.push(AERODROME_V2_SWAP_TOPIC0);
    }
    if matches!(swaps, SwapsArg::PancakeV3 | SwapsArg::All) {
        topics.push(PANCAKE_V3_SWAP_TOPIC0);
    }
    let mut out = Vec::new();
    if !topics.is_empty() {
        out.push(LogFilter {
            addresses: Vec::new(),
            topics: [Some(topics), None, None, None],
        });
    }
    if matches!(swaps, SwapsArg::UniswapV4 | SwapsArg::All) {
        let managers: Vec<Address> = VENUE_DEPLOYMENTS
            .iter()
            .filter(|d| {
                d.chain_id == chain_id
                    && d.venue == SwapVenue::UniswapV4
                    && d.role == AnchorRole::SwapEmitter
            })
            .map(|d| d.anchor)
            .collect();
        if managers.is_empty() {
            return Err(EvmSourceError::NotFound {
                what: format!("a pinned Uniswap v4 PoolManager for chain {chain_id}"),
            });
        }
        out.push(LogFilter {
            addresses: managers,
            topics: [Some(vec![V4_SWAP_TOPIC0]), None, None, None],
        });
    }
    if matches!(swaps, SwapsArg::FourMeme | SwapsArg::All) {
        let managers: Vec<Address> = VENUE_DEPLOYMENTS
            .iter()
            .filter(|d| {
                d.chain_id == chain_id
                    && matches!(d.venue, SwapVenue::FourMemeV1 | SwapVenue::FourMemeV2)
                    && d.role == AnchorRole::SwapEmitter
            })
            .map(|d| d.anchor)
            .collect();
        if managers.is_empty() {
            // `all` skips chains without a launchpad; asking for it is an error.
            if swaps == SwapsArg::FourMeme {
                return Err(EvmSourceError::NotFound {
                    what: format!("a pinned four.meme TokenManager for chain {chain_id}"),
                });
            }
        } else {
            out.push(LogFilter {
                addresses: managers,
                topics: [
                    Some(vec![
                        FOURMEME_V1_PURCHASE_TOPIC0,
                        FOURMEME_V1_SALE_TOPIC0,
                        FOURMEME_V2_PURCHASE_TOPIC0,
                        FOURMEME_V2_SALE_TOPIC0,
                    ]),
                    None,
                    None,
                    None,
                ],
            });
        }
    }
    Ok(out)
}

/// Distinct v2/v3 swap emitters of `txs` (sorted).
fn pool_emitters(txs: &[RawEvmTransaction]) -> Vec<(Address, PoolKind)> {
    let mut out = std::collections::BTreeSet::new();
    for l in txs.iter().flat_map(|t| &t.logs) {
        match l.topics.first() {
            Some(t) if *t == V3_SWAP_TOPIC0 => out.insert((l.address, PoolKind::V3)),
            Some(t) if *t == PANCAKE_V3_SWAP_TOPIC0 => out.insert((l.address, PoolKind::PancakeV3)),
            Some(t) if *t == V2_SWAP_EVENT_SIGNATURE => out.insert((l.address, PoolKind::V2)),
            Some(t) if *t == AERODROME_V2_SWAP_TOPIC0 => {
                out.insert((l.address, PoolKind::AerodromeV2))
            }
            _ => false,
        };
    }
    out.into_iter().collect()
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
            ..ScanLimits::default()
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
        swap: None,
        pool_metadata: Vec::new(),
        pool_metadata_block: None,
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
        Target::Swaps(swaps) => {
            let filters = swap_filters(*swaps, profile.chain_id)?;
            let out = scanner
                .scan_swap_logs(&filters, from, to, args.max_txs)
                .await?;
            scanned.transactions = out.transactions;
            scanned.log_requests = out.log_requests;
            scanned.log_splits = out.log_splits;
            let emitters = pool_emitters(&scanned.transactions);
            let skipped = emitters.len().saturating_sub(args.max_pools);
            let head = evm.block_number().await?;
            let tag = format!("{head:#x}");
            let concurrency = usize::try_from(args.concurrency).unwrap_or(8).max(1);
            // Topics are shared between pool families (Slipstream emits the
            // Uniswap v3 topic): the pinned factory of each emitter picks the
            // identity reads (`stable()` / `tickSpacing()`) and the factory
            // `getPool` signature.
            let gate = SwapVenueGate::new(profile.chain_id);
            let mut rows = stream::iter(emitters.into_iter().take(args.max_pools))
                .map(|(emitter, kind)| {
                    let tag = tag.clone();
                    let gate = &gate;
                    async move {
                        let venue = pool_venue(kind);
                        evm.pool_metadata_resolving(emitter, kind, &tag, &|f| {
                            gate.factory_venue(venue, f).and_then(pool_kind)
                        })
                        .await
                    }
                })
                .buffered(concurrency);
            while let Some(row) = rows.next().await {
                match row {
                    Ok(m) => scanned.pool_metadata.push(m),
                    Err(e) if e.is_budget_exhausted() || e.is_rate_limited() => {
                        completion = Completion::Incomplete(if e.is_rate_limited() {
                            "the provider kept rate limiting while reading pool metadata"
                                .to_string()
                        } else {
                            "request budget exhausted while reading pool metadata".to_string()
                        });
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            drop(rows);
            scanned.pool_metadata_block = Some(tag);
            scanned.swap = Some(SwapScanFacts {
                swap_logs: out.swap_logs,
                txs_before_cap: out.txs_before_cap,
                pools_skipped: skipped,
            });
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
            let source: scout_providers::WalletIndexer = BlockscoutEvmSource::new(cfg)?.into();
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
        (AERODROME_V2_SWAP_TOPIC0, "aerodrome_v2_swap"),
        (V3_SWAP_TOPIC0, "v3_swap"),
        (PANCAKE_V3_SWAP_TOPIC0, "pancake_v3_swap"),
        (FOURMEME_V1_PURCHASE_TOPIC0, "fourmeme_v1_purchase"),
        (FOURMEME_V1_SALE_TOPIC0, "fourmeme_v1_sale"),
        (FOURMEME_V2_PURCHASE_TOPIC0, "fourmeme_v2_purchase"),
        (FOURMEME_V2_SALE_TOPIC0, "fourmeme_v2_sale"),
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
    // Pools the emitters' own metadata admits (swap mode; empty otherwise).
    let mut gate = SwapVenueGate::new(profile.chain_id);
    let admission = admit_recorded(&mut gate, &s.pool_metadata);
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
    if let Some(f) = &s.swap {
        let _ = writeln!(
            out,
            "swap_scan: swap_logs={} txs_before_cap={} txs_kept={} get_logs_requests={} \
             range_splits={}",
            f.swap_logs,
            f.txs_before_cap,
            s.transactions.len(),
            s.log_requests,
            s.log_splits
        );
        let _ = writeln!(
            out,
            "pool_metadata: emitters_read={} skipped_over_max_pools={} at_block={} admitted={} \
             refused={}",
            s.pool_metadata.len(),
            f.pools_skipped,
            s.pool_metadata_block.as_deref().unwrap_or("-"),
            admission.admitted,
            admission.refused.len()
        );
        let mut swaps_by_emitter: BTreeMap<Address, u64> = BTreeMap::new();
        for l in s.transactions.iter().flat_map(|t| &t.logs) {
            if matches!(l.topics.first(), Some(t) if *t == V3_SWAP_TOPIC0
                || *t == PANCAKE_V3_SWAP_TOPIC0
                || *t == V2_SWAP_EVENT_SIGNATURE
                || *t == AERODROME_V2_SWAP_TOPIC0)
            {
                *swaps_by_emitter.entry(l.address).or_insert(0) += 1;
            }
        }
        let mut by_factory: BTreeMap<String, u64> = BTreeMap::new();
        for m in &s.pool_metadata {
            let show = |a: Option<Address>| a.map_or("none".to_string(), |a| format!("{a:#x}"));
            let verdict = match admission.refused.get(&m.emitter) {
                Some(why) => format!("refused({why})"),
                None => "admitted".to_string(),
            };
            let _ = writeln!(
                out,
                "  pool {:?} {:#x} swaps={} factory={} token0={} token1={} fee={} stable={} \
                 tick_spacing={} registered={} {}",
                m.kind,
                m.emitter,
                swaps_by_emitter.get(&m.emitter).copied().unwrap_or(0),
                show(m.factory),
                show(m.token0),
                show(m.token1),
                m.fee.map_or("n/a".to_string(), |f| f.to_string()),
                m.stable.map_or("n/a".to_string(), |v| v.to_string()),
                m.tick_spacing.map_or("n/a".to_string(), |v| v.to_string()),
                show(m.registered_pool),
                verdict
            );
            *by_factory.entry(show(m.factory)).or_insert(0) += 1;
        }
        for (factory, n) in &by_factory {
            let _ = writeln!(out, "  factory {factory} pools={n}");
        }
    }
    let _ = writeln!(out, "swap/venue topic counts by emitting address:");
    for ((label, addr), (n, gated)) in &counts {
        let _ = writeln!(out, "  {label} {addr:#x} count={n} gate={gated}");
    }
    if counts.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    // four.meme: how many gated launchpad events name the signer as account
    // (the rest are router/bot cases the extraction does not attribute).
    let (mut fm_total, mut fm_signer) = (0u64, 0u64);
    for tx in &s.transactions {
        for l in &tx.logs {
            if let GateOutcome::Verified(v) = gate.classify(l)
                && let Some(lp) = v.launchpad
            {
                fm_total += 1;
                fm_signer += u64::from(lp.account == tx.from);
            }
        }
    }
    if fm_total > 0 {
        let _ = writeln!(
            out,
            "fourmeme: gated_events={fm_total} account_is_tx_from={fm_signer} account_other={}",
            fm_total - fm_signer
        );
    }
    // What the ADR-020 rule would do with default config (no quote tokens).
    let cfg = EvmExtractionConfig::new(*profile, gate);
    let token_filter = match target {
        Target::Token(t) => Some(*t),
        Target::Wallet(_) | Target::Swaps(_) => None,
    };
    let (ex, sum) = extract_evm_trades(&s.transactions, &cfg, None, token_filter);
    let _ = writeln!(
        out,
        "extraction(no stable quote tokens, pinned venues + admitted pools): trades={} failed={} unknown_consideration={} duplicates={}",
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

fn pool_metadata_json(s: &Scanned) -> Value {
    let a = |x: Option<Address>| x.map_or(Value::Null, |a| json!(format!("{a:#x}")));
    Value::Array(
        s.pool_metadata
            .iter()
            .map(|m| {
                json!({
                    "emitter": format!("{:#x}", m.emitter),
                    "kind": m.kind.label(),
                    "block": s.pool_metadata_block,
                    "factory": a(m.factory),
                    "token0": a(m.token0),
                    "token1": a(m.token1),
                    "fee": m.fee,
                    "stable": m.stable,
                    "tick_spacing": m.tick_spacing,
                    "registered_pool": a(m.registered_pool),
                })
            })
            .collect(),
    )
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
        Target::Swaps(v) => ("swaps", v.label().to_string()),
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
        "pool_metadata": scanned.map(pool_metadata_json).unwrap_or(json!([])),
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
