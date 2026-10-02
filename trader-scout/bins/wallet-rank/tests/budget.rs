//! Offline wiremock CLI tests for `--max-requests` and terminal provider
//! errors. The binary is pointed at the mock through the test-only
//! `SCOUT_WALLET_RANK_ENDPOINT` override (the API key is never sent to it).
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

const BIN: &str = env!("CARGO_BIN_EXE_wallet-rank");
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
            .env("SCOUT_WALLET_RANK_ENDPOINT", endpoint)
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

fn excluded(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|l| l["kind"] == "wallet_excluded")
        .collect()
}

#[tokio::test]
async fn budget_smaller_than_needed_excludes_unscanned_wallets_exit_3() {
    let server = three_page_server().await;
    let out = run(server.uri(), args(&["--max-requests", "4"])).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("request budget exhausted after 4 requests (limit 4)"),
        "{}",
        out.stderr
    );
    let lines = jsonl(&out);
    assert_eq!(lines[0]["scan"]["requests_made"], 4);
    assert_eq!(lines[0]["scan"]["max_requests"], 4);
    let ex = excluded(&lines);
    assert_eq!(ex.len(), 3, "every input wallet is ranked or excluded");
    assert_eq!(ex[0]["wallet"]["address"], W1);
    assert_eq!(ex[1]["wallet"]["address"], W2);
    assert_eq!(ex[1]["primary_reason"], "provider_error");
    assert_eq!(ex[2]["wallet"]["address"], W3);
    assert_eq!(ex[2]["primary_reason"], "not_scanned");
    assert_eq!(ex[2]["scan_status"], "not_scanned");
    assert!(ex[2]["observed"].is_null());
    let summary = lines.last().unwrap();
    assert_eq!(summary["status"], "partial");
    assert_eq!(summary["stop"]["kind"], "budget_exhausted");
    assert_eq!(summary["excluded_by_primary_reason"]["not_scanned"], 1);
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
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}
