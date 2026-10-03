//! Offline wiremock CLI tests for the ADR-019 open-position valuation of
//! `wallet-stats`. The Helius mock serves the committed PumpSwap wallet page
//! for `getTransactionsForAddress` and a FAKE chain state (pools, vaults) for
//! `getMultipleAccounts`; the expected pool/mint pairs are taken from the
//! engine's own ledger of the same page. No network.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use futures::StreamExt as _;
use scout_api::{HistoryProvider, ScanRequest, ScanTask};
use scout_core::{AddressBytes, AssetKey, RawPayload, RawSolanaTransaction, SolanaPubkey};
use scout_dex_solana::{
    POOL_ACCOUNT_DISCRIMINATOR, PUMP_AMM_PROGRAM_ID_BYTES, SPL_TOKEN_PROGRAM_ID_BYTES,
    WRAPPED_SOL_MINT, amm_sell_quote, effective_quote_reserve,
};
use scout_engine::{
    LedgerDecoders, LedgerOptions, build_solana_wallet_ledger_venues, pump_amm_decoder,
    pump_bonding_curve_decoder, solana_mainnet_chain,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");
const KEY: &str = "SUPERSECRETKEY123";
const WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn b58(k: &SolanaPubkey) -> String {
    bs58::encode(k).into_string()
}

fn page() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/pumpswap_wallet_page_2026-10-02.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut page = v["pages"][0].clone();
    page["paginationToken"] = Value::Null;
    page
}

struct Fake {
    owner: SolanaPubkey,
    data: Vec<u8>,
}

fn pool_account(base_mint: SolanaPubkey, bv: SolanaPubkey, qv: SolanaPubkey, vq: i128) -> Fake {
    let mut d = POOL_ACCOUNT_DISCRIMINATOR.to_vec();
    d.push(255);
    d.extend(0u16.to_le_bytes());
    d.extend(pk(1));
    d.extend(base_mint);
    d.extend(WRAPPED_SOL_MINT);
    d.extend(pk(4));
    d.extend(bv);
    d.extend(qv);
    d.extend(0u64.to_le_bytes());
    d.extend(pk(8));
    d.extend([0, 0]);
    d.extend(vq.to_le_bytes());
    Fake {
        owner: PUMP_AMM_PROGRAM_ID_BYTES,
        data: d,
    }
}

fn token_account(mint: SolanaPubkey, amount: u64) -> Fake {
    let mut d = Vec::new();
    d.extend(mint);
    d.extend(pk(2));
    d.extend(amount.to_le_bytes());
    d.extend([0u8; 36]);
    d.push(1);
    d.resize(165, 0);
    Fake {
        owner: SPL_TOKEN_PROGRAM_ID_BYTES,
        data: d,
    }
}

/// Transactions of the fixture page decoded through the real provider.
async fn page_txs() -> Vec<RawSolanaTransaction> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": page()})),
        )
        .mount(&server)
        .await;
    let provider = scout_providers::HeliusProvider::new_with_endpoint(
        scout_rpc::RpcEndpoint::new(server.uri()),
        5_000,
        1,
    )
    .unwrap();
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::TokenMarketActivity {
                asset: AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pk(10))),
            },
            description: "t".to_string(),
        },
        CancellationToken::new(),
    );
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        if let RawPayload::SolanaTransaction(tx) = item.unwrap().payload {
            out.push(tx);
        }
    }
    out
}

/// `(pool, mint, open_amount, fee bps)` of every open position of the page.
struct Plan {
    pool: SolanaPubkey,
    mint: SolanaPubkey,
    amount: u64,
    lp: u64,
    protocol: u64,
    creator: u64,
}

async fn plans() -> Vec<Plan> {
    let txs = page_txs().await;
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let wallet: SolanaPubkey = bs58::decode(WALLET).into_vec().unwrap().try_into().unwrap();
    let l = build_solana_wallet_ledger_venues(
        &wallet,
        &txs,
        &LedgerDecoders {
            curve: &curve,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        },
        LedgerOptions::default(),
    )
    .unwrap();
    l.open_venues
        .iter()
        .map(|i| {
            let p = i.pool.unwrap();
            let scout_engine::FeeObservation::Amm {
                lp_bps,
                protocol_bps,
                creator_bps,
            } = p.fee.unwrap()
            else {
                panic!("curve fee on a pool")
            };
            Plan {
                pool: p.address,
                mint: i.mint,
                amount: u64::try_from(i.open_amount_raw).unwrap(),
                lp: lp_bps,
                protocol: protocol_bps,
                creator: creator_bps,
            }
        })
        .collect()
}

/// Reserves of the fake pool of plan `i`.
fn reserves(p: &Plan, i: usize) -> (u64, u64, i128) {
    let k = u64::try_from(i).unwrap();
    (
        p.amount * (3 + k),
        1_000_000_000 * (k + 1),
        i128::from(k) * 5,
    )
}

async fn helius(skip_pool: Option<usize>) -> MockServer {
    let plans = plans().await;
    let mut accounts: BTreeMap<String, Fake> = BTreeMap::new();
    for (i, p) in plans.iter().enumerate() {
        if skip_pool == Some(i) {
            continue;
        }
        let (bal_b, bal_q, vq) = reserves(p, i);
        let bv = pk(0xB0u8.wrapping_add(u8::try_from(2 * i).unwrap()));
        let qv = pk(0xB1u8.wrapping_add(u8::try_from(2 * i).unwrap()));
        accounts.insert(b58(&p.pool), pool_account(p.mint, bv, qv, vq));
        accounts.insert(b58(&bv), token_account(p.mint, bal_b));
        accounts.insert(b58(&qv), token_account(WRAPPED_SOL_MINT, bal_q));
    }
    let calls = Arc::new(AtomicU64::new(0));
    let tx_page = json!({"jsonrpc": "2.0", "id": 1, "result": page()});
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |req: &Request| {
            let v: Value = serde_json::from_slice(&req.body).unwrap();
            if v["method"] == "getTransactionsForAddress" {
                return ResponseTemplate::new(200).set_body_json(tx_page.clone());
            }
            assert_eq!(v["method"], "getMultipleAccounts");
            let slot = 9000 + calls.fetch_add(1, Ordering::SeqCst);
            let value: Vec<Value> = v["params"][0]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| match accounts.get(a.as_str().unwrap()) {
                    None => Value::Null,
                    Some(f) => json!({
                        "data": [base64::engine::general_purpose::STANDARD.encode(&f.data), "base64"],
                        "executable": false, "lamports": 1, "owner": b58(&f.owner),
                        "rentEpoch": 0, "space": f.data.len()
                    }),
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"context": {"slot": slot}, "value": value}
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
        cmd.args(["--input", "-", "--no-usd"])
            .args(extra)
            .env("SCOUT_HELIUS_API_KEY", KEY)
            .env("SCOUT_WALLET_STATS_ENDPOINT", h)
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

#[tokio::test]
async fn open_positions_are_valued_counted_in_the_budget_and_reported() {
    let h = helius(None).await;
    let out = run(&h, &["--format", "jsonl", "--detail", "full"]).await;
    assert!(
        out.code == 0 || out.code == 3,
        "{} {}",
        out.code,
        out.stderr
    );
    let lines = jsonl(&out);
    let meta = &lines[0];
    let ov = &meta["open_valuation"];
    assert_eq!(ov["enabled"], true);
    assert_eq!(ov["historical_window"], false);
    assert_eq!(ov["label"], "realizable_cp_quote");
    assert_eq!(ov["commitment"], "confirmed");
    assert_eq!(ov["account_calls"], 2);
    assert_eq!(ov["state_slot"], 9001);
    assert_eq!(ov["state_slot_min"], 9000);
    assert!(
        ov["version"]
            .as_str()
            .unwrap()
            .starts_with("solana-open-valuation/1")
    );
    // The two account reads count in the request budget (1 scan + 2).
    assert_eq!(meta["requests_made"], 3);
    let plans = plans().await;
    let totals = &ov["totals"];
    assert_eq!(totals["positions"], plans.len());
    assert_eq!(totals["valued"], plans.len());
    assert_eq!(totals["unvalued"], 0);

    let w = lines.iter().find(|l| l["kind"] == "wallet_stats").unwrap();
    assert_eq!(w["stats"]["open_valuation"]["valued"], plans.len());
    let mut sum = 0u128;
    let positions = w["open_positions"].as_array().unwrap();
    assert_eq!(positions.len(), plans.len());
    for p in positions {
        let plan = plans
            .iter()
            .find(|x| b58(&x.mint) == p["mint"].as_str().unwrap())
            .unwrap();
        let i = plans.iter().position(|x| x.mint == plan.mint).unwrap();
        let (bb, bq, vq) = reserves(plan, i);
        let q = amm_sell_quote(
            plan.amount,
            bb,
            effective_quote_reserve(bq, vq).unwrap(),
            plan.lp,
            plan.protocol,
            plan.creator,
        )
        .unwrap();
        assert_eq!(p["status"], "valued");
        assert_eq!(p["label"], "realizable_cp_quote");
        assert_eq!(p["venue"], "pump_amm");
        assert_eq!(p["venue_address"], b58(&plan.pool));
        assert_eq!(p["realizable_lamports"], q.net.to_string());
        assert_eq!(p["gross_lamports"], q.raw_out.to_string());
        assert_eq!(p["vault_slot"], 9001);
        assert_eq!(p["account_slot"], 9000);
        assert_eq!(p["fee_bps"]["lp"], plan.lp);
        assert!(p["price_impact_bps"].is_u64());
        assert!(p["value_usd"].is_null(), "--no-usd: no USD figure");
        assert!(
            p["unrealized_pnl_status"] == "known" || p["unrealized_pnl_status"] == "unknown_basis"
        );
        sum += u128::from(q.net);
    }
    assert_eq!(totals["realizable_lamports"], sum.to_string());
    assert!(out.stderr.contains("open valuation: positions="));
}

#[tokio::test]
async fn unfetchable_pool_is_unvalued_with_a_reason_and_never_zero() {
    let h = helius(Some(0)).await;
    let out = run(&h, &["--format", "jsonl", "--detail", "full"]).await;
    let lines = jsonl(&out);
    let totals = &lines[0]["open_valuation"]["totals"];
    assert_eq!(totals["unvalued"], 1);
    assert_eq!(totals["unvalued_by_reason"]["account_missing"], 1);
    let w = lines.iter().find(|l| l["kind"] == "wallet_stats").unwrap();
    let un: Vec<&Value> = w["open_positions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["status"] == "unvalued")
        .collect();
    assert_eq!(un.len(), 1);
    assert_eq!(un[0]["unvalued_reason"], "account_missing");
    assert!(un[0]["realizable_lamports"].is_null());
    assert!(un[0]["unrealized_pnl_lamports"].is_null());
}

#[tokio::test]
async fn no_valuation_makes_no_account_request_and_marks_positions_not_run() {
    let h = helius(None).await;
    let out = run(
        &h,
        &["--format", "jsonl", "--detail", "full", "--no-valuation"],
    )
    .await;
    let lines = jsonl(&out);
    assert_eq!(lines[0]["open_valuation"]["enabled"], false);
    assert_eq!(lines[0]["requests_made"], 1);
    let w = lines.iter().find(|l| l["kind"] == "wallet_stats").unwrap();
    assert!(w["stats"]["open_valuation"].is_null());
    assert!(
        w["open_positions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "unvalued" && p["unvalued_reason"] == "not_run")
    );
    assert!(out.stderr.contains("disabled (--no-valuation)"));
}

#[tokio::test]
async fn historical_window_is_not_valued_and_reads_no_accounts() {
    let h = helius(None).await;
    let out = run(
        &h,
        &[
            "--format",
            "jsonl",
            "--period",
            "30d",
            "--until",
            "2026-09-01T00:00:00Z",
        ],
    )
    .await;
    let lines = jsonl(&out);
    assert_eq!(lines[0]["open_valuation"]["historical_window"], true);
    assert_eq!(lines[0]["open_valuation"]["account_calls"], 0);
    for l in h.received_requests().await.unwrap() {
        let v: Value = serde_json::from_slice(&l.body).unwrap();
        assert_ne!(v["method"], "getMultipleAccounts");
    }
}

#[tokio::test]
async fn table_shows_the_valuation_columns() {
    let h = helius(None).await;
    let out = run(&h, &["--detail", "full"]).await;
    assert!(out.code == 0 || out.code == 3, "{}", out.stderr);
    let header = out.stdout.lines().next().unwrap();
    for col in ["open_valued", "open_realizable_sol", "open_unrealized_sol"] {
        assert!(header.contains(col), "{col}");
    }
    assert!(
        out.stdout
            .contains(" valuation=realizable_cp_quote venue=pump_amm")
    );
    assert!(!out.stdout.contains(KEY));
}
