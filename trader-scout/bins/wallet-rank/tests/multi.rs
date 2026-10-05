//! Offline CLI tests of MULTI-CHAIN `wallet-rank` (docs/CLI.md §4): Solana +
//! Base + Robinhood in one run, each chain served by its own mock.
//! `--quote usd` ranks every chain in ONE list (rows show their chain);
//! a native/stable unit ranks per chain and excludes the chains without it
//! (`quote_unit_not_on_chain`, not scanned); a chain whose key is missing
//! lands in exclusions with its reason and the others still run.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::as_conversions
)]

use std::io::Write;
use std::process::{Command, Stdio};

use scout_providers::evm_replay::{
    AlchemyInternalMode, EvmFixtureReplay, ExplorerReplay, ReplayReply,
};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-rank");
const HELIUS_KEY: &str = "HELIUSSECRET555";
const RH_SECRET: &str = "RHALCHEMYSECRET111";
const BASE_SECRET: &str = "BASEALCHEMYSECRET222";
const BS_KEY: &str = "BLOCKSCOUTSECRET333";
const SOL_WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";
const USDC_WALLET: &str = "9oC3XYAs2oeU39NeNFke8m3JGixMq7g8PfANsmsbKR8W";
const RH_W1: &str = "0x4e40ceacc9d16dad54f90daffd3a7291cacc0884";
const BASE_USDC: &str = "0xb0b21cef6df3cc3716193fd94880b58c2adb90b7";
const BASE_SELL: &str = "0x3484978c2680823516c6f409ff736180ccf62dfc";
const BASE_FIRST_BLOCK: i128 = 52_146_932;
const BASE_FIRST_TS: i128 = 1_791_083_211;
/// Base fixture window; Solana pages are rewritten into it. The Robinhood
/// fixture lies in another window, which its replay cannot resolve: that chain
/// FAILS here, a built-in "one chain down" case.
const BASE_WINDOW: [&str; 4] = [
    "--since",
    "2026-10-04T03:06:51Z",
    "--until",
    "2026-10-04T03:07:07Z",
];
const BASE_SINCE: i64 = 1_791_083_211;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fixtures_dir() -> String {
    format!(
        "{}/../../docs/p0/measurements/fixtures",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn json_file(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{}/{name}", fixtures_dir())).unwrap())
        .unwrap()
}

fn solana_page(name: &str) -> Value {
    let v = json_file(name);
    let mut page = v["pages"][0].clone();
    for tx in page["data"].as_array_mut().unwrap() {
        tx["blockTime"] = json!(BASE_SINCE + 4);
    }
    page["paginationToken"] = Value::Null;
    json!({"jsonrpc": "2.0", "id": 1, "result": page})
}

async fn helius_mock() -> MockServer {
    let sol = solana_page("pumpswap_wallet_page_2026-10-02.json");
    let usdc = solana_page("router_wallet_9oC3_page_2026-10-02.json");
    let s = MockServer::start().await;
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
        .mount(&s)
        .await;
    s
}

async fn robinhood_rpc() -> MockServer {
    let f = json_file("evm_robinhood_token_aiden_v4_2026-10-03.json");
    let replay = EvmFixtureReplay::from_fixture(&f).with_handler(Box::new(|m, _| {
        (m == "eth_call").then(|| ReplayReply::Result(json!(format!("0x{:064x}", 6))))
    }));
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&s)
        .await;
    s
}

async fn robinhood_explorer() -> MockServer {
    let f = json_file("evm_robinhood_token_aiden_v4_2026-10-03.json");
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ExplorerReplay::from_fixture(&f))
        .mount(&s)
        .await;
    s
}

async fn base_rpc() -> MockServer {
    let f = json_file("evm_base_swaps_all_2026-10-04.json");
    let replay = EvmFixtureReplay::from_fixture(&f)
        .with_alchemy_transfers(&f, AlchemyInternalMode::Unsupported)
        .with_handler(Box::new(|m, params| {
            if m == "eth_call" && params[0]["data"] == "0x313ce567" {
                return Some(ReplayReply::Result(json!(format!("0x{:064x}", 6))));
            }
            if m == "eth_getBlockByNumber" {
                let n = i128::from_str_radix(params[0].as_str()?.strip_prefix("0x")?, 16).ok()?;
                if n == 0 {
                    return None;
                }
                let ts = BASE_FIRST_TS + 2 * (n - BASE_FIRST_BLOCK);
                return Some(ReplayReply::Result(
                    json!({"number": format!("{n:#x}"), "timestamp": format!("{ts:#x}")}),
                ));
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

/// `YYYY-MM-DDTHH:MM:SSZ` -> unix seconds.
fn parse_iso(t: &str) -> i64 {
    let n = |a: usize, b: usize| t[a..b].parse::<i64>().unwrap();
    let (y, m, d) = (n(0, 4), n(5, 7), n(8, 10));
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + n(11, 13) * 3600 + n(14, 16) * 60 + n(17, 19)
}

/// Coinbase mock: for any product and requested page it returns one candle per
/// minute at a fixed close (SOL-USD 100, ETH-USD 3000).
async fn coinbase_mock() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(|req: &wiremock::Request| {
            let q: std::collections::HashMap<String, String> = req
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let start = parse_iso(&q["start"]);
            let price = if req.url.path().contains("SOL-USD") {
                100.0
            } else {
                3000.0
            };
            let candles: Vec<Value> = (0..300)
                .map(|i| json!([start + i * 60, price, price, price, price, 10.0]))
                .collect();
            ResponseTemplate::new(200).set_body_json(candles)
        })
        .mount(&s)
        .await;
    s
}

struct Mocks {
    helius: MockServer,
    rh_rpc: MockServer,
    rh_explorer: MockServer,
    base_rpc: MockServer,
    coinbase: MockServer,
}

async fn mocks() -> Mocks {
    Mocks {
        helius: helius_mock().await,
        rh_rpc: robinhood_rpc().await,
        rh_explorer: robinhood_explorer().await,
        base_rpc: base_rpc().await,
        coinbase: coinbase_mock().await,
    }
}

fn env_of(m: &Mocks) -> Vec<(&'static str, String)> {
    vec![
        ("SCOUT_HELIUS_API_KEY", HELIUS_KEY.to_string()),
        ("SCOUT_WALLET_RANK_ENDPOINT", m.helius.uri()),
        (
            "SCOUT_ROBINHOOD_RPC_URL",
            format!("{}/v2/{RH_SECRET}", m.rh_rpc.uri()),
        ),
        ("SCOUT_BLOCKSCOUT_API_KEY", BS_KEY.to_string()),
        ("SCOUT_EVM_BLOCKSCOUT_URL", m.rh_explorer.uri()),
        (
            "SCOUT_BASE_RPC_URL",
            format!("{}/v2/{BASE_SECRET}", m.base_rpc.uri()),
        ),
        ("SCOUT_COINBASE_ENDPOINT", m.coinbase.uri()),
    ]
}

async fn run(env: Vec<(&'static str, String)>, extra: &[&str], input: String) -> Out {
    let mut args: Vec<String> = [
        "--input",
        "-",
        "--rpc-rps",
        "5000",
        "--no-valuation",
        "--profile",
        "none",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    if !extra.contains(&"--format") {
        args.extend(["--format".to_string(), "jsonl".to_string()]);
    }
    args.extend(BASE_WINDOW.iter().map(|s| (*s).to_string()));
    args.extend(extra.iter().map(|s| (*s).to_string()));
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_remove("SCOUT_ROBINHOOD_RPC_URL")
            .env_remove("SCOUT_BASE_RPC_URL")
            .env_remove("SCOUT_BSC_RPC_URL")
            .env_remove("SCOUT_BLOCKSCOUT_API_KEY")
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

fn mixed_input() -> String {
    format!(
        "solana:{SOL_WALLET}\nbase:{BASE_USDC}\nsolana:{USDC_WALLET}\nrobinhood:{RH_W1}\nbase:{BASE_SELL}\n"
    )
}

fn records(o: &Out) -> Vec<Value> {
    o.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn of_kind<'a>(r: &'a [Value], kind: &str) -> Vec<&'a Value> {
    r.iter().filter(|v| v["kind"] == kind).collect()
}

fn no_secret(o: &Out) {
    for s in [HELIUS_KEY, RH_SECRET, BASE_SECRET, BS_KEY] {
        assert!(
            !o.stdout.contains(s) && !o.stderr.contains(s),
            "secret leaked: {s}"
        );
    }
}

fn chain_status<'a>(summary: &'a Value, name: &str) -> &'a Value {
    summary["chains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["chain"] == name)
        .unwrap()
}

#[tokio::test]
async fn usd_default_ranks_all_chains_in_one_list_with_chain_on_every_row() {
    let m = mocks().await;
    // No --quote: mixed input defaults to usd.
    let o = run(env_of(&m), &[], mixed_input()).await;
    no_secret(&o);
    let r = records(&o);
    let meta = &r[0];
    assert_eq!(meta["kind"], "run_meta");
    assert_eq!(meta["multi_chain"], true);
    assert_eq!(meta["quote"], "usd");
    let chains: Vec<&str> = meta["chains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["chain"].as_str().unwrap())
        .collect();
    assert_eq!(chains, ["solana", "base", "robinhood"]);
    assert_eq!(meta["chains"][0]["run_meta"]["rank_quote_unit"], "usd");
    assert_eq!(
        meta["chains"][1]["run_meta"]["scan"]["listing_kind"],
        "alchemy_transfers"
    );
    // ONE ranked list across chains, USD descending, rank 1..n.
    let ranked = of_kind(&r, "wallet_rank");
    assert!(ranked.len() >= 2, "{}", o.stdout);
    let ranks: Vec<u64> = ranked.iter().map(|x| x["rank"].as_u64().unwrap()).collect();
    assert_eq!(ranks, (1..=ranked.len() as u64).collect::<Vec<_>>());
    let pnl: Vec<i128> = ranked
        .iter()
        .map(|x| {
            assert_eq!(x["metrics"]["realized_net_pnl"]["unit"], "usd", "{x}");
            x["metrics"]["realized_net_pnl"]["raw"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert!(pnl.windows(2).all(|w| w[0] >= w[1]), "{pnl:?}");
    // Rows carry their chain; Base is scanned and priced but its fixture
    // window holds no CLOSED episode, so its wallets are `metric_unknown`
    // exclusions (never zero); cross-chain ORDER is covered by the engine test
    // `usd_ranking_across_chains_keeps_the_same_address_on_two_chains_apart`.
    assert!(ranked.iter().all(|x| x["wallet"]["chain"] == "solana"));
    let base_ex: Vec<&Value> = of_kind(&r, "wallet_excluded")
        .into_iter()
        .filter(|e| e["wallet"]["chain"] == "base")
        .collect();
    assert_eq!(base_ex.len(), 2);
    assert!(
        base_ex
            .iter()
            .all(|e| e["primary_reason"] == "metric_unknown")
    );
    // Every input wallet is in the ranking or the exclusions, with its chain.
    let all: Vec<(String, String)> = r
        .iter()
        .filter(|v| v["kind"] == "wallet_rank" || v["kind"] == "wallet_excluded")
        .map(|v| {
            (
                v["wallet"]["chain"].as_str().unwrap().to_string(),
                v["wallet"]["address"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(all.len(), 5, "{all:?}");
    for (c, a) in [
        ("solana", SOL_WALLET),
        ("base", BASE_USDC),
        ("solana", USDC_WALLET),
        ("robinhood", RH_W1),
        ("base", BASE_SELL),
    ] {
        assert!(all.contains(&(c.to_string(), a.to_string())), "{c} {a}");
    }
    // Robinhood's window cannot be resolved here: that chain is down, its
    // wallet is excluded with the reason, the other chains still ranked.
    let ex = of_kind(&r, "wallet_excluded");
    let rh = ex
        .iter()
        .find(|e| e["wallet"]["chain"] == "robinhood")
        .unwrap();
    assert_eq!(rh["primary_reason"], "provider_error");
    assert!(rh["error"].as_str().unwrap().contains("robinhood"));
    // Summary: overall + per chain; exit = partial (3), not 4.
    assert_eq!(o.code, 3, "{}", o.stderr);
    let s = r.last().unwrap();
    assert_eq!(s["kind"], "run_summary");
    assert_eq!(s["multi_chain"], true);
    assert_eq!(s["rank_quote_unit"], "usd");
    assert_eq!(s["input_wallets"], 5);
    assert_eq!(s["exit_code"], 3);
    assert_eq!(s["status"], "partial");
    assert_eq!(chain_status(s, "robinhood")["exit_code"], 4);
    assert_eq!(chain_status(s, "solana")["exit_code"], 0);
    assert_eq!(s["requests_made_by_chain"].as_object().unwrap().len(), 3);
    assert_eq!(s["requests_made_by_chain"]["solana"], 2);
    // USD prices were fetched (counted apart).
    assert!(s["requests_made_prices"].as_u64().unwrap() > 0);
    assert!(!m.helius.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_quote_ranks_per_chain_and_excludes_chains_without_the_unit() {
    let m = mocks().await;
    // usdc exists on Solana and Base, not on Robinhood (USDG there).
    let o = run(env_of(&m), &["--quote", "usdc"], mixed_input()).await;
    no_secret(&o);
    let r = records(&o);
    let ex = of_kind(&r, "wallet_excluded");
    let rh = ex
        .iter()
        .find(|e| e["wallet"]["chain"] == "robinhood")
        .unwrap();
    assert_eq!(rh["primary_reason"], "quote_unit_not_on_chain", "{rh}");
    assert_eq!(rh["scan_status"], "not_scanned");
    assert!(rh["observed"].is_null());
    // Not scanned at all: the Robinhood endpoints saw no request.
    assert!(m.rh_rpc.received_requests().await.unwrap().is_empty());
    assert!(m.rh_explorer.received_requests().await.unwrap().is_empty());
    // Separate rankings: both Solana and Base start at rank 1.
    let ranked = of_kind(&r, "wallet_rank");
    let rank1: Vec<&str> = ranked
        .iter()
        .filter(|x| x["rank"] == 1)
        .map(|x| x["wallet"]["chain"].as_str().unwrap())
        .collect();
    assert!(
        rank1.len() <= 2 && rank1.iter().all(|c| *c == "solana" || *c == "base"),
        "{rank1:?}"
    );
    let s = r.last().unwrap();
    assert_eq!(s["multi_chain"], true);
    assert_eq!(s["rank_quote_unit"], "usdc");
    assert!(s["excluded_by_primary_reason"]["quote_unit_not_on_chain"] == 1);
    assert_eq!(s["input_wallets"], 5);
    let rhs = chain_status(s, "robinhood");
    assert!(
        rhs["exit_code"].is_null(),
        "skipped chains have no exit code"
    );
    assert_eq!(s["requests_made_by_chain"]["robinhood"], 0);
    // No USD prices under a native unit.
    assert_eq!(s["requests_made_prices"], 0);
    assert!(m.coinbase.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn eth_quote_skips_solana_without_scanning_it() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &["--quote", "eth"],
        format!("solana:{SOL_WALLET}\nbase:{BASE_USDC}\n"),
    )
    .await;
    let r = records(&o);
    let ex = of_kind(&r, "wallet_excluded");
    let sol = ex
        .iter()
        .find(|e| e["wallet"]["chain"] == "solana")
        .unwrap();
    assert_eq!(sol["primary_reason"], "quote_unit_not_on_chain");
    assert!(m.helius.received_requests().await.unwrap().is_empty());
    assert!(!m.base_rpc.received_requests().await.unwrap().is_empty());
    assert_eq!(r.last().unwrap()["rank_quote_unit"], "eth");
}

#[tokio::test]
async fn a_quote_unit_on_no_input_chain_is_a_usage_error() {
    let m = mocks().await;
    let o = run(
        env_of(&m),
        &["--quote", "usdg"],
        format!("solana:{SOL_WALLET}\nbase:{BASE_USDC}\n"),
    )
    .await;
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("not a unit of any input chain"));
    assert!(o.stdout.is_empty());
    assert!(m.helius.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn one_chain_missing_its_key_is_excluded_with_the_reason_others_rank() {
    let m = mocks().await;
    let mut env = env_of(&m);
    env.retain(|(k, _)| *k != "SCOUT_HELIUS_API_KEY");
    let o = run(
        env,
        &[],
        format!("solana:{SOL_WALLET}\nbase:{BASE_USDC}\nbase:{BASE_SELL}\n"),
    )
    .await;
    no_secret(&o);
    assert_eq!(o.code, 3, "{}", o.stderr);
    let r = records(&o);
    let ex = of_kind(&r, "wallet_excluded");
    let sol = ex
        .iter()
        .find(|e| e["wallet"]["chain"] == "solana")
        .unwrap();
    assert_eq!(sol["primary_reason"], "provider_error");
    assert!(
        sol["error"]
            .as_str()
            .unwrap()
            .contains("SCOUT_HELIUS_API_KEY")
    );
    let base_rows = r
        .iter()
        .filter(|v| {
            (v["kind"] == "wallet_rank" || v["kind"] == "wallet_excluded")
                && v["wallet"]["chain"] == "base"
        })
        .count();
    assert_eq!(base_rows, 2);
    assert!(
        of_kind(&r, "wallet_excluded")
            .iter()
            .filter(|e| e["wallet"]["chain"] == "base")
            .all(|e| e["primary_reason"] != "provider_error")
    );
    let s = r.last().unwrap();
    assert_eq!(chain_status(s, "solana")["exit_code"], 4);
    assert_eq!(chain_status(s, "base")["exit_code"], 0);
    assert_eq!(s["exit_code"], 3);
}

#[tokio::test]
async fn every_scanned_chain_failing_is_exit_4() {
    let m = mocks().await;
    let o = run(
        vec![("SCOUT_COINBASE_ENDPOINT", m.coinbase.uri())],
        &[],
        format!("solana:{SOL_WALLET}\nbase:{BASE_USDC}\n"),
    )
    .await;
    assert_eq!(o.code, 4, "{}", o.stderr);
    let r = records(&o);
    assert_eq!(of_kind(&r, "wallet_excluded").len(), 2);
    assert_eq!(r.last().unwrap()["exit_code"], 4);
}

#[tokio::test]
async fn table_shows_the_chain_column_and_per_chain_footer() {
    let m = mocks().await;
    let mut env = env_of(&m);
    env.retain(|(k, _)| *k != "SCOUT_HELIUS_API_KEY");
    let o = run(env, &["--format", "table", "--quote", "usd"], mixed_input()).await;
    assert!(o.stdout.contains("chain"), "{}", o.stdout);
    assert!(o.stdout.contains("# chain: solana exit=4"), "{}", o.stdout);
    assert!(o.stdout.contains("# multi-chain exit code"));
}
