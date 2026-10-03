//! ADR-014 CLI tests (offline, wiremock): `--side`, the analysis window and
//! the extended output. The mock serves the committed pump.fun probe capture
//! (one curve BUY of AB48 by one wallet, one curve SELL of 67266... by
//! another) for every token.
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
const BUY_MINT: &str = "AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
const SELL_MINT: &str = "67266Ha2icrdCKHyrYKyG4oJyJ7RqheaGbuGd6vwXbLD";
const INPUT: &str = "solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n\
                     solana:67266Ha2icrdCKHyrYKyG4oJyJ7RqheaGbuGd6vwXbLD\n";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn server() -> MockServer {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/pump_bonding_curve_buy_probe.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut result = v["result"].clone();
    result["paginationToken"] = Value::Null;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": result})),
        )
        .mount(&server)
        .await;
    server
}

async fn run(endpoint: Option<String>, args: &[&str]) -> Out {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env("SCOUT_HELIUS_API_KEY", "KEY123")
            .env_remove("SCOUT_BUYER_INTERSECT_ENDPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(e) = endpoint {
            cmd.env("SCOUT_BUYER_INTERSECT_ENDPOINT", e);
        }
        let mut child = cmd.spawn().unwrap();
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

fn jsonl(out: &Out) -> Vec<Value> {
    out.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn matches(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|l| l["kind"] == "buyer_match")
        .collect()
}

const J: [&str; 5] = ["--input", "-", "--format", "jsonl", "--min-token-hits=1"];

fn with(extra: &[&'static str]) -> Vec<&'static str> {
    J.iter().chain(extra).copied().collect()
}

#[tokio::test]
async fn default_side_is_any_and_run_meta_records_scope_and_window() {
    let server = server().await;
    let out = run(Some(server.uri()), &with(&[])).await;
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let lines = jsonl(&out);
    let meta = &lines[0];
    assert_eq!(meta["side"], "any");
    assert_eq!(meta["window"]["source"], "none");
    assert!(meta["window"]["since"].is_null());
    assert_eq!(meta["scan_order"], "oldest_first");
    let scope = &meta["scope"];
    assert!(
        scope["qualification_version"]
            .as_str()
            .unwrap()
            .contains("ADR-014")
    );
    let recognized = scope["recognized"].as_str().unwrap();
    assert!(recognized.contains("PumpSwap") && recognized.contains("route"));
    let not_decoded = scope["not_decoded"].as_str().unwrap();
    assert!(not_decoded.contains("Raydium") && not_decoded.contains("Whirlpool"));
    assert!(scope["amm_program_id"].as_str().is_some());

    // One buyer of AB48 and one seller of 67266 hit under `any`.
    let m = matches(&lines);
    assert_eq!(m.len(), 2, "{m:?}");
    let mut seen: Vec<(String, Vec<String>)> = m
        .iter()
        .map(|r| {
            let t = &r["matched_tokens"][0];
            (
                t["token"].as_str().unwrap().to_string(),
                t["sides"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (SELL_MINT.to_string(), vec!["sell".to_string()]),
            (BUY_MINT.to_string(), vec!["buy".to_string()]),
        ]
    );
    for r in &m {
        let t = &r["matched_tokens"][0];
        let side = t["sides"][0].as_str().unwrap();
        assert_eq!(t[format!("{side}_count")], 1);
        let ev = &t[format!("first_{side}")];
        assert_eq!(ev["venue"], "bonding_curve");
        assert_eq!(ev["slot"], 452_380_124);
        assert!(ev["signature"].as_str().unwrap().len() >= 80);
        let other = if side == "buy" { "sell" } else { "buy" };
        assert!(t[format!("first_{other}")].is_null());
    }
    // run_summary per-token diagnostics.
    let summary = lines.last().unwrap();
    assert_eq!(summary["status"], "complete");
    let tok = &summary["tokens"][0];
    assert_eq!(tok["qualified_buyers"], 1);
    assert_eq!(tok["qualified_sellers"], 0);
    assert_eq!(tok["qualified_wallets"], 1);
    let d = &tok["diagnostics"];
    assert_eq!(d["qualified_ops"]["bonding_curve"]["buys"], 1);
    assert_eq!(d["qualified_ops"]["pump_amm"]["buys"], 0);
    assert_eq!(d["router_forwards_not_attributed"], 0);
    assert_eq!(d["idl_only_trades"], 0);
    assert_eq!(d["route_rejections"]["multi_asset"], 0);
    assert!(out.stderr.contains("side=any"), "{}", out.stderr);
}

#[tokio::test]
async fn side_buy_and_sell_narrow_the_hits_and_k_applies_per_side() {
    let server = server().await;
    let buy = run(Some(server.uri()), &with(&["--side", "buy"])).await;
    assert_eq!(buy.code, 0, "{}", buy.stderr);
    let lines = jsonl(&buy);
    assert_eq!(lines[0]["side"], "buy");
    let m = matches(&lines);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0]["matched_tokens"][0]["token"], BUY_MINT);
    assert_eq!(m[0]["matched_tokens"][0]["sides"], json!(["buy"]));

    let sell = run(Some(server.uri()), &with(&["--side=sell"])).await;
    assert_eq!(sell.code, 0, "{}", sell.stderr);
    let lines = jsonl(&sell);
    assert_eq!(lines[0]["side"], "sell");
    let m = matches(&lines);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0]["matched_tokens"][0]["token"], SELL_MINT);

    // K=2 needs two distinct tokens hit by ONE wallet: nobody under any
    // side here, and the run is still complete (exit 0, empty result).
    let k2 = run(
        Some(server.uri()),
        &["--input", "-", "--format", "jsonl", "--side", "any"],
    )
    .await;
    assert_eq!(k2.code, 0, "{}", k2.stderr);
    assert!(matches(&jsonl(&k2)).is_empty());
}

#[tokio::test]
async fn table_lines_carry_hit_count_and_side_markers() {
    let server = server().await;
    let out = run(Some(server.uri()), &["--input", "-", "--min-token-hits=1"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines.iter().all(|l| l.contains(" hit_count=1 ")));
    assert!(lines.iter().any(|l| l.ends_with(&format!("{BUY_MINT}=B"))));
    assert!(lines.iter().any(|l| l.ends_with(&format!("{SELL_MINT}=S"))));
}

#[tokio::test]
async fn window_flags_scan_newest_first_and_filter_by_block_time() {
    let server = server().await;
    // The capture's transactions are at 2026-10-01T19:22:10Z.
    let inside = run(
        Some(server.uri()),
        &with(&[
            "--since",
            "2026-10-01T00:00:00Z",
            "--until",
            "2026-10-02T00:00:00Z",
        ]),
    )
    .await;
    assert_eq!(inside.code, 0, "{}", inside.stderr);
    let lines = jsonl(&inside);
    let w = &lines[0]["window"];
    assert_eq!(w["source"], "explicit");
    assert_eq!(w["since"], "2026-10-01T00:00:00Z");
    assert_eq!(w["until"], "2026-10-02T00:00:00Z");
    assert_eq!(lines[0]["scan_order"], "newest_first");
    assert_eq!(matches(&lines).len(), 2);
    assert_eq!(
        lines.last().unwrap()["tokens"][0]["transactions_in_window"],
        5
    );
    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert!(
        bodies
            .iter()
            .all(|b| b.to_string().contains("\"sortOrder\":\"desc\"")),
        "{bodies:?}"
    );

    // A window that ends before the capture: nothing inside, complete
    // (history ended), not an error.
    let before = run(
        Some(server.uri()),
        &with(&[
            "--since",
            "2020-01-01T00:00:00Z",
            "--until",
            "2020-02-01T00:00:00Z",
        ]),
    )
    .await;
    assert_eq!(before.code, 0, "{}", before.stderr);
    let lines = jsonl(&before);
    assert!(matches(&lines).is_empty());
    assert_eq!(
        lines.last().unwrap()["tokens"][0]["transactions_in_window"],
        0
    );

    // A window starting after the capture: the boundary is reached at once.
    let after = run(
        Some(server.uri()),
        &with(&[
            "--since",
            "2026-10-01T19:22:11Z",
            "--until",
            "2026-10-02T00:00:00Z",
        ]),
    )
    .await;
    assert_eq!(after.code, 0, "{}", after.stderr);
    assert!(matches(&jsonl(&after)).is_empty());
}
