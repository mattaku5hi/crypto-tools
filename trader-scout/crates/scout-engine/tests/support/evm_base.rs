//! Shared helper: replay the committed live Base fixture
//! (`evm_base_swaps_all_2026-10-04.json`, blocks 52,146,932..52,146,939 have
//! receipts and 300 recorded transactions) through the REAL client stack
//! against a wiremock node that also answers `alchemy_getAssetTransfers`.
#![allow(dead_code)]

use std::path::PathBuf;

use scout_evm::BASE;
use scout_providers::evm_replay::{AlchemyInternalMode, EvmFixtureReplay};
use scout_providers::{EvmRpcClient, EvmRpcConfig};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

/// Window of the 8 receipt blocks: `[FIRST_TS, END_TS)` = blocks
/// 52,146,932..=52,146,939 (Base: 2 s blocks).
pub const FIRST_BLOCK: u64 = 52_146_932;
pub const FIRST_TS: u64 = 1_791_083_211;
pub const END_TS: u64 = 1_791_083_227;
pub const AS_OF: i64 = 1_791_100_000;

pub fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures/evm_base_swaps_all_2026-10-04.json")
}

pub fn fixture() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path()).unwrap()).unwrap()
}

/// Replay with exactly linear (2 s) block times for any block but genesis,
/// Alchemy transfers derived from the fixture (`internal` unsupported like
/// the BSC/Robinhood answer unless scripted), and synthetic `value = 0`
/// transactions for hashes the capture did not record.
pub fn replay(fixture: &Value, internal: AlchemyInternalMode) -> EvmFixtureReplay {
    EvmFixtureReplay::from_fixture(fixture)
        .with_alchemy_transfers(fixture, internal)
        .with_linear_block_times(FIRST_BLOCK, FIRST_TS, 2)
        .with_synthetic_unrecorded_txs(fixture)
}

/// As [`replay`], plus `eth_getLogs` from the recorded receipts under the
/// measured Alchemy free-tier cap of `logs_cap` blocks per request.
pub fn replay_capped(
    fixture: &Value,
    internal: AlchemyInternalMode,
    logs_cap: Option<u64>,
) -> EvmFixtureReplay {
    replay(fixture, internal).with_receipt_logs(fixture, logs_cap)
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
    EvmRpcClient::with_config(rpc, BASE, EvmRpcConfig::default())
}
