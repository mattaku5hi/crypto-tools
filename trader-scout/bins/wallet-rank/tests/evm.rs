//! Offline CLI tests of the EVM path of `wallet-rank` (ADR-020 step 2):
//! the real binary, the committed live Robinhood fixture replayed through
//! wiremock, fake secrets that must never be printed.
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
use wiremock::{Mock, MockServer};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-rank");
const RPC_SECRET: &str = "ALCHEMYSECRETKEY987";
const BS_KEY: &str = "BLOCKSCOUTSECRET123";
const W1: &str = "0x4e40ceacc9d16dad54f90daffd3a7291cacc0884";
const W2: &str = "0x41deeacdcdfebc9f0f549b7cbb8269a6b21d805d";

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

async fn serve() -> (MockServer, MockServer) {
    let replay = EvmFixtureReplay::from_fixture(&fixture()).with_handler(Box::new(|m, _| {
        (m == "eth_call").then(|| ReplayReply::Result(json!(format!("0x{:064x}", 6))))
    }));
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&rpc)
        .await;
    let ex = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ExplorerReplay::from_fixture(&fixture()))
        .mount(&ex)
        .await;
    (rpc, ex)
}

async fn run(rpc: &MockServer, ex: &MockServer, extra: &[&str], input: String) -> Out {
    let (rpc_url, ex_url) = (rpc.uri(), ex.uri());
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args([
            "--input",
            "-",
            "--since",
            "2026-10-03T17:54:46Z",
            "--until",
            "2026-10-03T18:04:46Z",
        ])
        .args(extra)
        .env_remove("SCOUT_HELIUS_API_KEY")
        .env(
            "SCOUT_ROBINHOOD_RPC_URL",
            format!("{rpc_url}/v2/{RPC_SECRET}"),
        )
        .env("SCOUT_BLOCKSCOUT_API_KEY", BS_KEY)
        .env("SCOUT_EVM_BLOCKSCOUT_URL", ex_url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    format!("robinhood:{W1}\nrobinhood:{W2}\n")
}

#[tokio::test]
async fn every_wallet_lands_in_exclusions_with_reasons_and_no_secret_leaks() {
    let (rpc, ex) = serve().await;
    let o = run(
        &rpc,
        &ex,
        &["--format", "jsonl", "--profile", "none"],
        input(),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    for s in [RPC_SECRET, BS_KEY] {
        assert!(!o.stdout.contains(s) && !o.stderr.contains(s), "leaked {s}");
    }
    let lines: Vec<Value> = o
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let meta = &lines[0];
    assert_eq!(meta["scope"]["chain"], "robinhood");
    assert_eq!(meta["quote_unit"], "wei");
    assert_eq!(meta["rank_quote_unit"], "eth");
    assert!(
        meta["ledger_version"]
            .as_str()
            .unwrap()
            .starts_with("evm-wallet-ledger")
    );
    // Unknown native proceeds: no closed episode, so the ranking metric is
    // unknown and both wallets are EXCLUDED (never ranked on a zero).
    let excluded: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "wallet_excluded")
        .collect();
    assert_eq!(excluded.len(), 2);
    for e in &excluded {
        assert_eq!(e["primary_reason"], "metric_unknown");
        assert_eq!(e["wallet"]["chain"], "robinhood");
    }
    assert!(lines.iter().all(|l| l["kind"] != "wallet_rank"));
    let summary = lines.last().unwrap();
    assert_eq!(summary["kind"], "run_summary");
    assert_eq!(summary["status"], "complete");
}

#[tokio::test]
async fn quote_units_are_validated_per_chain_family() {
    let (rpc, ex) = serve().await;
    let o = run(&rpc, &ex, &["--quote", "usdc"], input()).await;
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("not a quote unit of an EVM chain"));
    let o = run(
        &rpc,
        &ex,
        &["--quote", "usdg", "--profile", "none"],
        input(),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("realized_net_pnl_usdg"));
    let mixed = format!("robinhood:{W1}\nsolana:2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY\n");
    let o = run(&rpc, &ex, &[], mixed).await;
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("mixed Solana and EVM"));
    let o = run(
        &rpc,
        &ex,
        &["--quote", "eth"],
        "solana:2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY\n".to_string(),
    )
    .await;
    assert_eq!(o.code, 2);
}
