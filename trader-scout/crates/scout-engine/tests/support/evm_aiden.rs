//! Shared helper: replay the committed live Robinhood fixture
//! (`evm_robinhood_token_aiden_v4_2026-10-03.json`, 48 transactions of the
//! Aiden token over 2026-10-03T17:54:46Z..18:04:46Z) through the REAL
//! `EvmRpcClient`/`EvmHistoryScanner` against a wiremock node.
#![allow(dead_code)]

use std::path::PathBuf;

use alloy_primitives::{Address, address};
use scout_core::{ChainKey, RawEvmTransaction};
use scout_evm::ROBINHOOD;
use scout_providers::evm_replay::EvmFixtureReplay;
use scout_providers::{EvmHistoryScanner, EvmRpcClient, ScanLimits, TokenScanOutput};
use scout_rpc::{RpcClient, RpcEndpoint};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

pub const AIDEN: Address = address!("15e853bc1c69529bd0a16bab1a742645a3607e1f");
pub const POOL_MANAGER: Address = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
/// Window of the capture: `[since, until)`.
pub const SINCE: u64 = 1_791_050_086;
pub const UNTIL: u64 = 1_791_050_686;

pub fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures/evm_robinhood_token_aiden_v4_2026-10-03.json")
}

pub fn replay() -> EvmFixtureReplay {
    EvmFixtureReplay::from_path(&fixture_path()).expect("fixture readable")
}

pub async fn serve(replay: EvmFixtureReplay) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(replay)
        .mount(&server)
        .await;
    server
}

pub fn rpc_client(server: &MockServer) -> EvmRpcClient {
    let rpc = RpcClient::new(RpcEndpoint::new(server.uri()), 10_000, 1).expect("rpc client");
    EvmRpcClient::new(rpc, ROBINHOOD)
}

pub struct Scanned {
    pub rpc: EvmRpcClient,
    pub chain: ChainKey,
    pub scanner: EvmHistoryScanner,
    pub out: TokenScanOutput,
    pub blocks: (u64, u64),
}

pub async fn scan(server: &MockServer) -> Scanned {
    let rpc = rpc_client(server);
    let chain = rpc
        .preflight()
        .await
        .expect("preflight against recorded genesis");
    let scanner = EvmHistoryScanner::new(rpc.clone(), chain.clone(), ScanLimits::default());
    let blocks = scanner
        .resolve_window(Some(SINCE), Some(UNTIL))
        .await
        .expect("window")
        .expect("non-empty window");
    let out = scanner
        .scan_token(AIDEN, blocks.0, blocks.1)
        .await
        .expect("token scan replays the recorded calls");
    Scanned {
        rpc,
        chain,
        scanner,
        out,
        blocks,
    }
}

pub fn transactions(s: &Scanned) -> &[RawEvmTransaction] {
    &s.out.transactions
}
