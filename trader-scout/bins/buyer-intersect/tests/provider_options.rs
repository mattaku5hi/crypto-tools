//! Offline wiremock CLI tests for the provider request options
//! (`--page-limit`, `--provider-status-filter`, `--server-window`). The
//! binary is pointed at the mock through the test-only
//! `SCOUT_BUYER_INTERSECT_ENDPOINT` override.
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

const BIN: &str = env!("CARGO_BIN_EXE_buyer-intersect");
const KEY: &str = "SUPERSECRETKEY123";
const INPUT: &str = "solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n\
                     solana:NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump\n";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn run(endpoint: Option<String>, args: Vec<String>) -> Out {
    run_input(endpoint, args, INPUT).await
}

async fn run_input(endpoint: Option<String>, args: Vec<String>, input: &'static str) -> Out {
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env("SCOUT_HELIUS_API_KEY", KEY)
            .env_remove("SCOUT_BUYER_INTERSECT_ENDPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(e) = endpoint {
            cmd.env("SCOUT_BUYER_INTERSECT_ENDPOINT", e);
        }
        let mut child = cmd.spawn().unwrap();
        // The binary may exit (usage error) before reading stdin; a broken
        // pipe here is expected, the exit code is what the test checks.
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
    ["--input", "-", "--format", "jsonl"]
        .iter()
        .chain(extra)
        .map(|s| (*s).to_string())
        .collect()
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
    let out = run(Some(server.uri()), args(&[])).await;
    let options = first_request_options(&server).await;
    assert_eq!(
        options,
        json!({"transactionDetails": "full", "sortOrder": "asc", "limit": 500,
               "filters": {"status": "succeeded"}})
    );
    let m = meta(&out);
    assert_eq!(m["provider_options"]["page_limit"], 500);
    assert_eq!(m["provider_options"]["status"], "Succeeded");
    assert_eq!(m["provider_options"]["server_window"], true);
    assert!(
        out.stderr.contains("provider options: page_limit=500"),
        "{}",
        out.stderr
    );
}

#[tokio::test]
async fn explicit_opt_outs_reproduce_the_legacy_request() {
    let server = single_page_server().await;
    let out = run(
        Some(server.uri()),
        args(&[
            "--page-limit",
            "100",
            "--provider-status-filter",
            "any",
            "--server-window=false",
        ]),
    )
    .await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "asc", "limit": 100})
    );
    let m = meta(&out);
    assert_eq!(m["provider_options"]["page_limit"], 100);
    assert_eq!(m["provider_options"]["status"], "Any");
    assert_eq!(m["provider_options"]["server_window"], false);
}

#[tokio::test]
async fn page_limit_and_status_filter_reach_the_request_and_run_meta() {
    let server = single_page_server().await;
    let out = run(
        Some(server.uri()),
        args(&[
            "--page-limit",
            "400",
            "--provider-status-filter",
            "succeeded",
            "--max-pages-per-token",
            "3",
        ]),
    )
    .await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "asc", "limit": 400,
               "filters": {"status": "succeeded"}})
    );
    let m = meta(&out);
    assert_eq!(m["provider_options"]["status"], "Succeeded");
    assert_eq!(m["provider_options"]["tx_budget_per_token"], 1200);
    assert!(out.stderr.contains("status=succeeded"), "{}", out.stderr);
}

#[tokio::test]
async fn server_window_sends_block_time_filter_only_with_a_window() {
    let server = single_page_server().await;
    let out = run(
        Some(server.uri()),
        args(&[
            "--server-window",
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
               "filters": {"status": "succeeded",
                           "blockTime": {"gte": 1_785_542_400i64, "lt": 1_785_628_800i64}}})
    );
    let m = meta(&out);
    assert_eq!(m["provider_options"]["block_time_gte"], 1_785_542_400i64);
    assert_eq!(m["provider_options"]["server_window"], true);

    // With --server-window=false the same window sends no blockTime filter.
    let server = single_page_server().await;
    run(
        Some(server.uri()),
        args(&[
            "--server-window=false",
            "--provider-status-filter",
            "any",
            "--page-limit",
            "100",
            "--since",
            "2026-08-01T00:00:00Z",
            "--until",
            "2026-08-02T00:00:00Z",
        ]),
    )
    .await;
    assert_eq!(
        first_request_options(&server).await,
        json!({"transactionDetails": "full", "sortOrder": "desc", "limit": 100})
    );

    // --server-window without any window: no blockTime (status filter only).
    let server = single_page_server().await;
    run(Some(server.uri()), args(&["--server-window"])).await;
    assert_eq!(
        first_request_options(&server).await["filters"],
        json!({"status": "succeeded"})
    );
}

#[tokio::test]
async fn invalid_provider_option_values_exit_2() {
    for bad in [
        vec!["--page-limit", "0"],
        vec!["--page-limit", "1001"],
        vec!["--provider-status-filter", "failed"],
    ] {
        let out = run(None, args(&bad)).await;
        assert_eq!(out.code, 2, "{bad:?}: {}", out.stderr);
    }
}
