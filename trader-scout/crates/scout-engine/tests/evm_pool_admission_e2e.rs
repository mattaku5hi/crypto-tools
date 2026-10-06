//! End-to-end: the real `run_evm_buyer_intersect` over the committed PWPLT
//! fixture (`evm_robinhood_token_pwplt_v2v3v4_2026-10-04.json`, one Uniswap v3
//! swap through the pool `0x52e65b17...` and one v2-shape swap at
//! `0x8803c117...` inside the token's transactions) with the pool's metadata
//! served by an `eth_call` handler (as `evm-capture --swaps` records it).
//! Before pool admission existed this run counted the v3 swap as a swap-shaped
//! log outside the verified venue set and exited 3. Both pools are WETH/USDG
//! route hops (router `0x6aa80d…`) that never move PWPLT: since P3.18 an
//! unadmitted hop is `ungated_hop_swap_logs`, not a coverage gap.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::path::PathBuf;

use alloy_primitives::{Address, address};
use scout_core::{AddressBytes, AssetKey};
use scout_engine::{
    AnalysisWindow, EvmExtractionConfig, EvmRunInfo, SideFilter, TokenScanStatus,
    run_evm_buyer_intersect,
};
use scout_evm::ROBINHOOD;
use scout_providers::evm_replay::{EvmFixtureReplay, ReplayReply};
use scout_providers::{EvmHistoryScanner, EvmRpcClient, ScanLimits};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const PWPLT: Address = address!("04cc671f5678902a0d8c7c8ddae64d0d6928bfee");
const V3_POOL: Address = address!("52e65b17fb6e5ba00ed806f37afcd2daa50271ca");
const V3_FACTORY: Address = address!("1f7d7550b1b028f7571e69a784071f0205fd2efa");
const TOKEN0: Address = address!("0bd7d308f8e1639fab988df18a8011f41eacad73");
const TOKEN1: Address = address!("5fc5360d0400a0fd4f2af552add042d716f1d168");

fn word(a: Address) -> String {
    format!("0x{:0>64}", format!("{a:x}"))
}

fn revert() -> ReplayReply {
    ReplayReply::Error {
        code: 3,
        message: "execution reverted".to_string(),
    }
}

fn replay(serve_pool_metadata: bool) -> EvmFixtureReplay {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../docs/p0/measurements/fixtures/evm_robinhood_token_pwplt_v2v3v4_2026-10-04.json",
    );
    EvmFixtureReplay::from_path(&path)
        .unwrap()
        .with_handler(Box::new(move |m, params| {
            if m != "eth_call" {
                return None;
            }
            let to = params[0]["to"].as_str().unwrap_or("").to_ascii_lowercase();
            let data = params[0]["data"].as_str().unwrap_or("");
            if !serve_pool_metadata || to != format!("{V3_POOL:#x}") {
                return Some(revert());
            }
            Some(match &data[..10] {
                "0xc45a0155" => ReplayReply::Result(json!(word(V3_FACTORY))),
                "0x0dfe1681" => ReplayReply::Result(json!(word(TOKEN0))),
                "0xd21220a7" => ReplayReply::Result(json!(word(TOKEN1))),
                "0xddca3f43" => ReplayReply::Result(json!(format!("0x{:064x}", 100))),
                _ => revert(),
            })
        }))
}

async fn run(serve_pool_metadata: bool) -> scout_engine::EvmBuyerIntersectReport {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay(serve_pool_metadata))
        .mount(&server)
        .await;
    let rpc = EvmRpcClient::new(
        RpcClient::new(RpcEndpoint::new(server.uri()), 10_000, 1).unwrap(),
        ROBINHOOD,
    );
    let chain = rpc.preflight().await.unwrap();
    let scanner = EvmHistoryScanner::new(rpc, chain.clone(), ScanLimits::default());
    let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
    let info = EvmRunInfo::from_config(&cfg, "eth_getLogs token scan + RPC receipts");
    let window = AnalysisWindow::resolve(
        None,
        Some("2026-10-03T21:19:29Z"),
        Some("2026-10-03T23:14:29Z"),
        1_791_100_000,
    )
    .unwrap();
    let token = AssetKey::Token(chain, AddressBytes::Evm(PWPLT.into_array()));
    run_evm_buyer_intersect(&cfg, &scanner, &[token], 1, SideFilter::Any, &window, info)
        .await
        .unwrap()
}

#[tokio::test]
async fn admitted_and_hop_pools_are_not_coverage_gaps() {
    let before = run(false).await;
    let t0 = &before.per_token[0];
    assert!(matches!(t0.status, TokenScanStatus::Ok));
    // Both pools only move WETH/USDG (route hops): a token scan does not even
    // look them up (A10), and neither is a gap.
    assert_eq!((t0.pools_admitted, t0.pools_refused), (0, 0));
    assert_eq!((t0.ungated_swap_logs, t0.ungated_hop_swap_logs), (0, 2));
    assert!(!before.is_coverage_incomplete());

    let after = run(true).await;
    let t = &after.per_token[0];
    println!(
        "admitted={} refused={} ungated={} buyers={} sellers={} trades={:?}",
        t.pools_admitted,
        t.pools_refused,
        t.ungated_swap_logs,
        t.qualified_buyers,
        t.qualified_sellers,
        t.extraction.as_ref().map(|e| e.trades)
    );
    // Served metadata changes nothing: hop pools are never looked up in a
    // token scan (admission itself is covered in pool_admission.rs).
    assert_eq!((t.pools_admitted, t.pools_refused), (0, 0));
    assert_eq!((t.ungated_swap_logs, t.ungated_hop_swap_logs), (0, 2));
    assert!(
        !after.is_coverage_incomplete(),
        "{:?}",
        after.incomplete_reasons()
    );
}
