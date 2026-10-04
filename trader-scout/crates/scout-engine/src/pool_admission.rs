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
use scout_dex_evm::{PoolMetadata, SwapVenue, SwapVenueGate};
use scout_providers::{EvmRpcClient, EvmSourceError, PoolKind, PoolOnchainMetadata};

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
        SwapVenue::UniswapV4 => None,
    }
}

#[must_use]
pub fn pool_venue(kind: PoolKind) -> SwapVenue {
    match kind {
        PoolKind::V2 => SwapVenue::UniswapV2,
        PoolKind::V3 => SwapVenue::UniswapV3,
        PoolKind::AerodromeV2 => SwapVenue::AerodromeV2,
        PoolKind::Slipstream => SwapVenue::AerodromeSlipstream,
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
    let mut report = PoolAdmissionReport::default();
    for (n, (venue, emitter)) in pending.into_iter().enumerate() {
        if n >= max_lookups {
            report.deferred += 1;
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
    use scout_evm::{BSC, ROBINHOOD};
    use scout_rpc::{RpcClient, RpcEndpoint};
    use serde_json::{Value, json};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    const RH_FACTORY: Address = address!("1f7d7550b1b028f7571e69a784071f0205fd2efa");
    const BSC_V2_FACTORY: Address = address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73");
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
        // IdlOnly deployments: admitted at their own (unverified) level.
        for l in &txs[0].logs {
            assert!(matches!(
                gate.classify(l),
                scout_dex_evm::GateOutcome::Verified(v) if v.verification == VenueVerification::IdlOnly
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
        // PancakeSwap v2 on BSC: pinned factory, no init-code hash -> getPair.
        let pair = Address::repeat_byte(0x77);
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![(pair, BSC_V2_FACTORY, None)],
                record: Some((BSC_V2_FACTORY, pair)),
            })
            .mount(&s)
            .await;
        let client = rpc(&s, BSC, None).await;
        let txs = vec![tx_with(vec![swap(pair, V2_SWAP_EVENT_SIGNATURE, 128)])];
        let mut gate = SwapVenueGate::new(BSC.chain_id);
        let r = learn_pools(&mut gate, &client, &txs, 10).await.unwrap();
        assert_eq!((r.admitted, r.refused.len()), (1, 0), "{r:?}");
        assert!(
            calls(&s.received_requests().await.unwrap())
                .iter()
                .any(|c| c == "0xe6a43905")
        );
        // The deployment is IdlOnly, so is the pool.
        assert!(matches!(
            gate.classify(&txs[0].logs[0]),
            scout_dex_evm::GateOutcome::Verified(v) if v.verification == VenueVerification::IdlOnly
        ));

        // A factory that records another pair refuses it.
        let s2 = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Node {
                pools: vec![(pair, BSC_V2_FACTORY, None)],
                record: Some((BSC_V2_FACTORY, Address::repeat_byte(0x78))),
            })
            .mount(&s2)
            .await;
        let client2 = rpc(&s2, BSC, None).await;
        let mut gate2 = SwapVenueGate::new(BSC.chain_id);
        let r2 = learn_pools(&mut gate2, &client2, &txs, 10).await.unwrap();
        assert_eq!((r2.admitted, r2.refused.len()), (0, 1), "{r2:?}");
    }
}
