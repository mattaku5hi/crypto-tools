//! Offline CLI test of `wallet-stats` on BASE (ADR-020 amendment 4): the real
//! binary against the committed live Base fixture replayed by wiremock. The
//! keyed RPC answers `alchemy_getAssetTransfers` (the wallet-history indexer:
//! Blockscout is not configured), `eth_call decimals()` of USDC, and a
//! Coinbase ETH-USD mock. The RPC URL carries a fake key in its path: it may
//! not appear anywhere in stdout/stderr.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::{AlchemyInternalMode, EvmFixtureReplay, ReplayReply};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");
const RPC_SECRET: &str = "BASEALCHEMYSECRET4242";
/// USDC-quoted buy then sell (Uniswap v3) and a signed native sell.
const W_USDC: &str = "0xb0b21cef6df3cc3716193fd94880b58c2adb90b7";
const W_SELL: &str = "0x3484978c2680823516c6f409ff736180ccf62dfc";
const FIRST_BLOCK: i128 = 52_146_932;
const FIRST_TS: i128 = 1_791_083_211;
const WINDOW: [&str; 4] = [
    "--since",
    "2026-10-04T03:06:51Z",
    "--until",
    "2026-10-04T03:07:07Z",
];

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fixture() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/evm_base_swaps_all_2026-10-04.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

async fn rpc_mock(alchemy: bool) -> MockServer {
    let f = fixture();
    let mut replay = EvmFixtureReplay::from_fixture(&f);
    if alchemy {
        replay = replay.with_alchemy_transfers(&f, AlchemyInternalMode::Unsupported);
    }
    let replay = replay.with_handler(Box::new(|m, params| {
        if m == "eth_call" && params[0]["data"] == "0x313ce567" {
            return Some(ReplayReply::Result(json!(format!("0x{:064x}", 6))));
        }
        if m == "eth_getBlockByNumber" {
            let n = i128::from_str_radix(params[0].as_str()?.strip_prefix("0x")?, 16).ok()?;
            if n == 0 {
                return None;
            }
            let ts = FIRST_TS + 2 * (n - FIRST_BLOCK);
            return Some(ReplayReply::Result(
                json!({"number": format!("{n:#x}"), "timestamp": format!("{ts:#x}")}),
            ));
        }
        None
    }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn coinbase_mock() -> MockServer {
    let s = MockServer::start().await;
    let candles: Vec<Value> = (0..40)
        .map(|i| json!([1_791_083_000 + i * 60, 2999.0, 3001.0, 3000.0, 3000.0, 10.0]))
        .collect();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(candles))
        .mount(&s)
        .await;
    s
}

async fn run(env: Vec<(&'static str, String)>, args: Vec<String>, input: String) -> Out {
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_remove("SCOUT_ROBINHOOD_RPC_URL")
            .env_remove("SCOUT_BASE_RPC_URL")
            .env_remove("SCOUT_BSC_RPC_URL")
            .env_remove("SCOUT_BLOCKSCOUT_API_KEY")
            .env_remove("SCOUT_HELIUS_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        let o = child.wait_with_output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    })
    .await
    .unwrap()
}

fn args(extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = ["--input", "-", "--rpc-rps", "5000"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    a.extend(WINDOW.iter().map(|s| (*s).to_string()));
    a.extend(extra.iter().map(|s| (*s).to_string()));
    a
}

fn env_of(rpc: &MockServer, cb: &MockServer) -> Vec<(&'static str, String)> {
    vec![
        (
            "SCOUT_BASE_RPC_URL",
            format!("{}/v2/{RPC_SECRET}", rpc.uri()),
        ),
        ("SCOUT_COINBASE_ENDPOINT", cb.uri()),
    ]
}

#[tokio::test]
async fn base_wallets_through_alchemy_transfers_with_usdc_and_usd() {
    let rpc = rpc_mock(true).await;
    let cb = coinbase_mock().await;
    let o = run(
        env_of(&rpc, &cb),
        args(&["--format", "jsonl", "--detail", "full"]),
        format!("base:{W_USDC}\nbase:{W_SELL}\n"),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(!o.stdout.contains(RPC_SECRET) && !o.stderr.contains(RPC_SECRET));
    let lines: Vec<Value> = o
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        lines.len(),
        4,
        "run_meta + 2 cards + run_summary\n{}",
        o.stdout
    );
    let meta = &lines[0];
    assert_eq!(meta["scope"]["chain"], "base");
    assert_eq!(meta["scope"]["chain_id"], 8453);
    let venues = meta["scope"]["venues"].as_array().unwrap();
    for v in ["uniswap_v3", "aerodrome_slipstream"] {
        assert!(
            venues
                .iter()
                .any(|x| x["venue"] == v && x["verification"] == "FixtureVerified"),
            "{v}"
        );
    }
    assert_eq!(meta["scope"]["quote_assets"][0]["symbol"], "USDC");
    assert_eq!(
        meta["scope"]["quote_assets"][0]["address"],
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
    );
    assert_eq!(
        meta["scope"]["quote_assets"][0]["decimals_check"],
        "verified live: 6"
    );
    assert_eq!(meta["scan"]["listing_kind"], "alchemy_transfers");
    assert!(
        meta["scan"]["coverage_notes"][0]
            .as_str()
            .unwrap()
            .contains("failed-transaction")
    );
    assert!(
        o.stderr.contains("listing_kind=alchemy_transfers"),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("coverage note:"), "{}", o.stderr);
    assert_eq!(meta["pricing"]["products"], json!(["ETH-USD"]));
    // Card 1: USDC-quoted, exact, USD via par.
    let c1 = &lines[1];
    assert_eq!(c1["wallet"], json!({"chain": "base", "address": W_USDC}));
    assert_eq!(c1["status"], "ok");
    let st = &c1["stats"];
    assert_eq!(st["trades"]["buys"], 1);
    assert_eq!(st["trades"]["sells"], 1);
    assert_eq!(st["quote_units"][0]["unit"], "eth");
    assert_eq!(st["quote_units"][1]["unit"], "usdc");
    assert_eq!(st["route"]["route_swaps_by_quote"]["usdc"], 2);
    assert!(
        o.stdout.contains("usdc_par_assumed"),
        "USDC legs are valued at par with the existing label"
    );
    // Card 2: native sell, proceeds Unknown (no trace/archive/internal on this node).
    let c2 = &lines[2];
    assert_eq!(c2["wallet"]["address"], W_SELL);
    assert_eq!(c2["stats"]["diagnostics"]["unknown_disposals"], 1);
    assert_eq!(lines[3]["status"], "complete");
    // No Solana spelling.
    assert!(!o.stdout.contains("lamports"));
    // The indexer requests went to the keyed RPC only.
    let methods: Vec<String> = rpc
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter_map(|b| b["method"].as_str().map(str::to_string))
        .collect();
    assert!(methods.iter().any(|m| m == "alchemy_getAssetTransfers"));
    assert!(!methods.iter().any(|m| m == "eth_getLogs"), "{methods:?}");
}

#[tokio::test]
async fn base_without_an_alchemy_style_rpc_or_blockscout_key_is_exit_4() {
    let rpc = rpc_mock(false).await; // no alchemy_getAssetTransfers
    let cb = coinbase_mock().await;
    let o = run(
        env_of(&rpc, &cb),
        args(&["--format", "jsonl"]),
        format!("base:{W_USDC}\n"),
    )
    .await;
    assert_eq!(o.code, 4, "{}", o.stderr);
    assert!(
        o.stderr.contains("SCOUT_BLOCKSCOUT_API_KEY"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr
            .contains("does not answer alchemy_getAssetTransfers"),
        "{}",
        o.stderr
    );
    assert!(!o.stdout.contains(RPC_SECRET) && !o.stderr.contains(RPC_SECRET));
}
