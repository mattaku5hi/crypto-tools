//! ADR-020 / ADR-009 style verification of Uniswap v4 on Base against EVERY
//! committed `evm_base_*.json` fixture (data-driven, like `evm_base_venues.rs`;
//! mirrors `evm_uniswap_v4_robinhood.rs`).
//!
//! v4 convention (verified on Robinhood, re-checked here): the `Swap` amounts
//! are the SWAPPER's `BalanceDelta` (negative = the swapper pays the pool), so
//! for an ERC-20 currency `c` the PoolManager's net ERC-20 flow in the
//! transaction (Transfers to minus from it) satisfies
//! `net_into_PoolManager(c) == -sum(amount_i of the swaps whose side-i
//! currency is c)`, exactly. Native ETH (`address(0)`) has no ERC-20 log: that
//! side is NOT visible in logs and is only corroborated (`tx.value`).
//!
//! poolId -> currencies:
//! - `Initialize`: the pool's `Initialize` log is inside the capture window;
//! - `derived_from_poolmanager_transfers`: the tx has ONE pool; each swap side
//!   must be matched exactly by one token's PoolManager net, a side with no
//!   matching token is native and only allowed on side 0 (v4 sorts
//!   `currency0 < currency1`, so `address(0)` is always currency0), and for
//!   two ERC-20 sides the derived pair must be sorted.
//!
//! Not a sample (counted and printed, never silently dropped):
//! - `mixed`: the tx has PoolManager events other than Swap/Initialize
//!   (ModifyLiquidity, ERC-6909 mints/burns, Donate): the ERC-20 net then
//!   contains non-swap flow;
//! - `unattributable`: several pools in one tx and not every pool's
//!   `Initialize` is in the capture: intermediate currencies cancel out of the
//!   net, so no per-swap currency can be derived offline.
//!
//! Promotion rule: the Base v4 deployment may be `FixtureVerified` ONLY with
//! at least one sample and EVERY sample exact. A passing run with n >= 1 prints
//! `PROMOTABLE`; the flag in `gate.rs` must agree with the evidence in both
//! directions (flag set => n >= 1 and all exact).
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

use alloy_primitives::{Address, B256, I256, U256, address};
use scout_api::DecodeOutcome;
use scout_dex_evm::{
    SwapVenue, V4_INITIALIZE_TOPIC0, V4_SWAP_TOPIC0, VENUE_DEPLOYMENTS, VenueVerification,
    decode_v4_initialize, decode_v4_swap,
};
use scout_evm::{BASE, decode_erc20_transfer};
use scout_providers::evm_replay::EvmFixtureReplay;
use scout_providers::{EvmReceiptInfo, EvmRpcClient};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const BASE_CHAIN: u64 = 8453;
const POOL_MANAGER: Address = address!("498581ff718922c3f8e6a244956af099b2652b2b");

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

struct Loaded {
    name: String,
    receipts: Vec<EvmReceiptInfo>,
    /// `tx.value` of the recorded transactions.
    values: BTreeMap<B256, U256>,
    _server: MockServer,
}

async fn load(path: &PathBuf) -> Loaded {
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(fixture["chain_id"], BASE_CHAIN, "{path:?} is not Base");
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
    let mut values: BTreeMap<B256, U256> = BTreeMap::new();
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
            Some("eth_getTransactionByHash") if !c["result"].is_null() => {
                let h: B256 = c["params"][0].as_str().unwrap().parse().unwrap();
                let v: U256 = c["result"]["value"].as_str().unwrap().parse().unwrap();
                values.insert(h, v);
            }
            _ => {}
        }
    }
    Loaded {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        receipts: receipts.into_values().collect(),
        values,
        _server: server,
    }
}

/// PoolManager's per-token net ERC-20 flow in a transaction (to - from).
fn pm_net(r: &EvmReceiptInfo) -> BTreeMap<Address, I256> {
    let mut net: BTreeMap<Address, I256> = BTreeMap::new();
    for l in &r.logs {
        if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l) {
            let amt = I256::try_from(t.amount).unwrap();
            if t.to == POOL_MANAGER {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_add(amt).unwrap();
            }
            if t.from == POOL_MANAGER {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_sub(amt).unwrap();
            }
        }
    }
    net.retain(|_, v| *v != I256::ZERO);
    net
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Initialize,
    Derived,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Initialize => "Initialize",
            Self::Derived => "derived_from_poolmanager_transfers",
        }
    }
}

struct Sample {
    fixture: String,
    tx: B256,
    swaps: usize,
    pools: usize,
    source: Source,
    /// Per swap side: `Some(token)` ERC-20, `None` native/invisible.
    sides: Vec<[Option<Address>; 2]>,
    amounts: Vec<[i128; 2]>,
    exact: bool,
    why_inexact: String,
    /// Native side (side 0 = `address(0)`) present and the swapper paid it:
    /// `Some(tx.value == -amount0 summed)`; `None` = not applicable / tx not recorded.
    value_corroborates: Option<bool>,
}

#[derive(Default)]
struct Skipped {
    mixed: Vec<(String, B256, BTreeSet<B256>)>,
    unattributable: Vec<(String, B256, usize)>,
}

fn analyse(
    l: &Loaded,
    samples: &mut Vec<Sample>,
    skipped: &mut Skipped,
    swap_logs_seen: &mut usize,
) {
    // Initialize logs inside the capture: poolId -> currencies.
    let mut init: BTreeMap<B256, (Address, Address)> = BTreeMap::new();
    for r in &l.receipts {
        for log in &r.logs {
            if log.address == POOL_MANAGER && log.topics.first() == Some(&V4_INITIALIZE_TOPIC0) {
                match decode_v4_initialize(log) {
                    DecodeOutcome::Decoded(i) => {
                        init.insert(i.pool_id, (i.currency0, i.currency1));
                    }
                    other => panic!("Initialize must decode: {other:?}"),
                }
            }
        }
    }
    for r in &l.receipts {
        let pm_logs: Vec<_> = r
            .logs
            .iter()
            .filter(|g| g.address == POOL_MANAGER)
            .collect();
        let swap_logs: Vec<_> = pm_logs
            .iter()
            .filter(|g| g.topics.first() == Some(&V4_SWAP_TOPIC0))
            .collect();
        if swap_logs.is_empty() {
            continue;
        }
        *swap_logs_seen += swap_logs.len();
        let others: BTreeSet<B256> = pm_logs
            .iter()
            .filter_map(|g| g.topics.first().copied())
            .filter(|t| *t != V4_SWAP_TOPIC0 && *t != V4_INITIALIZE_TOPIC0)
            .collect();
        if !others.is_empty() {
            skipped.mixed.push((l.name.clone(), r.tx_hash, others));
            continue;
        }
        let swaps: Vec<_> = swap_logs
            .iter()
            .map(|g| match decode_v4_swap(g) {
                DecodeOutcome::Decoded(s) => s,
                other => panic!("gated v4 swap must decode: {other:?}"),
            })
            .collect();
        let amounts: Vec<[i128; 2]> = swaps.iter().map(|s| [s.amount0, s.amount1]).collect();
        let pools: BTreeSet<B256> = swaps.iter().map(|s| s.pool_id).collect();
        let net = pm_net(r);
        let all_known = pools.iter().all(|p| init.contains_key(p));
        let (source, sides, exact, why) = if all_known {
            // Currencies from Initialize: per-token aggregate over every swap.
            let sides: Vec<[Option<Address>; 2]> = swaps
                .iter()
                .map(|s| {
                    let (c0, c1) = init[&s.pool_id];
                    let f = |c: Address| (c != Address::ZERO).then_some(c);
                    [f(c0), f(c1)]
                })
                .collect();
            let mut expect: BTreeMap<Address, I256> = BTreeMap::new();
            for (sd, am) in sides.iter().zip(&amounts) {
                for i in 0..2 {
                    if let Some(tok) = sd[i] {
                        let e = expect.entry(tok).or_insert(I256::ZERO);
                        *e = e.checked_sub(I256::try_from(am[i]).unwrap()).unwrap();
                    }
                }
            }
            expect.retain(|_, v| *v != I256::ZERO);
            let ok = expect == net;
            let why = if ok {
                String::new()
            } else {
                format!("expected {expect:?} vs PoolManager net {net:?}")
            };
            (Source::Initialize, sides, ok, why)
        } else if pools.len() == 1 {
            // One pool: the sum of its swaps per side is matched by tokens.
            let sum = [
                amounts.iter().map(|a| a[0]).sum::<i128>(),
                amounts.iter().map(|a| a[1]).sum::<i128>(),
            ];
            let mut side_tok: [Option<Address>; 2] = [None, None];
            let mut used: BTreeSet<Address> = BTreeSet::new();
            let mut ambiguous = false;
            for i in 0..2 {
                if sum[i] == 0 {
                    continue;
                }
                let want = -I256::try_from(sum[i]).unwrap();
                let cands: Vec<Address> = net
                    .iter()
                    .filter(|(_, v)| **v == want)
                    .map(|(k, _)| *k)
                    .collect();
                match cands.len() {
                    1 => {
                        side_tok[i] = Some(cands[0]);
                        used.insert(cands[0]);
                    }
                    0 => {}
                    _ => ambiguous = true,
                }
            }
            let leftover: Vec<_> = net.keys().filter(|k| !used.contains(*k)).collect();
            let sorted_ok = match (side_tok[0], side_tok[1]) {
                (Some(a), Some(b)) => a < b,
                _ => true,
            };
            // A side without a matching token is native: only side 0.
            let native_ok = side_tok[1].is_some() || sum[1] == 0;
            let ok = !ambiguous && leftover.is_empty() && sorted_ok && native_ok;
            let why = if ok {
                String::new()
            } else {
                format!(
                    "sums {sum:?}, net {net:?}, matched {side_tok:?}, ambiguous {ambiguous}, \
                     leftover {leftover:?}, sorted {sorted_ok}, native-on-side-0 {native_ok}"
                )
            };
            (Source::Derived, vec![side_tok; swaps.len()], ok, why)
        } else {
            skipped
                .unattributable
                .push((l.name.clone(), r.tx_hash, pools.len()));
            continue;
        };
        // Corroboration of the invisible native side (side 0): tx.value.
        let native_sum0: i128 = sides
            .iter()
            .zip(&amounts)
            .filter(|(sd, _)| sd[0].is_none())
            .map(|(_, a)| a[0])
            .sum();
        let value_corroborates = (native_sum0 < 0)
            .then(|| l.values.get(&r.tx_hash))
            .flatten()
            .map(|v| *v == U256::from(native_sum0.unsigned_abs()));
        samples.push(Sample {
            fixture: l.name.clone(),
            tx: r.tx_hash,
            swaps: swaps.len(),
            pools: pools.len(),
            source,
            sides,
            amounts,
            exact,
            why_inexact: why,
            value_corroborates,
        });
    }
}

#[tokio::test]
async fn uniswap_v4_on_base_matches_poolmanager_token_deltas() {
    let paths = base_fixtures();
    let mut samples: Vec<Sample> = Vec::new();
    let mut skipped = Skipped::default();
    let mut swap_logs_seen = 0usize;
    for p in &paths {
        let l = load(p).await;
        analyse(&l, &mut samples, &mut skipped, &mut swap_logs_seen);
    }

    // ---- the evidence table (run with --nocapture to regenerate the doc).
    println!(
        "| # | fixture | tx | swaps | pools | currencies from | ERC-20 sides (swap 1) | exact | tx.value == -amount0 |"
    );
    println!(
        "|---|---------|----|-------|-------|-----------------|-----------------------|-------|----------------------|"
    );
    for (n, s) in samples.iter().enumerate() {
        println!(
            "| {} | {} | `{:#x}` | {} | {} | {} | {:?} | {} | {} |",
            n + 1,
            s.fixture,
            s.tx,
            s.swaps,
            s.pools,
            s.source.label(),
            s.sides[0],
            s.exact,
            s.value_corroborates
                .map_or("n/a".to_string(), |b| b.to_string()),
        );
    }
    let exact_n = samples.iter().filter(|s| s.exact).count();
    let init_n = samples
        .iter()
        .filter(|s| s.source == Source::Initialize)
        .count();
    let derived_n = samples.len() - init_n;
    let both_erc20 = samples
        .iter()
        .filter(|s| s.sides[0][0].is_some() && s.sides[0][1].is_some())
        .count();
    let native_n = samples.len() - both_erc20;
    let corr: Vec<bool> = samples
        .iter()
        .filter_map(|s| s.value_corroborates)
        .collect();
    let buys = samples
        .iter()
        .filter(|s| s.amounts.iter().any(|a| a[0] < 0))
        .count();
    println!(
        "v4 Base: swap logs in receipts {swap_logs_seen}; samples n={} exact={exact_n} \
         (Initialize {init_n}, derived {derived_n}); native-side pools {native_n}, \
         ERC-20/ERC-20 pools {both_erc20}; multi-swap txs {}; multi-pool txs {}; \
         tx.value corroboration {}/{}; mixed (not samples) {}; unattributable (not samples) {}; \
         samples with a negative amount0 {buys}",
        samples.len(),
        samples.iter().filter(|s| s.swaps > 1).count(),
        samples.iter().filter(|s| s.pools > 1).count(),
        corr.iter().filter(|b| **b).count(),
        corr.len(),
        skipped.mixed.len(),
        skipped.unattributable.len(),
    );
    for (f, h, t) in &skipped.mixed {
        println!("mixed {f} {h:#x} other PoolManager topics {t:?}");
    }
    for (f, h, k) in &skipped.unattributable {
        println!("unattributable {f} {h:#x} pools {k}");
    }

    // The equality is never loosened: any inexact sample fails the test
    // whatever the deployment flag says.
    let bad: Vec<String> = samples
        .iter()
        .filter(|s| !s.exact)
        .map(|s| format!("{} {:#x}: {}", s.fixture, s.tx, s.why_inexact))
        .collect();
    assert!(bad.is_empty(), "inexact v4 samples:\n{}", bad.join("\n"));
    // Sign convention check on every single-swap ERC-20 side: the swapper
    // paying (negative amount) means the PoolManager received (positive net)
    // is already implied by exactness; additionally require both directions
    // to be exercised when there is enough data.
    if samples.len() >= 10 {
        let any_pos = samples
            .iter()
            .flat_map(|s| s.amounts.iter())
            .any(|a| a[0] > 0);
        let any_neg = samples
            .iter()
            .flat_map(|s| s.amounts.iter())
            .any(|a| a[0] < 0);
        assert!(
            any_pos && any_neg,
            "both directions expected in the fixture"
        );
    }

    let dep = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == BASE_CHAIN && d.venue == SwapVenue::UniswapV4)
        .unwrap();
    assert_eq!(dep.anchor, POOL_MANAGER);
    if samples.is_empty() {
        assert_eq!(
            dep.verification,
            VenueVerification::IdlOnly,
            "no sample, no promotion"
        );
    } else {
        println!(
            "PROMOTABLE uniswap_v4 {POOL_MANAGER:#x} n={} exact={exact_n}",
            samples.len()
        );
        assert_eq!(
            dep.verification,
            VenueVerification::FixtureVerified,
            "n >= 1 and every sample exact: flip the Base v4 row to dep_fixture_verified"
        );
    }
}
