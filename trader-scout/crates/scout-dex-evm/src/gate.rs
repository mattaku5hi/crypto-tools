//! Contract-address gates for venue swap events (ADR-020 §2b, invariant #16).
//!
//! A swap-shaped log counts only when it is emitted by a **gated** address on
//! the right chain at or after the deployment's activation block:
//! - Uniswap v4: the singleton PoolManager (every pool's `Swap` is emitted
//!   there).
//! - Uniswap v3 / v2-style pairs: pool addresses are verified through the
//!   factory's `PoolCreated`/`PairCreated` log ([`SwapVenueGate::register_factory_log`]),
//!   i.e. the factory is the trust anchor and the pool must have been
//!   announced by it. CREATE2 recomputation
//!   ([`crate::v3_pool_address_create2`]) is a cross-check only, because the
//!   init-code hash is per-deployment and not yet pinned.
//!
//! Every deployment starts at [`VenueVerification::IdlOnly`]: addresses come
//! from vendor docs (research doc §2), ABI shape is decoded, but no live
//! golden fixture exists yet. Flip to `FixtureVerified` only together with a
//! committed fixture and its evidence test. Robinhood Chain's Uniswap v4
//! PoolManager is `FixtureVerified` (ADR-020 step 2: 45 live `Swap` events
//! of fixture `evm_robinhood_token_aiden_v4_2026-10-03.json`, each matching
//! the PoolManager's ERC-20 `Transfer` deltas exactly; see
//! `crates/scout-engine/tests/evm_uniswap_v4_robinhood.rs`). Everything else
//! stays `IdlOnly`. `active_from_block` is `0` ("not pinned") until the
//! deployment transaction is read from the chain (the public RPC has no
//! historical state, so it cannot be derived offline).

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, address};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

use crate::uniswap::{
    V3_SWAP_TOPIC0, V4_SWAP_TOPIC0, decode_v2_pair_created, decode_v3_pool_created, decode_v3_swap,
    decode_v4_swap,
};
use crate::v2_swap::{V2_SWAP_EVENT_SIGNATURE, decode_v2_style_swap};

/// Evidence level of a venue deployment (invariant #16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VenueVerification {
    /// ABI shape decoded; address from vendor docs; no live fixture.
    IdlOnly,
    /// Live golden fixture committed and asserted against balance deltas.
    FixtureVerified,
}

impl VenueVerification {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::IdlOnly => "IdlOnly",
            Self::FixtureVerified => "FixtureVerified",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SwapVenue {
    UniswapV2,
    UniswapV3,
    UniswapV4,
}

impl SwapVenue {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UniswapV2 => "uniswap_v2_style",
            Self::UniswapV3 => "uniswap_v3",
            Self::UniswapV4 => "uniswap_v4",
        }
    }
}

/// What the anchor address of a deployment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorRole {
    /// The anchor itself emits `Swap` (v4 PoolManager).
    SwapEmitter,
    /// The anchor is a factory; pools are learned from its creation logs.
    PoolFactory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueDeployment {
    pub chain_id: u64,
    pub venue: SwapVenue,
    pub anchor: Address,
    pub role: AnchorRole,
    /// First block of activity; `0` = not pinned yet.
    pub active_from_block: u64,
    pub verification: VenueVerification,
}

const fn dep(
    chain_id: u64,
    venue: SwapVenue,
    anchor: Address,
    role: AnchorRole,
) -> VenueDeployment {
    VenueDeployment {
        chain_id,
        venue,
        anchor,
        role,
        active_from_block: 0,
        verification: VenueVerification::IdlOnly,
    }
}

const fn dep_fixture_verified(
    chain_id: u64,
    venue: SwapVenue,
    anchor: Address,
    role: AnchorRole,
) -> VenueDeployment {
    VenueDeployment {
        chain_id,
        venue,
        anchor,
        role,
        active_from_block: 0,
        verification: VenueVerification::FixtureVerified,
    }
}

/// Deployments by chain. Sources: research doc §2.2-2.4 (Uniswap/Pancake
/// official deployment pages, 2026-10-03). Robinhood v4/v3 are the first
/// priority (ADR-020 §4); Base/BSC rows are present but equally IdlOnly.
pub const VENUE_DEPLOYMENTS: &[VenueDeployment] = &[
    // Robinhood Chain (4663): v4 is FixtureVerified (see module docs).
    dep_fixture_verified(
        4663,
        SwapVenue::UniswapV4,
        address!("8366a39cc670b4001a1121b8f6a443a643e40951"),
        AnchorRole::SwapEmitter,
    ),
    dep(
        4663,
        SwapVenue::UniswapV3,
        address!("1f7d7550b1b028f7571e69a784071f0205fd2efa"),
        AnchorRole::PoolFactory,
    ),
    // Base (8453)
    dep(
        8453,
        SwapVenue::UniswapV4,
        address!("498581ff718922c3f8e6a244956af099b2652b2b"),
        AnchorRole::SwapEmitter,
    ),
    dep(
        8453,
        SwapVenue::UniswapV3,
        address!("33128a8fC17869897dcE68Ed026d694621f6FDfD"),
        AnchorRole::PoolFactory,
    ),
    // BSC (56)
    dep(
        56,
        SwapVenue::UniswapV4,
        address!("28e2ea090877bf75740558f6bfb36a5ffee9e9df"),
        AnchorRole::SwapEmitter,
    ),
    dep(
        56,
        SwapVenue::UniswapV3,
        address!("dB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7"),
        AnchorRole::PoolFactory,
    ),
    // PancakeSwap v2 factory on BSC (v2 Swap/PairCreated shape).
    dep(
        56,
        SwapVenue::UniswapV2,
        address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73"),
        AnchorRole::PoolFactory,
    ),
];

/// A swap event that passed the address gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSwap {
    pub venue: SwapVenue,
    /// The contract that emitted the event (pool / PoolManager). Token flows
    /// to/from this address in the same tx tie the event to a token.
    pub emitter: Address,
    /// v4 pool id (hash); `None` for v2/v3.
    pub pool_id: Option<B256>,
    pub verification: VenueVerification,
    pub log_index: u64,
}

/// Result of gating one log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Not a swap-shaped log for any supported venue.
    NotSwap,
    /// Swap-shaped (topic0 + valid structure) but the emitter is not gated:
    /// never counted as venue evidence (invariant #16).
    UngatedEmitter {
        venue: SwapVenue,
        emitter: Address,
    },
    Verified(VerifiedSwap),
    /// Swap topic0 at a gated emitter with a broken structure (invariant #18).
    Malformed(String),
}

/// Per-chain gate. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SwapVenueGate {
    chain_id: u64,
    /// Learned pools: pool address -> (venue, verification, active_from).
    pools: BTreeMap<Address, (SwapVenue, VenueVerification, u64)>,
}

impl SwapVenueGate {
    #[must_use]
    pub fn new(chain_id: u64) -> Self {
        Self {
            chain_id,
            pools: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn deployments(&self) -> impl Iterator<Item = &'static VenueDeployment> + use<'_> {
        let chain_id = self.chain_id;
        VENUE_DEPLOYMENTS
            .iter()
            .filter(move |d| d.chain_id == chain_id)
    }

    /// Learn a pool from a factory creation log. Returns the pool when the
    /// log was emitted by a gated factory of this chain and decoded; `Err`
    /// for a structurally broken creation log from a gated factory; `Ok(None)`
    /// for anything else.
    pub fn register_factory_log(&mut self, log: &RawEvmLog) -> Result<Option<Address>, String> {
        let Some(d) = self
            .deployments()
            .find(|d| d.role == AnchorRole::PoolFactory && d.anchor == log.address)
        else {
            return Ok(None);
        };
        let pool = match d.venue {
            SwapVenue::UniswapV3 => match decode_v3_pool_created(log) {
                DecodeOutcome::Decoded(c) => c.pool,
                DecodeOutcome::NotMine => return Ok(None),
                DecodeOutcome::Malformed(m) => return Err(m),
            },
            SwapVenue::UniswapV2 => match decode_v2_pair_created(log) {
                DecodeOutcome::Decoded(c) => c.pair,
                DecodeOutcome::NotMine => return Ok(None),
                DecodeOutcome::Malformed(m) => return Err(m),
            },
            SwapVenue::UniswapV4 => return Ok(None),
        };
        self.pools
            .insert(pool, (d.venue, d.verification, d.active_from_block));
        Ok(Some(pool))
    }

    /// Number of factory-announced pools learned so far.
    #[must_use]
    pub fn known_pools(&self) -> usize {
        self.pools.len()
    }

    /// Mark all deployments' verification via a caller-supplied upgrade
    /// (used by tests and by callers that hold a fixture proof).
    pub fn register_known_pool(
        &mut self,
        venue: SwapVenue,
        pool: Address,
        verification: VenueVerification,
    ) {
        self.pools.insert(pool, (venue, verification, 0));
    }

    /// Classify one log.
    #[must_use]
    pub fn classify(&self, log: &RawEvmLog) -> GateOutcome {
        let Some(topic0) = log.topics.first() else {
            return GateOutcome::NotSwap;
        };
        let venue = if *topic0 == V4_SWAP_TOPIC0 {
            SwapVenue::UniswapV4
        } else if *topic0 == V3_SWAP_TOPIC0 {
            SwapVenue::UniswapV3
        } else if *topic0 == V2_SWAP_EVENT_SIGNATURE {
            SwapVenue::UniswapV2
        } else {
            return GateOutcome::NotSwap;
        };
        let gated = match venue {
            SwapVenue::UniswapV4 => self
                .deployments()
                .find(|d| {
                    d.venue == venue && d.role == AnchorRole::SwapEmitter && d.anchor == log.address
                })
                .map(|d| (d.verification, d.active_from_block)),
            _ => self
                .pools
                .get(&log.address)
                .filter(|(v, _, _)| *v == venue)
                .map(|(_, ver, from)| (*ver, *from)),
        };
        let Some((verification, active_from)) = gated else {
            return GateOutcome::UngatedEmitter {
                venue,
                emitter: log.address,
            };
        };
        if log.block_number < active_from {
            return GateOutcome::UngatedEmitter {
                venue,
                emitter: log.address,
            };
        }
        let (pool_id, log_index) = match venue {
            SwapVenue::UniswapV4 => match decode_v4_swap(log) {
                DecodeOutcome::Decoded(s) => (Some(s.pool_id), s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::UniswapV3 => match decode_v3_swap(log) {
                DecodeOutcome::Decoded(s) => (None, s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::UniswapV2 => match decode_v2_style_swap(log) {
                DecodeOutcome::Decoded(s) => (None, s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
        };
        GateOutcome::Verified(VerifiedSwap {
            venue,
            emitter: log.address,
            pool_id,
            verification,
            log_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, U256};

    use super::*;
    use crate::uniswap::{V2_PAIR_CREATED_TOPIC0, V3_POOL_CREATED_TOPIC0};

    const RH: u64 = 4663;

    fn log(address: Address, topics: Vec<B256>, data: Vec<u8>) -> RawEvmLog {
        RawEvmLog {
            address,
            topics,
            data: Bytes::from(data),
            block_number: 100,
            transaction_index: 0,
            log_index: 2,
        }
    }

    fn v4_swap_log(emitter: Address) -> RawEvmLog {
        log(
            emitter,
            vec![
                V4_SWAP_TOPIC0,
                B256::repeat_byte(1),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 192],
        )
    }

    #[test]
    fn robinhood_pool_manager_is_gated_fixture_verified() {
        let pm = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
        let gate = SwapVenueGate::new(RH);
        match gate.classify(&v4_swap_log(pm)) {
            GateOutcome::Verified(v) => {
                assert_eq!(v.venue, SwapVenue::UniswapV4);
                assert_eq!(v.verification, VenueVerification::FixtureVerified);
                assert_eq!(v.emitter, pm);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_robinhood_v4_is_fixture_verified() {
        for d in VENUE_DEPLOYMENTS {
            let expected = if d.chain_id == 4663 && d.venue == SwapVenue::UniswapV4 {
                VenueVerification::FixtureVerified
            } else {
                VenueVerification::IdlOnly
            };
            assert_eq!(d.verification, expected, "{d:?}");
        }
    }

    #[test]
    fn same_topic_at_other_address_or_chain_is_not_verified() {
        let pm = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
        let gate = SwapVenueGate::new(RH);
        assert!(matches!(
            gate.classify(&v4_swap_log(Address::repeat_byte(9))),
            GateOutcome::UngatedEmitter { .. }
        ));
        // Robinhood PoolManager address on Base is not a gated emitter.
        assert!(matches!(
            SwapVenueGate::new(8453).classify(&v4_swap_log(pm)),
            GateOutcome::UngatedEmitter { .. }
        ));
    }

    #[test]
    fn malformed_swap_at_gated_emitter_surfaces() {
        let pm = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
        let bad = log(
            pm,
            vec![V4_SWAP_TOPIC0, B256::ZERO, B256::ZERO],
            vec![0; 10],
        );
        assert!(matches!(
            SwapVenueGate::new(RH).classify(&bad),
            GateOutcome::Malformed(_)
        ));
    }

    #[test]
    fn v3_pool_verified_only_after_factory_announces_it() {
        let factory = address!("1f7d7550b1b028f7571e69a784071f0205fd2efa");
        let pool = Address::repeat_byte(0x77);
        let swap = log(
            pool,
            vec![
                V3_SWAP_TOPIC0,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 160],
        );
        let mut gate = SwapVenueGate::new(RH);
        assert!(matches!(
            gate.classify(&swap),
            GateOutcome::UngatedEmitter { .. }
        ));

        let mut pool_word = [0u8; 32];
        pool_word[12..].copy_from_slice(pool.as_slice());
        let mut data = vec![0u8; 32];
        data.extend_from_slice(&pool_word);
        let created = log(
            factory,
            vec![
                V3_POOL_CREATED_TOPIC0,
                Address::repeat_byte(1).into_word(),
                Address::repeat_byte(2).into_word(),
                B256::from(U256::from(3000u64).to_be_bytes::<32>()),
            ],
            data.clone(),
        );
        assert_eq!(gate.register_factory_log(&created), Ok(Some(pool)));
        assert!(
            matches!(gate.classify(&swap), GateOutcome::Verified(v) if v.venue == SwapVenue::UniswapV3)
        );

        // A creation-shaped log from a non-factory address teaches nothing.
        let mut fake = created.clone();
        fake.address = Address::repeat_byte(0xee);
        assert_eq!(gate.register_factory_log(&fake), Ok(None));
        // A v3 pool is not a v2 venue: same address with v2 topic is ungated.
        let v2 = log(
            pool,
            vec![
                V2_SWAP_EVENT_SIGNATURE,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 128],
        );
        assert!(matches!(
            gate.classify(&v2),
            GateOutcome::UngatedEmitter { .. }
        ));
        let _ = V2_PAIR_CREATED_TOPIC0;
    }

    #[test]
    fn non_swap_logs_are_not_swaps() {
        let l = log(Address::ZERO, vec![B256::ZERO], vec![]);
        assert_eq!(SwapVenueGate::new(RH).classify(&l), GateOutcome::NotSwap);
        let none = log(Address::ZERO, vec![], vec![]);
        assert_eq!(SwapVenueGate::new(RH).classify(&none), GateOutcome::NotSwap);
    }
}
