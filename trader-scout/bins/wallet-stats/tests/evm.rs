//! Offline CLI tests of the EVM path of `wallet-stats` (ADR-020 step 2):
//! the real binary against the committed live Robinhood fixture replayed by
//! wiremock (RPC + explorer) and a Coinbase ETH-USD mock. The RPC URL carries
//! a fake key in its path and the explorer key is a fake: neither may appear
//! anywhere in stdout/stderr.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::{EvmFixtureReplay, ExplorerReplay, ReplayReply};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");
const RPC_SECRET: &str = "ALCHEMYSECRETKEY987";
const BS_KEY: &str = "BLOCKSCOUTSECRET123";
const W1: &str = "0x4e40ceacc9d16dad54f90daffd3a7291cacc0884";
const W2: &str = "0x41deeacdcdfebc9f0f549b7cbb8269a6b21d805d";
const WINDOW: [&str; 4] = [
    "--since",
    "2026-10-03T17:54:46Z",
    "--until",
    "2026-10-03T18:04:46Z",
];

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fixture() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/evm_robinhood_token_aiden_v4_2026-10-03.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

async fn rpc_mock() -> MockServer {
    let replay = EvmFixtureReplay::from_fixture(&fixture()).with_handler(Box::new(|m, _| {
        // USDG decimals() = 6.
        (m == "eth_call").then(|| ReplayReply::Result(json!(format!("0x{:064x}", 6))))
    }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn explorer_mock() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ExplorerReplay::from_fixture(&fixture()))
        .mount(&s)
        .await;
    s
}

async fn coinbase_mock() -> MockServer {
    let s = MockServer::start().await;
    let candles: Vec<Value> = (0..40)
        .map(|i| json!([1_791_049_800 + i * 60, 2999.0, 3001.0, 3000.0, 3000.0, 10.0]))
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

struct Mocks {
    rpc: MockServer,
    explorer: MockServer,
    coinbase: MockServer,
}

async fn mocks() -> Mocks {
    Mocks {
        rpc: rpc_mock().await,
        explorer: explorer_mock().await,
        coinbase: coinbase_mock().await,
    }
}

fn env_of(m: &Mocks) -> Vec<(&'static str, String)> {
    vec![
        (
            "SCOUT_ROBINHOOD_RPC_URL",
            format!("{}/v2/{RPC_SECRET}", m.rpc.uri()),
        ),
        ("SCOUT_BLOCKSCOUT_API_KEY", BS_KEY.to_string()),
        ("SCOUT_EVM_BLOCKSCOUT_URL", m.explorer.uri()),
        ("SCOUT_COINBASE_ENDPOINT", m.coinbase.uri()),
    ]
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

fn input() -> String {
    format!("robinhood:{W1}\nrobinhood:{W2}\n")
}

fn no_secret(o: &Out) {
    for s in [RPC_SECRET, BS_KEY] {
        assert!(
            !o.stdout.contains(s) && !o.stderr.contains(s),
            "secret leaked: {s}"
        );
    }
}

fn keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, c) in m {
                out.push(k.clone());
                keys(c, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|c| keys(c, out)),
        _ => {}
    }
}

#[tokio::test]
async fn jsonl_cards_for_two_fixture_wallets_with_scope_and_no_secrets() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        args(&["--format", "jsonl", "--detail", "full"]),
        input(),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    no_secret(&o);
    let lines: Vec<Value> = o
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4, "run_meta + 2 cards + run_summary");
    let meta = &lines[0];
    assert_eq!(meta["kind"], "run_meta");
    assert_eq!(meta["scope"]["chain"], "robinhood");
    assert_eq!(meta["scope"]["chain_id"], 4663);
    let venues = meta["scope"]["venues"].as_array().unwrap();
    assert!(
        venues
            .iter()
            .any(|v| v["venue"] == "uniswap_v4" && v["verification"] == "FixtureVerified")
    );
    assert!(
        venues
            .iter()
            .any(|v| v["venue"] == "uniswap_v3" && v["verification"] == "FixtureVerified")
    );
    assert_eq!(meta["scope"]["quote_assets"][0]["symbol"], "USDG");
    assert_eq!(
        meta["scope"]["quote_assets"][0]["decimals_check"],
        "verified live: 6"
    );
    let nl = &meta["scope"]["native_leg"];
    assert!(nl["trace"].as_str().unwrap().starts_with("unsupported"));
    assert!(
        nl["archive_state"]
            .as_str()
            .unwrap()
            .starts_with("unsupported")
    );
    assert_eq!(nl["trades_by_source"]["logs_and_value_only"], 9);
    assert_eq!(meta["quote_unit"], "wei");
    assert_eq!(meta["pricing"]["products"], json!(["ETH-USD"]));
    assert!(
        meta["ledger_version"]
            .as_str()
            .unwrap()
            .starts_with("evm-wallet-ledger")
    );

    let c1 = &lines[1];
    assert_eq!(c1["kind"], "wallet_stats");
    assert_eq!(c1["wallet"], json!({"chain": "robinhood", "address": W1}));
    assert_eq!(c1["status"], "ok");
    let st = &c1["stats"];
    assert_eq!(
        (
            st["trades"]["buys"].as_u64(),
            st["trades"]["sells"].as_u64()
        ),
        (Some(2), Some(3))
    );
    assert_eq!(st["quote_unit"], "wei");
    assert_eq!(st["quote_units"][0]["unit"], "eth");
    assert_eq!(st["quote_units"][0]["decimals"], 18);
    assert_eq!(st["quote_units"][1]["unit"], "usdg");
    // Unknown native proceeds: nothing closed, no PnL claimed.
    assert_eq!(st["closed_episodes_known"], 0);
    assert_eq!(st["open_episodes"], 1);
    assert_eq!(st["diagnostics"]["unknown_disposals"], 3);
    assert_eq!(
        c1["open_positions"][0]["mint"],
        "0x15e853bc1c69529bd0a16bab1a742645a3607e1f"
    );
    assert_eq!(
        c1["open_positions"][0]["open_amount_raw"],
        "46472032518329289554142"
    );
    let c2 = &lines[2];
    assert_eq!(c2["wallet"]["address"], W2);
    assert_eq!(c2["stats"]["trades"]["buys"], 1);
    assert_eq!(lines[3]["kind"], "run_summary");
    assert_eq!(lines[3]["status"], "complete");

    // Nothing in the EVM records is named after lamports/SOL.
    let mut ks = Vec::new();
    for l in &lines {
        keys(l, &mut ks);
    }
    for k in &ks {
        assert!(
            k != "lamports" && k != "sol" && !k.split('_').any(|s| s == "lamports" || s == "sol"),
            "{k}"
        );
    }
    assert!(ks.iter().any(|k| k == "wei") && ks.iter().any(|k| k == "eth"));
}

#[tokio::test]
async fn table_output_uses_eth_columns_and_18_decimal_amounts() {
    let m = mocks().await;
    let o = run(env_of(&m), args(&["--detail", "full"]), input()).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    no_secret(&o);
    let header = o.stdout.lines().find(|l| l.starts_with("wallet")).unwrap();
    assert!(header.contains("realized_net_pnl_eth"), "{header}");
    assert!(header.contains("realized_pnl_usdg") && !header.contains("realized_pnl_usdt"));
    assert!(!header.contains("_sol"));
    assert!(o.stdout.contains(W1) && o.stdout.contains(W2));
    assert!(
        o.stdout
            .contains("mint=0x15e853bc1c69529bd0a16bab1a742645a3607e1f")
    );
    // Unknown, never zero: the headline is N/A with a reason.
    assert!(o.stdout.contains("N/A"), "{}", o.stdout);
    assert!(o.stderr.contains("protocol scope (robinhood chain id 4663"));
    assert!(o.stderr.contains("uniswap_v4=FixtureVerified"));
    assert!(o.stderr.contains("native leg sources: trace unsupported"));
}

#[tokio::test]
async fn refusals_have_the_documented_exit_codes() {
    let m = mocks().await;
    // Mixed Solana + EVM is a multi-chain run (CLI.md §5): Robinhood runs, the
    // Solana chain has no Helius key -> its card is `error`, exit 3 (partial).
    let mixed = format!("robinhood:{W1}\nsolana:2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY\n");
    let o = run(env_of(&m), args(&["--format", "jsonl"]), mixed).await;
    assert_eq!(o.code, 3, "{}", o.stderr);
    assert!(o.stdout.contains("SCOUT_HELIUS_API_KEY"), "{}", o.stdout);
    assert!(!m.rpc.received_requests().await.unwrap().is_empty());
    // Two EVM chains: Base has no RPC variable -> its card is `error`, exit 3.
    let two = format!("robinhood:{W1}\nbase:{W2}\n");
    let o = run(env_of(&m), args(&["--format", "jsonl"]), two).await;
    assert_eq!(o.code, 3, "{}", o.stderr);
    assert!(o.stdout.contains("SCOUT_BASE_RPC_URL"), "{}", o.stdout);
    // Missing explorer key: exit 4, names the variable.
    let mut env = env_of(&m);
    env.retain(|(k, _)| *k != "SCOUT_BLOCKSCOUT_API_KEY");
    let o = run(env, args(&[]), input()).await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("SCOUT_BLOCKSCOUT_API_KEY"));
    // BSC is enabled (ADR-020 amendment 5): only its RPC variable is missing;
    // the flag is accepted and changes nothing.
    let o = run(env_of(&m), args(&[]), format!("bsc:{W1}\n")).await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("SCOUT_BSC_RPC_URL"), "{}", o.stderr);
    assert!(!o.stderr.contains("not verified yet"), "{}", o.stderr);
    let o = run(
        env_of(&m),
        args(&["--allow-unverified-chain"]),
        format!("bsc:{W1}\n"),
    )
    .await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("SCOUT_BSC_RPC_URL"), "{}", o.stderr);
    // Base is enabled: without its RPC variable only that is missing.
    let o = run(env_of(&m), args(&[]), format!("base:{W1}\n")).await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("SCOUT_BASE_RPC_URL"), "{}", o.stderr);
    no_secret(&o);
}

#[tokio::test]
async fn chain_identity_mismatch_is_refused_before_any_scan() {
    let m = mocks().await;
    // The endpoint answers like Base (chain id 0x2105): exit 4.
    let replay = EvmFixtureReplay::from_fixture(&json!({"calls": []})).with_handler(Box::new(
        |method, _| (method == "eth_chainId").then(|| ReplayReply::Result(json!("0x2105"))),
    ));
    let wrong = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&wrong)
        .await;
    let mut env = env_of(&m);
    env[0] = (
        "SCOUT_ROBINHOOD_RPC_URL",
        format!("{}/v2/{RPC_SECRET}", wrong.uri()),
    );
    let o = run(env, args(&[]), input()).await;
    assert_eq!(o.code, 4, "{}", o.stderr);
    assert!(o.stderr.contains("preflight failed"));
    no_secret(&o);
    assert_eq!(m.explorer.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn wallet_scan_lists_through_the_indexer_and_never_calls_get_logs() {
    let m = mocks().await;
    let o = run(env_of(&m), args(&["--format", "jsonl"]), input()).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    no_secret(&o);
    let methods: Vec<String> = m
        .rpc
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter_map(|b| b["method"].as_str().map(str::to_string))
        .collect();
    assert!(o.stderr.contains("planned RPC requests:"), "{}", o.stderr);
    assert!(o.stderr.contains("rpc calls by method:"), "{}", o.stderr);
    assert!(
        !methods.iter().any(|x| x == "eth_getTransactionByHash"),
        "explorer rows describe the signed txs: {methods:?}"
    );
    assert!(
        !methods.iter().any(|x| x == "eth_getLogs"),
        "wallet scans never use window getLogs: {methods:?}"
    );
    // the explorer was asked for the window's listings, block-range restricted
    let q: Vec<String> = m
        .explorer
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.query().unwrap_or("").to_string())
        .collect();
    for action in ["txlist", "tokentx", "txlistinternal"] {
        assert!(
            q.iter().any(|u| u.contains(&format!("action={action}"))
                && u.contains("startblock=")
                && u.contains("endblock=")),
            "{action}: {q:?}"
        );
    }
    assert!(
        o.stderr.contains("routing: eth_getLogs -> not used"),
        "{}",
        o.stderr
    );
    let meta: Value = serde_json::from_str(o.stdout.lines().next().unwrap()).unwrap();
    assert!(
        meta["scan"]["logs_source"]
            .as_str()
            .unwrap()
            .starts_with("not used")
    );
    assert!(
        meta["scan"]["state_source"]
            .as_str()
            .unwrap()
            .starts_with("keyed rpc")
    );
}
