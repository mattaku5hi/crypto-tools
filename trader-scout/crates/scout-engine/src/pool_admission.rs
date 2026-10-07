//! Pool learning for Uniswap v2/v3-style venues (ADR-020 §2b, invariant #16).
//!
//! Instead of feeding the factory's `PoolCreated` logs (infeasible for a
//! token-centric scan: the creation can be anywhere in history), every
//! swap-shaped v2/v3 emitter found in the scanned transactions is checked
//! against the chain itself: [`learn_pools`] reads `factory()`, `token0()`,
//! `token1()` (and `fee()` for v3, and the factory's `getPool`/`getPair`
//! record only when the deployment pins no CREATE2 init-code hash) through
//! bounded, cached `eth_call`s that count against the request budget, then
//! asks [`SwapVenueGate::admit_pool`]. An emitter that is not admitted stays
//! an ungated coverage gap; nothing here widens what counts as venue
//! evidence beyond the gate's rule.

use std::collections::BTreeMap;

use alloy_primitives::Address;
use scout_core::RawEvmTransaction;
use scout_dex_evm::{
    AnchorRole, CurveMetadata, PoolMetadata, SwapVenue, SwapVenueGate, VENUE_DEPLOYMENTS,
};
use scout_providers::{
    CurveKind, CurveOnchainMetadata, EvmRpcClient, EvmSourceError, PoolKind, PoolOnchainMetadata,
};

/// Default cap on pool lookups per call (each costs 3-5 `eth_call`s).
pub const DEFAULT_MAX_POOL_LOOKUPS: usize = 256;

/// Block tag of the live admission reads: pool identity is immutable.
const LIVE_BLOCK: &str = "latest";

/// What one [`learn_pools`] call did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolAdmissionReport {
    /// Emitters looked up on chain in this call.
    pub lookups: u64,
    pub admitted: u64,
    /// Refused emitters with the reason (sanitised, bounded text).
    pub refused: BTreeMap<Address, String>,
    /// Pending emitters not looked up because the cap was reached (they stay
    /// ungated and are asked again by a later call).
    pub deferred: u64,
}

#[must_use]
pub fn pool_kind(venue: SwapVenue) -> Option<PoolKind> {
    match venue {
        SwapVenue::UniswapV2 => Some(PoolKind::V2),
        SwapVenue::UniswapV3 => Some(PoolKind::V3),
        SwapVenue::AerodromeV2 => Some(PoolKind::AerodromeV2),
        SwapVenue::AerodromeSlipstream => Some(PoolKind::Slipstream),
        SwapVenue::PancakeV3 => Some(PoolKind::PancakeV3),
        SwapVenue::UniswapV4
        | SwapVenue::FourMemeV1
        | SwapVenue::FourMemeV2
        | SwapVenue::PonsV2Curve
        | SwapVenue::BagsCurve
        | SwapVenue::FlapPortal => None,
    }
}

/// The curve ABI a launchpad-curve venue's emitters have (`None` for pools).
#[must_use]
pub fn curve_kind(venue: SwapVenue) -> Option<CurveKind> {
    match venue {
        SwapVenue::PonsV2Curve => Some(CurveKind::PonsV2),
        SwapVenue::BagsCurve => Some(CurveKind::Bags),
        _ => None,
    }
}

#[must_use]
pub fn curve_venue(kind: CurveKind) -> SwapVenue {
    match kind {
        CurveKind::PonsV2 => SwapVenue::PonsV2Curve,
        CurveKind::Bags => SwapVenue::BagsCurve,
    }
}

/// The gate's view of what a curve reported.
#[must_use]
pub fn gate_curve_metadata(m: &CurveOnchainMetadata) -> CurveMetadata {
    CurveMetadata {
        factory: m.factory,
        token: m.token,
        quote: m.quote,
        registered_curve: m.registered_curve,
    }
}

/// The chain's pinned curve factories of `venue` (the only ones the
/// confirmation is asked from).
#[must_use]
pub fn pinned_curve_factories(chain_id: u64, venue: SwapVenue) -> Vec<Address> {
    VENUE_DEPLOYMENTS
        .iter()
        .filter(|d| d.chain_id == chain_id && d.venue == venue && d.role == AnchorRole::PoolFactory)
        .map(|d| d.anchor)
        .collect()
}

/// Admit every recorded curve (offline: fixture `curve_metadata` rows).
pub fn admit_recorded_curves(
    gate: &mut SwapVenueGate,
    recorded: &[CurveOnchainMetadata],
) -> PoolAdmissionReport {
    let mut report = PoolAdmissionReport::default();
    for m in recorded {
        match gate.admit_curve(curve_venue(m.kind), m.emitter, &gate_curve_metadata(m)) {
            Ok(_) => report.admitted += 1,
            Err(r) => {
                report.refused.insert(m.emitter, r.to_string());
            }
        }
    }
    report
}

#[must_use]
pub fn pool_venue(kind: PoolKind) -> SwapVenue {
    match kind {
        PoolKind::V2 => SwapVenue::UniswapV2,
        PoolKind::V3 => SwapVenue::UniswapV3,
        PoolKind::AerodromeV2 => SwapVenue::AerodromeV2,
        PoolKind::Slipstream => SwapVenue::AerodromeSlipstream,
        PoolKind::PancakeV3 => SwapVenue::PancakeV3,
    }
}

/// The gate's view of what an emitter reported.
#[must_use]
pub fn gate_metadata(m: &PoolOnchainMetadata) -> PoolMetadata {
    PoolMetadata {
        factory: m.factory,
        token0: m.token0,
        token1: m.token1,
        fee: m.fee,
        stable: m.stable,
        tick_spacing: m.tick_spacing,
        registered_pool: m.registered_pool,
    }
}

/// Admit every recorded emitter (offline: fixture `pool_metadata` rows).
/// Returns the report; refusals are remembered by the gate.
pub fn admit_recorded(
    gate: &mut SwapVenueGate,
    recorded: &[PoolOnchainMetadata],
) -> PoolAdmissionReport {
    let mut report = PoolAdmissionReport::default();
    for m in recorded {
        match gate.admit_pool(pool_venue(m.kind), m.emitter, &gate_metadata(m)) {
            Ok(_) => report.admitted += 1,
            Err(r) => {
                report.refused.insert(m.emitter, r.to_string());
            }
        }
    }
    report
}

/// Learn the v2/v3 pools of `txs` into `gate` (at most `max_lookups` emitters
/// are read on chain). Emitters of a venue the chain pins no factory for cost
/// nothing. `Err` only for run-terminal failures (request budget, rate limit,
/// transport): pools admitted before it stay admitted.
///
/// # Errors
/// The failing [`EvmSourceError`].
pub async fn learn_pools(
    gate: &mut SwapVenueGate,
    rpc: &EvmRpcClient,
    txs: &[RawEvmTransaction],
    max_lookups: usize,
) -> Result<PoolAdmissionReport, EvmSourceError> {
    let pending = gate.pending_pool_emitters(txs.iter().flat_map(|t| &t.logs));
    learn_emitters(gate, rpc, pending, max_lookups).await
}

/// [`learn_pools`] for a token scan: only emitters that MOVE `token` in the
/// same transaction (by its ERC-20 `Transfer` logs) are looked up, most
/// frequent first. Other swap emitters are route hops (ADR-020 amendment 10)
/// and need no admission; looking them up first could spend the bound before
/// the token's own pools on a busy token (live 2026-10-06: a BSC token with
/// hundreds of hop pools ended with its main pools never looked up).
///
/// # Errors
/// As [`learn_pools`].
pub async fn learn_pools_for_token(
    gate: &mut SwapVenueGate,
    rpc: &EvmRpcClient,
    txs: &[RawEvmTransaction],
    token: Address,
    max_lookups: usize,
) -> Result<PoolAdmissionReport, EvmSourceError> {
    let mut relevant: Vec<&scout_core::RawEvmLog> = Vec::new();
    let mut freq: BTreeMap<Address, usize> = BTreeMap::new();
    for tx in txs {
        let movers: std::collections::BTreeSet<Address> = tx
            .logs
            .iter()
            .filter(|l| {
                l.address == token
                    && l.topics.len() == 3
                    && l.topics.first() == Some(&scout_evm::TRANSFER_TOPIC0)
            })
            .flat_map(|l| l.topics.iter().skip(1).map(|t| Address::from_word(*t)))
            .collect();
        for l in tx.logs.iter().filter(|l| movers.contains(&l.address)) {
            relevant.push(l);
            *freq.entry(l.address).or_insert(0) += 1;
        }
    }
    let mut pending = gate.pending_pool_emitters(relevant);
    pending.sort_by(|a, b| freq.get(&b.1).cmp(&freq.get(&a.1)).then_with(|| a.cmp(b)));
    learn_emitters(gate, rpc, pending, max_lookups).await
}

async fn learn_emitters(
    gate: &mut SwapVenueGate,
    rpc: &EvmRpcClient,
    pending: Vec<(SwapVenue, Address)>,
    max_lookups: usize,
) -> Result<PoolAdmissionReport, EvmSourceError> {
    let mut report = PoolAdmissionReport::default();
    for (n, (venue, emitter)) in pending.into_iter().enumerate() {
        if n >= max_lookups {
            report.deferred += 1;
            continue;
        }
        if let Some(kind) = curve_kind(venue) {
            // Launchpad curve: the pinned factory's own record decides.
            report.lookups += 1;
            let factories = pinned_curve_factories(gate.chain_id(), venue);
            let meta = rpc
                .curve_metadata(emitter, kind, &factories, LIVE_BLOCK)
                .await?;
            match gate.admit_curve(venue, emitter, &gate_curve_metadata(&meta)) {
                Ok(_) => report.admitted += 1,
                Err(r) => {
                    report.refused.insert(emitter, r.to_string());
                }
            }
            continue;
        }
        let Some(kind) = pool_kind(venue) else {
            continue;
        };
        report.lookups += 1;
        // The topic can be shared by several pool families: the pinned
        // factory says which one this emitter is (and so which identity
        // calls and registry signature apply).
        let mut meta = {
            let gate_ro: &SwapVenueGate = gate;
            rpc.pool_identity_resolving(emitter, kind, LIVE_BLOCK, &|f| {
                gate_ro.factory_venue(venue, f).and_then(pool_kind)
            })
            .await?
        };
        let venue = pool_venue(meta.kind);
        if let Some(factory) = meta.factory
            && gate.needs_registry_record(venue, factory)
        {
            meta.registered_pool = rpc.registered_pool(&meta, LIVE_BLOCK).await?;
        }
        match gate.admit_pool(venue, emitter, &gate_metadata(&meta)) {
            Ok(_) => report.admitted += 1,
            Err(r) => {
                report.refused.insert(emitter, r.to_string());
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, address};
    use scout_core::RawEvmLog;
    use scout_dex_evm::{
        UNISWAP_V3_CANONICAL_INIT_CODE_HASH, V2_SWAP_EVENT_SIGNATURE, V3_SWAP_TOPIC0,
        VenueVerification, v3_pool_address_create2,
    };
    use scout_evm::{BASE, BSC, ROBINHOOD};
    use scout_rpc::{RpcClient, RpcEndpoint};
    use serde_json::{Value, json};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    const RH_FACTORY: Address = address!("1f7d7550b1b028f7571e69a784071f0205fd2efa");
    /// Uniswap v2 on Base: pinned factory, no init-code hash (the record decides).
    const BASE_V2_FACTORY: Address = address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6");
    const PANCAKE_V3_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
    const PANCAKE_V3_DEPLOYER: Address = address!("41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9");
    const T0: Address = address!("0000000000000000000000000000000000000011");
    const T1: Address = address!("0000000000000000000000000000000000000022");

    fn word(a: Address) -> String {
        format!("0x{:0>64}", format!("{a:x}"))
    }

    /// Answers pool/factory calls by `(to, selector)`; unknown targets revert.
    struct Node {
        /// emitter -> (factory, fee for v3)
        pools: Vec<(Address, Address, Option<u32>)>,
        /// factory -> record returned by getPool/getPair
        record: Option<(Address, Address)>,
    }
    impl Respond for Node {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let reply = |r: Value| {
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":r}))
            };
            assert_eq!(body["method"], "eth_call");
            let to: Address = body["params"][0]["to"].as_str().unwrap().parse().unwrap();
            let data = body["params"][0]["data"].as_str().unwrap();
            let sel = &data[..10];
            if let Some((factory, rec)) = self.record
                && to == factory
                && (sel == "0x1698ee82" || sel == "0xe6a43905")
            {
                return reply(json!(word(rec)));
            }
            let Some((_, factory, fee)) = self.pools.iter().find(|(p, _, _)| *p == to) else {
                return ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,
                    "error":{"code":3,"message":"execution reverted"}}));
            };
            match sel {
                "0xc45a0155" => reply(json!(word(*factory))),
                "0x0dfe1681" => reply(json!(word(T0))),
                "0xd21220a7" => reply(json!(word(T1))),
                "0xddca3f43" => reply(json!(format!("0x{:064x}", fee.unwrap_or(0)))),
                other => panic!("unexpected selector {other}"),
            }
        }
    }

    async fn rpc(
        server: &MockServer,
        profile: scout_evm::EvmChainProfile,
        budget: Option<u64>,
    ) -> EvmRpcClient {
        let c = RpcClient::new(RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
            .with_max_total_requests(budget);
        EvmRpcClient::new(c, profile)
    }

    fn tx_with(logs: Vec<RawEvmLog>) -> RawEvmTransaction {
        RawEvmTransaction {
            chain: ROBINHOOD.verified_chain_key(),
            hash: alloy_primitives::B256::repeat_byte(1),
            block_number: 1,
            transaction_index: 0,
            block_time: 1,
            from: Address::repeat_byte(0xaa),
            to: None,
            value: alloy_primitives::U256::ZERO,
            status: scout_core::EvmTxStatus::Success,
            gas_used: 1,
            effective_gas_price: alloy_primitives::U256::ZERO,
            l1_fee: None,
            logs,
            internal_transfers: None,
            native_source: None,
            native_balance_diff: None,
        }
    }

    fn swap(addr: Address, topic0: alloy_primitives::B256, data: usize) -> RawEvmLog {
        RawEvmLog {
            address: addr,
            topics: vec![topic0, Address::ZERO.into_word(), Address::ZERO.into_word()],
            data: Bytes::from(vec![0u8; data]),
            block_number: 1,
            transaction_index: 0,
            log_index: 0,
        }
    }

    fn calls(reqs: &[Request]) -> Vec<String> {
        reqs.iter()
            .map(|r| {
                let b: Value = serde_json::from_slice(&r.body).unwrap();
                b["params"][0]["data"].as_str().unwrap()[..10].to_string()
            })
            .collect()
    }

    fn token_transfer(token: Address, from: Address, to: Address) -> RawEvmLog {
        RawEvmLog {
            address: token,
            topics: vec![scout_evm::TRANSFER_TOPIC0, from.into_word(), to.into_word()],
            data: Bytes::from(vec![0u8; 32]),
            block_number: 1,
            transaction_index: 0,
            log_index: 0,
        }
    }

    #[tokio::test]
    async fn token_scan_admission_looks_up_only_the_tokens_pools_most_frequent_first() {
        let token = Address::repeat_byte(0x77);
        let w = Address::repeat_byte(0xaa);
        let real =
            v3_pool_address_create2(RH_FACTORY, T0, T1, 500, UNISWAP_V3_CANONICAL_INIT_CODE_HASH);
        let fork = Address::repeat_byte(0xf0); // moves the token once
        let hop = Address::repeat_byte(0xe0); // never moves the token
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![
                    (real, RH_FACTORY, Some(500)),
                    (fork, Address::repeat_byte(0xfa), Some(500)),
                ],
                record: None,
            })
            .mount(&s)
            .await;
        let client = rpc(&s, ROBINHOOD, None).await;
        let txs = vec![
            tx_with(vec![
                token_transfer(token, fork, w),
                swap(fork, V3_SWAP_TOPIC0, 160),
                swap(hop, V3_SWAP_TOPIC0, 160),
            ]),
            tx_with(vec![
                token_transfer(token, real, w),
                swap(real, V3_SWAP_TOPIC0, 160),
            ]),
            tx_with(vec![
                token_transfer(token, w, real),
                swap(real, V3_SWAP_TOPIC0, 160),
            ]),
        ];
        // bound 1: the token's most frequent pool (real, 2 txs) goes first
        let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
        let r = learn_pools_for_token(&mut gate, &client, &txs, token, 1)
            .await
            .unwrap();
        assert_eq!(
            (r.lookups, r.admitted, r.refused.len(), r.deferred),
            (1, 1, 0, 1)
        );
        // unbounded: fork is looked up and refused; the hop is never asked
        let r = learn_pools_for_token(&mut gate, &client, &txs, token, 10)
            .await
            .unwrap();
        assert_eq!(
            (r.lookups, r.admitted, r.refused.len(), r.deferred),
            (1, 0, 1, 0)
        );
        assert!(
            gate.pending_pool_emitters(txs.iter().flat_map(|t| &t.logs))
                .contains(&(SwapVenue::UniswapV3, hop))
        );
    }

    #[tokio::test]
    async fn create2_pinned_chain_admits_without_a_registry_call_and_caches_refusals() {
        let real =
            v3_pool_address_create2(RH_FACTORY, T0, T1, 500, UNISWAP_V3_CANONICAL_INIT_CODE_HASH);
        let fork = Address::repeat_byte(0xf0); // reports another factory
        let notpool = Address::repeat_byte(0xe0); // reverts
        let notpair = Address::repeat_byte(0xd0); // reverts
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![
                    (real, RH_FACTORY, Some(500)),
                    (fork, Address::repeat_byte(0xfa), Some(500)),
                ],
                record: None,
            })
            .mount(&s)
            .await;
        let client = rpc(&s, ROBINHOOD, None).await;
        let txs = vec![tx_with(vec![
            swap(real, V3_SWAP_TOPIC0, 160),
            swap(fork, V3_SWAP_TOPIC0, 160),
            swap(notpool, V3_SWAP_TOPIC0, 160),
            // v2 topic at a non-pair: looked up (Robinhood pins a v2 factory), refused.
            swap(notpair, V2_SWAP_EVENT_SIGNATURE, 128),
        ])];
        let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
        let r = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!(
            (r.lookups, r.admitted, r.refused.len(), r.deferred),
            (4, 1, 3, 0)
        );
        assert!(
            r.refused[&fork].contains("not a pinned official factory"),
            "{r:?}"
        );
        assert!(r.refused[&notpool].contains("no factory"), "{r:?}");
        assert_eq!(gate.known_pools(), 1);
        let reqs = s.received_requests().await.unwrap();
        // v3 pool: 4 identity calls, no getPool (CREATE2 pinned); fork: 4; not pool: 3 (+fee).
        assert!(
            !calls(&reqs).iter().any(|c| c == "0x1698ee82"),
            "{:?}",
            calls(&reqs)
        );
        // A second call over the same transactions asks nothing new.
        let n = reqs.len();
        let again = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!(again, PoolAdmissionReport::default());
        assert_eq!(s.received_requests().await.unwrap().len(), n);
    }

    /// Base families: the pinned factory decides which identity calls and
    /// which `getPool` signature apply to a topic shared by several families.
    struct BaseNode {
        slip_pool: Address,
        slip_factory: Address,
        aero_pool: Address,
        aero_factory: Address,
    }
    impl Respond for BaseNode {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let reply = |r: String| {
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":r}))
            };
            let to: Address = body["params"][0]["to"].as_str().unwrap().parse().unwrap();
            let data = body["params"][0]["data"].as_str().unwrap();
            let sel = &data[..10];
            match (to, sel) {
                (t, "0xc45a0155") if t == self.slip_pool => reply(word(self.slip_factory)),
                (t, "0xc45a0155") if t == self.aero_pool => reply(word(self.aero_factory)),
                (_, "0x0dfe1681") => reply(word(T0)),
                (_, "0xd21220a7") => reply(word(T1)),
                (t, "0xd0c93a7c") if t == self.slip_pool => reply(format!("0x{:064x}", 200)),
                (t, "0x22be3de1") if t == self.aero_pool => reply(format!("0x{:064x}", 0)),
                (t, "0x28af8d0b") if t == self.slip_factory => reply(word(self.slip_pool)),
                (t, "0x79bc57d5") if t == self.aero_factory => reply(word(self.aero_pool)),
                other => panic!("unexpected call {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn base_slipstream_and_aerodrome_pools_are_resolved_by_their_factory() {
        let slip_pool = Address::repeat_byte(0xb1);
        let aero_pool = Address::repeat_byte(0xa1);
        let slip_factory = address!("f8f2eB4940CFE7d13603DDDD87f123820Fc061Ef");
        let aero_factory = address!("420DD381b31aEf6683db6B902084cB0FFECe40Da");
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(BaseNode {
                slip_pool,
                slip_factory,
                aero_pool,
                aero_factory,
            })
            .mount(&s)
            .await;
        let client = rpc(&s, scout_evm::BASE, None).await;
        let txs = vec![tx_with(vec![
            // Slipstream shares the v3 topic; Aerodrome v2 emits its own.
            swap(slip_pool, V3_SWAP_TOPIC0, 160),
            swap(aero_pool, scout_dex_evm::AERODROME_V2_SWAP_TOPIC0, 128),
        ])];
        let mut gate = SwapVenueGate::new(8453);
        let r = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!((r.lookups, r.admitted, r.refused.len()), (2, 2, 0), "{r:?}");
        // Base Aerodrome deployments are FixtureVerified (evm_base_venues.rs).
        for l in &txs[0].logs {
            assert!(matches!(
                gate.classify(l),
                scout_dex_evm::GateOutcome::Verified(v) if v.verification == VenueVerification::FixtureVerified
            ));
        }
    }

    #[tokio::test]
    async fn lookup_cap_defers_the_rest_and_budget_exhaustion_is_an_error() {
        let a = Address::repeat_byte(0xa1);
        let b = Address::repeat_byte(0xa2);
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![],
                record: None,
            })
            .mount(&s)
            .await;
        let client = rpc(&s, ROBINHOOD, None).await;
        let txs = vec![tx_with(vec![
            swap(a, V3_SWAP_TOPIC0, 160),
            swap(b, V3_SWAP_TOPIC0, 160),
        ])];
        let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
        let r = learn_pools(&mut gate, &client, &txs, 1).await.unwrap();
        assert_eq!((r.lookups, r.deferred, r.refused.len()), (1, 1, 1));
        // The deferred emitter is still pending for a later call.
        assert_eq!(gate.pending_pool_emitters(txs[0].logs.iter()).len(), 1);

        let tight = rpc(&s, ROBINHOOD, Some(2)).await;
        let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
        let e = learn_pools(&mut gate, &tight, &txs, 10).await.unwrap_err();
        assert!(e.is_budget_exhausted(), "{e}");
    }

    #[tokio::test]
    async fn without_a_pinned_hash_the_factory_record_is_fetched_and_decides() {
        // Uniswap v2 on Base: pinned factory, no init-code hash -> getPair.
        let pair = Address::repeat_byte(0x77);
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![(pair, BASE_V2_FACTORY, None)],
                record: Some((BASE_V2_FACTORY, pair)),
            })
            .mount(&s)
            .await;
        let client = rpc(&s, BASE, None).await;
        let txs = vec![tx_with(vec![swap(pair, V2_SWAP_EVENT_SIGNATURE, 128)])];
        let mut gate = SwapVenueGate::new(BASE.chain_id);
        let r = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!((r.admitted, r.refused.len()), (1, 0), "{r:?}");
        assert!(
            calls(&s.received_requests().await.unwrap())
                .iter()
                .any(|c| c == "0xe6a43905")
        );
        // Base's v2 deployment is FixtureVerified (evm_base_venues.rs), so is the pool.
        assert!(matches!(
            gate.classify(&txs[0].logs[0]),
            scout_dex_evm::GateOutcome::Verified(v) if v.verification == VenueVerification::FixtureVerified
        ));

        // A factory that records another pair refuses it.
        let s2 = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![(pair, BASE_V2_FACTORY, None)],
                record: Some((BASE_V2_FACTORY, Address::repeat_byte(0x78))),
            })
            .mount(&s2)
            .await;
        let client2 = rpc(&s2, BASE, None).await;
        let mut gate2 = SwapVenueGate::new(BASE.chain_id);
        let r2 = learn_pools(&mut gate2, &client2, &txs, 10).await.unwrap();
        assert_eq!((r2.admitted, r2.refused.len()), (0, 1), "{r2:?}");
    }

    #[tokio::test]
    async fn bsc_pancake_v3_pool_is_admitted_through_the_pool_deployer_create2() {
        let pool = v3_pool_address_create2(
            PANCAKE_V3_DEPLOYER,
            T0,
            T1,
            2500,
            scout_dex_evm::PANCAKE_V3_INIT_CODE_HASH,
        );
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![(pool, PANCAKE_V3_FACTORY, Some(2500))],
                record: None,
            })
            .mount(&s)
            .await;
        let client = rpc(&s, BSC, None).await;
        let txs = vec![tx_with(vec![swap(
            pool,
            scout_dex_evm::PANCAKE_V3_SWAP_TOPIC0,
            224,
        )])];
        let mut gate = SwapVenueGate::new(BSC.chain_id);
        let r = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!((r.lookups, r.admitted, r.refused.len()), (1, 1, 0), "{r:?}");
        // Pinned hash: identity reads only, no getPool.
        assert!(
            !calls(&s.received_requests().await.unwrap())
                .iter()
                .any(|c| c == "0x1698ee82")
        );
        assert!(matches!(
            gate.classify(&txs[0].logs[0]),
            scout_dex_evm::GateOutcome::Verified(v) if v.venue == SwapVenue::PancakeV3
        ));
        assert_eq!(pool_kind(SwapVenue::PancakeV3), Some(PoolKind::PancakeV3));
        assert_eq!(pool_venue(PoolKind::PancakeV3), SwapVenue::PancakeV3);
    }

    /// Robinhood launchpad curves: Pons V2 (`factory()` + `getLaunchedToken`)
    /// and Bags (`TOKEN()` + `curveForToken`) are admitted by the factory's
    /// own record; a curve the factory does not confirm stays a gap.
    struct CurveNode {
        pons_factory: Address,
        bags_factory: Address,
    }
    impl Respond for CurveNode {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let reply = |r: String| {
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":r}))
            };
            let to: Address = body["params"][0]["to"].as_str().unwrap().parse().unwrap();
            let sel = &body["params"][0]["data"].as_str().unwrap()[..10];
            let token = Address::repeat_byte(0x70);
            let revert = || {
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,
                    "error":{"code":3,"message":"execution reverted"}}))
            };
            match (to.as_slice()[19], sel) {
                // Pons curves 0xc1 (confirmed) and 0xc2 (factory records 0xc1).
                (0xc1 | 0xc2, "0xc45a0155") => reply(word(self.pons_factory)),
                (0xc1 | 0xc2, "0xfc0c546a") => reply(word(token)),
                (0xc1 | 0xc2, "0x3de35b79") => reply(word(Address::ZERO)),
                (_, "0x3cf28b5a") if to == self.pons_factory => reply(format!(
                    "0x{}{}{}",
                    &word(token)[2..],
                    &word(Address::repeat_byte(0xc1))[2..],
                    "00".repeat(32 * 13)
                )),
                // Bags curve 0xb1.
                (0xb1, "0x82bfefc8") => reply(word(token)),
                (0xb1, "0xad5c4648") => reply(word(Address::repeat_byte(0x99))),
                (_, "0x8580756c") if to == self.bags_factory => {
                    reply(word(Address::repeat_byte(0xb1)))
                }
                _ => revert(),
            }
        }
    }

    #[tokio::test]
    async fn launchpad_curves_are_admitted_by_the_factorys_record_and_nothing_else() {
        let pons_factory = address!("7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e");
        let bags_factory = address!("e8Cc4431adF8b5A847C113EF0c6af9043219Cb37");
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(CurveNode {
                pons_factory,
                bags_factory,
            })
            .mount(&s)
            .await;
        let client = rpc(&s, ROBINHOOD, None).await;
        let (pons, pons_dup, bags, junk) = (
            Address::repeat_byte(0xc1),
            Address::repeat_byte(0xc2),
            Address::repeat_byte(0xb1),
            Address::repeat_byte(0xdd),
        );
        let ev = |a: Address, t: alloy_primitives::B256| swap(a, t, 128);
        let txs = vec![tx_with(vec![
            ev(pons, scout_dex_evm::PONS_V2_CURVE_BUY_TOPIC0),
            ev(pons_dup, scout_dex_evm::PONS_V2_CURVE_SELL_TOPIC0),
            swap(bags, scout_dex_evm::BAGS_TOKENS_BOUGHT_TOPIC0, 320),
            ev(junk, scout_dex_evm::PONS_V2_CURVE_BUY_TOPIC0),
        ])];
        let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
        let r = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!(
            (r.lookups, r.admitted, r.refused.len(), r.deferred),
            (4, 2, 2, 0),
            "{r:?}"
        );
        assert!(r.refused[&pons_dup].contains("factory records"), "{r:?}");
        assert!(r.refused[&junk].contains("no factory"), "{r:?}");
        assert_eq!(
            gate.curve_identity(pons).map(|c| (c.token, c.quote)),
            Some((Address::repeat_byte(0x70), Some(Address::ZERO)))
        );
        assert!(gate.curve_identity(bags).is_some());
        assert_eq!(curve_kind(SwapVenue::BagsCurve), Some(CurveKind::Bags));
        assert_eq!(curve_venue(CurveKind::PonsV2), SwapVenue::PonsV2Curve);
        assert_eq!(
            pinned_curve_factories(4663, SwapVenue::PonsV2Curve),
            vec![pons_factory]
        );
        // A second call asks nothing new.
        let n = s.received_requests().await.unwrap().len();
        let again = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!(again, PoolAdmissionReport::default());
        assert_eq!(s.received_requests().await.unwrap().len(), n);
    }
}
