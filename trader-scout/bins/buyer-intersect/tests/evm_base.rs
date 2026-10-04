//! Offline CLI test of `buyer-intersect` on BASE (ADR-020 amendment 4): the
//! keyed RPC caps `eth_getLogs` at 10 blocks (Alchemy free tier), so a
//! window of 48 Base blocks is 5 requests per token; no public-RPC
//! auto-routing exists for Base. Replay over the committed live Base fixture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::EvmFixtureReplay;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const BIN: &str = env!("CARGO_BIN_EXE_buyer-intersect");
const SECRET: &str = "BASEKEYEDSECRET31337";
const TOKEN_A: &str = "0x07b3d902783c3c12b077508c3b5c00113d1291d0";
const TOKEN_B: &str = "0xacfe6019ed1a7dc6f7b508c02d1b04ec88cc21bf";
/// The 8 receipt blocks (52,146,932..=939) +- 20 blocks: 48 blocks.
const WINDOW: [&str; 4] = [
    "--since",
    "2026-10-04T03:06:11Z",
    "--until",
    "2026-10-04T03:07:47Z",
];

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn rpc() -> MockServer {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/evm_base_swaps_all_2026-10-04.json"
    );
    let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let replay = EvmFixtureReplay::from_fixture(&f)
        .with_linear_block_times(52_146_932, 1_791_083_211, 2)
        .with_receipt_logs(&f, Some(10))
        .with_synthetic_unrecorded_txs(&f)
        .with_handler(Box::new(|m, params| {
            // USDC decimals() = 6.
            (m == "eth_call" && params[0]["data"] == "0x313ce567").then(|| {
                scout_providers::evm_replay::ReplayReply::Result(json!(format!("0x{:064x}", 6)))
            })
        }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn run(rpc: &MockServer, extra: &[&str]) -> Out {
    run_in(rpc, WINDOW, extra).await
}

async fn run_in(rpc: &MockServer, window: [&'static str; 4], extra: &[&str]) -> Out {
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    let url = format!("{}/v2/{SECRET}", rpc.uri());
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(["--input", "-", "--rpc-rps", "5000", "--min-token-hits", "1"])
            .args(window)
            .args(extra)
            .env_remove("SCOUT_ROBINHOOD_RPC_URL")
            .env_remove("SCOUT_BASE_LOGS_RPC_URL")
            .env_remove("SCOUT_EVM_PUBLIC_RPC_URL")
            .env("SCOUT_BASE_RPC_URL", url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("base:{TOKEN_A}\nbase:{TOKEN_B}\n").as_bytes());
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

async fn get_logs(rpc: &MockServer) -> usize {
    rpc.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("\"eth_getLogs\""))
        .count()
}

#[tokio::test]
async fn base_token_scan_follows_the_10_block_cap_without_public_routing() {
    let s = rpc().await;
    let o = run(&s, &[]).await;
    // Exit 3 = partial: 7 Uniswap v4 trades of token A are IdlOnly on Base
    // (never qualify, reported as a lower bound).
    assert_eq!(o.code, 3, "{}", o.stderr);
    assert!(
        o.stderr.contains("IdlOnly venue deployment"),
        "{}",
        o.stderr
    );
    assert!(!o.stdout.contains(SECRET) && !o.stderr.contains(SECRET));
    // Engine replay numbers: 70 wallets bought or sold a token (K = 1).
    assert_eq!(o.stdout.lines().count(), 70, "{}", o.stdout);
    assert!(o.stderr.contains("caps eth_getLogs at 10"), "{}", o.stderr);
    assert!(o.stderr.contains("capped at 10 block(s)"), "{}", o.stderr);
    assert!(
        !o.stderr.contains("public Base") && !o.stderr.contains("keyless public"),
        "no public-RPC auto-routing for Base: {}",
        o.stderr
    );
    // 2 tokens x 5 spans (48 blocks) + the probe + <= 1 rejected attempt each.
    let n = get_logs(&s).await;
    assert!((11..=14).contains(&n), "eth_getLogs requests: {n}");
    assert!(
        o.stderr.contains("USDC="),
        "quote asset in scope: {}",
        o.stderr
    );
}

#[tokio::test]
async fn base_feasibility_estimate_refuses_before_scanning() {
    let s = rpc().await;
    // 2 h of Base = ~3,800 blocks: 2 tokens x 380 requests at a 10-block cap
    // is far over --max-requests 100 (which covers the window resolution).
    let o = run_in(
        &s,
        [
            "--since",
            "2026-10-04T01:00:00Z",
            "--until",
            "2026-10-04T03:07:47Z",
        ],
        &["--max-requests", "100"],
    )
    .await;
    assert_eq!(o.code, 4, "{}", o.stderr);
    assert!(
        o.stderr.contains("caps the block range at 10"),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("SCOUT_BASE_LOGS_RPC_URL"), "{}", o.stderr);
    assert!(!o.stdout.contains(SECRET) && !o.stderr.contains(SECRET));
    // Only the capability probe reached eth_getLogs: nothing was scanned.
    assert_eq!(get_logs(&s).await, 1);
    assert!(
        o.stderr.contains("760 requests") || o.stderr.contains("requests, over the limit of 100"),
        "{}",
        o.stderr
    );
}
