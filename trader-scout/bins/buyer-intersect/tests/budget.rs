//! Offline wiremock CLI tests for `--max-requests` and terminal provider
//! errors. The binary is pointed at the mock through the test-only
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

const INPUT3: &str = "solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n\
                      solana:NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump\n\
                      solana:GGf4EX9qbzxuboefDTEvqHdysHqtZSQC7Sahprjpump\n";

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
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
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

/// Every token's history is three pages: "" -> t1 -> t2 -> end.
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

#[tokio::test]
async fn budget_smaller_than_needed_is_partial_exit_3() {
    let server = three_page_server().await;
    // Token A needs 3 requests, token B gets 1 and is then cut off.
    let out = run(Some(server.uri()), args(&["--max-requests", "4"])).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("request budget exhausted after 4 requests (limit 4)"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("requests_made=4"), "{}", out.stderr);
    let lines = jsonl(&out);
    let meta = &lines[0];
    assert_eq!(meta["kind"], "run_meta");
    assert_eq!(meta["requests_made"], 4);
    assert_eq!(meta["budget"]["max_requests"], 4);
    let summary = lines.last().unwrap();
    assert_eq!(summary["kind"], "run_summary");
    assert_eq!(summary["status"], "partial");
    assert_eq!(summary["tokens"][0]["status"], "ok");
    assert_eq!(summary["tokens"][1]["status"], "failed");
    assert_eq!(
        summary["tokens"][1]["error_kind"]["kind"],
        "budget_exhausted"
    );
    assert_eq!(summary["tokens"][1]["error_kind"]["limit"], 4);
    assert!(summary["tokens"][1]["stop_reason"].is_null());
    assert!(
        summary["tokens"][1]["error"]
            .as_str()
            .unwrap()
            .contains("request budget exhausted")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn budget_hit_mid_run_marks_remaining_tokens_not_scanned_and_stops_requests() {
    let server = three_page_server().await;
    // A: 3 requests, B: 1 request then the budget is spent, C: untouched.
    let out = run_input(Some(server.uri()), args(&["--max-requests", "4"]), INPUT3).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("request budget exhausted after 4 requests (limit 4)"),
        "{}",
        out.stderr
    );
    let lines = jsonl(&out);
    let summary = lines.last().unwrap();
    assert_eq!(summary["status"], "partial");
    let tokens = summary["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 3, "N never shrinks");
    assert_eq!(tokens[0]["status"], "ok");
    assert_eq!(tokens[1]["status"], "failed");
    assert_eq!(tokens[1]["error_kind"]["kind"], "budget_exhausted");
    assert_eq!(tokens[2]["status"], "not_scanned");
    assert_eq!(tokens[2]["stop_reason"]["kind"], "budget_exhausted");
    assert_eq!(tokens[2]["stop_reason"]["limit"], 4);
    assert!(tokens[2]["transactions_scanned"].is_null());
    assert!(tokens[2]["qualified_buyers"].is_null());
    assert!(tokens[2]["diagnostics"].is_null());
    assert!(tokens[2]["error"].is_null());
    assert_eq!(lines[0]["requests_made"], 4);
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn rate_limit_mid_run_is_exit_3_with_typed_statuses_and_no_extra_requests() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    // Token A's three pages succeed; the 4th request (token B) is a
    // terminal 429 (Retry-After above the cap).
    Mock::given(method("POST"))
        .respond_with(move |_req: &wiremock::Request| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => ResponseTemplate::new(200).set_body_json(fixture_page(Some("t1"))),
                1 => ResponseTemplate::new(200).set_body_json(fixture_page(Some("t2"))),
                2 => ResponseTemplate::new(200).set_body_json(fixture_page(None)),
                _ => ResponseTemplate::new(429).insert_header("Retry-After", "3600"),
            }
        })
        .mount(&server)
        .await;
    let out = run_input(Some(server.uri()), args(&[]), INPUT3).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("rate limited; server asked to retry after 3600s (cap 60s)"),
        "{}",
        out.stderr
    );
    let lines = jsonl(&out);
    let summary = lines.last().unwrap();
    assert_eq!(summary["status"], "partial");
    let tokens = summary["tokens"].as_array().unwrap();
    assert_eq!(tokens[0]["status"], "ok");
    assert_eq!(tokens[1]["status"], "failed");
    assert_eq!(tokens[1]["error_kind"]["kind"], "rate_limited");
    assert_eq!(tokens[1]["error_kind"]["retry_after_secs"], 3600);
    assert_eq!(tokens[2]["status"], "not_scanned");
    assert_eq!(tokens[2]["stop_reason"]["kind"], "rate_limited");
    assert_eq!(tokens[2]["stop_reason"]["retry_after_secs"], 3600);
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn unlimited_run_still_reports_requests_made() {
    let server = three_page_server().await;
    let out = run(Some(server.uri()), args(&[])).await;
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let lines = jsonl(&out);
    assert_eq!(lines[0]["requests_made"], 6);
    assert!(lines[0]["budget"]["max_requests"].is_null());
    assert!(
        out.stderr
            .contains("requests_made=6 max_requests=unlimited"),
        "{}",
        out.stderr
    );
}

#[tokio::test]
async fn budget_exhausted_before_any_data_is_exit_3_without_records() {
    let server = three_page_server().await;
    // One request is made (page 1 of token A) and its data is seen, so
    // use a mock that never yields data to hit the no-report path.
    drop(server);
    let empty = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"jsonrpc": "2.0", "id": 1, "result": {"data": [], "paginationToken": "t1"}}),
        ))
        .mount(&empty)
        .await;
    let out = run(Some(empty.uri()), args(&["--max-requests", "1"])).await;
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("request budget exhausted after 1 requests (limit 1)"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.is_empty(), "no records may look complete");
}

#[tokio::test]
async fn long_retry_after_is_exit_4_with_message_and_no_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;
    let out = run(Some(server.uri()), args(&[])).await;
    assert_eq!(out.code, 4, "stderr: {}", out.stderr);
    assert!(
        out.stderr
            .contains("rate limited; server asked to retry after 3600s (cap 60s)"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.is_empty());
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
    // Terminal at once (no retry sleeping) and the run stops: exactly
    // one request, the second token is never requested.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn max_requests_zero_is_usage_error_exit_2() {
    for bad in ["0", "-1", "abc"] {
        let out = run(None, args(&[&format!("--max-requests={bad}")])).await;
        assert_eq!(out.code, 2, "value {bad}: {}", out.stderr);
    }
}
