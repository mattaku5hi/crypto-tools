//! Contract-address gates for venue swap events (ADR-020 §2b, invariant #16).
//!
//! A swap-shaped log counts only when it is emitted by a **gated** address on
//! the right chain at or after the deployment's activation block:
//! - Uniswap v4: the singleton PoolManager (every pool's `Swap` is emitted
//!   there).
//! - Uniswap v3 / v2-style pairs: a pool is admitted by
//!   [`SwapVenueGate::admit_pool`] from what the emitter reports on chain
//!   ([`PoolMetadata`], read through bounded `eth_call`s): (i) its `factory()`
//!   is a pinned official factory of this chain and venue, AND (ii) either its
//!   address is the CREATE2 address of `(factory, token0, token1[, fee],
//!   init-code hash)` when the deployment pins a hash
//!   ([`VenueDeployment::init_code_hash`], pinned only where it reproduces
//!   every fixture pool), or the factory's own `getPool`/`getPair` answer is
//!   the emitter. A pool nobody admitted stays a coverage gap
//!   ([`GateOutcome::UngatedEmitter`]).
//!
//! Every deployment starts at [`VenueVerification::IdlOnly`]: addresses come
//! from vendor docs (research doc §2), ABI shape is decoded, but no live
//! golden fixture exists yet. Flip to `FixtureVerified` only together with a
//! committed fixture and its evidence test. Robinhood Chain's Uniswap v4
//! PoolManager is `FixtureVerified` (ADR-020 step 2: 45 live `Swap` events
//! of fixture `evm_robinhood_token_aiden_v4_2026-10-03.json`, each matching
//! the PoolManager's ERC-20 `Transfer` deltas exactly; see
//! `crates/scout-engine/tests/evm_uniswap_v4_robinhood.rs`), and so is the
//! Robinhood Uniswap v3 and v2 factories' pool families (evidence:
//! `crates/scout-engine/tests/evm_uniswap_v2v3_robinhood.rs`,
//! `docs/p0/measurements/2026-10-04-uniswap-v2v3-robinhood-verification.md`).
//! Everything else stays `IdlOnly`. `active_from_block` is `0` ("not
//! pinned") until the deployment transaction is read from the chain (the
//! public RPC has no historical state, so it cannot be derived offline).

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, address, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

use crate::uniswap::{
    V3_SWAP_TOPIC0, V4_SWAP_TOPIC0, decode_v3_swap, decode_v4_swap, v2_pair_address_create2,
    v3_pool_address_create2,
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
    /// CREATE2 init-code hash of this factory's pools; `None` = not pinned
    /// (then the factory's `getPool`/`getPair` record is the check).
    pub init_code_hash: Option<B256>,
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
        init_code_hash: None,
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
        init_code_hash: None,
    }
}

/// Canonical Uniswap v3 pool init-code hash. Pinned for a deployment only
/// where it reproduces every fixture pool of that chain.
pub const UNISWAP_V3_CANONICAL_INIT_CODE_HASH: B256 =
    b256!("e34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54");
/// Canonical Uniswap v2 pair init-code hash (pinned for the Robinhood v2 factory: reproduces its 8 live pairs).
pub const UNISWAP_V2_CANONICAL_INIT_CODE_HASH: B256 =
    b256!("96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f");

const fn with_init_code_hash(mut d: VenueDeployment, hash: B256) -> VenueDeployment {
    d.init_code_hash = Some(hash);
    d
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
    // Uniswap v3 factory (developers.uniswap.org Robinhood deployments page).
    // The canonical init-code hash reproduces every fixture pool; FixtureVerified
    // by the v2/v3 evidence test.
    with_init_code_hash(
        dep_fixture_verified(
            4663,
            SwapVenue::UniswapV3,
            address!("1f7d7550b1b028f7571e69a784071f0205fd2efa"),
            AnchorRole::PoolFactory,
        ),
        UNISWAP_V3_CANONICAL_INIT_CODE_HASH,
    ),
    // Uniswap v2 factory (developers.uniswap.org v2 deployments, Robinhood
    // Factory; Router02 0x89e5db8b5aa49aa85ac63f691524311aeb649eba, fetched
    // 2026-10-04). The canonical v2 init-code hash reproduces all 8 live
    // pairs of the fixture (whose `getPair` records agree); FixtureVerified by
    // the v2/v3 evidence test.
    with_init_code_hash(
        dep_fixture_verified(
            4663,
            SwapVenue::UniswapV2,
            address!("8bceaa40b9acdfaedf85adf4ff01f5ad6517937f"),
            AnchorRole::PoolFactory,
        ),
        UNISWAP_V2_CANONICAL_INIT_CODE_HASH,
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
    // Uniswap v2 factory on Base (developers.uniswap.org v2 deployments,
    // fetched 2026-10-04; Router02 0x4752ba5dbc23f44d87826276bf6fd6b1c372ad24).
    dep(
        8453,
        SwapVenue::UniswapV2,
        address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6"),
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
    // Uniswap v2 factory on BNB Chain (developers.uniswap.org v2 deployments,
    // fetched 2026-10-04; Router02 0x4752ba5DBc23f44D87826276BF6fD6b1C372aD24).
    dep(
        56,
        SwapVenue::UniswapV2,
        address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6"),
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

/// What a v2/v3 swap emitter reports on chain (bounded `eth_call`s at some
/// block). Admission input; the gate trusts none of it beyond the checks in
/// [`SwapVenueGate::admit_pool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoolMetadata {
    /// `emitter.factory()`.
    pub factory: Option<Address>,
    /// `emitter.token0()` / `token1()`.
    pub token0: Option<Address>,
    pub token1: Option<Address>,
    /// `emitter.fee()` (v3).
    pub fee: Option<u32>,
    /// `factory.getPool(token0, token1, fee)` / `getPair(token0, token1)`.
    pub registered_pool: Option<Address>,
}

/// Why an emitter was not admitted as a pool (all are coverage gaps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolRejection {
    /// The emitter did not report `factory()` (not a pool of this kind).
    NoFactory,
    /// `factory()` is not a pinned factory of this chain and venue.
    UnpinnedFactory(Address),
    /// The emitter did not report `token0()`/`token1()` (or `fee()` for v3),
    /// or the tokens are not strictly ordered.
    IncompleteIdentity,
    /// The pinned init-code hash gives another address.
    Create2Mismatch { computed: Address },
    /// No pinned hash and the factory's own record is not this emitter.
    NotRegisteredByFactory { record: Option<Address> },
    /// `getPool`/`getPair` disagrees with the CREATE2 address (a pinned hash
    /// is cross-checked when the record was fetched).
    RecordMismatch { record: Address },
    /// The metadata lookup itself did not complete (budget/cap); the emitter
    /// stays a coverage gap.
    NotLookedUp(String),
}

impl std::fmt::Display for PoolRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFactory => write!(f, "emitter reports no factory()"),
            Self::UnpinnedFactory(a) => {
                write!(f, "factory {a:#x} is not a pinned official factory")
            }
            Self::IncompleteIdentity => write!(f, "token0/token1/fee missing or unordered"),
            Self::Create2Mismatch { computed } => {
                write!(f, "CREATE2 address is {computed:#x}, not the emitter")
            }
            Self::NotRegisteredByFactory { record } => match record {
                Some(r) => write!(f, "factory records {r:#x} for this pair, not the emitter"),
                None => write!(f, "factory has no record of this pair"),
            },
            Self::RecordMismatch { record } => {
                write!(
                    f,
                    "factory records {record:#x} for this pair, not the emitter"
                )
            }
            Self::NotLookedUp(why) => write!(f, "metadata not looked up: {why}"),
        }
    }
}

/// Per-chain gate. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SwapVenueGate {
    chain_id: u64,
    /// Admitted pools: pool address -> (venue, verification, active_from).
    pools: BTreeMap<Address, (SwapVenue, VenueVerification, u64)>,
    /// Emitters already checked and refused (never re-asked within a run).
    rejected: BTreeMap<Address, PoolRejection>,
}

impl SwapVenueGate {
    #[must_use]
    pub fn new(chain_id: u64) -> Self {
        Self {
            chain_id,
            pools: BTreeMap::new(),
            rejected: BTreeMap::new(),
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

    /// `true` when this chain has a pinned factory for `venue`: only then can
    /// a swap emitter of that venue ever be admitted (no metadata lookups are
    /// spent otherwise).
    #[must_use]
    pub fn has_factory(&self, venue: SwapVenue) -> bool {
        self.deployments()
            .any(|d| d.venue == venue && d.role == AnchorRole::PoolFactory)
    }

    /// `true` when the factory's `getPool`/`getPair` record is needed to admit
    /// a pool of `factory` (no pinned init-code hash). Lets callers skip that
    /// request when CREATE2 alone decides.
    #[must_use]
    pub fn needs_registry_record(&self, venue: SwapVenue, factory: Address) -> bool {
        self.deployments()
            .find(|d| d.venue == venue && d.role == AnchorRole::PoolFactory && d.anchor == factory)
            .is_some_and(|d| d.init_code_hash.is_none())
    }

    /// Swap-shaped v2/v3 emitters of `logs` that could still be admitted:
    /// not admitted, not refused yet, and the chain pins a factory for the
    /// venue. Sorted, deduplicated.
    #[must_use]
    pub fn pending_pool_emitters<'a>(
        &self,
        logs: impl IntoIterator<Item = &'a RawEvmLog>,
    ) -> Vec<(SwapVenue, Address)> {
        let mut out = std::collections::BTreeSet::new();
        for log in logs {
            let venue = match log.topics.first() {
                Some(t) if *t == V3_SWAP_TOPIC0 => SwapVenue::UniswapV3,
                Some(t) if *t == V2_SWAP_EVENT_SIGNATURE => SwapVenue::UniswapV2,
                _ => continue,
            };
            if !self.has_factory(venue)
                || self.pools.contains_key(&log.address)
                || self.rejected.contains_key(&log.address)
            {
                continue;
            }
            out.insert((venue, log.address));
        }
        out.into_iter().collect()
    }

    /// Admit (or refuse, remembering why) the v2/v3 pool `emitter` from its
    /// reported `meta`. On success the pool takes the verification level of
    /// its factory's deployment.
    ///
    /// # Errors
    /// The [`PoolRejection`] (also remembered: the emitter is not re-asked).
    pub fn admit_pool(
        &mut self,
        venue: SwapVenue,
        emitter: Address,
        meta: &PoolMetadata,
    ) -> Result<VenueVerification, PoolRejection> {
        match self.check_pool(venue, emitter, meta) {
            Ok((verification, active_from)) => {
                self.rejected.remove(&emitter);
                self.pools
                    .insert(emitter, (venue, verification, active_from));
                Ok(verification)
            }
            Err(r) => {
                self.rejected.insert(emitter, r.clone());
                Err(r)
            }
        }
    }

    /// Remember that the metadata of `emitter` could not be read (budget,
    /// cap): it stays ungated and is not re-asked in this run.
    pub fn refuse_pool(&mut self, emitter: Address, why: impl Into<String>) {
        self.rejected
            .insert(emitter, PoolRejection::NotLookedUp(why.into()));
    }

    fn check_pool(
        &self,
        venue: SwapVenue,
        emitter: Address,
        meta: &PoolMetadata,
    ) -> Result<(VenueVerification, u64), PoolRejection> {
        let factory = meta.factory.ok_or(PoolRejection::NoFactory)?;
        let d = self
            .deployments()
            .find(|d| d.venue == venue && d.role == AnchorRole::PoolFactory && d.anchor == factory)
            .ok_or(PoolRejection::UnpinnedFactory(factory))?;
        let (Some(t0), Some(t1)) = (meta.token0, meta.token1) else {
            return Err(PoolRejection::IncompleteIdentity);
        };
        if t0 >= t1 {
            return Err(PoolRejection::IncompleteIdentity);
        }
        if let Some(hash) = d.init_code_hash {
            let computed = match venue {
                SwapVenue::UniswapV3 => {
                    let fee = meta.fee.ok_or(PoolRejection::IncompleteIdentity)?;
                    v3_pool_address_create2(factory, t0, t1, fee, hash)
                }
                SwapVenue::UniswapV2 => v2_pair_address_create2(factory, t0, t1, hash),
                SwapVenue::UniswapV4 => return Err(PoolRejection::UnpinnedFactory(factory)),
            };
            if computed != emitter {
                return Err(PoolRejection::Create2Mismatch { computed });
            }
            if let Some(record) = meta.registered_pool
                && record != emitter
            {
                return Err(PoolRejection::RecordMismatch { record });
            }
        } else {
            if venue == SwapVenue::UniswapV3 && meta.fee.is_none() {
                return Err(PoolRejection::IncompleteIdentity);
            }
            if meta.registered_pool != Some(emitter) {
                return Err(PoolRejection::NotRegisteredByFactory {
                    record: meta.registered_pool,
                });
            }
        }
        Ok((d.verification, d.active_from_block))
    }

    /// Refusals so far (emitter, reason), for coverage reporting.
    pub fn rejections(&self) -> impl Iterator<Item = (&Address, &PoolRejection)> {
        self.rejected.iter()
    }

    /// Number of admitted pools so far.
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
    use alloy_primitives::Bytes;

    use super::*;

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
    fn only_robinhood_deployments_are_fixture_verified() {
        for d in VENUE_DEPLOYMENTS {
            let expected = if d.chain_id == 4663 {
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

    const RH_V3_FACTORY: Address = address!("1f7d7550b1b028f7571e69a784071f0205fd2efa");
    const T0: Address = address!("0000000000000000000000000000000000000011");
    const T1: Address = address!("0000000000000000000000000000000000000022");

    fn v3_swap_at(pool: Address) -> RawEvmLog {
        log(
            pool,
            vec![
                V3_SWAP_TOPIC0,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 160],
        )
    }

    fn v3_create2(fee: u32) -> Address {
        v3_pool_address_create2(
            RH_V3_FACTORY,
            T0,
            T1,
            fee,
            UNISWAP_V3_CANONICAL_INIT_CODE_HASH,
        )
    }

    fn v3_meta(fee: u32) -> PoolMetadata {
        PoolMetadata {
            factory: Some(RH_V3_FACTORY),
            token0: Some(T0),
            token1: Some(T1),
            fee: Some(fee),
            registered_pool: None,
        }
    }

    #[test]
    fn v3_pool_is_admitted_only_when_factory_and_create2_agree() {
        let pool = v3_create2(500);
        let swap = v3_swap_at(pool);
        let mut gate = SwapVenueGate::new(RH);
        assert!(matches!(
            gate.classify(&swap),
            GateOutcome::UngatedEmitter { .. }
        ));
        assert_eq!(
            gate.pending_pool_emitters([&swap]),
            vec![(SwapVenue::UniswapV3, pool)]
        );
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV3, pool, &v3_meta(500)),
            Ok(VenueVerification::FixtureVerified)
        );
        assert!(
            matches!(gate.classify(&swap), GateOutcome::Verified(v) if v.venue == SwapVenue::UniswapV3
                && v.verification == VenueVerification::FixtureVerified)
        );
        // Admitted pools are not asked again.
        assert!(gate.pending_pool_emitters([&swap]).is_empty());
        // A v3 pool is not a v2 venue: same address with the v2 topic is ungated.
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
    }

    #[test]
    fn v3_admission_refusals_are_typed_and_remembered() {
        let pool = v3_create2(500);
        let mut gate = SwapVenueGate::new(RH);
        // Wrong fee: CREATE2 gives another address.
        let e = gate
            .admit_pool(SwapVenue::UniswapV3, pool, &v3_meta(3000))
            .unwrap_err();
        assert!(
            matches!(e, PoolRejection::Create2Mismatch { computed } if computed == v3_create2(3000))
        );
        assert!(matches!(
            gate.classify(&v3_swap_at(pool)),
            GateOutcome::UngatedEmitter { .. }
        ));
        // Remembered: not pending any more, reported as a rejection.
        assert!(gate.pending_pool_emitters([&v3_swap_at(pool)]).is_empty());
        assert_eq!(gate.rejections().count(), 1);

        // A fork pool: right shape, other factory.
        let other = Address::repeat_byte(0xfa);
        let fork = PoolMetadata {
            factory: Some(other),
            ..v3_meta(500)
        };
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV3, Address::repeat_byte(5), &fork),
            Err(PoolRejection::UnpinnedFactory(other))
        );
        // Not a pool at all.
        assert_eq!(
            gate.admit_pool(
                SwapVenue::UniswapV3,
                Address::repeat_byte(6),
                &PoolMetadata::default()
            ),
            Err(PoolRejection::NoFactory)
        );
        // Missing fee, unordered tokens.
        let no_fee = PoolMetadata {
            fee: None,
            ..v3_meta(500)
        };
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV3, pool, &no_fee),
            Err(PoolRejection::IncompleteIdentity)
        );
        let swapped = PoolMetadata {
            token0: Some(T1),
            token1: Some(T0),
            ..v3_meta(500)
        };
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV3, pool, &swapped),
            Err(PoolRejection::IncompleteIdentity)
        );
        // A disagreeing factory record vetoes even a CREATE2 match.
        let rec = Address::repeat_byte(9);
        let vetoed = PoolMetadata {
            registered_pool: Some(rec),
            ..v3_meta(500)
        };
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV3, pool, &vetoed),
            Err(PoolRejection::RecordMismatch { record: rec })
        );
        // A later successful admission clears the refusal.
        assert!(
            gate.admit_pool(SwapVenue::UniswapV3, pool, &v3_meta(500))
                .is_ok()
        );
        assert!(gate.rejections().all(|(a, _)| *a != pool));
    }

    #[test]
    fn without_a_pinned_hash_the_factory_record_decides() {
        // PancakeSwap v2 on BSC: factory pinned, no init-code hash.
        let factory = address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73");
        let pair = Address::repeat_byte(0x77);
        let gate0 = SwapVenueGate::new(56);
        assert!(gate0.needs_registry_record(SwapVenue::UniswapV2, factory));
        assert!(!SwapVenueGate::new(RH).needs_registry_record(SwapVenue::UniswapV3, RH_V3_FACTORY));
        let meta = PoolMetadata {
            factory: Some(factory),
            token0: Some(T0),
            token1: Some(T1),
            fee: None,
            registered_pool: None,
        };
        let mut gate = gate0.clone();
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV2, pair, &meta),
            Err(PoolRejection::NotRegisteredByFactory { record: None })
        );
        let other = PoolMetadata {
            registered_pool: Some(Address::repeat_byte(1)),
            ..meta
        };
        assert!(matches!(
            gate.admit_pool(SwapVenue::UniswapV2, pair, &other),
            Err(PoolRejection::NotRegisteredByFactory { record: Some(_) })
        ));
        let good = PoolMetadata {
            registered_pool: Some(pair),
            ..meta
        };
        // Admitted, but at the deployment's own level: IdlOnly.
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV2, pair, &good),
            Ok(VenueVerification::IdlOnly)
        );
    }

    #[test]
    fn emitters_of_venues_without_a_pinned_factory_are_never_pending() {
        // A chain without a pinned v2 factory (Ethereum mainnet here): a
        // v2-shaped swap is a gap and costs no metadata lookup.
        let v2 = log(
            Address::repeat_byte(3),
            vec![
                V2_SWAP_EVENT_SIGNATURE,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 128],
        );
        let gate = SwapVenueGate::new(1);
        assert!(!gate.has_factory(SwapVenue::UniswapV2));
        assert!(gate.pending_pool_emitters([&v2]).is_empty());
        assert!(matches!(
            gate.classify(&v2),
            GateOutcome::UngatedEmitter { .. }
        ));
    }

    #[test]
    fn non_swap_logs_are_not_swaps() {
        let l = log(Address::ZERO, vec![B256::ZERO], vec![]);
        assert_eq!(SwapVenueGate::new(RH).classify(&l), GateOutcome::NotSwap);
        let none = log(Address::ZERO, vec![], vec![]);
        assert_eq!(SwapVenueGate::new(RH).classify(&none), GateOutcome::NotSwap);
    }
}
