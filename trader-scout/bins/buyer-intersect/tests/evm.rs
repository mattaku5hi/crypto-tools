//! Offline CLI tests of the EVM path of `buyer-intersect` (ADR-020 step 2):
//! the real binary over the committed live Robinhood fixture (Aiden token,
//! 2026-10-03T17:54:46Z..18:04:46Z) replayed by wiremock. The RPC URL carries
//! a fake key that must never be printed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::{EvmFixtureReplay, ReplayReply};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const BIN: &str = env!("CARGO_BIN_EXE_buyer-intersect");
const SECRET: &str = "ALCHEMYSECRETKEY987";
const AIDEN: &str = "0x15e853bc1c69529bd0a16bab1a742645a3607e1f";
const EMPTY: &str = "0x00000000000000000000000000000000000000c9";
const WINDOW: [&str; 4] = [
    "--since",
    "2026-10-03T17:54:46Z",
    "--until",
    "2026-10-03T18:04:46Z",
];

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn rpc() -> MockServer {
    rpc_with(false).await
}

/// `capped`: `eth_getLogs` answers like Alchemy's free tier (10-block cap)
/// for every window wider than 10 blocks.
async fn rpc_with(capped: bool) -> MockServer {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/evm_robinhood_token_aiden_v4_2026-10-03.json"
    );
    let replay = EvmFixtureReplay::from_path(std::path::Path::new(p))
        .unwrap()
        .with_handler(Box::new(move |m, params| {
            if capped && m == "eth_getLogs" {
                let hex = |v: &Value| {
                    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
                };
                let (a, b) = (hex(&params[0]["fromBlock"]), hex(&params[0]["toBlock"]));
                if b - a >= 10 {
                    return Some(ReplayReply::Error {
                        code: -32600,
                        message: format!(
                            "Under the Free tier plan, you can make eth_getLogs requests with up to a 10 block range. Based on your parameters, this block range should work: [{a:#x}, {:#x}]",
                            a + 9
                        ),
                    });
                }
            }
            if m == "eth_call" {
                return Some(ReplayReply::Result(json!(format!("0x{:064x}", 6))));
            }
            if m == "eth_getLogs"
                && params[0]["address"]
                    .as_str()
                    .is_some_and(|a| a.eq_ignore_ascii_case(EMPTY))
            {
                return Some(ReplayReply::Result(json!([])));
            }
            None
        }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn run(env: Vec<(&'static str, String)>, extra: &[&str], input: String) -> Out {
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(["--input", "-", "--rpc-rps", "5000"])
            .args(WINDOW)
            .args(extra)
            .env_remove("SCOUT_ROBINHOOD_RPC_URL")
            .env_remove("SCOUT_BASE_RPC_URL")
            .env_remove("SCOUT_HELIUS_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
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

fn env_of(s: &MockServer) -> Vec<(&'static str, String)> {
    vec![(
        "SCOUT_ROBINHOOD_RPC_URL",
        format!("{}/v2/{SECRET}", s.uri()),
    )]
}

fn input() -> String {
    format!("robinhood:{AIDEN}\nrobinhood:{EMPTY}\n")
}

#[tokio::test]
async fn aiden_window_reports_the_fixture_numbers_in_table_and_jsonl() {
    let s = rpc().await;
    // K = 1 over both sides: the 22 signers that traded Aiden in the window.
    let o = run(env_of(&s), &["--min-token-hits", "1"], input()).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(!o.stdout.contains(SECRET) && !o.stderr.contains(SECRET));
    assert_eq!(o.stdout.lines().count(), 22, "{}", o.stdout);
    for l in o.stdout.lines() {
        assert!(l.contains(&format!(" hit_count=1 {AIDEN}=")), "{l}");
    }
    assert!(o.stderr.contains("uniswap_v4=FixtureVerified"));
    assert!(
        o.stderr.contains("buyers=22 sellers=10 wallets=22"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr
            .contains("status=complete within declared protocol scope")
    );

    let o = run(
        env_of(&s),
        &["--min-token-hits", "1", "--side", "sell"],
        input(),
    )
    .await;
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout.lines().count(), 10);
    assert!(o.stdout.lines().all(|l| l.ends_with("=S")));

    // Default K = 2 over two tokens where one has no activity: nobody matches.
    let o = run(env_of(&s), &[], input()).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout.trim(), "");

    let o = run(
        env_of(&s),
        &["--min-token-hits", "1", "--format", "jsonl"],
        input(),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let lines: Vec<Value> = o
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 24);
    let meta = &lines[0];
    assert_eq!(meta["kind"], "run_meta");
    assert_eq!(meta["scope"]["chain"], "robinhood");
    assert_eq!(meta["side"], "any");
    assert_eq!(meta["input_token_count"], 2);
    let m = lines.iter().find(|l| l["kind"] == "buyer_match").unwrap();
    assert_eq!(m["wallet"]["chain"], "robinhood");
    let t = &m["matched_tokens"][0];
    assert_eq!(t["token"], AIDEN);
    assert_eq!(t["first_buy"]["venue"], "uniswap_v4");
    assert!(
        t["first_buy"]["signature"]
            .as_str()
            .unwrap()
            .starts_with("0x")
    );
    let summary = lines.last().unwrap();
    assert_eq!(summary["status"], "complete");
    assert_eq!(summary["records"], 22);
    let tok = &summary["tokens"][0];
    assert_eq!(tok["transactions_scanned"], 48);
    assert_eq!(tok["transfer_logs"], 278);
    assert_eq!(
        (
            tok["qualified_buyers"].as_u64(),
            tok["qualified_sellers"].as_u64()
        ),
        (Some(22), Some(10))
    );
    assert_eq!(tok["extraction"]["trades"], 44);
    assert_eq!(tok["extraction"]["nft_transfer_logs"], 2);
    assert_eq!(summary["tokens"][1]["transactions_scanned"], 0);
}

async fn count(s: &MockServer, m: &str) -> usize {
    s.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| {
            serde_json::from_slice::<Value>(&r.body).is_ok_and(|b| b["method"].as_str() == Some(m))
        })
        .count()
}

#[tokio::test]
async fn capped_keyed_rpc_sends_logs_to_the_public_endpoint_and_keeps_the_rest() {
    let keyed = rpc_with(true).await;
    let public = rpc().await;
    let mut env = env_of(&keyed);
    env.push(("SCOUT_EVM_PUBLIC_RPC_URL", public.uri()));
    let o = run(
        env,
        &["--min-token-hits", "1", "--format", "jsonl"],
        input(),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(!o.stdout.contains(SECRET) && !o.stderr.contains(SECRET));
    // keyed: one probe, no window logs; public: the real log scan
    assert_eq!(count(&keyed, "eth_getLogs").await, 1);
    assert!(count(&public, "eth_getLogs").await >= 2);
    // receipts stay on the keyed endpoint
    assert!(count(&keyed, "eth_getBlockReceipts").await > 0);
    assert_eq!(count(&public, "eth_getBlockReceipts").await, 0);
    assert!(o.stderr.contains("eth_getLogs ONLY"), "{}", o.stderr);
    assert!(
        o.stderr
            .contains("routing: eth_getLogs -> public robinhood rpc (auto")
    );
    let meta: Value = serde_json::from_str(o.stdout.lines().next().unwrap()).unwrap();
    assert_eq!(meta["kind"], "run_meta");
    assert!(
        meta["scope"]["logs_source"]
            .as_str()
            .unwrap()
            .contains("auto")
    );
    assert!(
        meta["scope"]["state_source"]
            .as_str()
            .unwrap()
            .starts_with("keyed rpc")
    );
    assert_eq!(meta["scope"]["rate_limits"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn refusals_have_the_documented_exit_codes() {
    let s = rpc().await;
    let mixed = format!("robinhood:{AIDEN}\nsolana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n");
    let o = run(env_of(&s), &[], mixed).await;
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("mixed Solana and EVM"));
    assert_eq!(
        s.received_requests().await.unwrap().len(),
        0,
        "nothing scanned"
    );
    let o = run(env_of(&s), &[], format!("bsc:{AIDEN}\nbsc:{EMPTY}\n")).await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("not verified yet"), "{}", o.stderr);
    // No RPC variable: Robinhood falls back to the public RPC (warning), but
    // here we only check the other chains' hard requirement.
    let o = run(
        vec![],
        &["--allow-unverified-chain"],
        format!("bsc:{AIDEN}\nbsc:{EMPTY}\n"),
    )
    .await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("SCOUT_BSC_RPC_URL"));
    // Base needs no flag any more: only its RPC variable.
    let o = run(vec![], &[], format!("base:{AIDEN}\nbase:{EMPTY}\n")).await;
    assert_eq!(o.code, 4);
    assert!(o.stderr.contains("SCOUT_BASE_RPC_URL"), "{}", o.stderr);
}
