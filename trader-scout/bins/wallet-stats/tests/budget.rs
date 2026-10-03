//! Offline wiremock CLI tests for `--max-requests` and terminal provider
//! errors. The binary is pointed at the mock through the test-only
//! `SCOUT_WALLET_STATS_ENDPOINT` override (the API key is never sent to it).
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
const W1: &str = "HgwBZM6kQE8qpYBdM2aXDxaEs5GTpDryNxuREeVP8f8B";
const W2: &str = "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1";
const W3: &str = "11111111111111111111111111111111";
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

/// Every wallet's history is three pages: "" -> t1 -> t2 -> end.
async fn three_page_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let next = match body["params"][1]["paginationToken"].as_str() {
                None => Some("t1"),
                Some("t1") => Some("t2"),
                _ => None,
            };
            ResponseTemplate::new(200).set_body_json(fixture_page(next))
        })
        .mount(&server)
        .await;
    server
}

fn jsonl(out: &Out) -> Vec<Value> {
    out.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn args(extra: &[&str]) -> Vec<String> {
    extra.iter().map(|s| (*s).to_string()).collect()
}

fn wallet_records(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|l| l["kind"] == "wallet_stats")
        .collect()
}

#[tokio::test]
async fn budget_smaller_than_needed_stops_with_not_scanned_wallets_exit_3() {
    let server = three_page_server().await;
    // W1 needs 3 requests, W2 gets 1 and is cut off, W3 is never requested.
    let out = run(server.uri(), args(&["--max-requests", "4"])).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("request budget exhausted after 4 requests (limit 4)"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("requests_made=4 max_requests=4"),
        "{}",
        out.stderr
    );
    let lines = jsonl(&out);
    assert_eq!(lines[0]["kind"], "run_meta");
    assert_eq!(lines[0]["requests_made"], 4);
    assert_eq!(lines[0]["budget"]["max_requests"], 4);
    let wallets = wallet_records(&lines);
    assert_eq!(wallets.len(), 3, "no wallet disappears");
    assert_eq!(wallets[0]["wallet"]["address"], W1);
    assert_ne!(wallets[0]["status"], "error");
    assert_ne!(wallets[0]["status"], "not_scanned");
    assert_eq!(wallets[1]["wallet"]["address"], W2);
    assert_eq!(wallets[1]["status"], "error");
    assert_eq!(wallets[1]["error_kind"]["kind"], "budget_exhausted");
    assert_eq!(wallets[1]["error_kind"]["limit"], 4);
    assert_eq!(wallets[2]["wallet"]["address"], W3);
    assert_eq!(wallets[2]["status"], "not_scanned");
    assert_eq!(wallets[2]["stop_reason"]["kind"], "budget_exhausted");
    assert_eq!(wallets[2]["stop_reason"]["limit"], 4);
    assert!(wallets[2]["stats"].is_null());
    assert!(wallets[2]["error"].is_null());
    let summary = lines.last().unwrap();
    assert_eq!(summary["kind"], "run_summary");
    assert_eq!(summary["status"], "partial");
    assert_eq!(summary["requests_made"], 4);
    assert_eq!(summary["stop"]["kind"], "budget_exhausted");
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn long_retry_after_on_first_wallet_is_exit_4_with_one_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;
    let out = run(server.uri(), args(&[])).await;
    assert_eq!(out.code, 4, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("rate limited; server asked to retry after 3600s (cap 60s)"),
        "{}",
        out.stderr
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    let lines = jsonl(&out);
    let wallets = wallet_records(&lines);
    assert_eq!(wallets.len(), 3);
    assert_eq!(wallets[0]["status"], "error");
    assert_eq!(wallets[0]["error_kind"]["kind"], "rate_limited");
    assert_eq!(wallets[0]["error_kind"]["retry_after_secs"], 3600);
    assert_eq!(wallets[1]["status"], "not_scanned");
    assert_eq!(wallets[2]["status"], "not_scanned");
    assert_eq!(wallets[2]["stop_reason"]["kind"], "rate_limited");
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn rate_limit_after_a_card_with_data_is_exit_3() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(move |_req: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(fixture_page(None))
            } else {
                ResponseTemplate::new(429).insert_header("Retry-After", "3600")
            }
        })
        .mount(&server)
        .await;
    let out = run(server.uri(), args(&[])).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn unlimited_run_still_reports_requests_made() {
    let server = three_page_server().await;
    let out = run(server.uri(), args(&[])).await;
    assert!(
        out.stderr
            .contains("requests_made=9 max_requests=unlimited"),
        "{}",
        out.stderr
    );
    let lines = jsonl(&out);
    assert_eq!(lines[0]["requests_made"], 9);
    assert!(lines[0]["budget"]["max_requests"].is_null());
}

#[tokio::test]
async fn max_requests_zero_is_usage_error_exit_2() {
    for bad in ["0", "-1", "abc"] {
        let out = run(
            "http://127.0.0.1:9".to_string(),
            args(&[&format!("--max-requests={bad}")]),
        )
        .await;
        assert_eq!(out.code, 2, "value {bad}: {}", out.stderr);
    }
}

fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// An endless history (every page has a continuation cursor) whose
/// transactions are all `age_days` old.
async fn aged_endless_server(age_days: i64) -> MockServer {
    let server = MockServer::start().await;
    let block_time = unix_now() - age_days * 86_400;
    Mock::given(method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let prev = body["params"][1]["paginationToken"].as_str().unwrap_or("p");
            let next = format!("{prev}x");
            let mut page = fixture_page(Some(&next));
            for tx in page["result"]["data"].as_array_mut().unwrap() {
                tx["blockTime"] = json!(block_time);
            }
            ResponseTemplate::new(200).set_body_json(page)
        })
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn period_window_stops_paging_at_the_boundary_page_and_is_echoed_in_run_meta() {
    // Every tx is 40 days old: the first page of each wallet already holds
    // a tx before `now - 30d`, so exactly one request per wallet is made
    // even though the provider would page forever.
    let server = aged_endless_server(40).await;
    let before = unix_now();
    let out = run(server.uri(), args(&["--period", "30d"])).await;
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    let lines = jsonl(&out);
    let w = &lines[0]["window"];
    assert_eq!(w["source"], "period");
    let (since, until, as_of) = (
        w["since_unix"].as_i64().unwrap(),
        w["until_unix"].as_i64().unwrap(),
        w["as_of_unix"].as_i64().unwrap(),
    );
    assert_eq!(until - since, 30 * 86_400);
    assert_eq!(until, as_of);
    assert!(as_of >= before && as_of <= unix_now());
    assert!(w["since"].as_str().unwrap().ends_with('Z'));
    assert!(w["until"].as_str().unwrap().ends_with('Z'));
    assert!(
        lines[0]["ledger_version"]
            .as_str()
            .unwrap()
            .starts_with("solana-wallet-ledger/10")
    );
}

#[tokio::test]
async fn explicit_window_is_echoed_and_budget_before_boundary_is_exit_3() {
    // Recent txs only: the boundary is never reached within 2 pages.
    let server = aged_endless_server(0).await;
    let out = run(
        server.uri(),
        args(&[
            "--since",
            "2026-08-01T00:00:00Z",
            "--until",
            "2026-09-01T00:00:00Z",
            "--max-pages-per-wallet",
            "2",
        ]),
    )
    .await;
    let lines = jsonl(&out);
    let w = &lines[0]["window"];
    assert_eq!(w["source"], "explicit");
    assert_eq!(w["since"], "2026-08-01T00:00:00Z");
    assert_eq!(w["until"], "2026-09-01T00:00:00Z");
    assert_eq!(w["since_unix"], 1_785_542_400_i64);
    assert_eq!(w["until_unix"], 1_788_220_800_i64);
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains("before the window start"),
        "{}",
        out.stderr
    );
}
