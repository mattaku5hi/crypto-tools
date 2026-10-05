//! Offline CLI tests of MULTI-CHAIN `wallet-stats` (docs/CLI.md §5): one run
//! over Solana + Base + Robinhood inputs, each chain served by its own mock
//! (Helius page, keyed Alchemy-style Base RPC, Robinhood RPC + explorer),
//! cards merged in the ORIGINAL input order, per-chain run_meta/run_summary,
//! per-chain budgets, one chain failing without stopping the others, and the
//! aggregated exit code. No network.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::{
    AlchemyInternalMode, EvmFixtureReplay, ExplorerReplay, ReplayReply,
};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");
const HELIUS_KEY: &str = "HELIUSSECRET555";
const RH_SECRET: &str = "RHALCHEMYSECRET111";
const BASE_SECRET: &str = "BASEALCHEMYSECRET222";
const BS_KEY: &str = "BLOCKSCOUTSECRET333";
const SOL: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";
const RH_W1: &str = "0x4e40ceacc9d16dad54f90daffd3a7291cacc0884";
const BASE_USDC: &str = "0xb0b21cef6df3cc3716193fd94880b58c2adb90b7";
const BASE_SELL: &str = "0x3484978c2680823516c6f409ff736180ccf62dfc";
const BASE_FIRST_BLOCK: i128 = 52_146_932;
const BASE_FIRST_TS: i128 = 1_791_083_211;
/// The Robinhood fixture's window; Solana's page is rewritten into it. The
/// Base fixture lies hours later, so its wallets legitimately have no
/// activity here (a chain with its own, different outcome).
const WINDOW: [&str; 4] = [
    "--since",
    "2026-10-03T17:54:46Z",
    "--until",
    "2026-10-03T18:04:46Z",
];
const WINDOW_SINCE: i64 = 1_791_050_086;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fixtures_dir() -> String {
    format!(
        "{}/../../docs/p0/measurements/fixtures",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn json_file(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{}/{name}", fixtures_dir())).unwrap())
        .unwrap()
}

async fn helius_mock() -> MockServer {
    let v = json_file("pumpswap_wallet_page_2026-10-02.json");
    let mut page = v["pages"][0].clone();
    for tx in page["data"].as_array_mut().unwrap() {
        tx["blockTime"] = json!(WINDOW_SINCE + 30);
    }
    page["paginationToken"] = Value::Null;
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(
            {"jsonrpc": "2.0", "id": 1, "result": page}
        )))
        .mount(&s)
        .await;
    s
}

async fn robinhood_rpc() -> MockServer {
    let f = json_file("evm_robinhood_token_aiden_v4_2026-10-03.json");
    let replay = EvmFixtureReplay::from_fixture(&f).with_handler(Box::new(|m, _| {
        (m == "eth_call").then(|| ReplayReply::Result(json!(format!("0x{:064x}", 6))))
    }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn robinhood_explorer() -> MockServer {
    let f = json_file("evm_robinhood_token_aiden_v4_2026-10-03.json");
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ExplorerReplay::from_fixture(&f))
        .mount(&s)
        .await;
    s
}

async fn base_rpc() -> MockServer {
    let f = json_file("evm_base_swaps_all_2026-10-04.json");
    let replay = EvmFixtureReplay::from_fixture(&f)
        .with_alchemy_transfers(&f, AlchemyInternalMode::Unsupported)
        .with_handler(Box::new(|m, params| {
            if m == "eth_call" && params[0]["data"] == "0x313ce567" {
                return Some(ReplayReply::Result(json!(format!("0x{:064x}", 6))));
            }
            if m == "eth_getBlockByNumber" {
                let n = i128::from_str_radix(params[0].as_str()?.strip_prefix("0x")?, 16).ok()?;
                if n == 0 {
                    return None;
                }
                let ts = BASE_FIRST_TS + 2 * (n - BASE_FIRST_BLOCK);
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

struct Mocks {
    helius: MockServer,
    rh_rpc: MockServer,
    rh_explorer: MockServer,
    base_rpc: MockServer,
    coinbase: MockServer,
}

async fn mocks() -> Mocks {
    Mocks {
        helius: helius_mock().await,
        rh_rpc: robinhood_rpc().await,
        rh_explorer: robinhood_explorer().await,
        base_rpc: base_rpc().await,
        coinbase: coinbase_mock().await,
    }
}

fn env_of(m: &Mocks) -> Vec<(&'static str, String)> {
    vec![
        ("SCOUT_HELIUS_API_KEY", HELIUS_KEY.to_string()),
        ("SCOUT_WALLET_STATS_ENDPOINT", m.helius.uri()),
        (
            "SCOUT_ROBINHOOD_RPC_URL",
            format!("{}/v2/{RH_SECRET}", m.rh_rpc.uri()),
        ),
        ("SCOUT_BLOCKSCOUT_API_KEY", BS_KEY.to_string()),
        ("SCOUT_EVM_BLOCKSCOUT_URL", m.rh_explorer.uri()),
        (
            "SCOUT_BASE_RPC_URL",
            format!("{}/v2/{BASE_SECRET}", m.base_rpc.uri()),
        ),
        ("SCOUT_COINBASE_ENDPOINT", m.coinbase.uri()),
    ]
}

async fn run(env: Vec<(&'static str, String)>, extra: &[&str], input: String) -> Out {
    let mut args: Vec<String> = ["--input", "-", "--rpc-rps", "5000", "--no-valuation"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    args.extend(WINDOW.iter().map(|s| (*s).to_string()));
    args.extend(extra.iter().map(|s| (*s).to_string()));
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

/// Interleaved on purpose: base, solana, robinhood, base.
fn mixed_input() -> String {
    format!("base:{BASE_USDC}\nsolana:{SOL}\nrobinhood:{RH_W1}\nbase:{BASE_SELL}\n")
}

fn records(o: &Out) -> Vec<Value> {
    o.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn no_secret(o: &Out) {
    for s in [HELIUS_KEY, RH_SECRET, BASE_SECRET, BS_KEY] {
        assert!(
            !o.stdout.contains(s) && !o.stderr.contains(s),
            "secret leaked: {s}"
        );
    }
}

#[tokio::test]
async fn mixed_chains_keep_input_order_with_per_chain_meta_and_summary() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &["--format", "jsonl", "--no-usd"],
        mixed_input(),
    )
    .await;
    no_secret(&o);
    assert!(o.code == 0 || o.code == 3, "{} {}", o.code, o.stderr);
    let lines = records(&o);
    assert_eq!(lines.len(), 6, "meta + 4 cards + summary\n{}", o.stdout);
    let meta = &lines[0];
    assert_eq!(meta["kind"], "run_meta");
    assert_eq!(meta["multi_chain"], true);
    assert_eq!(meta["input_wallet_count"], 4);
    // Chains in first-appearance order, each with its own sub run_meta.
    let chains: Vec<&str> = meta["chains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["chain"].as_str().unwrap())
        .collect();
    assert_eq!(chains, ["base", "solana", "robinhood"]);
    let sub = |name: &str| -> &Value {
        meta["chains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["chain"] == name)
            .unwrap()
    };
    assert_eq!(sub("base")["run_meta"]["scope"]["chain_id"], 8453);
    assert_eq!(
        sub("base")["run_meta"]["scan"]["listing_kind"],
        "alchemy_transfers"
    );
    assert_eq!(sub("robinhood")["run_meta"]["scope"]["chain_id"], 4663);
    assert_eq!(sub("solana")["run_meta"]["scan"]["provider"], "helius");
    assert_eq!(sub("base")["input_wallet_count"], 2);
    // Cards: the ORIGINAL input order across chains, each keeps its chain.
    let want = [
        ("base", BASE_USDC),
        ("solana", SOL),
        ("robinhood", RH_W1),
        ("base", BASE_SELL),
    ];
    for (i, (chain, addr)) in want.iter().enumerate() {
        let c = &lines[1 + i];
        assert_eq!(c["kind"], "wallet_stats", "{c}");
        assert_eq!(c["wallet"]["chain"], *chain, "card {i}");
        assert_eq!(c["wallet"]["address"], *addr, "card {i}");
        assert_ne!(c["status"], "error", "{c}");
    }
    // Native spelling per chain: no cross-contamination.
    assert!(lines[1]["stats"].to_string().contains("\"wei\""));
    assert!(lines[2]["stats"].to_string().contains("lamports"));
    // Summary: per chain and overall, requests counted per chain.
    let s = lines.last().unwrap();
    assert_eq!(s["kind"], "run_summary");
    assert_eq!(s["multi_chain"], true);
    assert_eq!(s["records"], 4);
    assert_eq!(s["chains"].as_array().unwrap().len(), 3);
    let by = s["requests_made_by_chain"].as_object().unwrap();
    assert!(
        by.len() == 3 && by.values().all(|v| v.as_u64().unwrap() > 0),
        "{by:?}"
    );
    assert_eq!(s["exit_code"], o.code);
    let order: Vec<&str> = s["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["wallet"]["address"].as_str().unwrap())
        .collect();
    assert_eq!(order, [BASE_USDC, SOL, RH_W1, BASE_SELL]);
    // Each chain used its own source.
    assert!(!m.helius.received_requests().await.unwrap().is_empty());
    assert!(!m.base_rpc.received_requests().await.unwrap().is_empty());
    assert!(!m.rh_rpc.received_requests().await.unwrap().is_empty());
    assert!(!m.rh_explorer.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn chain_concurrency_one_gives_the_same_cards() {
    let m = mocks().await;
    let a = run(
        env_of(&m),
        &["--format", "jsonl", "--no-usd", "--chain-concurrency", "1"],
        mixed_input(),
    )
    .await;
    let b = run(
        env_of(&m),
        &["--format", "jsonl", "--no-usd", "--chain-concurrency", "3"],
        mixed_input(),
    )
    .await;
    assert_eq!(a.code, b.code);
    let addrs = |o: &Out| -> Vec<(String, String, String)> {
        records(o)
            .iter()
            .filter(|r| r["kind"] == "wallet_stats")
            .map(|r| {
                (
                    r["wallet"]["chain"].to_string(),
                    r["wallet"]["address"].to_string(),
                    r["status"].to_string(),
                )
            })
            .collect()
    };
    assert_eq!(addrs(&a), addrs(&b));
}

#[tokio::test]
async fn one_chain_without_its_key_gets_error_cards_and_the_others_still_run() {
    let m = mocks().await;
    let mut env = env_of(&m);
    env.retain(|(k, _)| *k != "SCOUT_HELIUS_API_KEY");
    let o = run(env, &["--format", "jsonl", "--no-usd"], mixed_input()).await;
    no_secret(&o);
    let lines = records(&o);
    let cards: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "wallet_stats")
        .collect();
    assert_eq!(cards.len(), 4);
    let sol = cards[1];
    assert_eq!(sol["wallet"]["chain"], "solana");
    assert_eq!(sol["status"], "error");
    assert!(
        sol["error"]
            .as_str()
            .unwrap()
            .contains("SCOUT_HELIUS_API_KEY"),
        "{sol}"
    );
    assert!(sol["stats"].is_null(), "unknown, never zero: {sol}");
    for i in [0, 2, 3] {
        assert_ne!(cards[i]["status"], "error", "{}", cards[i]);
    }
    // Partial universe, not "everything failed": exit 3, never 4.
    assert_eq!(o.code, 3, "{}", o.stderr);
    let s = lines.last().unwrap();
    assert_eq!(s["status"], "partial");
    assert_eq!(s["exit_code"], 3);
    let solana_sub = s["chains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["chain"] == "solana")
        .unwrap();
    assert_eq!(solana_sub["exit_code"], 4);
    assert!(
        solana_sub["incomplete_reasons"][0]
            .as_str()
            .unwrap()
            .contains("SCOUT_HELIUS_API_KEY")
    );
    assert!(o.stderr.contains("chain solana: exit=4"), "{}", o.stderr);
}

#[tokio::test]
async fn every_chain_failing_is_exit_4_with_all_cards_present() {
    let m = mocks().await;
    // No keys at all: Solana (Helius), Base and Robinhood (RPC) all fail.
    let o = run(
        vec![("SCOUT_COINBASE_ENDPOINT", m.coinbase.uri())],
        &["--format", "jsonl", "--no-usd"],
        mixed_input(),
    )
    .await;
    assert_eq!(o.code, 4, "{}", o.stderr);
    let lines = records(&o);
    let cards: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "wallet_stats")
        .collect();
    assert_eq!(cards.len(), 4);
    assert!(cards.iter().all(|c| c["status"] == "error"));
    assert_eq!(lines.last().unwrap()["exit_code"], 4);
}

#[tokio::test]
async fn max_requests_is_per_chain_and_a_spent_budget_marks_only_that_chain() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &["--format", "jsonl", "--no-usd", "--max-requests", "1"],
        mixed_input(),
    )
    .await;
    no_secret(&o);
    assert_eq!(o.code, 3, "{}", o.stderr);
    let lines = records(&o);
    let s = lines.last().unwrap();
    let made = |name: &str| -> u64 {
        s["chains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["chain"] == name)
            .unwrap()["requests_made"]
            .as_u64()
            .unwrap()
    };
    // Solana has its OWN budget of 1 and used it fully; the EVM chains each
    // have their own and failed at their preflight with a typed reason.
    assert_eq!(made("solana"), 1);
    let cards: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "wallet_stats")
        .collect();
    assert_eq!(cards[1]["status"], "ok", "{}", cards[1]);
    for i in [0, 2, 3] {
        assert_eq!(cards[i]["status"], "error", "{}", cards[i]);
        assert!(
            cards[i]["error"]
                .as_str()
                .unwrap()
                .contains("request budget exhausted"),
            "{}",
            cards[i]
        );
        assert!(
            !cards[i]["error"]
                .as_str()
                .unwrap()
                .contains("wallet-stats:")
        );
    }
    assert!(
        o.stderr.matches("max_requests=1").count() >= 3,
        "{}",
        o.stderr
    );
}

#[tokio::test]
async fn table_output_groups_cards_by_chain() {
    let m = mocks().await;
    let o = run(env_of(&m), &["--no-usd"], mixed_input()).await;
    assert!(o.code == 0 || o.code == 3, "{} {}", o.code, o.stderr);
    assert!(o.stdout.contains("# chain: base"), "{}", o.stdout);
    assert!(o.stdout.contains("# chain: solana"));
    assert!(o.stdout.contains("# chain: robinhood"));
    assert!(o.stdout.contains("# multi-chain:"));
    for a in [BASE_USDC, SOL, RH_W1, BASE_SELL] {
        assert!(o.stdout.contains(a), "{a}");
    }
}

#[tokio::test]
async fn single_chain_input_is_unchanged_not_a_multi_chain_run() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &["--format", "jsonl", "--no-usd"],
        format!("robinhood:{RH_W1}\n"),
    )
    .await;
    let lines = records(&o);
    assert!(lines[0].get("multi_chain").is_none());
    assert_eq!(lines[0]["scope"]["chain"], "robinhood");
}
