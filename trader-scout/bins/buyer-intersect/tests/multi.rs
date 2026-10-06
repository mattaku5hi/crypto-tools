//! Offline CLI tests of MULTI-CHAIN `buyer-intersect` (docs/CLI.md §3):
//! tokens are chain-scoped, the run is partitioned by chain (each with its own
//! source and budget), matches are concatenated with their chain and never
//! merged across chains. Solana (Helius mock, times rewritten into the Base
//! window), Base (capped keyed RPC replay) and a Robinhood token whose window
//! its replay cannot resolve (a chain that fails while the others finish).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::{EvmFixtureReplay, ReplayReply};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_buyer-intersect");
const HELIUS_KEY: &str = "HELIUSKEY777";
const BASE_SECRET: &str = "BASEKEYEDSECRET31337";
const RH_SECRET: &str = "RHKEYEDSECRET4242";
const BUY_MINT: &str = "AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
const SELL_MINT: &str = "67266Ha2icrdCKHyrYKyG4oJyJ7RqheaGbuGd6vwXbLD";
const TOKEN_A: &str = "0x07b3d902783c3c12b077508c3b5c00113d1291d0";
const TOKEN_B: &str = "0xacfe6019ed1a7dc6f7b508c02d1b04ec88cc21bf";
const AIDEN: &str = "0x15e853bc1c69529bd0a16bab1a742645a3607e1f";
const WINDOW: [&str; 4] = [
    "--since",
    "2026-10-04T03:06:11Z",
    "--until",
    "2026-10-04T03:07:47Z",
];
/// Inside the window above.
const IN_WINDOW: i64 = 1_791_083_181;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fixtures() -> String {
    format!(
        "{}/../../docs/p0/measurements/fixtures",
        env!("CARGO_MANIFEST_DIR")
    )
}

async fn helius() -> MockServer {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/pump_bonding_curve_buy_probe.json", fixtures()))
            .unwrap(),
    )
    .unwrap();
    let mut result = v["result"].clone();
    for tx in result["data"].as_array_mut().unwrap() {
        tx["blockTime"] = json!(IN_WINDOW);
    }
    result["paginationToken"] = Value::Null;
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": result})),
        )
        .mount(&s)
        .await;
    s
}

async fn base_rpc() -> MockServer {
    let f: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/evm_base_swaps_all_2026-10-04.json", fixtures()))
            .unwrap(),
    )
    .unwrap();
    let replay = EvmFixtureReplay::from_fixture(&f)
        .with_linear_block_times(52_146_932, 1_791_083_211, 2)
        .with_receipt_logs(&f, Some(10))
        .with_synthetic_unrecorded_txs(&f)
        .with_handler(Box::new(|m, params| {
            (m == "eth_call" && params[0]["data"] == "0x313ce567")
                .then(|| ReplayReply::Result(json!(format!("0x{:064x}", 6))))
        }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn robinhood_rpc() -> MockServer {
    let p = format!(
        "{}/evm_robinhood_token_aiden_v4_2026-10-03.json",
        fixtures()
    );
    let replay = EvmFixtureReplay::from_path(std::path::Path::new(&p))
        .unwrap()
        .with_handler(Box::new(|m, _| {
            (m == "eth_call").then(|| ReplayReply::Result(json!(format!("0x{:064x}", 6))))
        }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

struct Mocks {
    helius: MockServer,
    base: MockServer,
    rh: MockServer,
}

async fn mocks() -> Mocks {
    Mocks {
        helius: helius().await,
        base: base_rpc().await,
        rh: robinhood_rpc().await,
    }
}

fn env_of(m: &Mocks) -> Vec<(&'static str, String)> {
    vec![
        ("SCOUT_HELIUS_API_KEY", HELIUS_KEY.to_string()),
        ("SCOUT_BUYER_INTERSECT_ENDPOINT", m.helius.uri()),
        (
            "SCOUT_BASE_RPC_URL",
            format!("{}/v2/{BASE_SECRET}", m.base.uri()),
        ),
        (
            "SCOUT_ROBINHOOD_RPC_URL",
            format!("{}/v2/{RH_SECRET}", m.rh.uri()),
        ),
    ]
}

async fn run(env: Vec<(&'static str, String)>, extra: &[&str], input: String) -> Out {
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        // Base replay fixture: the capped-eth_getLogs path (`auto` would list
        // transfers via alchemy_getAssetTransfers, not recorded).
        cmd.args([
            "--input",
            "-",
            "--rpc-rps",
            "5000",
            "--evm-token-listing",
            "logs",
        ])
        .args(WINDOW)
        .args(&extra)
        .env_remove("SCOUT_ROBINHOOD_RPC_URL")
        .env_remove("SCOUT_BASE_RPC_URL")
        .env_remove("SCOUT_BASE_LOGS_RPC_URL")
        .env_remove("SCOUT_EVM_PUBLIC_RPC_URL")
        .env_remove("SCOUT_HELIUS_API_KEY")
        .env_remove("SCOUT_BUYER_INTERSECT_ENDPOINT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        if !extra.iter().any(|a| a == "--format") {
            cmd.args(["--format", "jsonl"]);
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

fn input() -> String {
    format!(
        "solana:{BUY_MINT}\nbase:{TOKEN_A}\nsolana:{SELL_MINT}\nbase:{TOKEN_B}\nrobinhood:{AIDEN}\n"
    )
}

fn records(o: &Out) -> Vec<Value> {
    o.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn no_secret(o: &Out) {
    for s in [HELIUS_KEY, BASE_SECRET, RH_SECRET] {
        assert!(
            !o.stdout.contains(s) && !o.stderr.contains(s),
            "secret leaked: {s}"
        );
    }
}

#[tokio::test]
async fn matches_are_per_chain_with_per_chain_meta_and_aggregated_exit() {
    let m = mocks().await;
    let o = run(env_of(&m), &["--min-token-hits", "1"], input()).await;
    no_secret(&o);
    let r = records(&o);
    let meta = &r[0];
    assert_eq!(meta["kind"], "run_meta");
    assert_eq!(meta["multi_chain"], true);
    let chains: Vec<&str> = meta["chains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["chain"].as_str().unwrap())
        .collect();
    assert_eq!(chains, ["solana", "base", "robinhood"]);
    assert_eq!(meta["chains"][0]["input_token_count"], 2);
    assert_eq!(meta["chains"][1]["run_meta"]["kind"], "run_meta");
    assert_eq!(meta["chains"][2]["status"], "failed");
    // Matches carry their chain; Solana and Base both produced some.
    let matches: Vec<&Value> = r.iter().filter(|v| v["kind"] == "buyer_match").collect();
    let by = |c: &str| matches.iter().filter(|v| v["wallet"]["chain"] == c).count();
    assert!(by("solana") >= 1, "{}", o.stdout);
    assert_eq!(by("base"), 74, "Base replay numbers (K = 1)");
    assert_eq!(by("robinhood"), 0);
    assert_eq!(by("solana") + by("base"), matches.len());
    // K counts within a chain: each chain has 2 tokens, so no hit_count exceeds 2.
    for v in &matches {
        assert!(v["hit_count"].as_u64().unwrap() <= 2, "{v}");
    }
    // Robinhood's window cannot be resolved: that chain failed, the other
    // chains finished -> partial (3), not infrastructure (4).
    assert_eq!(o.code, 3, "{}", o.stderr);
    let s = r.last().unwrap();
    assert_eq!(s["kind"], "run_summary");
    assert_eq!(s["multi_chain"], true);
    assert_eq!(s["exit_code"], 3);
    assert_eq!(s["matches"], matches.len());
    let by_chain = s["requests_made_by_chain"].as_object().unwrap();
    assert!(by_chain["solana"].as_u64().unwrap() > 0 && by_chain["base"].as_u64().unwrap() > 0);
    assert!(!m.helius.received_requests().await.unwrap().is_empty());
    assert!(!m.base.received_requests().await.unwrap().is_empty());
    assert!(
        s["incomplete_reasons"]
            .to_string()
            .contains("chain robinhood")
    );
}

#[tokio::test]
async fn a_chain_with_fewer_tokens_than_k_is_skipped_not_scanned() {
    let m = mocks().await;
    // K = 2 (default): Solana has 2 tokens, Base 1 -> Base cannot match.
    let o = run(
        env_of(&m),
        &[],
        format!("solana:{BUY_MINT}\nsolana:{SELL_MINT}\nbase:{TOKEN_A}\n"),
    )
    .await;
    no_secret(&o);
    assert!(o.code == 0 || o.code == 3, "{} {}", o.code, o.stderr);
    assert!(o.stderr.contains("chain base: skipped"), "{}", o.stderr);
    assert!(m.base.received_requests().await.unwrap().is_empty());
    let r = records(&o);
    assert_eq!(r[0]["chains"][1]["status"], "skipped");
}

#[tokio::test]
async fn no_chain_with_k_tokens_is_a_usage_error() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &[],
        format!("solana:{BUY_MINT}\nbase:{TOKEN_A}\n"),
    )
    .await;
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stdout.is_empty());
    assert!(m.helius.received_requests().await.unwrap().is_empty());
    assert!(m.base.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_chain_without_its_key_fails_alone_the_others_still_report() {
    let m = mocks().await;
    let mut env = env_of(&m);
    env.retain(|(k, _)| *k != "SCOUT_HELIUS_API_KEY");
    let o = run(
        env,
        &["--min-token-hits", "1"],
        format!("solana:{BUY_MINT}\nbase:{TOKEN_A}\nsolana:{SELL_MINT}\nbase:{TOKEN_B}\n"),
    )
    .await;
    no_secret(&o);
    assert_eq!(o.code, 3, "{}", o.stderr);
    let r = records(&o);
    let s = r.last().unwrap();
    let sol = s["chains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["chain"] == "solana")
        .unwrap();
    assert_eq!(sol["exit_code"], 4);
    assert!(
        sol["incomplete_reasons"][0]
            .as_str()
            .unwrap()
            .contains("SCOUT_HELIUS_API_KEY")
    );
    let matches = r.iter().filter(|v| v["kind"] == "buyer_match").count();
    assert_eq!(matches, 74);
}

#[tokio::test]
async fn every_chain_failing_is_exit_4() {
    let m = mocks().await;
    let o = run(
        vec![],
        &["--min-token-hits", "1"],
        format!("solana:{BUY_MINT}\nbase:{TOKEN_A}\n"),
    )
    .await;
    assert_eq!(o.code, 4, "{}", o.stderr);
    let r = records(&o);
    assert_eq!(r.last().unwrap()["exit_code"], 4);
    drop(m);
}

#[tokio::test]
async fn table_output_lists_matches_under_a_chain_header() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &["--min-token-hits", "1", "--format", "table"],
        input(),
    )
    .await;
    assert!(o.stdout.contains("# chain: solana"), "{}", o.stdout);
    assert!(o.stdout.contains("# chain: base"));
    assert!(o.stdout.contains("# multi-chain exit code"));
}
