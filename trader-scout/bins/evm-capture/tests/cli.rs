//! Offline CLI tests (wiremock); the RPC URL comes from an env var, as in
//! production, and carries a fake key in its path to prove redaction.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Command;

use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_evm-capture");
const SECRET: &str = "SUPERSECRETKEY123";
const TOKEN: &str = "0x00000000000000000000000000000000000000c1";
const WALLET: &str = "0x00000000000000000000000000000000000000aa";
const PM: &str = "0x8366a39cc670b4001a1121b8f6a443a643e40951";
const GENESIS: &str = "0xaad15f3d702aaea00caf3e9bb56395efe9127bc3b31b24921abf1eee3409305c";
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const V4_SWAP: &str = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn run(env: Vec<(&'static str, String)>, args: Vec<String>) -> Out {
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_remove("EVM_TEST_RPC")
            .env_remove("SCOUT_BLOCKSCOUT_API_KEY")
            .env_remove("SCOUT_EVM_CAPTURE_BLOCKSCOUT_URL")
            .env_remove("SCOUT_ROBINHOOD_LOGS_RPC_URL")
            .env_remove("SCOUT_BASE_LOGS_RPC_URL")
            .env_remove("SCOUT_BSC_LOGS_RPC_URL");
        for (k, v) in env {
            cmd.env(k, v);
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

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("evm-capture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().into_owned()
}

fn word(v: u64) -> String {
    format!("0x{v:064x}")
}

fn topic_addr(a: &str) -> String {
    format!("0x000000000000000000000000{}", a.trim_start_matches("0x"))
}

fn receipt(h: &str) -> Value {
    json!({"transactionHash": h, "blockNumber":"0x7",
        "transactionIndex":"0x1","status":"0x1","gasUsed":"0x64","effectiveGasPrice":"0x2",
        "logs":[
          {"address": TOKEN, "topics":[TRANSFER, topic_addr(WALLET), topic_addr(PM)], "data": word(100),
           "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x0"},
          {"address": PM, "topics":[V4_SWAP, word(1), topic_addr(PM)], "data": format!("0x{}", "00".repeat(192)),
           "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x1"}]})
}

/// Chain: block 7 holds one tx (hash 0xa1..) with a token Transfer into the
/// PoolManager and a v4 Swap at the gated PoolManager.
struct Chain {
    chain_id: &'static str,
}

impl Respond for Chain {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let h = format!("0x{}", "a1".repeat(32));
        let result = match body["method"].as_str().unwrap() {
            "eth_chainId" => json!(self.chain_id),
            "eth_blockNumber" => json!("0x10"),
            "eth_getBlockByNumber" if body["params"][0] == "0x0" => {
                json!({"hash": GENESIS, "timestamp": "0x3e8"})
            }
            "eth_getBlockByNumber" => json!({"timestamp": "0x3e8"}),
            "eth_getLogs" => {
                json!([{"address": TOKEN, "topics":[TRANSFER, topic_addr(WALLET), topic_addr(PM)],
                "data": word(100), "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x0"}])
            }
            "eth_getBlockReceipts" => json!([receipt(&h)]),
            // `--receipts auto` (default): the block's only tx, one by one.
            "eth_getTransactionReceipt" => receipt(&h),
            "eth_getTransactionByHash" => {
                json!({"hash": h, "from": WALLET, "to": PM, "value":"0x0",
                "blockNumber":"0x7","transactionIndex":"0x1"})
            }
            other => panic!("unexpected {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }
}

fn base_args(extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = ["--chain", "robinhood", "--rpc-url-env", "EVM_TEST_RPC"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    a.extend(extra.iter().map(|s| (*s).to_string()));
    a
}

fn rpc_env(server: &MockServer) -> Vec<(&'static str, String)> {
    vec![("EVM_TEST_RPC", format!("{}/v2/{SECRET}", server.uri()))]
}

#[tokio::test]
async fn argument_errors_exit_2_and_missing_env_exits_4() {
    let none = run(vec![], base_args(&["--token", TOKEN])).await;
    assert_eq!(none.code, 2, "{}", none.stderr); // no window
    let both = run(
        vec![],
        base_args(&[
            "--token",
            TOKEN,
            "--wallet",
            WALLET,
            "--from-block",
            "1",
            "--to-block",
            "2",
        ]),
    )
    .await;
    assert_eq!(both.code, 2);
    let bad = run(
        vec![],
        base_args(&["--token", "nothex", "--from-block", "1", "--to-block", "2"]),
    )
    .await;
    assert_eq!(bad.code, 2);
    let env = run(
        vec![],
        base_args(&["--token", TOKEN, "--from-block", "1", "--to-block", "2"]),
    )
    .await;
    assert_eq!(env.code, 4);
    assert!(env.stderr.contains("EVM_TEST_RPC"));
}

#[tokio::test]
async fn token_capture_summarizes_writes_redacted_fixture() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Chain { chain_id: "0x1237" })
        .mount(&s)
        .await;
    let out_path = tmp("token.json");
    let out = run(
        rpc_env(&s),
        base_args(&[
            "--token",
            TOKEN,
            "--from-block",
            "0",
            "--to-block",
            "16",
            "--out",
            &out_path,
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains(SECRET), "{text}");
    }
    assert!(out.stdout.contains("preflight=ok"));
    assert!(
        out.stdout.contains("transactions=1 logs=2"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("token_transfer_logs=1"));
    assert!(
        out.stdout
            .contains(&format!("v4_swap {PM} count=1 gate=gated")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("no_trade no_quote_leg=1") || out.stdout.contains("trades="),
        "{}",
        out.stdout
    );

    let fixture = std::fs::read_to_string(&out_path).unwrap();
    assert!(!fixture.contains(SECRET) && !fixture.contains("127.0.0.1"));
    let v: Value = serde_json::from_str(&fixture).unwrap();
    assert_eq!(v["chain_id"], 4663);
    assert_eq!(v["incomplete"], false);
    let methods: Vec<&str> = v["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["method"].as_str().unwrap())
        .collect();
    for m in [
        "eth_chainId",
        "eth_getLogs",
        "eth_getBlockReceipts",
        "eth_getTransactionByHash",
    ] {
        assert!(methods.contains(&m), "{m} missing in {methods:?}");
    }
}

#[tokio::test]
async fn time_window_resolves_through_block_times() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Chain { chain_id: "0x1237" })
        .mount(&s)
        .await;
    // Every block has timestamp 1000: window [0, 2000) covers 0..=0x10.
    let out = run(
        rpc_env(&s),
        base_args(&[
            "--token",
            TOKEN,
            "--since",
            "1970-01-01T00:00:00Z",
            "--until",
            "1970-01-01T00:33:20Z",
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("window_blocks=0..=16"),
        "{}",
        out.stdout
    );
}

#[tokio::test]
async fn chain_id_mismatch_aborts_with_exit_4() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Chain { chain_id: "0x2105" })
        .mount(&s)
        .await;
    let out = run(
        rpc_env(&s),
        base_args(&["--token", TOKEN, "--from-block", "0", "--to-block", "16"]),
    )
    .await;
    assert_eq!(out.code, 4);
    assert!(out.stderr.contains("chain id mismatch"), "{}", out.stderr);
    assert!(!out.stderr.contains(SECRET));
    assert!(out.stdout.is_empty());
}

#[tokio::test]
async fn exhausted_budget_exits_3_and_marks_fixture_incomplete() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Chain { chain_id: "0x1237" })
        .mount(&s)
        .await;
    let out_path = tmp("budget.json");
    let out = run(
        rpc_env(&s),
        base_args(&[
            "--token",
            TOKEN,
            "--from-block",
            "0",
            "--to-block",
            "16",
            "--max-requests",
            "3",
            "--out",
            &out_path,
        ]),
    )
    .await;
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(out.stderr.contains("INCOMPLETE"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(v["incomplete"], true);
}

#[tokio::test]
async fn wallet_mode_uses_blockscout_and_never_leaks_its_key() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Chain { chain_id: "0x1237" })
        .mount(&rpc)
        .await;
    let bs = MockServer::start().await;
    let h = format!("0x{}", "a1".repeat(32));
    Mock::given(method("GET"))
        .respond_with(|req: &Request| {
            let action = req.url.query_pairs().find(|(k, _)| k == "action").map(|(_, v)| v.into_owned()).unwrap();
            let body = if action == "txlist" {
                json!({"status":"1","message":"OK","result":[{"hash":format!("0x{}", "a1".repeat(32)),
                    "blockNumber":"7","timeStamp":"1","from":WALLET,"to":PM,"value":"0","gasUsed":"100",
                    "gasPrice":"2","isError":"0"}]})
            } else {
                json!({"status":"2","message":"internal transactions not yet processed","result":[]})
            };
            ResponseTemplate::new(200).set_body_json(body)
        })
        .mount(&bs)
        .await;
    let _ = h;
    let mut env = rpc_env(&rpc);
    env.push(("SCOUT_BLOCKSCOUT_API_KEY", "BLOCKSCOUTKEY999".to_string()));
    env.push(("SCOUT_EVM_CAPTURE_BLOCKSCOUT_URL", bs.uri()));
    let out = run(
        env,
        base_args(&["--wallet", WALLET, "--from-block", "0", "--to-block", "16"]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout
            .contains("txlist_complete=true internal_transfers_complete=false"),
        "{}",
        out.stdout
    );
    assert!(!out.stdout.contains("BLOCKSCOUTKEY999") && !out.stderr.contains("BLOCKSCOUTKEY999"));
}

// ---- swap-topic mode ------------------------------------------------------

const V3_SWAP: &str = "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67";
const V3_FACTORY: &str = "0x1f7d7550b1b028f7571e69a784071f0205fd2efa";
const T0: &str = "0x0000000000000000000000000000000000000011";
const T1: &str = "0x0000000000000000000000000000000000000022";
/// An emitter with the v3 topic that is no pool (its calls revert).
const FAKE: &str = "0x00000000000000000000000000000000000000fa";

fn v3_pool() -> String {
    let p = scout_dex_evm::v3_pool_address_create2(
        V3_FACTORY.parse().unwrap(),
        T0.parse().unwrap(),
        T1.parse().unwrap(),
        500,
        scout_dex_evm::UNISWAP_V3_CANONICAL_INIT_CODE_HASH,
    );
    format!("{p:#x}")
}

/// Block 7, tx 0xa1.. at index 1 holds two v3-topic swaps: one at the real
/// pool, one at `FAKE`. `eth_call` answers by selector.
struct SwapChain;

impl SwapChain {
    fn swap_log(addr: &str, log_index: u64) -> Value {
        json!({"address": addr, "topics":[V3_SWAP, topic_addr(PM), topic_addr(WALLET)],
            "data": format!("0x{}", "00".repeat(160)), "blockNumber":"0x7",
            "transactionIndex":"0x1","logIndex":format!("{log_index:#x}")})
    }
}

impl Respond for SwapChain {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let h = format!("0x{}", "a1".repeat(32));
        let pool = v3_pool();
        let result = match body["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x1237"),
            "eth_blockNumber" => json!("0x10"),
            "eth_getBlockByNumber" if body["params"][0] == "0x0" => {
                json!({"hash": GENESIS, "timestamp": "0x3e8"})
            }
            "eth_getBlockByNumber" => json!({"timestamp": "0x3e8"}),
            "eth_getLogs" => {
                if body["params"][0].get("address").is_some() {
                    json!([{"address": PM, "topics":[V4_SWAP, word(1), topic_addr(PM)],
                        "data": format!("0x{}", "00".repeat(192)), "blockNumber":"0x7",
                        "transactionIndex":"0x1","logIndex":"0x2"}])
                } else {
                    json!([Self::swap_log(&pool, 0), Self::swap_log(FAKE, 1)])
                }
            }
            "eth_getBlockReceipts" => json!([{"transactionHash": h, "blockNumber":"0x7",
                "transactionIndex":"0x1","status":"0x1","gasUsed":"0x64","effectiveGasPrice":"0x2",
                "logs":[Self::swap_log(&pool, 0), Self::swap_log(FAKE, 1)]}]),
            "eth_getTransactionByHash" => json!({"hash": h, "from": WALLET, "to": PM,
                "value":"0x0", "blockNumber":"0x7","transactionIndex":"0x1"}),
            "eth_call" => {
                let to = body["params"][0]["to"]
                    .as_str()
                    .unwrap()
                    .to_ascii_lowercase();
                let data = body["params"][0]["data"].as_str().unwrap().to_string();
                if to == FAKE {
                    return ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0",
                        "id":1,"error":{"code":3,"message":"execution reverted"}}));
                }
                let addr_word = |a: &str| format!("0x{:0>64}", a.trim_start_matches("0x"));
                match &data[..10] {
                    "0xc45a0155" => json!(addr_word(V3_FACTORY)),
                    "0x0dfe1681" => json!(addr_word(T0)),
                    "0xd21220a7" => json!(addr_word(T1)),
                    "0xddca3f43" => json!(word(500)),
                    "0x1698ee82" => json!(addr_word(&pool)),
                    other => panic!("unexpected selector {other}"),
                }
            }
            other => panic!("unexpected {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }
}

#[tokio::test]
async fn swaps_flag_is_exclusive_with_token_and_wallet() {
    for other in [["--token", TOKEN], ["--wallet", WALLET]] {
        let out = run(
            vec![],
            base_args(&[
                "--swaps",
                "uniswap-v3",
                other[0],
                other[1],
                "--from-block",
                "1",
                "--to-block",
                "2",
            ]),
        )
        .await;
        assert_eq!(out.code, 2, "{}", out.stderr);
    }
    let bad = run(
        vec![],
        base_args(&[
            "--swaps",
            "uniswap-v9",
            "--from-block",
            "1",
            "--to-block",
            "2",
        ]),
    )
    .await;
    assert_eq!(bad.code, 2);
}

#[tokio::test]
async fn swap_scan_records_pool_metadata_and_prints_factories() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SwapChain)
        .mount(&s)
        .await;
    let out_path = tmp("swaps.json");
    let out = run(
        rpc_env(&s),
        base_args(&[
            "--swaps",
            "uniswap-v3",
            "--from-block",
            "0",
            "--to-block",
            "16",
            "--out",
            &out_path,
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains(SECRET), "{text}");
    }
    let pool = v3_pool();
    assert!(
        out.stdout.contains("transactions=1 logs=2"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("swap_scan: swap_logs=2 txs_before_cap=1 txs_kept=1"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains(&format!("v3_swap {pool} count=1 gate=gated")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains(&format!("v3_swap {FAKE} count=1 gate=ungated")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains(&format!("factory {V3_FACTORY} pools=1")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("admitted=1 refused=1"),
        "{}",
        out.stdout
    );

    // The v3 scan has no address filter and asks topic0 = v3 Swap only.
    let reqs = s.received_requests().await.unwrap();
    let logs: Vec<Value> = reqs
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["method"] == "eth_getLogs")
        .collect();
    assert_eq!(logs.len(), 1);
    assert!(logs[0]["params"][0].get("address").is_none());
    assert_eq!(logs[0]["params"][0]["topics"], json!([V3_SWAP]));

    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(v["scope"]["swaps"], "uniswap-v3");
    assert_eq!(v["incomplete"], false);
    let rows = v["pool_metadata"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let real = rows.iter().find(|r| r["emitter"] == pool.as_str()).unwrap();
    assert_eq!(real["kind"], "v3");
    assert_eq!(real["factory"], V3_FACTORY);
    assert_eq!(real["fee"], 500);
    assert_eq!(real["registered_pool"], pool.as_str());
    assert_eq!(real["block"], "0x10");
    let fake = rows.iter().find(|r| r["emitter"] == FAKE).unwrap();
    assert!(fake["factory"].is_null() && fake["fee"].is_null());
    // The eth_calls are in `calls` (a revert as a marker) so a replay can
    // answer them offline.
    let calls = v["calls"].as_array().unwrap();
    assert!(
        calls
            .iter()
            .any(|c| c["method"] == "eth_call" && c["result"] == json!({"reverted": true}))
    );
    assert!(
        calls
            .iter()
            .any(|c| c["method"] == "eth_call" && c["result"] == word(500))
    );
}

#[tokio::test]
async fn swaps_all_adds_the_pool_manager_scan_and_max_pools_bounds_lookups() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SwapChain)
        .mount(&s)
        .await;
    let out = run(
        rpc_env(&s),
        base_args(&[
            "--swaps",
            "all",
            "--max-pools",
            "1",
            "--from-block",
            "0",
            "--to-block",
            "16",
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("swap_logs=3"), "{}", out.stdout);
    assert!(
        out.stdout
            .contains("emitters_read=1 skipped_over_max_pools=1"),
        "{}",
        out.stdout
    );
    let reqs = s.received_requests().await.unwrap();
    let logs: Vec<Value> = reqs
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["method"] == "eth_getLogs")
        .collect();
    assert_eq!(logs.len(), 2);
    assert!(logs.iter().any(|l| l["params"][0]["address"] == PM));
}

/// Answers `status` (with an optional `Retry-After`) to the first `n`
/// requests, then delegates to the chain.
struct Flaky {
    left: std::sync::atomic::AtomicUsize,
    status: u16,
    retry_after: Option<&'static str>,
    inner: Chain,
}

impl Respond for Flaky {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        use std::sync::atomic::Ordering;
        if self
            .left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            let t = ResponseTemplate::new(self.status);
            return match self.retry_after {
                Some(v) => t.insert_header("Retry-After", v),
                None => t,
            };
        }
        self.inner.respond(req)
    }
}

fn token_args(extra: &[&str]) -> Vec<String> {
    let mut a = vec!["--token", TOKEN, "--from-block", "0", "--to-block", "16"];
    a.extend_from_slice(extra);
    base_args(&a)
}

#[tokio::test]
async fn a_429_backs_off_halves_the_rate_and_the_capture_continues() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Flaky {
            left: std::sync::atomic::AtomicUsize::new(2),
            status: 429,
            retry_after: None,
            inner: Chain { chain_id: "0x1237" },
        })
        .mount(&s)
        .await;
    let out = run(rpc_env(&s), token_args(&[])).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("transactions=1 logs=2"),
        "{}",
        out.stdout
    );
    assert!(out.stderr.contains("client rate halved"), "{}", out.stderr);
    let line = out
        .stderr
        .lines()
        .find(|l| l.contains("rate limit: rpc:"))
        .unwrap_or_else(|| panic!("no rate limit line: {}", out.stderr));
    assert!(!line.contains("(0 halving(s)"), "{line}");
    assert!(!out.stderr.contains(SECRET) && !out.stdout.contains(SECRET));
}

#[tokio::test]
async fn a_429_that_asks_for_too_long_a_wait_stops_incomplete_not_failed() {
    // Every request (the preflight included) is a 429 asking for an hour.
    let s2 = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Flaky {
            left: std::sync::atomic::AtomicUsize::new(usize::MAX),
            status: 429,
            retry_after: Some("3600"),
            inner: Chain { chain_id: "0x1237" },
        })
        .mount(&s2)
        .await;
    let out_path = tmp("ratelimited.json");
    let out = run(rpc_env(&s2), token_args(&["--out", &out_path])).await;
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(out.stderr.contains("INCOMPLETE") && out.stderr.contains("429"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(v["incomplete"], true);
    assert!(out.stderr.contains("rate limit: rpc:"), "{}", out.stderr);
}

#[tokio::test]
async fn the_limiter_paces_every_attempt_and_is_reported() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Chain { chain_id: "0x1237" })
        .mount(&s)
        .await;
    let started = std::time::Instant::now();
    let out = run(rpc_env(&s), token_args(&["--rpc-rps", "4"])).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let requests = s.received_requests().await.unwrap().len();
    assert!(requests > 4, "scenario must exceed the burst: {requests}");
    let line = out
        .stderr
        .lines()
        .find(|l| l.contains("rate limit: rpc:"))
        .unwrap_or_else(|| panic!("{}", out.stderr));
    assert!(
        line.contains("rate 4.000/s -> 4.000/s (0 halving(s)"),
        "{line}"
    );
    assert!(
        line.contains(&format!("{requests} request(s) paced")),
        "{line} vs {requests}"
    );
    assert!(!line.contains(" 0 ms spent"), "{line}");
    // 4 burst tokens, then 4/s: the rest took real time.
    let min_ms = u128::try_from(requests - 4).unwrap() * 250; // 1000 ms / 4 per s
    assert!(started.elapsed().as_millis() + 250 >= min_ms);
}

#[tokio::test]
async fn cu_budget_conflicts_with_rps_and_logs_route_to_the_logs_endpoint() {
    let both = run(
        vec![],
        token_args(&["--rpc-rps", "5", "--rpc-cu-per-sec", "300"]),
    )
    .await;
    assert_eq!(both.code, 2, "{}", both.stderr);

    let main = MockServer::start().await;
    let logs = MockServer::start().await;
    for srv in [&main, &logs] {
        Mock::given(method("POST"))
            .respond_with(Chain { chain_id: "0x1237" })
            .mount(srv)
            .await;
    }
    const LOGS_SECRET: &str = "LOGSSECRETKEY999";
    let mut env = rpc_env(&main);
    env.push((
        "SCOUT_ROBINHOOD_LOGS_RPC_URL",
        format!("{}/v2/{LOGS_SECRET}", logs.uri()),
    ));
    let out = run(env, token_args(&["--rpc-cu-per-sec", "300"])).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let methods = |reqs: Vec<Request>| -> Vec<String> {
        reqs.iter()
            .map(|r| {
                serde_json::from_slice::<Value>(&r.body).unwrap()["method"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    let on_logs = methods(logs.received_requests().await.unwrap());
    let on_main = methods(main.received_requests().await.unwrap());
    assert!(!on_logs.is_empty() && on_logs.iter().all(|m| m == "eth_getLogs"));
    assert!(!on_main.iter().any(|m| m == "eth_getLogs"));
    assert!(on_main.iter().any(|m| m == "eth_getBlockReceipts"));
    assert!(out.stderr.contains("rate limit: rpc:"), "{}", out.stderr);
    assert!(
        out.stderr.contains("rate limit: logs rpc:"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("300.000/s"), "{}", out.stderr);
    for text in [&out.stdout, &out.stderr] {
        assert!(
            !text.contains(LOGS_SECRET) && !text.contains(SECRET),
            "{text}"
        );
    }
}

// ---- Base venues (Aerodrome v2 / Slipstream) ------------------------------

const BASE_GENESIS: &str = "0xf712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd";
const AERO_SWAP: &str = "0xb3e2773606abfd36b5bd91394b3a54d1398336c65005baf7bf7a05efeffaf75b";
const AERO_FACTORY: &str = "0x420dd381b31aef6683db6b902084cb0ffece40da";
const SLIP_FACTORY: &str = "0x5e7bb104d84c7cb9b682aac2f3d509f5f406809a";
const AERO_POOL: &str = "0x00000000000000000000000000000000000000a1";
const SLIP_POOL: &str = "0x00000000000000000000000000000000000000b1";

struct BaseVenues;

impl BaseVenues {
    fn logs() -> Vec<Value> {
        vec![
            json!({"address": SLIP_POOL, "topics":[V3_SWAP, topic_addr(PM), topic_addr(WALLET)],
                "data": format!("0x{}", "00".repeat(160)), "blockNumber":"0x7",
                "transactionIndex":"0x1","logIndex":"0x0"}),
            json!({"address": AERO_POOL, "topics":[AERO_SWAP, topic_addr(PM), topic_addr(WALLET)],
                "data": format!("0x{}", "00".repeat(128)), "blockNumber":"0x7",
                "transactionIndex":"0x1","logIndex":"0x1"}),
        ]
    }
}

impl Respond for BaseVenues {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let h = format!("0x{}", "a1".repeat(32));
        let result = match body["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x2105"),
            "eth_blockNumber" => json!("0x10"),
            "eth_getBlockByNumber" if body["params"][0] == "0x0" => {
                json!({"hash": BASE_GENESIS, "timestamp": "0x0"})
            }
            "eth_getBlockByNumber" => json!({"timestamp": "0x3e8"}),
            "eth_getLogs" => {
                if body["params"][0].get("address").is_some() {
                    json!([])
                } else {
                    json!(Self::logs())
                }
            }
            "eth_getBlockReceipts" => json!([{"transactionHash": h, "blockNumber":"0x7",
                "transactionIndex":"0x1","status":"0x1","gasUsed":"0x64",
                "effectiveGasPrice":"0x2","l1Fee":"0x5","logs": Self::logs()}]),
            "eth_getTransactionByHash" => json!({"hash": h, "from": WALLET, "to": PM,
                "value":"0x0", "blockNumber":"0x7","transactionIndex":"0x1"}),
            "eth_call" => {
                let to = body["params"][0]["to"]
                    .as_str()
                    .unwrap()
                    .to_ascii_lowercase();
                let data = body["params"][0]["data"].as_str().unwrap().to_string();
                let addr_word = |a: &str| format!("0x{:0>64}", a.trim_start_matches("0x"));
                let (factory, pool) = if to == AERO_POOL {
                    (AERO_FACTORY, AERO_POOL)
                } else if to == SLIP_POOL {
                    (SLIP_FACTORY, SLIP_POOL)
                } else if to == AERO_FACTORY || to == SLIP_FACTORY {
                    // factory record: getPool(bool) / getPool(int24)
                    let pool = if to == AERO_FACTORY {
                        AERO_POOL
                    } else {
                        SLIP_POOL
                    };
                    match &data[..10] {
                        "0x79bc57d5" if to == AERO_FACTORY => {
                            assert!(
                                data.ends_with(&format!("{:064x}", 1)),
                                "stable=true: {data}"
                            );
                            return ResponseTemplate::new(200).set_body_json(
                                json!({"jsonrpc":"2.0","id":1,"result":addr_word(pool)}),
                            );
                        }
                        "0x28af8d0b" if to == SLIP_FACTORY => {
                            assert!(data.ends_with(&format!("{:064x}", 100)), "spacing: {data}");
                            return ResponseTemplate::new(200).set_body_json(
                                json!({"jsonrpc":"2.0","id":1,"result":addr_word(pool)}),
                            );
                        }
                        other => panic!("unexpected factory selector {other} at {to}"),
                    }
                } else {
                    panic!("unexpected eth_call target {to}");
                };
                let _ = pool;
                match &data[..10] {
                    "0xc45a0155" => json!(addr_word(factory)),
                    "0x0dfe1681" => json!(addr_word(T0)),
                    "0xd21220a7" => json!(addr_word(T1)),
                    "0x22be3de1" if to == AERO_POOL => json!(word(1)),
                    "0xd0c93a7c" if to == SLIP_POOL => json!(word(100)),
                    // a Slipstream pool has no `fee()` of its own here
                    "0xddca3f43" => {
                        return ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0",
                            "id":1,"error":{"code":3,"message":"execution reverted"}}));
                    }
                    other => panic!("unexpected pool selector {other} at {to}"),
                }
            }
            other => panic!("unexpected {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }
}

#[tokio::test]
async fn base_capture_reads_aerodrome_stable_and_slipstream_tick_spacing() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(BaseVenues)
        .mount(&s)
        .await;
    let out_path = tmp("base_venues.json");
    let args: Vec<String> = [
        "--chain",
        "base",
        "--rpc-url-env",
        "EVM_TEST_RPC",
        "--swaps",
        "all",
        "--from-block",
        "0",
        "--to-block",
        "16",
        "--out",
        &out_path,
    ]
    .iter()
    .map(|a| (*a).to_string())
    .collect();
    let out = run(rpc_env(&s), args).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("chain=base chain_id=8453"),
        "{}",
        out.stdout
    );
    // Both pools are admitted by their pinned factories' own records.
    assert!(
        out.stdout.contains("admitted=2 refused=0"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("stable=true"), "{}", out.stdout);
    assert!(out.stdout.contains("tick_spacing=100"), "{}", out.stdout);
    assert!(
        out.stdout
            .contains(&format!("aerodrome_v2_swap {AERO_POOL} count=1 gate=gated")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains(&format!("v3_swap {SLIP_POOL} count=1 gate=gated")),
        "{}",
        out.stdout
    );
    // `all` asks v2 + v3 + Aerodrome topics in ONE address-less filter.
    let reqs = s.received_requests().await.unwrap();
    let first_logs = reqs
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .find(|b| b["method"] == "eth_getLogs" && b["params"][0].get("address").is_none())
        .unwrap();
    let topics = first_logs["params"][0]["topics"][0].as_array().unwrap();
    assert!(topics.iter().any(|t| t == AERO_SWAP) && topics.iter().any(|t| t == V3_SWAP));

    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    let rows = v["pool_metadata"].as_array().unwrap();
    let aero = rows.iter().find(|r| r["emitter"] == AERO_POOL).unwrap();
    assert_eq!(aero["kind"], "aerodrome_v2");
    assert_eq!(aero["stable"], true);
    assert_eq!(aero["factory"], AERO_FACTORY);
    assert_eq!(aero["registered_pool"], AERO_POOL);
    let slip = rows.iter().find(|r| r["emitter"] == SLIP_POOL).unwrap();
    assert_eq!(slip["kind"], "slipstream");
    assert_eq!(slip["tick_spacing"], 100);
    assert!(slip["fee"].is_null());
    assert_eq!(slip["registered_pool"], SLIP_POOL);
}

// --- BSC: Pancake v3 and four.meme swap scans (chain id 56).

const BSC_GENESIS: &str = "0x0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b";
const FM_V1: &str = "0xec4549cadce5da21df6e6422d448034b5233bfbc";
const FM_V2: &str = "0x5c952063c7fc8610ffdb798152d69f0b9550762b";
const FM_V2_PURCHASE: &str = "0x7db52723a3b2cdd6164364b3b766e65e540d7be48ffa89582956d8eaebe62942";
const PANCAKE_V3_SWAP: &str = "0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83";
const PANCAKE_V3_FACTORY: &str = "0x0bfbcf9fa4f9c56b0f40a671ad40e0805a091865";
const PANCAKE_V3_DEPLOYER: &str = "0x41ff9aa7e16b8b1a8a8dc4f0efacd93d02d071c9";
const FM_TOKEN: &str = "0x00000000000000000000000000000000000000c1";

fn bsc_args(extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = ["--chain", "bsc", "--rpc-url-env", "EVM_TEST_RPC"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    a.extend(extra.iter().map(|s| (*s).to_string()));
    a
}

fn fm_v2_event(account: &str) -> Value {
    // TokenPurchase(token, account, price, amount, cost, fee, offers, funds)
    let w = |a: &str| format!("{:0>64}", a.trim_start_matches("0x"));
    let data = format!("0x{}{}{}", w(FM_TOKEN), w(account), "00".repeat(32 * 6));
    json!({"address": FM_V2, "topics":[FM_V2_PURCHASE], "data": data,
        "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x1"})
}

/// BSC block 7, tx 0xa1.. (from WALLET, value 5) with a token Transfer from
/// the four.meme V2 manager to WALLET plus the manager's purchase event, and a
/// Pancake v3 pool swap in the same tx.
struct BscChain {
    pancake_pool: String,
}

impl Respond for BscChain {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let h = format!("0x{}", "a1".repeat(32));
        let pancake_log = json!({"address": self.pancake_pool,
            "topics":[PANCAKE_V3_SWAP, topic_addr(PM), topic_addr(WALLET)],
            "data": format!("0x{}", "00".repeat(224)), "blockNumber":"0x7",
            "transactionIndex":"0x1","logIndex":"0x2"});
        let result = match body["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x38"),
            "eth_blockNumber" => json!("0x10"),
            "eth_getBlockByNumber" if body["params"][0] == "0x0" => {
                json!({"hash": BSC_GENESIS, "timestamp": "0x3e8"})
            }
            "eth_getBlockByNumber" => json!({"timestamp": "0x3e8"}),
            "eth_getLogs" => {
                let f = &body["params"][0];
                let t0 = &f["topics"][0];
                let asks_pancake = t0 == PANCAKE_V3_SWAP
                    || t0
                        .as_array()
                        .is_some_and(|a| a.contains(&json!(PANCAKE_V3_SWAP)));
                if asks_pancake && f.get("address").is_none() {
                    json!([pancake_log])
                } else {
                    json!([fm_v2_event(WALLET)])
                }
            }
            "eth_getBlockReceipts" => json!([{"transactionHash": h, "blockNumber":"0x7",
                "transactionIndex":"0x1","status":"0x1","gasUsed":"0x64","effectiveGasPrice":"0x2",
                "logs":[
                  {"address": FM_TOKEN, "topics":[TRANSFER, topic_addr(FM_V2), topic_addr(WALLET)],
                   "data": word(1000), "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x0"},
                  fm_v2_event(WALLET), pancake_log]}]),
            "eth_getTransactionByHash" => json!({"hash": h, "from": WALLET, "to": FM_V2,
                "value":"0x5", "blockNumber":"0x7","transactionIndex":"0x1"}),
            "eth_call" => {
                let data = body["params"][0]["data"].as_str().unwrap().to_string();
                let addr_word = |a: &str| format!("0x{:0>64}", a.trim_start_matches("0x"));
                match &data[..10] {
                    "0xc45a0155" => json!(addr_word(PANCAKE_V3_FACTORY)),
                    "0x0dfe1681" => json!(addr_word(T0)),
                    "0xd21220a7" => json!(addr_word(T1)),
                    "0xddca3f43" => json!(word(2500)),
                    "0x1698ee82" => json!(addr_word(&self.pancake_pool)),
                    other => panic!("unexpected selector {other}"),
                }
            }
            other => panic!("unexpected {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }
}

fn pancake_pool() -> String {
    let p = scout_dex_evm::v3_pool_address_create2(
        PANCAKE_V3_DEPLOYER.parse().unwrap(),
        T0.parse().unwrap(),
        T1.parse().unwrap(),
        2500,
        scout_dex_evm::PANCAKE_V3_INIT_CODE_HASH,
    );
    format!("{p:#x}")
}

async fn get_logs_filters(s: &MockServer) -> Vec<Value> {
    s.received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["method"] == "eth_getLogs")
        .map(|b| b["params"][0].clone())
        .collect()
}

#[tokio::test]
async fn bsc_fourmeme_scan_filters_by_manager_and_topics_and_reports_account_vs_signer() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(BscChain {
            pancake_pool: pancake_pool(),
        })
        .mount(&s)
        .await;
    let out_path = tmp("bsc_fourmeme.json");
    let out = run(
        rpc_env(&s),
        bsc_args(&[
            "--swaps",
            "fourmeme",
            "--from-block",
            "0",
            "--to-block",
            "16",
            "--out",
            &out_path,
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains(SECRET), "{text}");
    }
    let filters = get_logs_filters(&s).await;
    assert_eq!(filters.len(), 1, "{filters:?}");
    assert_eq!(filters[0]["address"], json!([FM_V1, FM_V2]));
    let topics = filters[0]["topics"][0].as_array().unwrap();
    assert_eq!(topics.len(), 4);
    assert!(topics.contains(&json!(FM_V2_PURCHASE)));
    assert!(
        out.stdout
            .contains(&format!("fourmeme_v2_purchase {FM_V2} count=1 gate=gated")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("launchpad: gated_events=1 account_is_tx_from=1 account_other=0"),
        "{}",
        out.stdout
    );
    // Booked from the wallet's own deltas (token in, tx.value out).
    assert!(out.stdout.contains("trades=1"), "{}", out.stdout);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(v["scope"]["swaps"], "fourmeme");
    assert_eq!(v["chain_id"], 56);
    // The managers are singletons: no metadata row for them (the only row is
    // the Pancake pool that happens to sit in the same transaction).
    assert!(
        v["pool_metadata"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["emitter"] != FM_V2)
    );
}

#[tokio::test]
async fn bsc_pancake_v3_scan_uses_its_own_topic_and_records_pool_metadata() {
    let s = MockServer::start().await;
    let pool = pancake_pool();
    Mock::given(method("POST"))
        .respond_with(BscChain {
            pancake_pool: pool.clone(),
        })
        .mount(&s)
        .await;
    let out_path = tmp("bsc_pancake_v3.json");
    let out = run(
        rpc_env(&s),
        bsc_args(&[
            "--swaps",
            "pancake-v3",
            "--from-block",
            "0",
            "--to-block",
            "16",
            "--out",
            &out_path,
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let filters = get_logs_filters(&s).await;
    assert_eq!(filters.len(), 1);
    assert!(filters[0].get("address").is_none());
    // A single topic0 is sent as a plain string.
    assert_eq!(filters[0]["topics"][0], PANCAKE_V3_SWAP);
    assert!(
        out.stdout
            .contains(&format!("pancake_v3_swap {pool} count=1 gate=gated")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains(&format!("factory {PANCAKE_V3_FACTORY} pools=1")),
        "{}",
        out.stdout
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    let rows = v["pool_metadata"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "pancake_v3");
    assert_eq!(rows[0]["fee"], 2500);
    assert_eq!(rows[0]["factory"], PANCAKE_V3_FACTORY);
    assert_eq!(rows[0]["registered_pool"], pool.as_str());
}

#[tokio::test]
async fn swaps_all_on_bsc_includes_pancake_and_fourmeme_and_fourmeme_needs_a_pinned_manager() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(BscChain {
            pancake_pool: pancake_pool(),
        })
        .mount(&s)
        .await;
    let out = run(
        rpc_env(&s),
        bsc_args(&["--swaps", "all", "--from-block", "0", "--to-block", "16"]),
    )
    .await;
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let filters = get_logs_filters(&s).await;
    // v2/v3/aerodrome/pancake topics (no address), the v4 manager, four.meme.
    assert_eq!(filters.len(), 3, "{filters:?}");
    let topic_sets: Vec<&Value> = filters.iter().map(|f| &f["topics"][0]).collect();
    assert!(topic_sets.iter().any(|t| {
        t.as_array()
            .is_some_and(|a| a.contains(&json!(PANCAKE_V3_SWAP)))
    }));
    assert!(
        filters
            .iter()
            .any(|f| f["address"] == json!([FM_V1, FM_V2]))
    );
    // Robinhood pins no four.meme manager: asking for it is a typed error.
    let rh = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SwapChain)
        .mount(&rh)
        .await;
    let out = run(
        rpc_env(&rh),
        base_args(&[
            "--swaps",
            "fourmeme",
            "--from-block",
            "0",
            "--to-block",
            "16",
        ]),
    )
    .await;
    assert_eq!(out.code, 4, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("four.meme TokenManager"),
        "{}",
        out.stderr
    );
}

const PONS_FACTORY: &str = "0x7ed598bcef8bd9edd8c97a195c6d13f40801ec7e";
const PONS_CURVE: &str = "0x00000000000000000000000000000000000000e1";
const PONS_BUY: &str = "0xec36bf571f136799e8dc0b0b8bea4b04d8bd3d43de838aab0d5fc21d4cbfc455";
const PONS_SELL: &str = "0x8113d738abdcb6b38357e9d53a54a7157861a09031b453651f0fe7fe151f59df";
const BAGS_BOUGHT: &str = "0x6d9c6fad0db13f6f7fca7124777996deaeb1949d0750a4874c18611ff5d436b9";

fn pons_buy_event(buyer: &str, recipient: &str) -> Value {
    json!({"address": PONS_CURVE, "topics":[PONS_BUY, topic_addr(buyer), topic_addr(recipient)],
        "data": format!("0x{}", "00".repeat(128)),
        "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x1"})
}

/// Robinhood block 7, tx 0xa1.. (from WALLET, value 5): a token Transfer from
/// the Pons V2 curve to WALLET and the curve's `CurveBuy` for WALLET.
/// Topic-only `eth_getLogs` above 30,000 blocks is refused like the public RPC.
struct PonsChain;

impl Respond for PonsChain {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let h = format!("0x{}", "a1".repeat(32));
        let addr_word = |a: &str| format!("0x{:0>64}", a.trim_start_matches("0x"));
        let result = match body["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x1237"),
            "eth_blockNumber" => json!("0x10"),
            "eth_getBlockByNumber" if body["params"][0] == "0x0" => {
                json!({"hash": GENESIS, "timestamp": "0x3e8"})
            }
            "eth_getBlockByNumber" => json!({"timestamp": "0x3e8"}),
            "eth_getLogs" => json!([pons_buy_event(WALLET, WALLET)]),
            "eth_getBlockReceipts" => json!([{"transactionHash": h, "blockNumber":"0x7",
                "transactionIndex":"0x1","status":"0x1","gasUsed":"0x64","effectiveGasPrice":"0x2",
                "logs":[
                  {"address": TOKEN, "topics":[TRANSFER, topic_addr(PONS_CURVE), topic_addr(WALLET)],
                   "data": word(1000), "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x0"},
                  pons_buy_event(WALLET, WALLET)]}]),
            "eth_getTransactionByHash" => json!({"hash": h, "from": WALLET, "to": PONS_CURVE,
                "value":"0x5", "blockNumber":"0x7","transactionIndex":"0x1"}),
            "eth_call" => {
                let to = body["params"][0]["to"]
                    .as_str()
                    .unwrap()
                    .to_ascii_lowercase();
                let data = body["params"][0]["data"].as_str().unwrap().to_string();
                match (&data[..10], to.as_str()) {
                    ("0xc45a0155", PONS_CURVE) => json!(addr_word(PONS_FACTORY)),
                    ("0xfc0c546a", PONS_CURVE) => json!(addr_word(TOKEN)),
                    ("0x3de35b79", PONS_CURVE) => json!(word(0)),
                    ("0x3cf28b5a", PONS_FACTORY) => json!(format!(
                        "0x{}{}{}",
                        &addr_word(TOKEN)[2..],
                        &addr_word(PONS_CURVE)[2..],
                        "00".repeat(32 * 13)
                    )),
                    other => panic!("unexpected call {other:?}"),
                }
            }
            other => panic!("unexpected {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }
}

#[tokio::test]
async fn robinhood_pons_scan_is_topic_only_reads_curve_metadata_and_books_the_signer() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(PonsChain)
        .mount(&s)
        .await;
    let out_path = tmp("rh_pons.json");
    let out = run(
        rpc_env(&s),
        base_args(&[
            "--swaps",
            "pons",
            "--from-block",
            "0",
            "--to-block",
            "16",
            "--out",
            &out_path,
        ]),
    )
    .await;
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains(SECRET), "{text}");
    }
    let filters = get_logs_filters(&s).await;
    assert_eq!(filters.len(), 1, "{filters:?}");
    // Topic-only (any emitter): Pons buy and sell, nothing else.
    assert!(filters[0].get("address").is_none(), "{filters:?}");
    assert_eq!(filters[0]["topics"][0], json!([PONS_BUY, PONS_SELL]));
    assert!(
        out.stdout.contains("curve_metadata: emitters_read=1"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("admitted=1 refused=0"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains(&format!(
            "pons_v2_curve_buy {PONS_CURVE} count=1 gate=gated"
        )),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("launchpad: gated_events=1 account_is_tx_from=1 account_other=0 recipient_other_than_account=0"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("trades=1"), "{}", out.stdout);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(v["scope"]["swaps"], "pons");
    let row = &v["curve_metadata"][0];
    assert_eq!(row["emitter"], PONS_CURVE);
    assert_eq!(row["kind"], "pons_v2");
    assert_eq!(row["factory"], PONS_FACTORY);
    assert_eq!(row["token"], TOKEN);
    assert_eq!(row["registered_curve"], PONS_CURVE);
    assert_eq!(row["quote"], format!("0x{}", "00".repeat(20)));
    // The curve reads are in the recorded calls too (replayable).
    let calls = v["calls"].as_array().unwrap();
    assert!(
        calls
            .iter()
            .any(|c| c["method"] == "eth_call" && c["params"][0]["to"] == PONS_FACTORY)
    );
}

#[tokio::test]
async fn swaps_all_on_robinhood_adds_curve_topics_and_curves_need_a_pinned_factory() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(PonsChain)
        .mount(&s)
        .await;
    let out = run(
        rpc_env(&s),
        base_args(&["--swaps", "all", "--from-block", "0", "--to-block", "16"]),
    )
    .await;
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let filters = get_logs_filters(&s).await;
    let topic_only: Vec<&Value> = filters
        .iter()
        .filter(|f| f.get("address").is_none())
        .collect();
    assert_eq!(topic_only.len(), 1, "{filters:?}");
    let t = topic_only[0]["topics"][0].as_array().unwrap();
    for topic in [PONS_BUY, PONS_SELL, BAGS_BOUGHT] {
        assert!(t.contains(&json!(topic)), "{t:?}");
    }
    // BSC pins neither family: `all` skips them, asking for one is a typed error.
    let bsc = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(BscChain {
            pancake_pool: pancake_pool(),
        })
        .mount(&bsc)
        .await;
    for (flag, what) in [("pons", "Pons V2 factory"), ("bags", "BagsFactory")] {
        let out = run(
            rpc_env(&bsc),
            bsc_args(&["--swaps", flag, "--from-block", "0", "--to-block", "16"]),
        )
        .await;
        assert_eq!(out.code, 4, "{}{}", out.stdout, out.stderr);
        assert!(out.stderr.contains(what), "{}", out.stderr);
    }
}
