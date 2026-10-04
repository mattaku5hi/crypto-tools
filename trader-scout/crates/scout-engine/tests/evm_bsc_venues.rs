//! ADR-020 / ADR-009 style verification of the BSC venues (PancakeSwap v2,
//! Uniswap v2/v3, PancakeSwap v3, four.meme TokenManager V1/V2) against EVERY
//! committed `evm_bsc_*.json` fixture (data-driven: a new `evm-capture --chain
//! bsc --swaps all` fixture is picked up by dropping it into the fixtures
//! directory). Mirrors `evm_base_venues.rs`.
//!
//! The promotion rule is the Base one: a deployment may be `FixtureVerified`
//! ONLY when it has at least one admitted sample (n >= 1) and ALL its admitted
//! samples are exact; any inexact admitted sample fails the test whatever the
//! flag says. A passing run with n >= 1 for an `IdlOnly` row prints
//! `PROMOTABLE <venue> <anchor>`: flip that row in `gate.rs` and commit the
//! fixture.
//!
//! Pool venues (Pancake v2 and Uniswap v2 are both v2-style, told apart by
//! their pinned factory; Uniswap v3; Pancake v3): admission is the gate's rule
//! over the fixture's recorded `pool_metadata` (pinned factory, then CREATE2
//! with the pinned hash — for Pancake v3 the deployer is the PoolDeployer — or
//! the factory's own record). Verification per (tx, pool): the sum of the
//! pool's `Swap` amounts plus its other token-moving events equals the pool's
//! ERC-20 net flow (v3-family amounts are the pool's view; v2 amounts are
//! `amountIn - amountOut`). Two columns are reported:
//!
//! - `strict`: the Base rule, plain equality with the other pool events
//!   (Mint/Burn/Collect/Flash/...) accounted;
//! - `exact` (what promotion needs): `strict` after three accounting
//!   classes the first BSC samples exposed (1 and 2 follow from the pool code):
//!   (1) POSITION: both pool families emit `Swap`/`Mint`/`Burn`/`Collect`/
//!   `Flash` AFTER the token movements of the call (v2 `swap`: payouts, then
//!   `_update`, then `Swap`; v3: callback payment verified, then `Swap`), so an
//!   ERC-20 transfer to/from the pool positioned after the pool's LAST own
//!   event in the tx cannot belong to it (a `skim`/donation/token hook:
//!   counted and printed, never silently dropped; a flow BEFORE the last event
//!   is never excluded); (2) v3-family pools only check
//!   `balanceBefore + amountIn <= balanceAfter`, so a payer may overpay the
//!   INPUT token (residual >= 0 on a token whose pool amount is positive);
//!   the output side is always exactly the event. The overpay is counted and
//!   printed per sample; (3) ROUND TRIP: a pool transfer of a token to an
//!   address that earlier in the same tx sent that same token to the pool,
//!   positioned after the pool's first own event and not after its last one
//!   (a bot's `skim`-after-swap loop; the exploratory fixture has two such
//!   Pancake v2 transactions: one needs class 1, the other classes 1 and 3), is a return of the swap input, not part of any
//!   event; it is subtracted only when the residual equals it EXACTLY (so it
//!   cannot hide a mis-decoded amount) and printed per sample.
//!
//! PancakeSwap v3's two trailing
//! `protocolFeesToken*` words are decoded and checked for the invariants the
//! exploratory fixture showed (fee only on the token the pool received, never
//! above that amount) but are not part of any flow: protocol fees stay in the
//! pool until `CollectProtocol`.
//!
//! four.meme (launchpad; no pools, the manager itself emits): per (tx,
//! manager, token) the events must equal the TokenManager's OWN ERC-20 net
//! flow of `token`: `net = sum(TokenSale.amount) - sum(TokenPurchase.amount)`
//! (the curve holds the supply: a buy pays `amount` out of the manager, a sale
//! pays `amount` into it; V1 `tokenAmount`). Native side, derived from the ABI
//! (V1 `etherAmount`, V2 `cost` + `fee`; the buyer sends BNB): for a DIRECT buy
//! (tx.to = manager, account = tx.from, buys only) `tx.value` must be
//! `cost + fee` or `cost` (or 0 for a BEP20-quoted curve); anything else is
//! inexact. Which of the two native forms is right, and that nothing is
//! refunded, MUST BE CONFIRMED LIVE: no fixture holds manager balances and the
//! BNB leg of a sale is an internal transfer. Until a live four.meme fixture
//! exists the rows stay `IdlOnly`.
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
    AnchorRole, FourMemeTrade, GateOutcome, LaunchpadSide, PANCAKE_V3_SWAP_TOPIC0, SwapVenue,
    SwapVenueGate, V2_SWAP_EVENT_SIGNATURE, V3_SWAP_TOPIC0, VENUE_DEPLOYMENTS, VenueVerification,
    decode_fourmeme_trade, decode_pancake_v3_swap, decode_v2_style_swap, decode_v3_swap,
};
use scout_engine::{admit_recorded, pool_venue};
use scout_evm::{BSC, decode_erc20_transfer};
use scout_providers::evm_replay::EvmFixtureReplay;
use scout_providers::{EvmReceiptInfo, EvmRpcClient, EvmTxInfo, PoolKind, PoolOnchainMetadata};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const BSC_CHAIN: u64 = 56;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/p0/measurements/fixtures")
}

fn bsc_fixtures() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(fixtures_dir()) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = dir
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("evm_bsc_") && n.ends_with(".json"))
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
    txs: BTreeMap<B256, EvmTxInfo>,
    recorded: Vec<PoolOnchainMetadata>,
    rpc: EvmRpcClient,
    _server: MockServer,
}

async fn load(path: &PathBuf) -> Loaded {
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        fixture["chain_id"], BSC_CHAIN,
        "{path:?} is not a BSC fixture"
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(EvmFixtureReplay::from_fixture(&fixture))
        .mount(&server)
        .await;
    let rpc = EvmRpcClient::new(
        RpcClient::new(RpcEndpoint::new(server.uri()), 10_000, 1).unwrap(),
        BSC,
    );
    let mut receipts: BTreeMap<B256, EvmReceiptInfo> = BTreeMap::new();
    let mut txs: BTreeMap<B256, EvmTxInfo> = BTreeMap::new();
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
                txs.insert(h, rpc.transaction_by_hash(h).await.unwrap());
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
        txs,
        recorded,
        rpc,
        _server: server,
    }
}

/// The account's per-token net ERC-20 flow in a transaction (to - from).
fn account_net(r: &EvmReceiptInfo, account: Address) -> BTreeMap<Address, I256> {
    let mut net: BTreeMap<Address, I256> = BTreeMap::new();
    for l in &r.logs {
        if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l) {
            let amt = I256::try_from(t.amount).unwrap();
            if t.to == account {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_add(amt).unwrap();
            }
            if t.from == account {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_sub(amt).unwrap();
            }
        }
    }
    net
}

type EventRow = (B256, [usize; 2], i8);

/// Non-swap pool events that move the pool's tokens, topics from the
/// canonical ABI signatures (keccak at run time, never typed in). Each row is
/// `(topic0, words of (amount0, amount1) in the data, sign into the pool)`.
/// PancakeSwap v3 keeps Uniswap v3's Mint/Collect/Flash/CollectProtocol/Burn
/// signatures.
fn pool_event_table(venue: SwapVenue) -> Vec<EventRow> {
    let t = |s: &str| alloy_primitives::keccak256(s.as_bytes());
    match venue {
        SwapVenue::UniswapV3 | SwapVenue::PancakeV3 => vec![
            (
                t("Mint(address,address,int24,int24,uint128,uint256,uint256)"),
                [2, 3],
                1,
            ),
            (
                t("Collect(address,address,int24,int24,uint128,uint128)"),
                [1, 2],
                -1,
            ),
            (
                t("Flash(address,address,uint256,uint256,uint256,uint256)"),
                [2, 3],
                1,
            ),
            (
                t("CollectProtocol(address,address,uint128,uint128)"),
                [0, 1],
                -1,
            ),
            (
                t("Burn(address,int24,int24,uint128,uint256,uint256)"),
                [0, 0],
                0,
            ),
        ],
        SwapVenue::UniswapV2 => vec![
            (t("Mint(address,uint256,uint256)"), [0, 1], 1),
            (t("Burn(address,uint256,uint256,address)"), [0, 1], -1),
        ],
        _ => Vec::new(),
    }
}

/// Signed token flows of the pool's other events in the tx:
/// `(flow0, flow1, events counted)`.
fn other_pool_flows(r: &EvmReceiptInfo, pool: Address, venue: SwapVenue) -> (I256, I256, usize) {
    let table = pool_event_table(venue);
    let word = |d: &[u8], i: usize| {
        let b = d.get(i * 32..i * 32 + 32).expect("event data word");
        I256::try_from(U256::from_be_slice(b)).unwrap()
    };
    let (mut f0, mut f1, mut n) = (I256::ZERO, I256::ZERO, 0usize);
    for l in r.logs.iter().filter(|l| l.address == pool) {
        let Some(t) = l.topics.first() else { continue };
        let Some((_, w, sign)) = table.iter().find(|(x, _, _)| x == t) else {
            continue;
        };
        n += 1;
        if *sign == 0 {
            continue;
        }
        let (a, b) = (word(&l.data, w[0]), word(&l.data, w[1]));
        if *sign > 0 {
            f0 = f0.checked_add(a).unwrap();
            f1 = f1.checked_add(b).unwrap();
        } else {
            f0 = f0.checked_sub(a).unwrap();
            f1 = f1.checked_sub(b).unwrap();
        }
    }
    (f0, f1, n)
}

fn swap_groups(r: &EvmReceiptInfo) -> BTreeMap<(Address, B256), Vec<usize>> {
    let mut m: BTreeMap<(Address, B256), Vec<usize>> = BTreeMap::new();
    for (i, l) in r.logs.iter().enumerate() {
        if let Some(t) = l.topics.first()
            && (*t == V3_SWAP_TOPIC0
                || *t == V2_SWAP_EVENT_SIGNATURE
                || *t == PANCAKE_V3_SWAP_TOPIC0)
        {
            m.entry((l.address, *t)).or_default().push(i);
        }
    }
    m
}

fn admit_fixture(l: &Loaded) -> SwapVenueGate {
    let mut gate = SwapVenueGate::new(BSC_CHAIN);
    let report = admit_recorded(&mut gate, &l.recorded);
    for (pool, why) in &report.refused {
        println!("  {} refused {pool:#x}: {why}", l.name);
    }
    gate
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
    /// Mint/Burn/Collect/Flash/... events of the pool in the same tx.
    other_events: usize,
    /// Plain equality (Base rule).
    strict: bool,
    /// Equality after the position / input-overpay facts (module docs).
    exact: bool,
    /// Pool transfers positioned after the pool's last own event (signed).
    post_event: [I256; 2],
    /// v3-family input-side overpay (net - events, > 0 only).
    overpay: [I256; 2],
    /// Round-trip returns of the swap input (positive amounts, class 3).
    round_trip: [I256; 2],
    /// PancakeSwap v3 only: the protocol-fee words obey the observed
    /// invariants (`None` for other venues).
    protocol_fee_ok: Option<bool>,
}

/// One (tx, pool) sample. `tokens` = the pool's (token0, token1).
#[allow(clippy::too_many_arguments)]
fn pool_row(
    fixture: &str,
    r: &EvmReceiptInfo,
    pool: Address,
    topic: B256,
    idxs: &[usize],
    venue: SwapVenue,
    tokens: Option<(Address, Address)>,
    gate: &SwapVenueGate,
    factory: Option<Address>,
) -> Row {
    let (mut sum0, mut sum1) = (I256::ZERO, I256::ZERO);
    let mut admitted = tokens.is_some();
    let mut pf_ok: Option<bool> = None;
    let i256 = |x: U256| I256::try_from(x).unwrap();
    for i in idxs {
        let log = &r.logs[*i];
        admitted &= matches!(
            gate.classify(log),
            GateOutcome::Verified(ref v) if v.venue == venue
        );
        let (a0, a1) = match (venue, topic) {
            (SwapVenue::PancakeV3, _) => {
                let DecodeOutcome::Decoded(s) = decode_pancake_v3_swap(log) else {
                    panic!("pancake v3 swap must decode: {log:?}");
                };
                // Observed invariant: a protocol fee is taken from the token
                // the pool RECEIVED and never exceeds that amount.
                let ok = |amount: I256, fee: u128| {
                    fee == 0 || (amount.is_positive() && amount >= i256(U256::from(fee)))
                };
                let this =
                    ok(s.amount0, s.protocol_fees_token0) && ok(s.amount1, s.protocol_fees_token1);
                pf_ok = Some(pf_ok.unwrap_or(true) && this);
                (s.amount0, s.amount1)
            }
            (SwapVenue::UniswapV3, _) => {
                let DecodeOutcome::Decoded(s) = decode_v3_swap(log) else {
                    panic!("v3-shaped swap must decode: {log:?}");
                };
                (s.amount0, s.amount1)
            }
            (SwapVenue::UniswapV2, _) => {
                let DecodeOutcome::Decoded(s) = decode_v2_style_swap(log) else {
                    panic!("v2-shaped swap must decode: {log:?}");
                };
                (
                    i256(s.amount0_in).checked_sub(i256(s.amount0_out)).unwrap(),
                    i256(s.amount1_in).checked_sub(i256(s.amount1_out)).unwrap(),
                )
            }
            (other, _) => panic!("{} is not a BSC pool venue", other.label()),
        };
        sum0 = sum0.checked_add(a0).unwrap();
        sum1 = sum1.checked_add(a1).unwrap();
    }
    let (f0, f1, other_events) = other_pool_flows(r, pool, venue);
    sum0 = sum0.checked_add(f0).unwrap();
    sum1 = sum1.checked_add(f1).unwrap();
    let (t0, t1) = tokens.unwrap_or((Address::ZERO, Address::ZERO));
    // Position of the pool's last own event (swap-family or table event).
    let own_topics: BTreeSet<B256> = pool_event_table(venue)
        .into_iter()
        .map(|(t, _, _)| t)
        .chain([
            V2_SWAP_EVENT_SIGNATURE,
            V3_SWAP_TOPIC0,
            PANCAKE_V3_SWAP_TOPIC0,
        ])
        .collect();
    let last_event = r
        .logs
        .iter()
        .rposition(|l| {
            l.address == pool && l.topics.first().is_some_and(|t| own_topics.contains(t))
        })
        .unwrap_or(0);
    let flow = |after: bool, token: Address| -> I256 {
        let mut net = I256::ZERO;
        for (pos, l) in r.logs.iter().enumerate() {
            if (pos > last_event) != after {
                continue;
            }
            if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l)
                && t.token == token
            {
                let amt = I256::try_from(t.amount).unwrap();
                if t.to == pool {
                    net = net.checked_add(amt).unwrap();
                }
                if t.from == pool {
                    net = net.checked_sub(amt).unwrap();
                }
            }
        }
        net
    };
    let (pre0, pre1) = (flow(false, t0), flow(false, t1));
    let (post0, post1) = (flow(true, t0), flow(true, t1));
    let n0 = pre0.checked_add(post0).unwrap();
    let n1 = pre1.checked_add(post1).unwrap();
    let strict = tokens.is_some() && n0 == sum0 && n1 == sum1;
    let v3_family = matches!(venue, SwapVenue::UniswapV3 | SwapVenue::PancakeV3);
    // Class 3: returns to an earlier sender inside (first event, last event].
    let first_event = r
        .logs
        .iter()
        .position(|l| l.address == pool && l.topics.first().is_some_and(|t| own_topics.contains(t)))
        .unwrap_or(0);
    let round_trip = |token: Address| -> I256 {
        let mut total = I256::ZERO;
        for (pos, l) in r.logs.iter().enumerate() {
            if pos <= first_event || pos > last_event {
                continue;
            }
            let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l) else {
                continue;
            };
            if t.token != token || t.from != pool {
                continue;
            }
            let sent_earlier = r.logs.iter().take(pos).any(|e| {
                matches!(decode_erc20_transfer(e), DecodeOutcome::Decoded(x)
                    if x.token == token && x.from == t.to && x.to == pool)
            });
            if sent_earlier {
                total = total
                    .checked_add(I256::try_from(t.amount).unwrap())
                    .unwrap();
            }
        }
        total
    };
    let (rt0, rt1) = (round_trip(t0), round_trip(t1));
    let side_ok = |pre: I256, sum: I256, rt: I256| {
        pre == sum
            || (v3_family && sum.is_positive() && pre > sum)
            || (rt.is_positive() && pre.checked_add(rt).unwrap() == sum)
    };
    let exact = tokens.is_some() && side_ok(pre0, sum0, rt0) && side_ok(pre1, sum1, rt1);
    let rt_used = |pre: I256, sum: I256, rt: I256| {
        if pre != sum && rt.is_positive() && pre.checked_add(rt).unwrap() == sum {
            rt
        } else {
            I256::ZERO
        }
    };
    let over = |pre: I256, sum: I256| {
        if v3_family && sum.is_positive() && pre > sum {
            pre.checked_sub(sum).unwrap()
        } else {
            I256::ZERO
        }
    };
    Row {
        fixture: fixture.to_string(),
        tx: r.tx_hash,
        pool,
        venue,
        factory,
        admitted,
        swaps_in_group: idxs.len(),
        other_events,
        amounts: [sum0, sum1],
        net: [n0, n1],
        strict,
        exact,
        post_event: [post0, post1],
        overpay: [over(pre0, sum0), over(pre1, sum1)],
        round_trip: [rt_used(pre0, sum0, rt0), rt_used(pre1, sum1, rt1)],
        protocol_fee_ok: pf_ok,
    }
}

async fn all_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for path in bsc_fixtures() {
        let l = load(&path).await;
        let gate = admit_fixture(&l);
        for r in &l.receipts {
            for ((pool, topic), idxs) in swap_groups(r) {
                let meta = l.recorded.iter().find(|m| m.emitter == pool);
                // The pool's venue: its recorded family; emitters without a
                // record are reported under the topic's first venue and never
                // admitted (Pancake v3 pools of a capture that predates the
                // Pancake topic are exactly that).
                let venue =
                    meta.map_or_else(|| SwapVenue::for_topic(&topic)[0], |m| pool_venue(m.kind));
                let tokens = meta.and_then(|m| Some((m.token0?, m.token1?)));
                rows.push(pool_row(
                    &l.name,
                    r,
                    pool,
                    topic,
                    &idxs,
                    venue,
                    tokens,
                    &gate,
                    meta.and_then(|m| m.factory),
                ));
            }
        }
    }
    rows
}

/// Anchors of the BSC deployments: `(venue, anchor, verification, role,
/// has_hash)`.
fn bsc_deployments() -> Vec<(SwapVenue, Address, VenueVerification, AnchorRole, bool)> {
    VENUE_DEPLOYMENTS
        .iter()
        .filter(|d| d.chain_id == BSC_CHAIN)
        .map(|d| {
            (
                d.venue,
                d.anchor,
                d.verification,
                d.role,
                d.init_code_hash.is_some(),
            )
        })
        .collect()
}

// --- four.meme -------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeForm {
    /// Not a direct BNB buy (router/bot account, sell, mixed): no claim.
    NotApplicable,
    /// `tx.value == sum(cost) + sum(fee)`.
    CostPlusFee,
    /// `tx.value == sum(cost)`.
    CostOnly,
    /// `tx.value == 0`: a BEP20-quoted curve (quote leg is a token flow).
    ZeroValue,
    /// Anything else (refund, bundle): inexact.
    Other,
}

struct FmRow {
    fixture: String,
    tx: B256,
    manager: Address,
    venue: SwapVenue,
    token: Address,
    events: usize,
    account_is_signer: bool,
    expected_net: I256,
    manager_net: I256,
    token_exact: bool,
    native: NativeForm,
}

impl FmRow {
    fn exact(&self) -> bool {
        self.token_exact && self.native != NativeForm::Other
    }
}

async fn fourmeme_rows() -> Vec<FmRow> {
    let mut rows = Vec::new();
    let gate = SwapVenueGate::new(BSC_CHAIN);
    for path in bsc_fixtures() {
        let l = load(&path).await;
        for r in &l.receipts {
            let mut groups: BTreeMap<(Address, Address), Vec<FourMemeTrade>> = BTreeMap::new();
            let mut venues: BTreeMap<Address, SwapVenue> = BTreeMap::new();
            for log in &r.logs {
                let GateOutcome::Verified(v) = gate.classify(log) else {
                    continue;
                };
                if v.launchpad.is_none() {
                    continue;
                }
                let DecodeOutcome::Decoded(t) = decode_fourmeme_trade(log) else {
                    panic!("gated four.meme event must decode: {log:?}");
                };
                venues.insert(t.manager, v.venue);
                groups.entry((t.manager, t.token)).or_default().push(t);
            }
            for ((manager, token), evs) in groups {
                let i = |x: U256| I256::try_from(x).unwrap();
                let (mut expected, mut cost, mut fee) = (I256::ZERO, U256::ZERO, U256::ZERO);
                for e in &evs {
                    expected = match e.side {
                        LaunchpadSide::Buy => expected.checked_sub(i(e.token_amount)).unwrap(),
                        LaunchpadSide::Sell => expected.checked_add(i(e.token_amount)).unwrap(),
                    };
                    cost = cost.checked_add(e.quote_amount).unwrap();
                    fee = fee.checked_add(e.fee).unwrap();
                }
                let manager_net = account_net(r, manager)
                    .get(&token)
                    .copied()
                    .unwrap_or(I256::ZERO);
                let tx = l.txs.get(&r.tx_hash);
                let signer = tx.map(|t| t.from).or(r.from);
                let account_is_signer = evs.iter().all(|e| Some(e.account) == signer);
                let direct = tx.is_some_and(|t| t.to == Some(manager));
                let buys_only = evs.iter().all(|e| e.side == LaunchpadSide::Buy);
                let native = match tx {
                    Some(t) if direct && account_is_signer && buys_only => {
                        if t.value == cost.saturating_add(fee) {
                            NativeForm::CostPlusFee
                        } else if t.value == cost {
                            NativeForm::CostOnly
                        } else if t.value.is_zero() {
                            NativeForm::ZeroValue
                        } else {
                            NativeForm::Other
                        }
                    }
                    _ => NativeForm::NotApplicable,
                };
                rows.push(FmRow {
                    fixture: l.name.clone(),
                    tx: r.tx_hash,
                    manager,
                    venue: venues[&manager],
                    token,
                    events: evs.len(),
                    account_is_signer,
                    expected_net: expected,
                    manager_net,
                    token_exact: expected == manager_net,
                    native,
                });
            }
        }
    }
    rows
}

// --- tests -----------------------------------------------------------------

#[tokio::test]
async fn bsc_pool_samples_match_pool_deltas_and_promotion_needs_exact_evidence() {
    let fixtures = bsc_fixtures();
    println!("evm_bsc_* fixtures: {}", fixtures.len());
    let rows = all_rows().await;
    println!(
        "| # | fixture | tx | pool | venue | factory | admitted | swaps | other | event amount0 | pool net0 | event amount1 | pool net1 | post-event0 | post-event1 | overpay0 | overpay1 | roundtrip0 | roundtrip1 | strict | exact |"
    );
    println!(
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for (n, r) in rows.iter().enumerate() {
        println!(
            "| {} | {} | `{:#x}` | `{:#x}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            n + 1,
            r.fixture,
            r.tx,
            r.pool,
            r.venue.label(),
            r.factory.map_or("-".to_string(), |f| format!("{f:#x}")),
            r.admitted,
            r.swaps_in_group,
            r.other_events,
            r.amounts[0],
            r.net[0],
            r.amounts[1],
            r.net[1],
            r.post_event[0],
            r.post_event[1],
            r.overpay[0],
            r.overpay[1],
            r.round_trip[0],
            r.round_trip[1],
            r.strict,
            r.exact
        );
    }
    // Every admitted sample passes, whatever the flags say.
    let bad: Vec<String> = rows
        .iter()
        .filter(|r| r.admitted && !r.exact)
        .map(|r| {
            format!(
                "{} tx {:#x} pool {:#x} ({}): event {:?} vs pool net {:?}",
                r.fixture,
                r.tx,
                r.pool,
                r.venue.label(),
                r.amounts,
                r.net
            )
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} inexact admitted sample(s):\n{}",
        bad.len(),
        bad.join("\n")
    );
    // PancakeSwap v3's extra words obey the observed invariants wherever the
    // pool was admitted.
    let bad_fee: Vec<String> = rows
        .iter()
        .filter(|r| r.admitted && r.protocol_fee_ok == Some(false))
        .map(|r| format!("{} tx {:#x} pool {:#x}", r.fixture, r.tx, r.pool))
        .collect();
    assert!(
        bad_fee.is_empty(),
        "Pancake v3 protocol-fee words break the observed invariants:\n{}",
        bad_fee.join("\n")
    );
    for (venue, factory, ..) in bsc_deployments()
        .into_iter()
        .filter(|d| d.3 == AnchorRole::PoolFactory)
    {
        let of = |r: &&Row| r.admitted && r.venue == venue && r.factory == Some(factory);
        println!(
            "{} {factory:#x}: admitted {} strict {} exact {} (post-event pool flow in {} samples, input overpay in {}, round trip in {})",
            venue.label(),
            rows.iter().filter(of).count(),
            rows.iter().filter(of).filter(|r| r.strict).count(),
            rows.iter().filter(of).filter(|r| r.exact).count(),
            rows.iter()
                .filter(of)
                .filter(|r| r.post_event.iter().any(|x| !x.is_zero()))
                .count(),
            rows.iter()
                .filter(of)
                .filter(|r| r.overpay.iter().any(|x| !x.is_zero()))
                .count(),
            rows.iter()
                .filter(of)
                .filter(|r| r.round_trip.iter().any(|x| !x.is_zero()))
                .count()
        );
    }
    let unadmitted_pancake = rows
        .iter()
        .filter(|r| r.venue == SwapVenue::PancakeV3 && !r.admitted)
        .count();
    println!(
        "pancake_v3 samples without recorded pool metadata (not admitted): {unadmitted_pancake}"
    );
    // Promotion is per deployment (factory): n >= 1 admitted samples, all
    // exact (checked above) before `FixtureVerified`.
    for (venue, factory, verification, role, _) in bsc_deployments() {
        if role != AnchorRole::PoolFactory {
            continue;
        }
        let n = rows
            .iter()
            .filter(|r| r.admitted && r.venue == venue && r.factory == Some(factory))
            .count();
        let n_exact = rows
            .iter()
            .filter(|r| r.admitted && r.exact && r.venue == venue && r.factory == Some(factory))
            .count();
        println!(
            "{} {factory:#x}: {n} admitted sample(s), {n_exact} exact, verification {}",
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
                "PROMOTABLE {} {factory:#x}: {n} sample(s), all exact; flip its row to \
                 `dep_fixture_verified(` in gate.rs and commit the fixture",
                venue.label()
            );
        }
    }
}

#[tokio::test]
async fn fourmeme_samples_match_the_managers_token_flow_and_promotion_needs_exact_evidence() {
    let rows = fourmeme_rows().await;
    println!("four.meme (tx, manager, token) samples: {}", rows.len());
    for (n, r) in rows.iter().enumerate() {
        println!(
            "| {} | {} | `{:#x}` | {} `{:#x}` | token `{:#x}` | events {} | account=signer {} | expected manager net {} | manager net {} | token exact {} | native {:?} |",
            n + 1,
            r.fixture,
            r.tx,
            r.venue.label(),
            r.manager,
            r.token,
            r.events,
            r.account_is_signer,
            r.expected_net,
            r.manager_net,
            r.token_exact,
            r.native
        );
    }
    let bad: Vec<String> = rows
        .iter()
        .filter(|r| !r.exact())
        .map(|r| {
            format!(
                "{} tx {:#x} manager {:#x}: expected net {} vs {} (native {:?})",
                r.fixture, r.tx, r.manager, r.expected_net, r.manager_net, r.native
            )
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} inexact four.meme sample(s):\n{}",
        bad.len(),
        bad.join("\n")
    );
    let by_form = |f: NativeForm| rows.iter().filter(|r| r.native == f).count();
    println!(
        "native forms: cost+fee {} cost {} zero-value {} n/a {}",
        by_form(NativeForm::CostPlusFee),
        by_form(NativeForm::CostOnly),
        by_form(NativeForm::ZeroValue),
        by_form(NativeForm::NotApplicable)
    );
    let signer_other = rows.iter().filter(|r| !r.account_is_signer).count();
    println!(
        "samples whose event account is not tx.from (router/bot, never attributed): {signer_other}"
    );
    for (venue, manager, verification, role, _) in bsc_deployments() {
        if role != AnchorRole::SwapEmitter
            || !matches!(venue, SwapVenue::FourMemeV1 | SwapVenue::FourMemeV2)
        {
            continue;
        }
        let n = rows.iter().filter(|r| r.manager == manager).count();
        println!(
            "{} {manager:#x}: {n} sample(s), all exact, verification {}",
            venue.label(),
            verification.label()
        );
        if verification == VenueVerification::FixtureVerified {
            assert!(
                n >= 1,
                "{} {manager:#x} is FixtureVerified without a four.meme sample",
                venue.label()
            );
        } else if n >= 1 {
            println!(
                "PROMOTABLE {} {manager:#x}: {n} sample(s), all exact; flip its `dep(` to \
                 `dep_fixture_verified(` in gate.rs and commit the fixture",
                venue.label()
            );
        }
    }
}

#[tokio::test]
async fn pools_reporting_a_pinned_factory_are_admitted_and_nothing_else_is() {
    let pinned: BTreeSet<Address> = bsc_deployments()
        .iter()
        .filter(|d| d.3 == AnchorRole::PoolFactory)
        .map(|d| d.1)
        .collect();
    let mut checked = 0usize;
    for path in bsc_fixtures() {
        let l = load(&path).await;
        let gate = admit_fixture(&l);
        for m in &l.recorded {
            let (probe_topic, data_len) = match m.kind {
                PoolKind::V2 => (V2_SWAP_EVENT_SIGNATURE, 128),
                PoolKind::V3 | PoolKind::Slipstream => (V3_SWAP_TOPIC0, 160),
                PoolKind::PancakeV3 => (PANCAKE_V3_SWAP_TOPIC0, 224),
                PoolKind::AerodromeV2 => (scout_dex_evm::AERODROME_V2_SWAP_TOPIC0, 128),
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
    println!("pools reporting a pinned BSC factory, all admitted: {checked}");
}

#[tokio::test]
async fn pinned_init_code_hashes_reproduce_every_recorded_pool_of_their_factory() {
    for (venue, factory, _, role, has_hash) in bsc_deployments() {
        if role != AnchorRole::PoolFactory || !has_hash {
            continue;
        }
        let dep = VENUE_DEPLOYMENTS
            .iter()
            .find(|d| d.chain_id == BSC_CHAIN && d.anchor == factory)
            .unwrap();
        let hash = dep.init_code_hash.unwrap();
        // Pancake v3 pools are deployed by the PoolDeployer.
        let deployer = dep.pool_deployer.unwrap_or(factory);
        let (mut total, mut reproduced) = (0usize, 0usize);
        for path in bsc_fixtures() {
            let l = load(&path).await;
            for m in l.recorded.iter().filter(|m| m.factory == Some(factory)) {
                let (Some(t0), Some(t1)) = (m.token0, m.token1) else {
                    continue;
                };
                total += 1;
                let c2 = match venue {
                    SwapVenue::UniswapV3 | SwapVenue::PancakeV3 => m.fee.map(|fee| {
                        scout_dex_evm::v3_pool_address_create2(deployer, t0, t1, fee, hash)
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
    for path in bsc_fixtures() {
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

/// Pancake v3 swap emitters of a fixture that carries no recorded metadata
/// (a capture that predates the Pancake topic): `(token0, token1, fee)` are
/// DERIVED from the pool's own ERC-20 Transfers and the four Pancake fee
/// tiers, kept only where the CREATE2 address from the PoolDeployer with the
/// pinned hash equals the emitter (a cryptographic match). This is evidence
/// for the pinned hash and for the pool-delta equation, NOT admission:
/// `factory()` was never read, so these pools stay un-admitted and the
/// deployment stays `IdlOnly` until a live capture records them.
#[tokio::test]
async fn derived_pancake_v3_pools_reproduce_create2_and_match_pool_deltas() {
    let dep = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == BSC_CHAIN && d.venue == SwapVenue::PancakeV3)
        .unwrap();
    let hash = dep.init_code_hash.unwrap();
    let deployer = dep.pool_deployer.unwrap();
    let gate = SwapVenueGate::new(BSC_CHAIN);
    let (mut pools, mut derived, mut swaps, mut exact) = (BTreeSet::new(), BTreeSet::new(), 0, 0);
    let (mut strict_n, mut overpay_n, mut post_n) = (0, 0, 0);
    let mut failures = Vec::new();
    for path in bsc_fixtures() {
        let l = load(&path).await;
        let recorded: BTreeSet<Address> = l.recorded.iter().map(|m| m.emitter).collect();
        for r in &l.receipts {
            for ((pool, topic), idxs) in swap_groups(r) {
                if topic != PANCAKE_V3_SWAP_TOPIC0 || recorded.contains(&pool) {
                    continue;
                }
                pools.insert(pool);
                let toks: Vec<Address> = account_net(r, pool).keys().copied().collect();
                let [a, b] = toks[..] else { continue };
                let (t0, t1) = if a < b { (a, b) } else { (b, a) };
                let fee = [100u32, 500, 2_500, 10_000].into_iter().find(|fee| {
                    scout_dex_evm::v3_pool_address_create2(deployer, t0, t1, *fee, hash) == pool
                });
                if fee.is_none() {
                    continue;
                }
                derived.insert(pool);
                swaps += 1;
                let row = pool_row(
                    &l.name,
                    r,
                    pool,
                    topic,
                    &idxs,
                    SwapVenue::PancakeV3,
                    Some((t0, t1)),
                    &gate,
                    None,
                );
                assert_eq!(row.protocol_fee_ok, Some(true), "{:#x} {:#x}", row.tx, pool);
                strict_n += usize::from(row.strict);
                overpay_n += usize::from(row.overpay.iter().any(|x| !x.is_zero()));
                post_n += usize::from(row.post_event.iter().any(|x| !x.is_zero()));
                if row.exact {
                    exact += 1;
                } else {
                    failures.push(format!(
                        "tx {:#x} pool {pool:#x}: {:?} vs {:?}",
                        row.tx, row.amounts, row.net
                    ));
                }
            }
        }
    }
    println!(
        "DERIVED (not live, not admitted) pancake_v3: {} emitters without metadata, {} reproduce \
         CREATE2 from (PoolDeployer, tokens, fee), {swaps} (tx, pool) samples, {strict_n} strict, \
         {exact} exact (input overpay in {overpay_n}, post-event pool flow in {post_n})",
        pools.len(),
        derived.len()
    );
    assert_eq!(
        derived.len(),
        pools.len(),
        "a Pancake v3 emitter does not match the pinned hash"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn bsc_deployments_are_pinned_with_sources() {
    use std::str::FromStr;
    let a = |s: &str| Address::from_str(s).unwrap();
    let on_bsc: Vec<(SwapVenue, Address)> = bsc_deployments().iter().map(|d| (d.0, d.1)).collect();
    for (venue, addr) in [
        (
            SwapVenue::UniswapV2,
            "0x8909Dc15e40173Ff4699343b6eB8132c65e18eC6",
        ),
        (
            SwapVenue::UniswapV2,
            "0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73",
        ),
        (
            SwapVenue::UniswapV3,
            "0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7",
        ),
        (
            SwapVenue::UniswapV4,
            "0x28e2ea090877bf75740558f6bfb36a5ffee9e9df",
        ),
        (
            SwapVenue::PancakeV3,
            "0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865",
        ),
        (
            SwapVenue::FourMemeV1,
            "0xEC4549caDcE5DA21Df6E6422d448034B5233bFbC",
        ),
        (
            SwapVenue::FourMemeV2,
            "0x5c952063c7fc8610FFDB798152D69F0B9550762b",
        ),
    ] {
        assert!(
            on_bsc.contains(&(venue, a(addr))),
            "{} {addr}",
            venue.label()
        );
    }
    assert_eq!(on_bsc.len(), 7);
    // PancakeSwap v3 pools come from the PoolDeployer.
    let p3 = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == BSC_CHAIN && d.venue == SwapVenue::PancakeV3)
        .unwrap();
    assert_eq!(
        p3.pool_deployer,
        Some(a("0x41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9"))
    );
    // The two launchpad rows are singletons that emit the events themselves.
    for d in VENUE_DEPLOYMENTS
        .iter()
        .filter(|d| d.chain_id == BSC_CHAIN)
        .filter(|d| matches!(d.venue, SwapVenue::FourMemeV1 | SwapVenue::FourMemeV2))
    {
        assert_eq!((d.role, d.init_code_hash), (AnchorRole::SwapEmitter, None));
    }
    // Without any evm_bsc_* fixture nothing on BSC may claim FixtureVerified
    // (the sample tests above check the evidence when fixtures exist).
    if bsc_fixtures().is_empty() {
        for d in VENUE_DEPLOYMENTS.iter().filter(|d| d.chain_id == BSC_CHAIN) {
            assert_eq!(d.verification, VenueVerification::IdlOnly, "{d:?}");
        }
    }
}

#[test]
fn bsc_is_enabled_through_verified_venues_and_valued_in_bnb() {
    // The CLI gate: a chain runs without --allow-unverified-chain only with at
    // least one FixtureVerified venue.
    assert!(scout_engine::chain_has_verified_venue(BSC_CHAIN));
    assert!(scout_engine::chain_has_verified_venue(8453));
    assert!(scout_engine::chain_has_verified_venue(4663));
    assert!(!scout_engine::chain_has_verified_venue(1));
    // Native BNB is valued through BNB-USD (not ETH-USD); other chains keep ETH.
    let bsc = scout_engine::ChainDisplay::evm(&BSC);
    assert_eq!(bsc.native_label, "bnb");
    assert_eq!(bsc.quote_units(), &[scout_ledger::QuoteUnit::Wei]);
    assert_eq!(
        scout_engine::quote_asset_on(&bsc, scout_ledger::QuoteUnit::Wei),
        Some(scout_pricing::QuoteAsset::Bnb)
    );
    let base = scout_engine::ChainDisplay::evm(&scout_evm::BASE);
    assert_eq!(
        scout_engine::quote_asset_on(&base, scout_ledger::QuoteUnit::Wei),
        Some(scout_pricing::QuoteAsset::Eth)
    );
    // No stable quote asset is pinned on BSC yet (USDT/USDC unverified).
    assert!(BSC.quote_assets.is_empty());
}
