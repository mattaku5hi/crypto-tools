//! Offline wiremock CLI tests for the ADR-019 open-exposure valuation of
//! `wallet-rank` (flags, JSONL fields, `open_exposure_unvalued` exclusion).
//! The Helius mock serves the committed PumpSwap wallet page for
//! `getTransactionsForAddress` and an EMPTY chain (every account missing)
//! for `getMultipleAccounts`; the valued path is covered by the engine and
//! `wallet-stats` tests. No network.
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
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-rank");
const KEY: &str = "SUPERSECRETKEY123";
const WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn helius() -> MockServer {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/pumpswap_wallet_page_2026-10-02.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut page = v["pages"][0].clone();
    page["paginationToken"] = Value::Null;
    let tx_page = json!({"jsonrpc": "2.0", "id": 1, "result": page});
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |req: &Request| {
            let v: Value = serde_json::from_slice(&req.body).unwrap();
            if v["method"] == "getTransactionsForAddress" {
                return ResponseTemplate::new(200).set_body_json(tx_page.clone());
            }
            let n = v["params"][0].as_array().unwrap().len();
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"context": {"slot": 4242}, "value": vec![Value::Null; n]}
            }))
        })
        .mount(&server)
        .await;
    server
}

async fn run(h: &MockServer, extra: &[&str]) -> Out {
    let h = h.uri();
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(["--input", "-", "--profile", "none", "--format", "jsonl"])
            .args(extra)
            .env("SCOUT_HELIUS_API_KEY", KEY)
            .env("SCOUT_WALLET_RANK_ENDPOINT", h)
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

fn lines(out: &Out) -> Vec<Value> {
    out.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The single wallet record (ranked or excluded) and its metrics.
fn wallet(lines: &[Value]) -> (&Value, &Value) {
    let r = lines
        .iter()
        .find(|l| l["kind"] == "wallet_rank" || l["kind"] == "wallet_excluded")
        .unwrap();
    let m = if r["kind"] == "wallet_rank" {
        &r["metrics"]
    } else {
        &r["observed"]
    };
    (r, m)
}

#[tokio::test]
async fn unvalued_exposure_is_reported_with_reasons_and_budget_counts_the_reads() {
    let h = helius().await;
    let out = run(&h, &[]).await;
    assert!(
        out.code == 0 || out.code == 3,
        "{} {}",
        out.code,
        out.stderr
    );
    let l = lines(&out);
    let meta = &l[0];
    assert_eq!(meta["open_valuation"]["enabled"], true);
    assert_eq!(meta["open_valuation"]["state_slot"], 4242);
    assert_eq!(meta["thresholds"]["require_valued_open"], false);
    assert!(
        meta["rank_version"]
            .as_str()
            .unwrap()
            .starts_with("solana-wallet-rank/6")
    );
    // 1 scan page + 1 account pass (every pool missing -> no vault pass).
    assert_eq!(meta["scan"]["requests_made"], 2);
    let (r, m) = wallet(&l);
    let ox = &m["open_exposure"];
    assert_eq!(ox["status"], "unvalued");
    let n = ox["positions"].as_u64().unwrap();
    assert!(n >= 10);
    assert_eq!(ox["totals"]["unvalued"], n);
    assert_eq!(ox["totals"]["unvalued_by_reason"]["account_missing"], n);
    assert!(ox["details"].as_array().unwrap().iter().all(|d| {
        d["status"] == "unvalued"
            && d["unvalued_reason"] == "account_missing"
            && d["realizable_lamports"].is_null()
    }));
    // Without --require-valued-open the wallet is not excluded for exposure.
    let reasons = r["reasons"].as_array().cloned().unwrap_or_default();
    assert!(!reasons.contains(&json!("open_exposure_unvalued")));
}

#[tokio::test]
async fn require_valued_open_excludes_with_open_exposure_unvalued() {
    let h = helius().await;
    let out = run(&h, &["--require-valued-open"]).await;
    assert!(
        out.code == 0 || out.code == 3,
        "{} {}",
        out.code,
        out.stderr
    );
    let l = lines(&out);
    assert_eq!(l[0]["thresholds"]["require_valued_open"], true);
    let (r, _) = wallet(&l);
    assert_eq!(r["kind"], "wallet_excluded");
    assert!(
        r["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("open_exposure_unvalued"))
    );
}

#[tokio::test]
async fn no_valuation_skips_the_reads_and_conflicts_with_require_valued_open() {
    let h = helius().await;
    let out = run(&h, &["--no-valuation"]).await;
    let l = lines(&out);
    assert_eq!(l[0]["open_valuation"]["enabled"], false);
    assert_eq!(l[0]["scan"]["requests_made"], 1);
    let (_, m) = wallet(&l);
    assert!(
        m["open_exposure"]["details"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["unvalued_reason"] == "not_run")
    );
    assert!(m["open_exposure"]["totals"].is_null());

    let bad = run(&h, &["--no-valuation", "--require-valued-open"]).await;
    assert_eq!(bad.code, 2, "{}", bad.stderr);
    assert!(!bad.stdout.contains(KEY));
}
