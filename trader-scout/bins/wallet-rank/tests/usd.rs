//! Offline wiremock CLI tests for `wallet-rank --quote usd` (ADR-018):
//! two wallets with different quote units -- a PumpSwap wallet trading in
//! SOL and a router wallet trading in USDC (par) -- are ranked in ONE USD
//! column. The Helius mock serves each committed wallet page per requested
//! address (block times rewritten into the recorded Coinbase candle window);
//! the Coinbase mock serves the orchestrator-recorded SOL-USD candles
//! (2026-10-03 recording of 2026-10-02 12:00-12:05 UTC). No network.
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

const BIN: &str = env!("CARGO_BIN_EXE_wallet-rank");
const KEY: &str = "SUPERSECRETKEY123";
const SOL_WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";
const USDC_WALLET: &str = "9oC3XYAs2oeU39NeNFke8m3JGixMq7g8PfANsmsbKR8W";
/// 2026-10-02T12:00:00Z, the first recorded candle (close 121.89).
const T0: i64 = 1_790_942_400;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fixture_page(name: &str) -> Value {
    let p = format!(
        "{}/../../docs/p0/measurements/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let mut page = v["pages"][0].clone();
    for tx in page["data"].as_array_mut().unwrap() {
        tx["blockTime"] = json!(T0 + 30);
    }
    page["paginationToken"] = Value::Null;
    json!({"jsonrpc": "2.0", "id": 1, "result": page})
}

async fn helius() -> MockServer {
    let server = MockServer::start().await;
    let sol = fixture_page("pumpswap_wallet_page_2026-10-02.json");
    let usdc = fixture_page("router_wallet_9oC3_page_2026-10-02.json");
    Mock::given(method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let page = if body["params"][0] == USDC_WALLET {
                usdc.clone()
            } else {
                sol.clone()
            };
            ResponseTemplate::new(200).set_body_json(page)
        })
        .mount(&server)
        .await;
    server
}

async fn coinbase() -> MockServer {
    let server = MockServer::start().await;
    let body = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/coinbase_sol_usd_candles_1m_2026-10-02T1200Z_recorded.json"
    ))
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/products/SOL-USD/candles"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    server
}

async fn run(h: &MockServer, c: &MockServer, extra: &[&str]) -> Out {
    let (h, c) = (h.uri(), c.uri());
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(["--input", "-", "--profile", "none", "--no-valuation"])
            .args(extra)
            .env("SCOUT_HELIUS_API_KEY", KEY)
            .env("SCOUT_WALLET_RANK_ENDPOINT", h)
            .env("SCOUT_COINBASE_ENDPOINT", c)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("solana:{SOL_WALLET}\nsolana:{USDC_WALLET}\n").as_bytes());
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

fn usd_unit(metrics: &Value) -> &Value {
    metrics["quote_units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["unit"] == "usd")
        .unwrap()
}

#[tokio::test]
async fn quote_usd_prices_ranks_in_usd_and_counts_price_requests_apart() {
    let h = helius().await;
    let c = coinbase().await;
    let out = run(&h, &c, &["--format", "jsonl", "--quote", "usd"]).await;
    assert!(
        out.code == 0 || out.code == 3,
        "{} {}",
        out.code,
        out.stderr
    );
    let lines = jsonl(&out);
    let meta = &lines[0];
    assert_eq!(meta["rank_quote_unit"], "usd");
    assert!(
        meta["rank_version"]
            .as_str()
            .unwrap()
            .starts_with("solana-wallet-rank/6")
    );
    assert!(
        meta["ledger_version"]
            .as_str()
            .unwrap()
            .starts_with("solana-wallet-ledger/13")
    );
    let n_coinbase = c.received_requests().await.unwrap().len();
    assert_eq!(n_coinbase, 1);
    assert_eq!(meta["pricing"]["enabled"], true);
    assert_eq!(meta["pricing"]["source"], "coinbase-exchange-candles-1m");
    assert_eq!(meta["pricing"]["staleness_limit_minutes"], 5);
    assert_eq!(meta["pricing"]["requests_made_prices"], 1);
    assert_eq!(meta["requests_made_prices"], 1);
    assert_eq!(
        meta["scan"]["requests_made"], 2,
        "one scan request per wallet"
    );
    assert_eq!(meta["pricing"]["wallets_priced"], 2);
    let cov = &meta["pricing"]["coverage"];
    assert!(cov["legs"].as_u64().unwrap() > 0);
    assert_eq!(cov["unpriced"], 0, "{cov}");
    assert!(
        cov["usdc_par_legs"].as_u64().unwrap() > 0,
        "the router wallet's USDC legs are valued at par and counted: {cov}"
    );
    // Every input wallet is accounted for, each carries its USD unit block.
    let records: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "wallet_rank" || l["kind"] == "wallet_excluded")
        .collect();
    assert_eq!(records.len(), 2);
    for r in &records {
        let m = if r["kind"] == "wallet_rank" {
            &r["metrics"]
        } else {
            &r["observed"]
        };
        assert_eq!(m["quote"], "usd");
        let u = usd_unit(m);
        assert_eq!(u["decimals"], 8);
        assert!(m["usd"]["price_coverage"]["legs"].as_u64().unwrap() > 0);
    }
    // Ranked wallets are ordered by USD net PnL, descending.
    let ranked: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "wallet_rank")
        .collect();
    let pnl: Vec<i128> = ranked
        .iter()
        .map(|r| {
            r["metrics"]["realized_net_pnl"]["raw"]
                .as_str()
                .unwrap()
                .parse::<i128>()
                .unwrap()
        })
        .collect();
    assert!(pnl.windows(2).all(|w| w[0] >= w[1]), "{pnl:?}");
    for r in &ranked {
        assert_eq!(r["metrics"]["realized_net_pnl"]["unit"], "usd");
        let dec = r["metrics"]["realized_net_pnl"]["decimal"]
            .as_str()
            .unwrap();
        assert_eq!(dec.rsplit('.').next().unwrap().len(), 8, "{dec}");
    }
    let summary = lines.last().unwrap();
    assert_eq!(summary["rank_quote_unit"], "usd");
    assert_eq!(summary["requests_made_prices"], 1);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
}

#[tokio::test]
async fn table_header_names_the_usd_unit_and_notes_the_pricing() {
    let h = helius().await;
    let c = coinbase().await;
    let out = run(&h, &c, &["--quote", "usd"]).await;
    let head = out.stdout.lines().next().unwrap();
    assert!(head.contains("realized_net_pnl_usd"), "{head}");
    assert!(
        out.stdout
            .contains("usd pricing: source=coinbase-exchange-candles-1m")
    );
    assert!(out.stdout.contains("quote=usd"));
}

#[tokio::test]
async fn other_quotes_never_fetch_prices() {
    let h = helius().await;
    let c = coinbase().await;
    let out = run(&h, &c, &["--format", "jsonl", "--quote", "sol"]).await;
    assert_eq!(c.received_requests().await.unwrap().len(), 0);
    let lines = jsonl(&out);
    assert_eq!(lines[0]["pricing"]["enabled"], false);
    assert_eq!(lines[0]["requests_made_prices"], 0);
    for r in lines
        .iter()
        .filter(|l| l["kind"] == "wallet_rank" || l["kind"] == "wallet_excluded")
    {
        let m = if r["kind"] == "wallet_rank" {
            &r["metrics"]
        } else {
            &r["observed"]
        };
        assert!(m["usd"].is_null());
    }
}

#[tokio::test]
async fn price_budget_bounds_retries_and_unpriced_legs_stay_unknown() {
    let h = helius().await;
    // Budget of 1 attempt against a failing provider: 1 attempt, retries cut.
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&bad)
        .await;
    let out = run(
        &h,
        &bad,
        &[
            "--format",
            "jsonl",
            "--quote",
            "usd",
            "--max-price-requests",
            "1",
        ],
    )
    .await;
    assert_eq!(bad.received_requests().await.unwrap().len(), 1);
    let lines = jsonl(&out);
    assert_eq!(lines[0]["pricing"]["requests_made_prices"], 1);
    let cov = &lines[0]["pricing"]["coverage"];
    assert!(
        cov["priced"].as_u64().unwrap() > 0,
        "USDC par legs stay priced"
    );
    assert!(cov["unpriced"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn max_price_requests_zero_is_a_usage_error() {
    let h = helius().await;
    let c = coinbase().await;
    let out = run(&h, &c, &["--max-price-requests", "0"]).await;
    assert_eq!(out.code, 2);
}
