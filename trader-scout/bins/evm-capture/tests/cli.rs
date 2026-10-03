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
            .env_remove("SCOUT_EVM_CAPTURE_BLOCKSCOUT_URL");
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
            "eth_getBlockReceipts" => json!([{"transactionHash": h, "blockNumber":"0x7",
                "transactionIndex":"0x1","status":"0x1","gasUsed":"0x64","effectiveGasPrice":"0x2",
                "logs":[
                  {"address": TOKEN, "topics":[TRANSFER, topic_addr(WALLET), topic_addr(PM)], "data": word(100),
                   "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x0"},
                  {"address": PM, "topics":[V4_SWAP, word(1), topic_addr(PM)], "data": format!("0x{}", "00".repeat(192)),
                   "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x1"}]}]),
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
