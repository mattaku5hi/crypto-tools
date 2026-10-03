//! Offline wiremock CLI tests for the provider request options
//! (`--page-limit`, `--server-window`, `--token-accounts`) of wallet-stats. The
//! binary is pointed at the mock through the test-only endpoint override.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");
const KEY: &str = "SUPERSECRETKEY123";
const INPUT: &str = "solana:HgwBZM6kQE8qpYBdM2aXDxaEs5GTpDryNxuREeVP8f8B\n\
                     solana:5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1\n\
                     solana:11111111111111111111111111111111\n";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn run(endpoint: String, extra: Vec<String>) -> Out {
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(["--input", "-", "--format", "jsonl"])
            .args(extra)
            .env("SCOUT_HELIUS_API_KEY", KEY)
            .env("SCOUT_WALLET_STATS_ENDPOINT", endpoint)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        // The binary may exit before reading stdin; a broken pipe is
        // expected, the exit code is what the test checks.
        let _ = child.stdin.take().unwrap().write_all(INPUT.as_bytes());
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

fn fixture_page(next: Option<&str>) -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/pump_bonding_curve_buy_probe.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut result = v["result"].clone();
    result["paginationToken"] = next.map_or(Value::Null, |t| json!(t));
    json!({"jsonrpc": "2.0", "id": 1, "result": result})
}

fn args(extra: &[&str]) -> Vec<String> {
    extra.iter().map(|s| (*s).to_string()).collect()
}

async fn single_page_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture_page(None)))
        .mount(&server)
        .await;
    server
}

async fn first_request_options(server: &MockServer) -> Value {
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    body["params"][1].clone()
}

fn meta(out: &Out) -> Value {
    serde_json::from_str(out.stdout.lines().next().unwrap()).unwrap()
}

#[tokio::test]
async fn defaults_send_the_live_verified_request_and_echo_options() {
    let server = single_page_server().await;
    let out = run(server.uri(), vec![]).await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "desc", "limit": 500,
               "filters": {"tokenAccounts": "balanceChanged"}})
    );
    let po = &meta(&out)["scan"]["provider_options"];
    assert_eq!(po["page_limit"], 500);
    assert_eq!(po["token_accounts"], "BalanceChanged");
    assert_eq!(po["status"], "Any");
    assert_eq!(po["server_window"], true);
    assert!(
        out.stderr.contains("provider options: page_limit=500"),
        "{}",
        out.stderr
    );
}

#[tokio::test]
async fn defaults_with_a_window_add_block_time_to_the_filters() {
    let server = single_page_server().await;
    run(
        server.uri(),
        args(&[
            "--since",
            "2026-08-01T00:00:00Z",
            "--until",
            "2026-08-02T00:00:00Z",
        ]),
    )
    .await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "desc", "limit": 500,
               "filters": {
                   "blockTime": {"gte": 1_785_542_400i64, "lt": 1_785_628_800i64},
                   "tokenAccounts": "balanceChanged"}})
    );
}

#[tokio::test]
async fn explicit_opt_outs_reproduce_the_legacy_request() {
    let server = single_page_server().await;
    let out = run(
        server.uri(),
        args(&[
            "--page-limit",
            "100",
            "--server-window=false",
            "--token-accounts",
            "none",
        ]),
    )
    .await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "desc", "limit": 100})
    );
    let po = &meta(&out)["scan"]["provider_options"];
    assert_eq!(po["page_limit"], 100);
    assert_eq!(po["token_accounts"], "None");
    assert_eq!(po["status"], "Any");
    assert_eq!(po["server_window"], false);
}

#[tokio::test]
async fn options_reach_the_request_without_a_status_filter() {
    let server = single_page_server().await;
    let out = run(
        server.uri(),
        args(&[
            "--page-limit",
            "300",
            "--token-accounts",
            "balance-changed",
            "--server-window",
            "--since",
            "2026-08-01T00:00:00Z",
            "--until",
            "2026-08-02T00:00:00Z",
            "--max-pages-per-wallet",
            "4",
        ]),
    )
    .await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "desc", "limit": 300,
               "filters": {
                   "blockTime": {"gte": 1_785_542_400i64, "lt": 1_785_628_800i64},
                   "tokenAccounts": "balanceChanged"}})
    );
    let po = &meta(&out)["scan"]["provider_options"];
    assert_eq!(po["token_accounts"], "BalanceChanged");
    assert_eq!(po["server_window"], true);
    assert_eq!(po["tx_budget_per_wallet"], 1200);
    assert_eq!(po["status"], "Any");
}

#[tokio::test]
async fn server_window_without_a_window_and_flag_off_send_no_block_time() {
    let server = single_page_server().await;
    run(server.uri(), args(&["--server-window"])).await;
    assert_eq!(
        first_request_options(&server).await["filters"],
        json!({"tokenAccounts": "balanceChanged"})
    );

    let server = single_page_server().await;
    run(
        server.uri(),
        args(&[
            "--server-window=false",
            "--token-accounts",
            "none",
            "--since",
            "2026-08-01T00:00:00Z",
            "--until",
            "2026-08-02T00:00:00Z",
        ]),
    )
    .await;
    assert!(
        first_request_options(&server)
            .await
            .get("filters")
            .is_none()
    );
}

#[tokio::test]
async fn invalid_provider_option_values_exit_2() {
    for bad in [
        vec!["--page-limit", "0"],
        vec!["--page-limit", "1001"],
        vec!["--token-accounts", "all"],
        vec!["--provider-status-filter", "succeeded"],
    ] {
        let out = run("http://127.0.0.1:1".to_string(), args(&bad)).await;
        assert_eq!(out.code, 2, "{bad:?}: {}", out.stderr);
    }
}
