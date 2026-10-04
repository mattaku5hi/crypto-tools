//! ADR-020 / ADR-009 style verification of the Base venues (Uniswap v2/v3,
//! Aerodrome v2, Aerodrome Slipstream) against EVERY committed
//! `evm_base_*.json` fixture (data-driven: a new `evm-capture --chain base
//! --swaps all` fixture is picked up by dropping it into the fixtures
//! directory). Mirrors `evm_uniswap_v2v3_robinhood.rs`.
//!
//! With no `evm_base_*` fixture (n = 0) every test passes vacuously and the
//! Base deployments stay `IdlOnly`. The rule that makes promotion
//! evidence-based: a deployment may be `FixtureVerified` ONLY when its
//! factory has at least one admitted sample (n >= 1) and ALL its admitted
//! samples are exact; any inexact admitted sample fails the test whatever the
//! flag says. A passing run with n >= 1 prints `PROMOTABLE <venue> <factory>`:
//! flip that row's `dep(` to `dep_fixture_verified(` in `gate.rs` (one line)
//! and commit the fixture.
//!
//! Admission is the gate's rule over the fixture's recorded `pool_metadata`
//! rows (offline replay of the capture's `eth_call`s): factory pinned for the
//! venue, then the CREATE2 address (only where a hash is pinned) or the
//! factory's own `getPool`/`getPair` record names the emitter. Aerodrome v2
//! and Slipstream pools are minimal-proxy clones: the record decides.
//!
//! Verification: the pool's ERC-20 net flow in the transaction equals the
//! `Swap` event amounts, exactly. v3/Slipstream: `net_into_pool(token_i) ==
//! amount_i`. v2 (Uniswap): `net_into_pool(token_i) == amount_iIn -
//! amount_iOut`. Aerodrome v2 moves its swap fee from the pool to its
//! PoolFees contract inside the swap, after the amounts are computed: for it
//! the sample is exact when `net_into_pool(token_i) + fee_i == amount_iIn -
//! amount_iOut` where `fee_i` is the pool's outgoing transfers of the INPUT
//! token to a recipient that is neither the swap's `to` nor `sender`
//! (`fee_adjusted` column; strict equality is reported next to it). If the
//! live data shows another layout this is where it surfaces.
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
    AERODROME_V2_SWAP_TOPIC0, GateOutcome, SwapVenue, SwapVenueGate, V2_SWAP_EVENT_SIGNATURE,
    V3_SWAP_TOPIC0, VENUE_DEPLOYMENTS, VenueVerification, decode_aerodrome_v2_swap,
    decode_v2_style_swap, decode_v3_swap,
};
use scout_engine::{admit_recorded, pool_venue};
use scout_evm::{BASE, decode_erc20_transfer};
use scout_providers::evm_replay::EvmFixtureReplay;
use scout_providers::{EvmReceiptInfo, EvmRpcClient, PoolKind, PoolOnchainMetadata};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const BASE_CHAIN: u64 = 8453;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/p0/measurements/fixtures")
}

fn base_fixtures() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(fixtures_dir()) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = dir
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("evm_base_") && n.ends_with(".json"))
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
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        fixture["chain_id"], BASE_CHAIN,
        "{path:?} is not a Base fixture"
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(EvmFixtureReplay::from_fixture(&fixture))
        .mount(&server)
        .await;
    let rpc = EvmRpcClient::new(
        RpcClient::new(RpcEndpoint::new(server.uri()), 10_000, 1).unwrap(),
        BASE,
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
                .map(|r| PoolOnchainMetadata {
                    emitter: opt_addr(&r["emitter"]).expect("emitter"),
                    kind: PoolKind::from_label(r["kind"].as_str().unwrap())
                        .unwrap_or_else(|| panic!("kind {}", r["kind"])),
                    factory: opt_addr(&r["factory"]),
                    token0: opt_addr(&r["token0"]),
                    token1: opt_addr(&r["token1"]),
                    fee: r["fee"].as_u64().map(|f| u32::try_from(f).unwrap()),
                    stable: r["stable"].as_bool(),
                    tick_spacing: r["tick_spacing"]
                        .as_i64()
                        .map(|t| i32::try_from(t).unwrap()),
                    registered_pool: opt_addr(&r["registered_pool"]),
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

/// Aerodrome v2: per token, what the pool sent to recipients other than the
/// swaps' own `to`/`sender` (the PoolFees contract), summed.
fn fee_outflows(
    r: &EvmReceiptInfo,
    pool: Address,
    not_these: &BTreeSet<Address>,
) -> BTreeMap<Address, I256> {
    let mut out: BTreeMap<Address, I256> = BTreeMap::new();
    for l in &r.logs {
        if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l)
            && t.from == pool
            && !not_these.contains(&t.to)
            && t.to != pool
        {
            let e = out.entry(t.token).or_insert(I256::ZERO);
            *e = e.checked_add(I256::try_from(t.amount).unwrap()).unwrap();
        }
    }
    out
}

struct Row {
    fixture: String,
    tx: B256,
    pool: Address,
    venue: SwapVenue,
    factory: Option<Address>,
    admitted: bool,
    swaps_in_group: usize,
    amounts: [I256; 2],
    net: [I256; 2],
    strict_exact: bool,
    /// Exact under the venue's own rule (Aerodrome v2: fee-adjusted).
    exact: bool,
}

fn swap_groups(r: &EvmReceiptInfo) -> BTreeMap<(Address, B256), Vec<usize>> {
    let mut m: BTreeMap<(Address, B256), Vec<usize>> = BTreeMap::new();
    for (i, l) in r.logs.iter().enumerate() {
        if let Some(t) = l.topics.first()
            && (*t == V3_SWAP_TOPIC0
                || *t == V2_SWAP_EVENT_SIGNATURE
                || *t == AERODROME_V2_SWAP_TOPIC0)
        {
            m.entry((l.address, *t)).or_default().push(i);
        }
    }
    m
}

fn admit_fixture(l: &Loaded) -> SwapVenueGate {
    let mut gate = SwapVenueGate::new(BASE_CHAIN);
    let report = admit_recorded(&mut gate, &l.recorded);
    for (pool, why) in &report.refused {
        println!("  {} refused {pool:#x}: {why}", l.name);
    }
    gate
}

async fn all_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for path in base_fixtures() {
        let l = load(&path).await;
        let gate = admit_fixture(&l);
        for r in &l.receipts {
            for ((pool, topic), idxs) in swap_groups(r) {
                let meta = l.recorded.iter().find(|m| m.emitter == pool);
                // The pool's venue: its recorded family; unknown emitters
                // are reported under the topic's first venue and never admitted.
                let venue =
                    meta.map_or_else(|| SwapVenue::for_topic(&topic)[0], |m| pool_venue(m.kind));
                let (mut sum0, mut sum1) = (I256::ZERO, I256::ZERO);
                let mut admitted = meta.is_some();
                let mut counterparties: BTreeSet<Address> = BTreeSet::new();
                let i256 = |x: U256| I256::try_from(x).unwrap();
                for i in &idxs {
                    let log = &r.logs[*i];
                    admitted &= matches!(
                        gate.classify(log),
                        GateOutcome::Verified(ref v) if v.venue == venue
                    );
                    let (a0, a1) = match venue {
                        SwapVenue::UniswapV3 | SwapVenue::AerodromeSlipstream => {
                            let DecodeOutcome::Decoded(s) = decode_v3_swap(log) else {
                                panic!("v3-shaped swap must decode: {log:?}");
                            };
                            (s.amount0, s.amount1)
                        }
                        SwapVenue::UniswapV2 | SwapVenue::AerodromeV2 => {
                            let d = if venue == SwapVenue::AerodromeV2 {
                                decode_aerodrome_v2_swap(log)
                            } else {
                                decode_v2_style_swap(log)
                            };
                            let DecodeOutcome::Decoded(s) = d else {
                                panic!("v2-shaped swap must decode: {log:?}");
                            };
                            counterparties.insert(s.to);
                            counterparties.insert(s.sender);
                            (
                                i256(s.amount0_in).checked_sub(i256(s.amount0_out)).unwrap(),
                                i256(s.amount1_in).checked_sub(i256(s.amount1_out)).unwrap(),
                            )
                        }
                        SwapVenue::UniswapV4 => unreachable!("v4 has no pool emitters"),
                    };
                    sum0 = sum0.checked_add(a0).unwrap();
                    sum1 = sum1.checked_add(a1).unwrap();
                }
                let net = pool_net(r, pool);
                let (t0, t1) = meta
                    .and_then(|m| Some((m.token0?, m.token1?)))
                    .unwrap_or((Address::ZERO, Address::ZERO));
                let n0 = net.get(&t0).copied().unwrap_or(I256::ZERO);
                let n1 = net.get(&t1).copied().unwrap_or(I256::ZERO);
                let known = t0 != Address::ZERO;
                let strict_exact = known && n0 == sum0 && n1 == sum1;
                let exact = if venue == SwapVenue::AerodromeV2 && known {
                    let fees = fee_outflows(r, pool, &counterparties);
                    let f0 = fees.get(&t0).copied().unwrap_or(I256::ZERO);
                    let f1 = fees.get(&t1).copied().unwrap_or(I256::ZERO);
                    // A fee leaves only on the input side of the swap.
                    let sane = (f0 == I256::ZERO || sum0 > I256::ZERO)
                        && (f1 == I256::ZERO || sum1 > I256::ZERO);
                    sane && n0.checked_add(f0) == Some(sum0) && n1.checked_add(f1) == Some(sum1)
                } else {
                    strict_exact
                };
                rows.push(Row {
                    fixture: l.name.clone(),
                    tx: r.tx_hash,
                    pool,
                    venue,
                    factory: meta.and_then(|m| m.factory),
                    admitted,
                    swaps_in_group: idxs.len(),
                    amounts: [sum0, sum1],
                    net: [n0, n1],
                    strict_exact,
                    exact,
                });
            }
        }
    }
    rows
}

fn base_pool_factories() -> Vec<(SwapVenue, Address, VenueVerification, bool)> {
    VENUE_DEPLOYMENTS
        .iter()
        .filter(|d| d.chain_id == BASE_CHAIN && d.role == scout_dex_evm::AnchorRole::PoolFactory)
        .map(|d| {
            (
                d.venue,
                d.anchor,
                d.verification,
                d.init_code_hash.is_some(),
            )
        })
        .collect()
}

#[tokio::test]
async fn base_samples_match_pool_deltas_and_promotion_needs_exact_evidence() {
    let fixtures = base_fixtures();
    println!("evm_base_* fixtures: {}", fixtures.len());
    let rows = all_rows().await;
    println!(
        "| # | fixture | tx | pool | venue | factory | admitted | swaps | event amount0 | pool net0 | event amount1 | pool net1 | strict | exact |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (n, r) in rows.iter().enumerate() {
        println!(
            "| {} | {} | `{:#x}` | `{:#x}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            n + 1,
            r.fixture,
            r.tx,
            r.pool,
            r.venue.label(),
            r.factory.map_or("-".to_string(), |f| format!("{f:#x}")),
            r.admitted,
            r.swaps_in_group,
            r.amounts[0],
            r.net[0],
            r.amounts[1],
            r.net[1],
            r.strict_exact,
            r.exact
        );
    }
    // Every admitted sample passes, whatever the flags say.
    for r in rows.iter().filter(|r| r.admitted) {
        assert!(
            r.exact,
            "{} tx {:#x} pool {:#x} ({}): event {:?} vs pool net {:?}",
            r.fixture,
            r.tx,
            r.pool,
            r.venue.label(),
            r.amounts,
            r.net
        );
    }
    // Promotion is per deployment (factory): n >= 1 admitted samples, all
    // exact (checked above) before `FixtureVerified`.
    for (venue, factory, verification, _) in base_pool_factories() {
        let n = rows
            .iter()
            .filter(|r| r.admitted && r.venue == venue && r.factory == Some(factory))
            .count();
        println!(
            "{} {factory:#x}: {n} admitted sample(s), verification {}",
            venue.label(),
            verification.label()
        );
        if verification == VenueVerification::FixtureVerified {
            assert!(
                n >= 1,
                "{} {factory:#x} is FixtureVerified without an admitted exact sample",
                venue.label()
            );
        } else if n >= 1 {
            println!(
                "PROMOTABLE {} {factory:#x}: {n} sample(s), all exact; flip its `dep(` to \
                 `dep_fixture_verified(` in gate.rs and commit the fixture",
                venue.label()
            );
        }
    }
}

#[tokio::test]
async fn pools_reporting_a_pinned_factory_are_admitted_and_nothing_else_is() {
    let pinned: BTreeSet<Address> = base_pool_factories().iter().map(|f| f.1).collect();
    let mut checked = 0usize;
    for path in base_fixtures() {
        let l = load(&path).await;
        let gate = admit_fixture(&l);
        for m in &l.recorded {
            let probe_topic = match m.kind {
                PoolKind::V2 => V2_SWAP_EVENT_SIGNATURE,
                PoolKind::V3 | PoolKind::Slipstream => V3_SWAP_TOPIC0,
                PoolKind::AerodromeV2 => AERODROME_V2_SWAP_TOPIC0,
            };
            let data_len = match m.kind {
                PoolKind::V3 | PoolKind::Slipstream => 160,
                _ => 128,
            };
            let probe = scout_core::RawEvmLog {
                address: m.emitter,
                topics: vec![probe_topic, B256::ZERO, B256::ZERO],
                data: vec![0u8; data_len].into(),
                block_number: u64::MAX,
                transaction_index: 0,
                log_index: 0,
            };
            let admitted = matches!(gate.classify(&probe), GateOutcome::Verified(_));
            match m.factory {
                Some(f) if pinned.contains(&f) => {
                    checked += 1;
                    assert!(
                        admitted,
                        "{}: pool {:#x} ({}) reports the pinned factory {f:#x} but the gate \
                         refuses it (registry record / CREATE2 / identity mismatch)",
                        l.name,
                        m.emitter,
                        m.kind.label()
                    );
                }
                _ => assert!(
                    !admitted,
                    "{}: pool {:#x} admitted without a pinned factory",
                    l.name, m.emitter
                ),
            }
        }
    }
    println!("pools reporting a pinned Base factory, all admitted: {checked}");
}

#[tokio::test]
async fn pinned_init_code_hashes_reproduce_every_recorded_pool_of_their_factory() {
    for (venue, factory, _, has_hash) in base_pool_factories() {
        if !has_hash {
            continue;
        }
        let dep = VENUE_DEPLOYMENTS
            .iter()
            .find(|d| d.chain_id == BASE_CHAIN && d.anchor == factory)
            .unwrap();
        let hash = dep.init_code_hash.unwrap();
        let (mut total, mut reproduced) = (0usize, 0usize);
        for path in base_fixtures() {
            let l = load(&path).await;
            for m in l.recorded.iter().filter(|m| m.factory == Some(factory)) {
                let (Some(t0), Some(t1)) = (m.token0, m.token1) else {
                    continue;
                };
                total += 1;
                let c2 = match venue {
                    SwapVenue::UniswapV3 => m.fee.map(|fee| {
                        scout_dex_evm::v3_pool_address_create2(factory, t0, t1, fee, hash)
                    }),
                    SwapVenue::UniswapV2 => Some(scout_dex_evm::v2_pair_address_create2(
                        factory, t0, t1, hash,
                    )),
                    _ => None,
                };
                if c2 == Some(m.emitter) {
                    reproduced += 1;
                }
            }
        }
        println!(
            "{} {factory:#x}: hash reproduces {reproduced}/{total} recorded pools",
            venue.label()
        );
        assert_eq!(
            reproduced,
            total,
            "{} {factory:#x}: the pinned init-code hash does not reproduce every recorded pool; \
             unpin it (the factory record then decides)",
            venue.label()
        );
    }
}

#[tokio::test]
async fn recorded_eth_calls_replay_to_the_recorded_pool_metadata() {
    // The live admission path answered from the fixture's recorded eth_calls
    // must equal the `pool_metadata` rows (what the engine would read).
    for path in base_fixtures() {
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

#[test]
fn base_deployments_are_pinned_with_sources_and_idl_only_until_flipped() {
    use std::str::FromStr;
    let a = |s: &str| Address::from_str(s).unwrap();
    let on_base: Vec<(SwapVenue, Address)> = VENUE_DEPLOYMENTS
        .iter()
        .filter(|d| d.chain_id == BASE_CHAIN)
        .map(|d| (d.venue, d.anchor))
        .collect();
    for (venue, addr) in [
        (
            SwapVenue::UniswapV2,
            "0x8909Dc15e40173Ff4699343b6eB8132c65e18eC6",
        ),
        (
            SwapVenue::UniswapV3,
            "0x33128a8fC17869897dcE68Ed026d694621f6FDfD",
        ),
        (
            SwapVenue::UniswapV4,
            "0x498581ff718922c3f8e6a244956af099b2652b2b",
        ),
        (
            SwapVenue::AerodromeV2,
            "0x420DD381b31aEf6683db6B902084cB0FFECe40Da",
        ),
        (
            SwapVenue::AerodromeSlipstream,
            "0x5e7BB104d84c7CB9B682AaC2F3d509f5F406809A",
        ),
        (
            SwapVenue::AerodromeSlipstream,
            "0xaDe65c38CD4849aDBA595a4323a8C7DdfE89716a",
        ),
        (
            SwapVenue::AerodromeSlipstream,
            "0xf8f2eB4940CFE7d13603DDDD87f123820Fc061Ef",
        ),
    ] {
        assert!(
            on_base.contains(&(venue, a(addr))),
            "{} {addr}",
            venue.label()
        );
    }
    assert_eq!(on_base.len(), 7);
    // Without a fixture nothing on Base may claim FixtureVerified (checked
    // with evidence in the sample test above when fixtures exist).
    if base_fixtures().is_empty() {
        for d in VENUE_DEPLOYMENTS
            .iter()
            .filter(|d| d.chain_id == BASE_CHAIN)
        {
            assert_eq!(d.verification, VenueVerification::IdlOnly, "{d:?}");
        }
    }
}
