//! Offline CLI tests (wiremock). The binary is pointed at the mock via
//! the test-only `SCOUT_CAPTURE_ENDPOINT` override.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Command;

use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_scout-capture");
const KEY: &str = "SUPERSECRETKEY123";
// 32 bytes of 0x01 in base58 is not needed; use the pump program id.
const ADDRESS: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn run(endpoint: Option<String>, key: Option<&str>, args: Vec<String>) -> Out {
    let key = key.map(str::to_string);
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(args).env_remove("SCOUT_HELIUS_API_KEY");
        if let Some(k) = key {
            cmd.env("SCOUT_HELIUS_API_KEY", k);
        }
        if let Some(e) = endpoint {
            cmd.env("SCOUT_CAPTURE_ENDPOINT", e);
        }
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    })
    .await
    .unwrap()
}

fn fixture_result() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/pump_bonding_curve_buy_probe.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["result"].clone()
}

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("scout-capture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().into_owned()
}

fn args(extra: &[&str]) -> Vec<String> {
    let mut a = vec!["--address".to_string(), ADDRESS.to_string()];
    a.extend(extra.iter().map(|s| (*s).to_string()));
    a
}

fn simple_tx(sig: &str) -> Value {
    json!({"slot": 1, "transactionIndex": 0,
        "transaction": {"signatures": [sig], "message": {"accountKeys": [], "instructions": []}},
        "meta": {"err": null}})
}

#[tokio::test]
async fn missing_key_exits_4() {
    let out = run(None, None, args(&[])).await;
    assert_eq!(out.code, 4);
    assert!(out.stderr.contains("configuration required"));
}

#[tokio::test]
async fn invalid_address_exits_2() {
    let out = run(
        None,
        Some(KEY),
        vec!["--address".into(), "notbase58!!".into()],
    )
    .await;
    assert_eq!(out.code, 2);
}

#[tokio::test]
async fn two_pages_are_written_verbatim() {
    let server = MockServer::start().await;
    let page1 = json!({"data": [simple_tx("sigA")], "paginationToken": "tok1", "extra": {"x": 1}});
    let page2 = json!({"data": [simple_tx("sigB")]});
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"params": [ADDRESS, {
            "transactionDetails": "full", "sortOrder": "asc", "limit": 100,
            "paginationToken": "tok1"}]})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": page2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method": "getTransactionsForAddress",
            "params": [ADDRESS, {"transactionDetails": "full", "sortOrder": "asc", "limit": 100}]}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": page1})),
        )
        .mount(&server)
        .await;
    let path = tmp("two_pages.json");
    let out = run(
        Some(server.uri()),
        Some(KEY),
        args(&["--sort", "asc", "--max-pages", "3", "--out", &path]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["pages"], json!([page1, page2]));
    assert_eq!(doc["request"]["address"], ADDRESS);
    assert_eq!(doc["request"]["sort"], "asc");
    assert_eq!(doc["request"]["max_pages"], 3);
    assert_eq!(doc["request"]["method"], "getTransactionsForAddress");
    assert!(doc["captured_at_utc"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn keep_signatures_filters_per_page() {
    let server = MockServer::start().await;
    let page = json!({"data": [simple_tx("sigA"), simple_tx("sigB"), simple_tx("sigC")]});
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": page})),
        )
        .mount(&server)
        .await;
    let path = tmp("keep.json");
    let out = run(
        Some(server.uri()),
        Some(KEY),
        args(&["--out", &path, "--keep-signatures", "sigA,sigC"]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let sigs: Vec<&str> = doc["pages"][0]["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["transaction"]["signatures"][0].as_str().unwrap())
        .collect();
    assert_eq!(sigs, ["sigA", "sigC"]);
}

#[tokio::test]
async fn summary_classifies_fixture_variants() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": fixture_result()})),
        )
        .mount(&server)
        .await;
    let out = run(Some(server.uri()), Some(KEY), args(&[])).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let agg: Vec<&str> = out
        .stdout
        .lines()
        .skip_while(|l| !l.starts_with("# aggregate"))
        .skip(1)
        .take_while(|l| !l.is_empty())
        .collect();
    assert_eq!(
        agg,
        [
            "2\tbuy\ttrue\t25\t18",
            "1\tbuy_exact_quote_in_v2\tfalse\t24\t27",
            "2\tnon_trade:<anchor-event-cpi>\ttrue\t382\t1",
            "2\tnon_trade:<anchor-event-cpi>\ttrue\t383\t1",
            "1\tsell\ttrue\t24\t16",
            "1\tsell_v2\ttrue\t24\t26",
        ]
    );
    // One line per pump instruction; failed v2 tx is top-level.
    assert_eq!(
        out.stdout.lines().filter(|l| l.starts_with("sig=")).count(),
        9
    );
    assert!(
        out.stdout.contains(
            "tx_ok=false where=top variant=buy_exact_quote_in_v2 data_len=24 accounts=27"
        )
    );
    // Failed-only variant has no sample signatures; buy has two.
    assert!(
        out.stdout
            .contains("buy_exact_quote_in_v2: (none successful)")
    );
    let buy = out.stdout.lines().find(|l| l.starts_with("buy: ")).unwrap();
    assert_eq!(buy.trim_start_matches("buy: ").split(',').count(), 2);
}

#[tokio::test]
async fn error_echoing_url_and_key_is_redacted() {
    let server = MockServer::start().await;
    let msg = format!("bad request to https://mainnet.helius-rpc.com/?api-key={KEY} key {KEY}");
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": msg}}),
        ))
        .mount(&server)
        .await;
    let path = tmp("never_written.json");
    let out = run(Some(server.uri()), Some(KEY), args(&["--out", &path])).await;
    assert_eq!(out.code, 4);
    assert!(!out.stderr.contains(KEY), "{}", out.stderr);
    assert!(!out.stdout.contains(KEY));
    assert!(out.stderr.contains("<redacted>"), "{}", out.stderr);
    assert!(!std::path::Path::new(&path).exists());
}

#[tokio::test]
async fn written_file_never_contains_key() {
    let server = MockServer::start().await;
    let page = json!({"data": [simple_tx("sigA")]});
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": page})),
        )
        .mount(&server)
        .await;
    let path = tmp("nokey.json");
    let out = run(Some(server.uri()), Some(KEY), args(&["--out", &path])).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!std::fs::read_to_string(&path).unwrap().contains(KEY));
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

fn paged_ok(next: Option<&str>) -> Value {
    json!({"jsonrpc": "2.0", "id": 1,
        "result": {"data": [simple_tx("sigA")], "paginationToken": next}})
}

#[tokio::test]
async fn budget_exhausted_mid_capture_is_exit_3_and_marked_incomplete() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged_ok(Some("more"))))
        .mount(&server)
        .await;
    let path = tmp("budget.json");
    let out = run(
        Some(server.uri()),
        Some(KEY),
        args(&["--max-pages", "5", "--max-requests", "2", "--out", &path]),
    )
    .await;
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("request budget exhausted after 2 requests (limit 2)"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("requests_made=2"), "{}", out.stderr);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["pages"].as_array().unwrap().len(), 2);
    assert_eq!(doc["requests_made"], 2);
    assert!(
        doc["incomplete"]
            .as_str()
            .unwrap()
            .contains("budget exhausted")
    );
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn unlimited_capture_reports_requests_made_and_not_incomplete() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged_ok(None)))
        .mount(&server)
        .await;
    let path = tmp("complete.json");
    let out = run(Some(server.uri()), Some(KEY), args(&["--out", &path])).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("requests_made=1 max_requests=unlimited"),
        "{}",
        out.stderr
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(doc["incomplete"].is_null());
    assert_eq!(doc["requests_made"], 1);
}

#[tokio::test]
async fn long_retry_after_is_exit_4_with_message_and_no_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;
    let out = run(Some(server.uri()), Some(KEY), args(&[])).await;
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("rate limited; server asked to retry after 3600s (cap 60s)"),
        "{}",
        out.stderr
    );
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn max_requests_zero_is_usage_error_exit_2() {
    let out = run(None, Some(KEY), args(&["--max-requests", "0"])).await;
    assert_eq!(out.code, 2, "{}", out.stderr);
}
