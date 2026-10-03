//! Offline wiremock CLI tests for the ADR-018 USD view of `wallet-stats`:
//! the binary talks to a Helius mock (the committed PumpSwap wallet page,
//! block times rewritten into the recorded Coinbase candle window) and to a
//! Coinbase mock serving the orchestrator-recorded SOL-USD candles
//! (2026-10-03 recording of 2026-10-02 12:00-12:05 UTC) through the
//! `SCOUT_COINBASE_ENDPOINT` override. No network.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");
const KEY: &str = "SUPERSECRETKEY123";
const WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";
/// 2026-10-02T12:00:00Z, the first recorded candle (close 121.89).
const T0: i64 = 1_790_942_400;
const PAGE_SECONDS: i64 = 300 * 60;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn candles_body() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/coinbase_sol_usd_candles_1m_2026-10-02T1200Z_recorded.json"
    ))
    .unwrap()
}

/// The committed PumpSwap wallet page with every `blockTime` rewritten:
/// transaction `i` gets `T0 + 30 + (i % spread) * PAGE_SECONDS`.
fn helius_page(spread: i64) -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/pumpswap_wallet_page_2026-10-02.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut page = v["pages"][0].clone();
    for (i, tx) in page["data"].as_array_mut().unwrap().iter_mut().enumerate() {
        let k = i64::try_from(i).unwrap().rem_euclid(spread);
        tx["blockTime"] = json!(T0 + 30 + k * PAGE_SECONDS);
    }
    page["paginationToken"] = Value::Null;
    json!({"jsonrpc": "2.0", "id": 1, "result": page})
}

async fn helius(spread: i64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(helius_page(spread)))
        .mount(&server)
        .await;
    server
}

async fn coinbase_ok() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products/SOL-USD/candles"))
        .respond_with(ResponseTemplate::new(200).set_body_string(candles_body()))
        .mount(&server)
        .await;
    server
}

async fn run(helius: &MockServer, coinbase: &MockServer, extra: &[&str]) -> Out {
    let (h, c) = (helius.uri(), coinbase.uri());
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(["--input", "-", "--no-valuation"])
            .args(extra)
            .env("SCOUT_HELIUS_API_KEY", KEY)
            .env("SCOUT_WALLET_STATS_ENDPOINT", h)
            .env("SCOUT_COINBASE_ENDPOINT", c)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("solana:{WALLET}\n").as_bytes());
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

fn jsonl(out: &Out) -> Vec<Value> {
    out.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn wallet_record(lines: &[Value]) -> &Value {
    lines.iter().find(|l| l["kind"] == "wallet_stats").unwrap()
}

#[tokio::test]
async fn usd_block_is_priced_from_the_recorded_candles_and_counted_separately() {
    let h = helius(1).await;
    let c = coinbase_ok().await;
    let out = run(&h, &c, &["--format", "jsonl", "--detail", "full"]).await;
    assert!(
        out.code == 0 || out.code == 3,
        "{} {}",
        out.code,
        out.stderr
    );
    let lines = jsonl(&out);
    let meta = &lines[0];
    let coinbase_requests = c.received_requests().await.unwrap().len();
    assert_eq!(coinbase_requests, 1, "all legs sit in one 300-minute page");
    let p = &meta["pricing"];
    assert_eq!(p["enabled"], true);
    assert_eq!(p["source"], "coinbase-exchange-candles-1m");
    assert_eq!(p["products"], json!(["SOL-USD", "USDT-USD"]));
    assert_eq!(p["staleness_limit_minutes"], 5);
    assert_eq!(p["granularity_seconds"], 60);
    assert_eq!(p["price_field"], "close");
    assert_eq!(p["endpoint_overridden"], true);
    assert!(
        p["usdc_assumption"]
            .as_str()
            .unwrap()
            .starts_with("usdc_par_assumed")
    );
    assert!(
        p["policy_version"]
            .as_str()
            .unwrap()
            .starts_with("usd-execution-pricing/1")
    );
    assert_eq!(p["requests_made_prices"], 1);
    assert_eq!(meta["requests_made_prices"], 1);
    assert_eq!(p["prefetch"]["pages_fetched"], 1);
    assert_eq!(p["wallets_priced"], 1);
    // The scan request counter does not include the price request.
    assert_eq!(meta["requests_made"], 1);
    assert!(meta["budget"]["max_price_requests"].is_null());
    let w = wallet_record(&lines);
    let usd = &w["stats"]["usd"];
    assert!(usd.is_object(), "{usd}");
    let cov = &usd["price_coverage"];
    let legs = cov["legs"].as_u64().unwrap();
    assert!(legs > 0, "{cov}");
    assert_eq!(
        cov["priced"].as_u64().unwrap() + cov["unpriced"].as_u64().unwrap(),
        legs
    );
    assert_eq!(cov["unpriced"], 0, "{cov}");
    assert_eq!(cov["by_label"]["cex_reference_1m"].as_u64().unwrap(), legs);
    assert_eq!(cov["usdc_par_legs"], 0);
    // Known USD figures are exact 8-dp strings, same sign as the SOL ones
    // (one positive price for every leg).
    if let (Some(u), Some(s)) = (
        usd["realized_trade_pnl"]["decimal"].as_str(),
        w["stats"]["realized_trade_pnl"]["lamports"].as_str(),
    ) {
        let frac = u.rsplit('.').next().unwrap();
        assert_eq!(frac.len(), 8, "{u}");
        let neg = |x: &str| x.starts_with('-');
        if s != "0" && u != "0.00000000" {
            assert_eq!(neg(u), neg(s), "usd {u} vs lamports {s}");
        }
    }
    // Native figures are present and unchanged in shape.
    assert!(
        w["stats"]["ledger_version"]
            .as_str()
            .unwrap()
            .contains("/12")
    );
    // Per-episode USD view in full detail.
    let eps = w["episodes"].as_array().unwrap();
    assert!(eps.iter().all(|e| e["usd"].is_object()));
    assert!(
        eps.iter()
            .filter(|e| e["usd"]["outcome"] == "closed_known")
            .all(|e| e["usd"]["pnl_decimal"].is_string())
    );
    assert!(
        eps.iter()
            .filter(|e| e["usd"]["outcome"] != "closed_known")
            .all(|e| e["usd"]["pnl_decimal"].is_null())
    );
    let summary = lines.last().unwrap();
    assert_eq!(summary["requests_made_prices"], 1);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
    // The price request carried a User-Agent and no key.
    let req = &c.received_requests().await.unwrap()[0];
    assert!(req.headers.get("user-agent").is_some());
    assert!(req.url.query().unwrap().contains("granularity=60"));
    assert!(!req.url.as_str().contains(KEY));
}

#[tokio::test]
async fn table_has_usd_columns_and_coverage() {
    let h = helius(1).await;
    let c = coinbase_ok().await;
    let out = run(&h, &c, &[]).await;
    let head = out.stdout.lines().next().unwrap();
    for col in [
        "realized_pnl_usd",
        "pnl_lower_bound_usd",
        "usd_price_coverage",
    ] {
        assert!(head.contains(col), "{col}: {head}");
    }
    let row = out.stdout.lines().nth(1).unwrap();
    assert!(row.contains('%'), "coverage cell: {row}");
    assert!(
        out.stderr
            .contains("usd pricing: source=coinbase-exchange-candles-1m")
    );
}

#[tokio::test]
async fn no_usd_makes_no_price_request_and_has_no_usd_block() {
    let h = helius(1).await;
    let c = coinbase_ok().await;
    let out = run(&h, &c, &["--format", "jsonl", "--no-usd"]).await;
    assert_eq!(c.received_requests().await.unwrap().len(), 0);
    let lines = jsonl(&out);
    assert_eq!(lines[0]["pricing"]["enabled"], false);
    assert_eq!(lines[0]["requests_made_prices"], 0);
    assert!(wallet_record(&lines)["stats"]["usd"].is_null());
    assert!(out.stderr.contains("disabled (--no-usd)"));
    let table = run(&h, &c, &["--no-usd"]).await;
    assert!(
        table
            .stdout
            .lines()
            .nth(1)
            .unwrap()
            .contains("N/A (usd not priced)")
    );
}

#[tokio::test]
async fn provider_failure_is_unknown_with_its_class_and_does_not_change_the_exit_code() {
    let h = helius(1).await;
    let ok = coinbase_ok().await;
    let baseline = run(&h, &ok, &["--format", "jsonl", "--no-usd"]).await;
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&bad)
        .await;
    let out = run(&h, &bad, &["--format", "jsonl"]).await;
    assert_eq!(out.code, baseline.code, "{}", out.stderr);
    let lines = jsonl(&out);
    let cov = &wallet_record(&lines)["stats"]["usd"]["price_coverage"];
    let legs = cov["legs"].as_u64().unwrap();
    assert!(legs > 0);
    assert_eq!(cov["priced"], 0);
    assert_eq!(cov["unpriced"].as_u64().unwrap(), legs);
    assert_eq!(
        cov["unpriced_by_reason"]["provider_http_503"]
            .as_u64()
            .unwrap(),
        legs
    );
    // 3 attempts (retries) for the single page, counted as price requests.
    assert_eq!(lines[0]["requests_made_prices"], 3);
    assert_eq!(bad.received_requests().await.unwrap().len(), 3);
    // Unknown is never zero: no USD PnL figure.
    let usd = &wallet_record(&lines)["stats"]["usd"];
    assert_eq!(usd["closed_known"], 0);
    assert!(usd["realized_trade_pnl"].is_null());
}

#[tokio::test]
async fn price_budget_is_separate_bounded_and_makes_the_run_incomplete() {
    // Four 300-minute pages are needed; one request is allowed.
    let h = helius(4).await;
    let c = coinbase_ok().await;
    let out = run(&h, &c, &["--format", "jsonl", "--max-price-requests", "1"]).await;
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert_eq!(c.received_requests().await.unwrap().len(), 1);
    let lines = jsonl(&out);
    let p = &lines[0]["pricing"];
    assert_eq!(p["requests_made_prices"], 1);
    assert_eq!(p["max_price_requests"], 1);
    assert_eq!(lines[0]["budget"]["max_price_requests"], 1);
    assert_eq!(p["prefetch"]["pages_fetched"], 1);
    assert_eq!(p["prefetch"]["pages_skipped_budget"], 3);
    let cov = &wallet_record(&lines)["stats"]["usd"]["price_coverage"];
    assert!(
        cov["unpriced_by_reason"]["request_budget_exhausted"]
            .as_u64()
            .unwrap()
            > 0
    );
    let summary = lines.last().unwrap();
    assert_eq!(summary["status"], "partial");
    assert!(
        summary["incomplete_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r
                .as_str()
                .unwrap()
                .contains("price request budget exhausted"))
    );
    assert_eq!(lines[0]["requests_made"], 1, "scan budget untouched");
}

#[tokio::test]
async fn max_price_requests_zero_is_a_usage_error() {
    let h = helius(1).await;
    let c = coinbase_ok().await;
    let out = run(&h, &c, &["--max-price-requests", "0"]).await;
    assert_eq!(out.code, 2);
}
