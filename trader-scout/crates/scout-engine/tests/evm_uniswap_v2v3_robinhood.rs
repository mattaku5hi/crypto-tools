//! ADR-020 / ADR-009 style verification of Uniswap v3 (and the v2 `Swap`
//! shape) on Robinhood Chain against EVERY committed `evm_robinhood_*.json`
//! fixture (data-driven: a new `evm-capture --swaps` fixture is picked up by
//! dropping it into the fixtures directory).
//!
//! Samples: every `Swap` of a v2/v3 emitter in every receipt the fixture
//! recorded (`eth_getBlockReceipts` of whole blocks and
//! `eth_getTransactionReceipt`), not only the transactions the capture was
//! about.
//!
//! Admission (the gate's rule, `SwapVenueGate::admit_pool`): the emitter's
//! `factory()` is the pinned official factory AND its address is the CREATE2
//! address of `(factory, token0, token1, fee, init-code hash)` (pinned hash)
//! or the factory's `getPool`/`getPair` record names it. Pool metadata comes
//! from the fixture's recorded `pool_metadata` rows (offline replay of the
//! capture's `eth_call`s). A fixture captured before `pool_metadata`
//! existed has none: for those, token0/token1 are DERIVED from the pool's
//! ERC-20 flows in the swap's transaction and the fee from a search over the
//! standard fee tiers, and the pinned factory is ASSUMED; the CREATE2
//! equality then carries the proof (another factory or another init code
//! gives another address). That source is recorded per row.
//!
//! Verification: the pool's ERC-20 net flow in the transaction equals the
//! Swap event amounts, exactly. v3: `amount0/amount1` are the POOL's view
//! (positive = the pool received that token): `net_into_pool(token_i) ==
//! amount_i`. v2: `net_into_pool(token_i) == amount_iIn - amount_iOut`.
//! Several swaps of one pool in a transaction are compared as a group (sum of
//! amounts == net flow).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use alloy_primitives::{Address, B256, I256, U256};
use scout_api::DecodeOutcome;
use scout_dex_evm::{
    GateOutcome, SwapVenue, SwapVenueGate, VENUE_DEPLOYMENTS, VenueVerification,
    decode_v2_style_swap, decode_v3_swap,
};
use scout_engine::{admit_recorded, pool_venue};
use scout_evm::{ROBINHOOD, decode_erc20_transfer};
use scout_providers::evm_replay::EvmFixtureReplay;
use scout_providers::{EvmReceiptInfo, EvmRpcClient, PoolKind, PoolOnchainMetadata};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const FEE_TIERS: [u32; 5] = [100, 500, 2500, 3000, 10_000];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/p0/measurements/fixtures")
}

fn robinhood_fixtures() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("evm_robinhood_") && n.ends_with(".json"))
        })
        .collect();
    v.sort();
    v
}

fn hex_u64(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.strip_prefix("0x")?, 16).ok()
}

fn opt_addr(v: &Value) -> Option<Address> {
    v.as_str().and_then(|s| s.parse().ok())
}

struct Loaded {
    name: String,
    receipts: Vec<EvmReceiptInfo>,
    recorded: Vec<PoolOnchainMetadata>,
    rpc: EvmRpcClient,
    _server: MockServer,
}

async fn load(path: &PathBuf) -> Loaded {
    let text = std::fs::read_to_string(path).unwrap();
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(EvmFixtureReplay::from_fixture(&fixture))
        .mount(&server)
        .await;
    let rpc = EvmRpcClient::new(
        RpcClient::new(RpcEndpoint::new(server.uri()), 10_000, 1).unwrap(),
        ROBINHOOD,
    );
    let mut receipts: BTreeMap<B256, EvmReceiptInfo> = BTreeMap::new();
    for c in fixture["calls"].as_array().unwrap() {
        match c["method"].as_str() {
            Some("eth_getBlockReceipts") if !c["result"].is_null() => {
                let n = hex_u64(&c["params"][0]).expect("block number param");
                for r in rpc.block_receipts(n).await.unwrap() {
                    receipts.insert(r.tx_hash, r);
                }
            }
            Some("eth_getTransactionReceipt") if !c["result"].is_null() => {
                let h: B256 = c["params"][0].as_str().unwrap().parse().unwrap();
                receipts.insert(h, rpc.transaction_receipt(h).await.unwrap());
            }
            _ => {}
        }
    }
    let recorded = fixture
        .get("pool_metadata")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                // Families Robinhood pins no factory for (a `--swaps all` capture also
                // reads Pancake v3 / Aerodrome shaped emitters) are not this test's.
                .filter_map(|r| {
                    let kind = match r["kind"].as_str().unwrap() {
                        "v2" => PoolKind::V2,
                        "v3" => PoolKind::V3,
                        _ => return None,
                    };
                    Some(PoolOnchainMetadata {
                        emitter: opt_addr(&r["emitter"]).expect("emitter"),
                        kind,
                        factory: opt_addr(&r["factory"]),
                        token0: opt_addr(&r["token0"]),
                        token1: opt_addr(&r["token1"]),
                        fee: r["fee"].as_u64().map(|f| u32::try_from(f).unwrap()),
                        stable: None,
                        tick_spacing: None,
                        registered_pool: opt_addr(&r["registered_pool"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Loaded {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        receipts: receipts.into_values().collect(),
        recorded,
        rpc,
        _server: server,
    }
}

/// The pool's per-token net ERC-20 flow in a transaction (to - from).
fn pool_net(r: &EvmReceiptInfo, pool: Address) -> BTreeMap<Address, I256> {
    let mut net: BTreeMap<Address, I256> = BTreeMap::new();
    for l in &r.logs {
        if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l) {
            let amt = I256::try_from(t.amount).unwrap();
            if t.to == pool {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_add(amt).unwrap();
            }
            if t.from == pool {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_sub(amt).unwrap();
            }
        }
    }
    net
}

struct Row {
    fixture: String,
    tx: B256,
    pool: Address,
    kind: PoolKind,
    metadata_source: &'static str,
    admitted: bool,
    swaps_in_group: usize,
    /// Event amounts (v3: amount0/amount1; v2: in - out) summed over the group.
    amounts: [I256; 2],
    net: [I256; 2],
    exact: bool,
}

fn swap_emitters(r: &EvmReceiptInfo) -> BTreeMap<(Address, PoolKind), Vec<usize>> {
    let mut m: BTreeMap<(Address, PoolKind), Vec<usize>> = BTreeMap::new();
    for (i, l) in r.logs.iter().enumerate() {
        match l.topics.first() {
            Some(t) if *t == scout_dex_evm::V3_SWAP_TOPIC0 => {
                m.entry((l.address, PoolKind::V3)).or_default().push(i);
            }
            Some(t) if *t == scout_dex_evm::V2_SWAP_EVENT_SIGNATURE => {
                m.entry((l.address, PoolKind::V2)).or_default().push(i);
            }
            _ => {}
        }
    }
    m
}

/// Admit the fixture's pools; returns the gate and where each emitter's
/// metadata came from.
fn admit_fixture(l: &Loaded) -> (SwapVenueGate, BTreeMap<Address, &'static str>) {
    let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
    let mut source: BTreeMap<Address, &'static str> = BTreeMap::new();
    let report = admit_recorded(&mut gate, &l.recorded);
    for m in &l.recorded {
        source.insert(m.emitter, "recorded eth_calls");
        if let Some(why) = report.refused.get(&m.emitter) {
            println!("  {} refused {:#x}: {why}", l.name, m.emitter);
        }
    }
    // Emitters without recorded metadata: derive (v3 only; see module docs).
    let factory = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == 4663 && d.venue == SwapVenue::UniswapV3)
        .unwrap()
        .anchor;
    for r in &l.receipts {
        for ((pool, kind), _) in swap_emitters(r) {
            if source.contains_key(&pool) || kind != PoolKind::V3 {
                continue;
            }
            let toks: Vec<Address> = pool_net(r, pool).into_keys().collect();
            let [t0, t1] = toks[..] else { continue };
            for fee in FEE_TIERS {
                let meta = scout_dex_evm::PoolMetadata {
                    factory: Some(factory),
                    token0: Some(t0),
                    token1: Some(t1),
                    fee: Some(fee),
                    stable: None,
                    tick_spacing: None,
                    registered_pool: None,
                };
                if gate.admit_pool(SwapVenue::UniswapV3, pool, &meta).is_ok() {
                    source.insert(pool, "derived (CREATE2 search)");
                    break;
                }
            }
        }
    }
    (gate, source)
}

async fn all_rows() -> (Vec<Row>, Vec<(String, usize)>) {
    let mut rows = Vec::new();
    let mut meta_counts = Vec::new();
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        let (gate, source) = admit_fixture(&l);
        meta_counts.push((l.name.clone(), l.recorded.len()));
        for r in &l.receipts {
            let nets_by_pool: BTreeMap<(Address, PoolKind), Vec<usize>> = swap_emitters(r);
            for ((pool, kind), idxs) in nets_by_pool {
                let net = pool_net(r, pool);
                let (mut sum0, mut sum1) = (I256::ZERO, I256::ZERO);
                let mut admitted = true;
                for i in &idxs {
                    let log = &r.logs[*i];
                    let verdict = gate.classify(log);
                    admitted &= matches!(
                        &verdict,
                        GateOutcome::Verified(v) if v.venue == pool_venue(kind)
                    );
                    let (a0, a1) = match kind {
                        PoolKind::V3 => {
                            let DecodeOutcome::Decoded(s) = decode_v3_swap(log) else {
                                panic!("v3 swap must decode");
                            };
                            (s.amount0, s.amount1)
                        }
                        PoolKind::AerodromeV2 | PoolKind::Slipstream | PoolKind::PancakeV3 => {
                            unreachable!("Robinhood swap_emitters only yields v2/v3")
                        }
                        PoolKind::V2 => {
                            let DecodeOutcome::Decoded(s) = decode_v2_style_swap(log) else {
                                panic!("v2 swap must decode");
                            };
                            let i = |x: U256| I256::try_from(x).unwrap();
                            (
                                i(s.amount0_in).checked_sub(i(s.amount0_out)).unwrap(),
                                i(s.amount1_in).checked_sub(i(s.amount1_out)).unwrap(),
                            )
                        }
                    };
                    sum0 = sum0.checked_add(a0).unwrap();
                    sum1 = sum1.checked_add(a1).unwrap();
                }
                // Token sides: from metadata when known, else the two tokens
                // of the pool's flows ordered by address.
                let meta = l.recorded.iter().find(|m| m.emitter == pool);
                let (t0, t1) = match meta.and_then(|m| Some((m.token0?, m.token1?))) {
                    Some(p) => p,
                    None => {
                        let toks: Vec<Address> = net.keys().copied().collect();
                        match toks[..] {
                            [a, b] => (a, b),
                            _ => (Address::ZERO, Address::ZERO),
                        }
                    }
                };
                let n0 = net.get(&t0).copied().unwrap_or(I256::ZERO);
                let n1 = net.get(&t1).copied().unwrap_or(I256::ZERO);
                rows.push(Row {
                    fixture: l.name.clone(),
                    tx: r.tx_hash,
                    pool,
                    kind,
                    metadata_source: source.get(&pool).copied().unwrap_or("none"),
                    admitted,
                    swaps_in_group: idxs.len(),
                    amounts: [sum0, sum1],
                    net: [n0, n1],
                    exact: t0 != Address::ZERO && n0 == sum0 && n1 == sum1,
                });
            }
        }
    }
    (rows, meta_counts)
}

#[tokio::test]
async fn uniswap_v3_on_robinhood_matches_pool_token_deltas() {
    let fixtures = robinhood_fixtures();
    assert!(!fixtures.is_empty(), "no evm_robinhood_*.json fixtures");
    let (rows, meta_counts) = all_rows().await;

    println!(
        "| # | fixture | tx | pool | kind | metadata | admitted | swaps | event amount0 | pool net0 | event amount1 | pool net1 | exact |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (n, r) in rows.iter().enumerate() {
        println!(
            "| {} | {} | `{:#x}` | `{:#x}` | {:?} | {} | {} | {} | {} | {} | {} | {} | {} |",
            n + 1,
            r.fixture,
            r.tx,
            r.pool,
            r.kind,
            r.metadata_source,
            r.admitted,
            r.swaps_in_group,
            r.amounts[0],
            r.net[0],
            r.amounts[1],
            r.net[1],
            r.exact
        );
    }
    println!("pool_metadata rows per fixture: {meta_counts:?}");

    let v3: Vec<&Row> = rows.iter().filter(|r| r.kind == PoolKind::V3).collect();
    let admitted_v3: Vec<&Row> = v3.iter().copied().filter(|r| r.admitted).collect();
    println!(
        "v3 swap groups: {} total, {} admitted, {} exact among admitted",
        v3.len(),
        admitted_v3.len(),
        admitted_v3.iter().filter(|r| r.exact).count()
    );
    // Every admitted v3 sample passes, and there is at least one.
    assert!(
        !admitted_v3.is_empty(),
        "no admitted v3 sample in the fixtures"
    );
    for r in &admitted_v3 {
        assert!(
            r.exact,
            "tx {:#x} pool {:#x}: event {:?} vs pool net {:?}",
            r.tx, r.pool, r.amounts, r.net
        );
    }
    // Both trade directions of token0 occur across the samples is NOT
    // required (a small fixture may hold one); the sign convention is pinned
    // by exactness itself (positive = pool received).

    // The deployment is FixtureVerified only because every sample passed.
    let dep = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == 4663 && d.venue == SwapVenue::UniswapV3)
        .unwrap();
    assert_eq!(dep.verification, VenueVerification::FixtureVerified);
    assert_eq!(
        dep.anchor,
        "0x1f7d7550b1b028f7571e69a784071f0205fd2efa"
            .parse::<Address>()
            .unwrap()
    );
    assert_eq!(
        dep.init_code_hash,
        Some(scout_dex_evm::UNISWAP_V3_CANONICAL_INIT_CODE_HASH)
    );
    assert_eq!(dep.active_from_block, 0);
}

#[tokio::test]
async fn v3_pools_that_the_factory_did_not_create_are_not_admitted() {
    let (rows, _) = all_rows().await;
    // A v3-shaped emitter is admitted only if it reproduces from the pinned
    // factory; the rest are coverage gaps (never samples, never promoted).
    let unadmitted: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == PoolKind::V3 && !r.admitted)
        .collect();
    for r in &unadmitted {
        println!(
            "unadmitted v3 emitter {:#x} in {:#x} ({})",
            r.pool, r.tx, r.metadata_source
        );
    }
    // The Pons/other forks, if any, must be among the unadmitted: nothing
    // admitted may come from a different factory.
    let admitted: BTreeSet<Address> = rows
        .iter()
        .filter(|r| r.kind == PoolKind::V3 && r.admitted)
        .map(|r| r.pool)
        .collect();
    assert!(admitted.is_disjoint(&unadmitted.iter().map(|r| r.pool).collect()));

    // The pinned init-code hash is only sound if no pool that REPORTS the
    // pinned factory fails to reproduce. Checkable wherever the fixture
    // recorded `factory()` (fixtures from `evm-capture --swaps`).
    let pinned = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == 4663 && d.venue == SwapVenue::UniswapV3)
        .unwrap()
        .anchor;
    let mut checked = 0usize;
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        let (gate, _) = admit_fixture(&l);
        for m in l
            .recorded
            .iter()
            .filter(|m| m.kind == PoolKind::V3 && m.factory == Some(pinned))
        {
            checked += 1;
            let swap_like = scout_dex_evm::V3_SWAP_TOPIC0;
            let probe = scout_core::RawEvmLog {
                address: m.emitter,
                topics: vec![swap_like, B256::ZERO, B256::ZERO],
                data: vec![0u8; 160].into(),
                block_number: 0,
                transaction_index: 0,
                log_index: 0,
            };
            assert!(
                matches!(gate.classify(&probe), GateOutcome::Verified(_)),
                "{}: pool {:#x} reports the pinned factory but does not reproduce from the pinned init-code hash",
                l.name,
                m.emitter
            );
        }
    }
    println!("pools reporting the pinned factory checked against the hash: {checked}");
}

#[tokio::test]
async fn uniswap_v2_on_robinhood_admitted_pairs_match_pair_deltas() {
    let (rows, _) = all_rows().await;
    let v2: Vec<&Row> = rows.iter().filter(|r| r.kind == PoolKind::V2).collect();
    let admitted: Vec<&Row> = v2.iter().copied().filter(|r| r.admitted).collect();
    println!(
        "v2 swap groups: {} total, {} admitted, {} exact among admitted",
        v2.len(),
        admitted.len(),
        admitted.iter().filter(|r| r.exact).count()
    );
    for r in &v2 {
        println!(
            "v2 pair {:#x} in {:#x} ({}, admitted {}): event {:?} vs pair net {:?} exact={}",
            r.pool, r.tx, r.fixture, r.admitted, r.amounts, r.net, r.exact
        );
    }
    // Every admitted v2 sample passes, and there is at least one.
    assert!(
        !admitted.is_empty(),
        "no admitted v2 sample in the fixtures"
    );
    for r in &admitted {
        assert!(
            r.exact,
            "tx {:#x} pair {:#x}: event {:?} vs pair net {:?}",
            r.tx, r.pool, r.amounts, r.net
        );
    }
    // Admitted pairs come from recorded metadata (never derived), and the
    // pinned factory's record named each one.
    let pinned = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == 4663 && d.venue == SwapVenue::UniswapV2)
        .unwrap();
    for r in &admitted {
        assert_eq!(r.metadata_source, "recorded eth_calls");
    }
    // The canonical v2 init-code hash is pinned iff it reproduces every pair
    // that reports this factory (and `getPair` agrees for each).
    let mut reproduced = 0usize;
    let mut total = 0usize;
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        for m in l
            .recorded
            .iter()
            .filter(|m| m.kind == PoolKind::V2 && m.factory == Some(pinned.anchor))
        {
            total += 1;
            let (t0, t1) = (m.token0.unwrap(), m.token1.unwrap());
            assert_eq!(m.registered_pool, Some(m.emitter), "{:#x}", m.emitter);
            let c2 = scout_dex_evm::v2_pair_address_create2(
                pinned.anchor,
                t0,
                t1,
                scout_dex_evm::UNISWAP_V2_CANONICAL_INIT_CODE_HASH,
            );
            if c2 == m.emitter {
                reproduced += 1;
            }
        }
    }
    println!(
        "v2 pairs reporting the pinned factory: {total}; canonical hash reproduces {reproduced}"
    );
    assert!(total >= 1);
    assert_eq!(
        pinned.init_code_hash.is_some(),
        reproduced == total,
        "pin the init-code hash iff it reproduces every pair"
    );
    assert_eq!(
        pinned.init_code_hash,
        Some(scout_dex_evm::UNISWAP_V2_CANONICAL_INIT_CODE_HASH)
    );
    assert_eq!(pinned.verification, VenueVerification::FixtureVerified);
    assert_eq!(
        pinned.anchor,
        "0x8bceaa40b9acdfaedf85adf4ff01f5ad6517937f"
            .parse::<Address>()
            .unwrap()
    );
    assert_eq!(pinned.active_from_block, 0);
    // The BSC v2 factories are pinned; their evidence lives in
    // evm_bsc_venues.rs (both FixtureVerified there).
    assert_eq!(
        VENUE_DEPLOYMENTS
            .iter()
            .filter(|d| d.chain_id == 56 && d.venue == SwapVenue::UniswapV2)
            .count(),
        2
    );
}

#[tokio::test]
async fn recorded_eth_calls_replay_to_the_recorded_pool_metadata() {
    // The live admission path (`pool_identity`/`registered_pool`) answered
    // from the fixture's recorded eth_calls must equal the `pool_metadata`
    // rows: what the orchestrator captures is what the engine would read.
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        for m in &l.recorded {
            let got = l
                .rpc
                .pool_metadata(m.emitter, m.kind, "latest")
                .await
                .unwrap();
            assert_eq!(&got, m, "{} {:#x}", l.name, m.emitter);
        }
    }
}
