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
//! Aerodrome (Base) pools are CREATE2 minimal-proxy clones, so no init-code
//! hash applies: v2 pools are admitted by `factory()` plus the factory's
//! `getPool(token0, token1, stable)` record, Slipstream CL pools by one of
//! the three pinned factory generations plus `getPool(token0, token1,
//! tickSpacing)`. Their swap topics are shared with Uniswap (Slipstream =
//! v3's; Aerodrome v2 = its own Solidly-style topic, see
//! [`crate::AERODROME_V2_SWAP_TOPIC0`]), so the pool's admitted venue, not
//! the topic, says which family it is.
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
//! Base's Uniswap v2/v3 factories, Aerodrome v2 factory and the three Slipstream factories are `FixtureVerified` by `crates/scout-engine/tests/evm_base_venues.rs` (fixture `evm_base_swaps_all_2026-10-04.json`, `docs/p0/measurements/2026-10-04-base-venues-verification.md`). BSC's PancakeSwap v2, Uniswap v3 and Uniswap v2 factories are `FixtureVerified` by `crates/scout-engine/tests/evm_bsc_venues.rs` (fixture `evm_bsc_swaps_all_2026-10-04.json`, `docs/p0/measurements/2026-10-04-bsc-venues-verification.md`); PancakeSwap v3 (its own `Swap` topic, CREATE2 deployer = PoolDeployer) became FixtureVerified with the live recapture `evm_bsc_swaps_all_2026-10-04b.json`; the four.meme TokenManager V2 (emitter-anchored launchpad events that name token and account) is FixtureVerified on the ADR-015/017 standard (token side exact 4/4, wallet never pays less than `cost + fee`; the 2 launch buys carry a live-confirmed 2-10 % native surcharge; n = 4, small; ADR-020 amendment 7); V1 has no samples and stays IdlOnly. Everything else stays `IdlOnly`. `active_from_block` is `0` ("not
//! pinned") until the deployment transaction is read from the chain (the
//! public RPC has no historical state, so it cannot be derived offline).

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, address, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

use crate::curves::{
    BAGS_TOKENS_BOUGHT_TOPIC0, BAGS_TOKENS_SOLD_TOPIC0, PONS_V2_CURVE_BUY_TOPIC0,
    PONS_V2_CURVE_SELL_TOPIC0,
};
use crate::curves::{CurveFamily, decode_curve_trade};
use crate::flap::{FLAP_TOKEN_BOUGHT_TOPIC0, FLAP_TOKEN_SOLD_TOPIC0, decode_flap_trade};
use crate::fourmeme::{
    FOURMEME_V1_PURCHASE_TOPIC0, FOURMEME_V1_SALE_TOPIC0, FOURMEME_V2_PURCHASE_TOPIC0,
    FOURMEME_V2_SALE_TOPIC0, FourMemeVersion, LaunchpadSide, decode_fourmeme_trade,
};
use crate::pancake::{PANCAKE_V3_SWAP_TOPIC0, decode_pancake_v3_swap};
use crate::uniswap::{
    V3_SWAP_TOPIC0, V4_SWAP_TOPIC0, decode_v3_swap, decode_v4_swap, v2_pair_address_create2,
    v3_pool_address_create2,
};
use crate::v2_swap::{
    AERODROME_V2_SWAP_TOPIC0, V2_SWAP_EVENT_SIGNATURE, decode_aerodrome_v2_swap,
    decode_v2_style_swap,
};

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
    /// Aerodrome (Velodrome-style) v2 pools: volatile/stable, CREATE2 clones
    /// admitted through the factory's `getPool(token0, token1, stable)`.
    AerodromeV2,
    /// Aerodrome Slipstream CL pools (Uniswap-v3-shaped `Swap`), CREATE2
    /// clones admitted through `getPool(token0, token1, tickSpacing)`; several
    /// factory generations are live.
    AerodromeSlipstream,
    /// PancakeSwap v3 pools (BSC): Uniswap-v3-shaped pools with their own
    /// `Swap` topic (two extra protocol-fee words), CREATE2-deployed by the
    /// PoolDeployer, `factory()` = the PancakeSwap v3 factory.
    PancakeV3,
    /// four.meme TokenManager V1 (bonding curve): the manager itself emits
    /// `TokenPurchase`/`TokenSale` (V1 layout).
    FourMemeV1,
    /// four.meme TokenManager2 V2 (bonding curve), V2 layout.
    FourMemeV2,
    /// Pons V2 per-token bonding curves (Robinhood Chain): the CURVE emits
    /// `CurveBuy`/`CurveSell`; admitted by [`SwapVenueGate::admit_curve`]
    /// (`factory()` pinned AND the factory's `getLaunchedToken(token)` names
    /// the emitter).
    PonsV2Curve,
    /// Bags per-token bonding curves (Robinhood Chain): `TokensBought`/
    /// `TokensSold`; admitted when `BagsFactory.curveForToken(curve.TOKEN())`
    /// is the emitter.
    BagsCurve,
    /// Flap (flap.sh) Portal (BSC): one contract emits `TokenBought` /
    /// `TokenSold` for every bonding-curve token (ADR-020 amendment 12).
    FlapPortal,
}

impl SwapVenue {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UniswapV2 => "uniswap_v2_style",
            Self::UniswapV3 => "uniswap_v3",
            Self::UniswapV4 => "uniswap_v4",
            Self::AerodromeV2 => "aerodrome_v2",
            Self::AerodromeSlipstream => "aerodrome_slipstream",
            Self::PancakeV3 => "pancake_v3",
            Self::FourMemeV1 => "fourmeme_v1",
            Self::FourMemeV2 => "fourmeme_v2",
            Self::PonsV2Curve => "pons_v2_curve",
            Self::BagsCurve => "bags_curve",
            Self::FlapPortal => "flap_portal",
        }
    }

    /// `true` for venues whose anchor address itself emits the swap event
    /// (a singleton: v4 PoolManager, four.meme TokenManagers); no pool
    /// admission applies.
    #[must_use]
    pub const fn is_emitter_anchored(self) -> bool {
        matches!(
            self,
            Self::UniswapV4 | Self::FourMemeV1 | Self::FourMemeV2 | Self::FlapPortal
        )
    }

    /// Venues that can emit the swap event of `topic0` (several pool families
    /// share a topic): the address gate decides which one a pool belongs to.
    #[must_use]
    pub fn for_topic(topic0: &B256) -> &'static [SwapVenue] {
        if *topic0 == V4_SWAP_TOPIC0 {
            &[SwapVenue::UniswapV4]
        } else if *topic0 == V3_SWAP_TOPIC0 {
            &[SwapVenue::UniswapV3, SwapVenue::AerodromeSlipstream]
        } else if *topic0 == V2_SWAP_EVENT_SIGNATURE {
            &[SwapVenue::UniswapV2, SwapVenue::AerodromeV2]
        } else if *topic0 == AERODROME_V2_SWAP_TOPIC0 {
            &[SwapVenue::AerodromeV2]
        } else if *topic0 == PANCAKE_V3_SWAP_TOPIC0 {
            &[SwapVenue::PancakeV3]
        } else if *topic0 == FOURMEME_V1_PURCHASE_TOPIC0 || *topic0 == FOURMEME_V1_SALE_TOPIC0 {
            &[SwapVenue::FourMemeV1]
        } else if *topic0 == FOURMEME_V2_PURCHASE_TOPIC0 || *topic0 == FOURMEME_V2_SALE_TOPIC0 {
            &[SwapVenue::FourMemeV2]
        } else if *topic0 == PONS_V2_CURVE_BUY_TOPIC0 || *topic0 == PONS_V2_CURVE_SELL_TOPIC0 {
            &[SwapVenue::PonsV2Curve]
        } else if *topic0 == BAGS_TOKENS_BOUGHT_TOPIC0 || *topic0 == BAGS_TOKENS_SOLD_TOPIC0 {
            &[SwapVenue::BagsCurve]
        } else if *topic0 == FLAP_TOKEN_BOUGHT_TOPIC0 || *topic0 == FLAP_TOKEN_SOLD_TOPIC0 {
            &[SwapVenue::FlapPortal]
        } else {
            &[]
        }
    }

    /// `true` for per-token curve venues (the curve emits; admitted by
    /// [`SwapVenueGate::admit_curve`], not as a pool).
    #[must_use]
    pub const fn is_curve(self) -> bool {
        matches!(self, Self::PonsV2Curve | Self::BagsCurve)
    }

    /// `true` when both venues can emit the same swap topic (a pool of either
    /// is told apart by its factory).
    #[must_use]
    pub fn shares_topic_with(self, other: SwapVenue) -> bool {
        self == other
            || matches!(
                (self, other),
                (Self::UniswapV3, Self::AerodromeSlipstream)
                    | (Self::AerodromeSlipstream, Self::UniswapV3)
                    | (Self::UniswapV2, Self::AerodromeV2)
                    | (Self::AerodromeV2, Self::UniswapV2)
            )
    }
}

/// What the anchor address of a deployment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorRole {
    /// The anchor itself emits `Swap` (v4 PoolManager).
    SwapEmitter,
    /// The anchor is a factory; pools are learned from its creation logs
    /// (curve venues: the factory that confirms a curve).
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
    /// CREATE2 deployer when it is not the factory (PancakeSwap v3: the
    /// PoolDeployer deploys the pools); `None` = the factory deploys.
    pub pool_deployer: Option<Address>,
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
        pool_deployer: None,
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
        pool_deployer: None,
    }
}

/// Canonical Uniswap v3 pool init-code hash. Pinned for a deployment only
/// where it reproduces every fixture pool of that chain.
pub const UNISWAP_V3_CANONICAL_INIT_CODE_HASH: B256 =
    b256!("e34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54");
/// Canonical Uniswap v2 pair init-code hash (pinned for the Robinhood v2 factory: reproduces its 8 live pairs).
pub const UNISWAP_V2_CANONICAL_INIT_CODE_HASH: B256 =
    b256!("96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f");

/// Canonical PancakeSwap v2 pair init-code hash (the widely used value). Pinned
/// for the BSC PancakeSwap v2 factory: it reproduces every one of the 119
/// recorded pairs of `evm_bsc_swaps_all_2026-10-04.json` (the test
/// `pinned_init_code_hashes_reproduce_every_recorded_pool_of_their_factory`
/// re-asserts it on every `evm_bsc_*` fixture).
pub const PANCAKE_V2_INIT_CODE_HASH: B256 =
    b256!("00fb7f630766e6a796048ea87d01acd3068e8ff67d078148a3fa3f4a84f69bd5");
/// PancakeSwap v3 pool init-code hash (CREATE2 deployer = the PoolDeployer,
/// NOT the factory). Not published on the official address pages; derived
/// from the exploratory fixture, where it reproduces 64 of 64 swap emitters
/// from `(PoolDeployer, token0, token1, fee)` (tokens and fee taken from the
/// pool's own Transfers and tried over the four Pancake fee tiers: a CREATE2
/// match is cryptographic). Confirmed live: it reproduces 40 of 40 pools of
/// `evm_bsc_swaps_all_2026-10-04b.json` that report `factory()` = the pinned
/// factory and a matching `getPool`.
pub const PANCAKE_V3_INIT_CODE_HASH: B256 =
    b256!("6ce8eb472fa82df5469c6ab6d485f17c3ad13c8cd7af59b3d4a8026c5ce0f7e2");

const fn with_pool_deployer(mut d: VenueDeployment, deployer: Address) -> VenueDeployment {
    d.pool_deployer = Some(deployer);
    d
}

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
    // Base (8453). Uniswap v4: 92 of 92 attributable fixture samples match the
    // PoolManager's ERC-20 net deltas exactly (`evm_uniswap_v4_base.rs`).
    dep_fixture_verified(
        8453,
        SwapVenue::UniswapV4,
        address!("498581ff718922c3f8e6a244956af099b2652b2b"),
        AnchorRole::SwapEmitter,
    ),
    // Uniswap v3 factory on Base (developers.uniswap.org v3-base-deployments,
    // research doc 2.2). The canonical init-code hash is a CANDIDATE: it
    // binds admission to CREATE2 (cross-checked with the factory's record
    // when fetched). `evm_base_venues.rs` asserts it reproduces every
    // recorded pool of this factory; if it does not, drop the hash (the
    // factory record then decides) before flipping to `dep_fixture_verified`.
    with_init_code_hash(
        dep_fixture_verified(
            8453,
            SwapVenue::UniswapV3,
            address!("33128a8fC17869897dcE68Ed026d694621f6FDfD"),
            AnchorRole::PoolFactory,
        ),
        UNISWAP_V3_CANONICAL_INIT_CODE_HASH,
    ),
    // Uniswap v2 factory on Base (developers.uniswap.org v2 deployments,
    // fetched 2026-10-04; Router02 0x4752ba5dbc23f44d87826276bf6fd6b1c372ad24).
    dep_fixture_verified(
        8453,
        SwapVenue::UniswapV2,
        address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6"),
        AnchorRole::PoolFactory,
    ),
    // Aerodrome v2 (Velodrome-style) PoolFactory on Base (research doc 2.2,
    // github.com/aerodrome-finance/contracts README, DOC). Pools are CREATE2
    // minimal-proxy clones, admitted through the factory's
    // `getPool(token0, token1, stable)` record (no init-code hash). IdlOnly
    // until `evm_base_*` fixtures pass `evm_base_venues.rs`; then flip this
    // row to `dep_fixture_verified`.
    dep_fixture_verified(
        8453,
        SwapVenue::AerodromeV2,
        address!("420DD381b31aEf6683db6B902084cB0FFECe40Da"),
        AnchorRole::PoolFactory,
    ),
    // Aerodrome Slipstream CL PoolFactory generations on Base (research doc
    // 2.2, github.com/aerodrome-finance/slipstream README, DOC): initial,
    // "Gauge Caps", "Gauges V3". Admitted through the factory's
    // `getPool(token0, token1, tickSpacing)` record. IdlOnly until verified
    // (flip each row to `dep_fixture_verified` once a fixture of THAT
    // generation passes).
    dep_fixture_verified(
        8453,
        SwapVenue::AerodromeSlipstream,
        address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A"),
        AnchorRole::PoolFactory,
    ),
    dep_fixture_verified(
        8453,
        SwapVenue::AerodromeSlipstream,
        address!("aDe65c38CD4849aDBA595a4323a8C7DdfE89716a"),
        AnchorRole::PoolFactory,
    ),
    dep_fixture_verified(
        8453,
        SwapVenue::AerodromeSlipstream,
        address!("f8f2eB4940CFE7d13603DDDD87f123820Fc061Ef"),
        AnchorRole::PoolFactory,
    ),
    // BSC (56). Sources: Uniswap docs (v2/v3/v4), developer.pancakeswap.finance
    // (v2/v3 factories, PoolDeployer), four-meme-community/fourmeme-docs @
    // 5f7f589b (TokenManager V1/V2); evidence: `evm_bsc_venues.rs` over
    // `evm_bsc_*` fixtures (docs/p0/measurements/2026-10-04-bsc-venues-verification.md).
    // Rows flip to `dep_fixture_verified` only with n >= 1 admitted samples
    // that are all exact (see the test's header for what "exact" accounts).
    dep(
        56,
        SwapVenue::UniswapV4,
        address!("28e2ea090877bf75740558f6bfb36a5ffee9e9df"),
        AnchorRole::SwapEmitter,
    ),
    // Uniswap v3 factory on BNB Chain (developers.uniswap.org v3 BNB deployments).
    with_init_code_hash(
        dep_fixture_verified(
            56,
            SwapVenue::UniswapV3,
            address!("dB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7"),
            AnchorRole::PoolFactory,
        ),
        UNISWAP_V3_CANONICAL_INIT_CODE_HASH,
    ),
    // Uniswap v2 factory on BNB Chain (developers.uniswap.org v2 deployments,
    // fetched 2026-10-04; Router02 0x4752ba5DBc23f44D87826276BF6fD6b1C372aD24).
    // Verified on ONE admitted sample (1 pair): small n.
    with_init_code_hash(
        dep_fixture_verified(
            56,
            SwapVenue::UniswapV2,
            address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6"),
            AnchorRole::PoolFactory,
        ),
        UNISWAP_V2_CANONICAL_INIT_CODE_HASH,
    ),
    // PancakeSwap v2 factory (developer.pancakeswap.finance/contracts/v2/addresses;
    // Router 0x10ED43C718714eb63d5aA57B78B54704E256024E). v2-style Swap.
    // 420 admitted samples (222 pairs, two fixtures): 417 strictly equal to the
    // pair's net flow; 2 bot "skim after swap" transactions (position /
    // round-trip classes) and 1 fee-on-transfer output (class 4) are exact
    // only under the named, printed accounting classes of the test.
    with_init_code_hash(
        dep_fixture_verified(
            56,
            SwapVenue::UniswapV2,
            address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73"),
            AnchorRole::PoolFactory,
        ),
        PANCAKE_V2_INIT_CODE_HASH,
    ),
    // PancakeSwap v3 (developer.pancakeswap.finance/contracts/v3/addresses):
    // Factory 0x0BFb..., PoolDeployer 0x41ff... (CREATE2 deployer), SwapRouter
    // 0x1b81D678ffb9C0263b24A97847620C99d213eB14, Smart Router
    // 0x13f4EA83D0bd40E75C8222255bc855a974568Dd4.
    // FixtureVerified by the live recapture `evm_bsc_swaps_all_2026-10-04b.json`
    // (40 pools with live `factory()`/`getPool`; 130 samples, all strictly
    // equal to the pool net flow).
    with_init_code_hash(
        with_pool_deployer(
            dep_fixture_verified(
                56,
                SwapVenue::PancakeV3,
                address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865"),
                AnchorRole::PoolFactory,
            ),
            address!("41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9"),
        ),
        PANCAKE_V3_INIT_CODE_HASH,
    ),
    // four.meme TokenManager V1 / TokenManager2 V2 (official integration guide,
    // fourmeme-docs @ 5f7f589b): the managers emit TokenPurchase/TokenSale.
    dep(
        56,
        SwapVenue::FourMemeV1,
        address!("EC4549caDcE5DA21Df6E6422d448034B5233bFbC"),
        AnchorRole::SwapEmitter,
    ),
    // Flap Portal (docs.flap.sh deployed contracts, BNB Chain): FixtureVerified
    // by `evm_bsc_flap_portal.rs` — event token amount = the Portal's own token
    // transfer, 470/470 (261 buys, 209 sells, 38 tokens), ADR-020 amendment 12.
    dep_fixture_verified(
        56,
        SwapVenue::FlapPortal,
        crate::flap::FLAP_PORTAL_BSC,
        AnchorRole::SwapEmitter,
    ),
    // V2: FixtureVerified on the ADR-015/017 standard (token side exact 4/4, quote
    // side never better for the wallet than the event, native surcharge 2-10 %
    // confirmed live by archive balance diff; n = 4, small; amendment 7).
    dep_fixture_verified(
        56,
        SwapVenue::FourMemeV2,
        address!("5c952063c7fc8610FFDB798152D69F0B9550762b"),
        AnchorRole::SwapEmitter,
    ),
    // Robinhood launchpads (ADR-020 amendment 8). Per-token curves are
    // admitted through their factory's own record. Pons V2 is FixtureVerified
    // by `evm_robinhood_pons_curves_2026-10-05.json` (n = 200 (tx, curve)
    // samples, token side 200/200 exact; quote side ERC-20 exact 29, native
    // exact 12, rest unclaimed); Bags stays IdlOnly (no live samples).
    // Pons V2 factory (official ponsdotdev/pons-labs @ 44a3db91).
    dep_fixture_verified(
        4663,
        SwapVenue::PonsV2Curve,
        address!("7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e"),
        AnchorRole::PoolFactory,
    ),
    // BagsFactory (docs.bags.fm/robinhood/contracts; bagsfm/bags-idl @ e55767f7).
    dep(
        4663,
        SwapVenue::BagsCurve,
        address!("e8Cc4431adF8b5A847C113EF0c6af9043219Cb37"),
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
    /// Launchpad events name the trading account (four.meme: token too; curves:
    /// the token is the admitted curve's); `None` for pool swaps.
    pub launchpad: Option<LaunchpadEvidence>,
}

/// What a launchpad trade event says (evidence of a swap, never amounts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchpadEvidence {
    pub token: Address,
    /// Trader named by the event; the extraction books it only for the signer.
    pub account: Address,
    pub side: LaunchpadSide,
    /// Curve events only: who receives the output. A recipient other than
    /// `account` is a swap-with-receiver: never attributed (invariant #2).
    pub recipient: Option<Address>,
}

/// What a per-token launchpad curve reports on chain (Pons V2 / Bags),
/// read through bounded `eth_call`s. Admission input for
/// [`SwapVenueGate::admit_curve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurveMetadata {
    /// Pons V2: `curve.factory()`. Bags (the curve has no such getter): the
    /// pinned BagsFactory the confirmation was asked from.
    pub factory: Option<Address>,
    /// Pons V2 `token()` / Bags `TOKEN()`.
    pub token: Option<Address>,
    /// Quote asset: Pons V2 `pairToken()` (`Some(ZERO)` = native ETH), Bags
    /// `WETH()`; `None` = not read. Recorded for evidence, not an admission input.
    pub quote: Option<Address>,
    /// The factory's own record for `token`'s curve: Pons V2
    /// `getLaunchedToken(token).curve` (only if the struct's `token` word is
    /// `token`), Bags `curveForToken(token)`; `None` = not asked, reverted,
    /// zero address or inconsistent.
    pub registered_curve: Option<Address>,
}

/// What an admitted curve is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurveIdentity {
    pub token: Address,
    pub quote: Option<Address>,
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
    /// `emitter.stable()` (Aerodrome v2).
    pub stable: Option<bool>,
    /// `emitter.tickSpacing()` (Slipstream).
    pub tick_spacing: Option<i32>,
    /// `factory.getPool(token0, token1, fee|stable|tickSpacing)` /
    /// `getPair(token0, token1)`.
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
    /// Admitted launchpad curves (also in `pools`): curve -> its token.
    curves: BTreeMap<Address, CurveIdentity>,
    /// Emitters already checked and refused (never re-asked within a run).
    rejected: BTreeMap<Address, PoolRejection>,
}

impl SwapVenueGate {
    #[must_use]
    pub fn new(chain_id: u64) -> Self {
        Self {
            chain_id,
            pools: BTreeMap::new(),
            curves: BTreeMap::new(),
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

    /// The pinned pool family of `factory` on this chain among the venues
    /// that share `venue`'s swap topic (e.g. a Slipstream factory for a
    /// Uniswap-v3-topic emitter); `None` = not a pinned factory of that
    /// family. Lets lookups read the right identity (`stable()`/`tickSpacing()`).
    #[must_use]
    pub fn factory_venue(&self, venue: SwapVenue, factory: Address) -> Option<SwapVenue> {
        self.deployments()
            .find(|d| {
                d.role == AnchorRole::PoolFactory
                    && d.anchor == factory
                    && d.venue.shares_topic_with(venue)
            })
            .map(|d| d.venue)
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
            let candidates = log.topics.first().map_or(&[][..], SwapVenue::for_topic);
            // v4 / four.meme are emitter-anchored (no pool lookup); other topics are not swaps.
            let Some(venue) = candidates
                .iter()
                .copied()
                .find(|v| !v.is_emitter_anchored() && self.has_factory(*v))
            else {
                continue;
            };
            if self.pools.contains_key(&log.address) || self.rejected.contains_key(&log.address) {
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

    /// Admit (or refuse, remembering why) the launchpad curve `emitter` from
    /// its reported `meta` (Pons V2 / Bags, see [`CurveMetadata`]): (i) the
    /// factory is a pinned factory of this chain and venue, (ii) the curve
    /// reports its token, AND (iii) the factory's own record for that token
    /// is this emitter. Nothing else admits a curve.
    ///
    /// # Errors
    /// The [`PoolRejection`] (also remembered: the emitter is not re-asked).
    pub fn admit_curve(
        &mut self,
        venue: SwapVenue,
        emitter: Address,
        meta: &CurveMetadata,
    ) -> Result<VenueVerification, PoolRejection> {
        match self.check_curve(venue, meta, emitter) {
            Ok((verification, active_from, token)) => {
                self.rejected.remove(&emitter);
                self.pools
                    .insert(emitter, (venue, verification, active_from));
                self.curves.insert(
                    emitter,
                    CurveIdentity {
                        token,
                        quote: meta.quote,
                    },
                );
                Ok(verification)
            }
            Err(r) => {
                self.rejected.insert(emitter, r.clone());
                Err(r)
            }
        }
    }

    fn check_curve(
        &self,
        venue: SwapVenue,
        meta: &CurveMetadata,
        emitter: Address,
    ) -> Result<(VenueVerification, u64, Address), PoolRejection> {
        if !venue.is_curve() {
            return Err(PoolRejection::NoFactory);
        }
        let factory = meta.factory.ok_or(PoolRejection::NoFactory)?;
        let d = self
            .deployments()
            .find(|d| d.venue == venue && d.role == AnchorRole::PoolFactory && d.anchor == factory)
            .ok_or(PoolRejection::UnpinnedFactory(factory))?;
        let token = meta.token.ok_or(PoolRejection::IncompleteIdentity)?;
        if meta.registered_curve != Some(emitter) {
            return Err(PoolRejection::NotRegisteredByFactory {
                record: meta.registered_curve,
            });
        }
        Ok((d.verification, d.active_from_block, token))
    }

    /// What an admitted curve reports (its token and quote asset).
    #[must_use]
    pub fn curve_identity(&self, curve: Address) -> Option<CurveIdentity> {
        self.curves.get(&curve).copied()
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
        // Aerodrome pools are minimal-proxy clones: no init-code hash, the
        // factory's record decides.
        let hash = d.init_code_hash.filter(|_| {
            matches!(
                venue,
                SwapVenue::UniswapV2 | SwapVenue::UniswapV3 | SwapVenue::PancakeV3
            )
        });
        if let Some(hash) = hash {
            let computed = match venue {
                SwapVenue::UniswapV3 | SwapVenue::PancakeV3 => {
                    let fee = meta.fee.ok_or(PoolRejection::IncompleteIdentity)?;
                    // Pancake v3 pools are deployed by the PoolDeployer.
                    let deployer = d.pool_deployer.unwrap_or(factory);
                    v3_pool_address_create2(deployer, t0, t1, fee, hash)
                }
                SwapVenue::UniswapV2 => v2_pair_address_create2(factory, t0, t1, hash),
                SwapVenue::UniswapV4
                | SwapVenue::AerodromeV2
                | SwapVenue::AerodromeSlipstream
                | SwapVenue::FourMemeV1
                | SwapVenue::FourMemeV2
                | SwapVenue::PonsV2Curve
                | SwapVenue::BagsCurve
                | SwapVenue::FlapPortal => {
                    return Err(PoolRejection::UnpinnedFactory(factory));
                }
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
            let identity_ok = match venue {
                SwapVenue::UniswapV3 | SwapVenue::PancakeV3 => meta.fee.is_some(),
                SwapVenue::AerodromeV2 => meta.stable.is_some(),
                SwapVenue::AerodromeSlipstream => meta.tick_spacing.is_some(),
                SwapVenue::UniswapV2
                | SwapVenue::UniswapV4
                | SwapVenue::FourMemeV1
                | SwapVenue::FourMemeV2
                | SwapVenue::PonsV2Curve
                | SwapVenue::BagsCurve
                | SwapVenue::FlapPortal => true,
            };
            if !identity_ok {
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
        let candidates = SwapVenue::for_topic(topic0);
        let Some(first) = candidates.first().copied() else {
            return GateOutcome::NotSwap;
        };
        let (venue, gated) = if first.is_emitter_anchored() {
            (
                first,
                self.deployments()
                    .find(|d| {
                        d.venue == first
                            && d.role == AnchorRole::SwapEmitter
                            && d.anchor == log.address
                    })
                    .map(|d| (d.verification, d.active_from_block)),
            )
        } else {
            // The pool's own admitted venue (several families share a topic).
            match self
                .pools
                .get(&log.address)
                .filter(|(v, _, _)| candidates.contains(v))
            {
                Some((v, ver, from)) => (*v, Some((*ver, *from))),
                None => (first, None),
            }
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
        let mut launchpad = None;
        let (pool_id, log_index) = match venue {
            SwapVenue::UniswapV4 => match decode_v4_swap(log) {
                DecodeOutcome::Decoded(s) => (Some(s.pool_id), s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::PancakeV3 => match decode_pancake_v3_swap(log) {
                DecodeOutcome::Decoded(s) => (None, s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::FourMemeV1 | SwapVenue::FourMemeV2 => match decode_fourmeme_trade(log) {
                DecodeOutcome::Decoded(t) => {
                    let expected = if venue == SwapVenue::FourMemeV1 {
                        FourMemeVersion::V1
                    } else {
                        FourMemeVersion::V2
                    };
                    if t.version != expected {
                        return GateOutcome::NotSwap;
                    }
                    launchpad = Some(LaunchpadEvidence {
                        token: t.token,
                        account: t.account,
                        side: t.side,
                        recipient: None,
                    });
                    (None, t.log_index)
                }
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::FlapPortal => match decode_flap_trade(log) {
                DecodeOutcome::Decoded(t) => {
                    launchpad = Some(LaunchpadEvidence {
                        token: t.token,
                        account: t.account,
                        side: t.side,
                        recipient: None,
                    });
                    (None, t.log_index)
                }
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::PonsV2Curve | SwapVenue::BagsCurve => match decode_curve_trade(log) {
                DecodeOutcome::Decoded(t) => {
                    let expected = if venue == SwapVenue::PonsV2Curve {
                        CurveFamily::PonsV2
                    } else {
                        CurveFamily::Bags
                    };
                    if t.family != expected {
                        return GateOutcome::NotSwap;
                    }
                    // The curve's token is what the factory-confirmed admission
                    // recorded; an address without it is not an admitted curve.
                    let Some(identity) = self.curves.get(&log.address) else {
                        return GateOutcome::UngatedEmitter {
                            venue,
                            emitter: log.address,
                        };
                    };
                    launchpad = Some(LaunchpadEvidence {
                        token: identity.token,
                        account: t.account,
                        side: t.side,
                        recipient: Some(t.recipient),
                    });
                    (None, t.log_index)
                }
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::UniswapV3 | SwapVenue::AerodromeSlipstream => match decode_v3_swap(log) {
                DecodeOutcome::Decoded(s) => (None, s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::UniswapV2 => match decode_v2_style_swap(log) {
                DecodeOutcome::Decoded(s) => (None, s.log_index),
                DecodeOutcome::Malformed(m) => return GateOutcome::Malformed(m),
                DecodeOutcome::NotMine => return GateOutcome::NotSwap,
            },
            SwapVenue::AerodromeV2 => match decode_aerodrome_v2_swap(log) {
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
            launchpad,
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
    fn only_evidenced_deployments_are_fixture_verified() {
        for d in VENUE_DEPLOYMENTS {
            // Robinhood (all), Base (all; v4: evm_uniswap_v4_base.rs, the rest:
            // evm_base_venues.rs) and the four BSC v2/v3 factories
            // and Pancake v3 factories plus the four.meme V2 manager of evm_bsc_venues.rs
            // (four.meme V1: no samples; BSC Uniswap v4 has no test).
            let bsc_verified = d.chain_id == 56
                && [
                    address!("dB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7"),
                    address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6"),
                    address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73"),
                    address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865"),
                    address!("5c952063c7fc8610FFDB798152D69F0B9550762b"),
                    // Flap Portal: evm_bsc_flap_portal.rs (470/470).
                    address!("e2cE6ab80874Fa9Fa2aAE65D277Dd6B8e65C9De0"),
                ]
                .contains(&d.anchor);
            // Robinhood launchpad curves: Pons V2 FixtureVerified (n = 200,
            // `evm_robinhood_launchpads.rs`), Bags IdlOnly (no live samples).
            let expected = if d.venue == SwapVenue::BagsCurve {
                VenueVerification::IdlOnly
            } else if d.chain_id == 4663 || d.chain_id == 8453 || bsc_verified {
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
            stable: None,
            tick_spacing: None,
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
        // Uniswap v2 on Base: factory pinned, no init-code hash.
        let factory = address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6");
        let pair = Address::repeat_byte(0x77);
        let gate0 = SwapVenueGate::new(8453);
        assert!(gate0.needs_registry_record(SwapVenue::UniswapV2, factory));
        assert!(!SwapVenueGate::new(RH).needs_registry_record(SwapVenue::UniswapV3, RH_V3_FACTORY));
        let meta = PoolMetadata {
            factory: Some(factory),
            token0: Some(T0),
            token1: Some(T1),
            fee: None,
            stable: None,
            tick_spacing: None,
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
        // Admitted at the deployment's own level (Base v2: FixtureVerified).
        assert_eq!(
            gate.admit_pool(SwapVenue::UniswapV2, pair, &good),
            Ok(VenueVerification::FixtureVerified)
        );
    }

    const BSC: u64 = 56;
    const PANCAKE_V2_FACTORY: Address = address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73");
    const PANCAKE_V3_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
    const PANCAKE_V3_DEPLOYER: Address = address!("41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9");
    const FOURMEME_V1: Address = address!("EC4549caDcE5DA21Df6E6422d448034B5233bFbC");
    const FOURMEME_V2: Address = address!("5c952063c7fc8610FFDB798152D69F0B9550762b");

    fn pancake_v3_swap_at(pool: Address) -> RawEvmLog {
        log(
            pool,
            vec![
                PANCAKE_V3_SWAP_TOPIC0,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 224],
        )
    }

    fn pancake_v3_create2(deployer: Address, fee: u32) -> Address {
        v3_pool_address_create2(deployer, T0, T1, fee, PANCAKE_V3_INIT_CODE_HASH)
    }

    #[test]
    fn pancake_v3_pools_are_created_by_the_pool_deployer_not_the_factory() {
        let pool = pancake_v3_create2(PANCAKE_V3_DEPLOYER, 2500);
        let swap = pancake_v3_swap_at(pool);
        let mut gate = SwapVenueGate::new(BSC);
        // Ungated until admitted; asked as PancakeV3.
        assert!(matches!(
            gate.classify(&swap),
            GateOutcome::UngatedEmitter {
                venue: SwapVenue::PancakeV3,
                ..
            }
        ));
        assert_eq!(
            gate.pending_pool_emitters([&swap]),
            vec![(SwapVenue::PancakeV3, pool)]
        );
        assert!(!gate.needs_registry_record(SwapVenue::PancakeV3, PANCAKE_V3_FACTORY));
        let meta = PoolMetadata {
            factory: Some(PANCAKE_V3_FACTORY),
            token0: Some(T0),
            token1: Some(T1),
            fee: Some(2500),
            ..PoolMetadata::default()
        };
        assert_eq!(
            gate.admit_pool(SwapVenue::PancakeV3, pool, &meta),
            Ok(VenueVerification::FixtureVerified)
        );
        assert!(matches!(
            gate.classify(&swap),
            GateOutcome::Verified(v) if v.venue == SwapVenue::PancakeV3 && v.launchpad.is_none()
        ));
        // The factory as CREATE2 deployer is a different address: refused.
        let wrong = pancake_v3_create2(PANCAKE_V3_FACTORY, 2500);
        assert_ne!(wrong, pool);
        assert!(matches!(
            gate.admit_pool(SwapVenue::PancakeV3, wrong, &meta),
            Err(PoolRejection::Create2Mismatch { computed }) if computed == pool
        ));
        // Another factory (a Uniswap v3 fork) and a missing fee are refused.
        let fork = PoolMetadata {
            factory: Some(Address::repeat_byte(0xfa)),
            ..meta
        };
        assert!(matches!(
            gate.admit_pool(SwapVenue::PancakeV3, Address::repeat_byte(5), &fork),
            Err(PoolRejection::UnpinnedFactory(_))
        ));
        assert_eq!(
            gate.admit_pool(
                SwapVenue::PancakeV3,
                pool,
                &PoolMetadata { fee: None, ..meta }
            ),
            Err(PoolRejection::IncompleteIdentity)
        );
        // A Pancake v3 factory does not admit Uniswap-v3 venue pools and the
        // Uniswap v3 topic is not the Pancake topic.
        assert!(matches!(
            gate.admit_pool(SwapVenue::UniswapV3, Address::repeat_byte(6), &meta),
            Err(PoolRejection::UnpinnedFactory(_))
        ));
        assert!(matches!(
            gate.classify(&v3_swap_at(pool)),
            GateOutcome::UngatedEmitter { .. }
        ));
        // Another chain does not pin Pancake v3: the log is a counted gap and
        // costs no lookup.
        let other = SwapVenueGate::new(1);
        assert!(other.pending_pool_emitters([&swap]).is_empty());
        assert!(matches!(
            other.classify(&swap),
            GateOutcome::UngatedEmitter { .. }
        ));
        // A malformed Pancake swap at an admitted pool surfaces.
        let mut bad = pancake_v3_swap_at(pool);
        bad.data = vec![0u8; 160].into();
        assert!(matches!(gate.classify(&bad), GateOutcome::Malformed(_)));
    }

    #[test]
    fn bsc_pancake_v2_pins_its_hash_and_v2_style_pairs_are_admitted() {
        let pair = v2_pair_address_create2(PANCAKE_V2_FACTORY, T0, T1, PANCAKE_V2_INIT_CODE_HASH);
        let meta = PoolMetadata {
            factory: Some(PANCAKE_V2_FACTORY),
            token0: Some(T0),
            token1: Some(T1),
            ..PoolMetadata::default()
        };
        let mut gate = SwapVenueGate::new(BSC);
        assert!(!gate.needs_registry_record(SwapVenue::UniswapV2, PANCAKE_V2_FACTORY));
        assert!(gate.admit_pool(SwapVenue::UniswapV2, pair, &meta).is_ok());
        // A different address with the same identity is refused.
        assert!(matches!(
            gate.admit_pool(SwapVenue::UniswapV2, Address::repeat_byte(3), &meta),
            Err(PoolRejection::Create2Mismatch { .. })
        ));
        // The Uniswap v2 factory pinned on BSC is a different factory.
        assert!(
            gate.factory_venue(
                SwapVenue::UniswapV2,
                address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6")
            )
            .is_some()
        );
    }

    fn fourmeme_log(manager: Address, topic: B256, words: usize, account: Address) -> RawEvmLog {
        let mut data = vec![0u8; words * 32];
        data[12..32].copy_from_slice(Address::repeat_byte(0x70).as_slice());
        data[44..64].copy_from_slice(account.as_slice());
        log(manager, vec![topic], data)
    }

    #[test]
    fn fourmeme_managers_are_emitter_anchored_and_name_token_and_account() {
        let acct = Address::repeat_byte(0x71);
        let gate = SwapVenueGate::new(BSC);
        for (manager, topic, words, venue, side) in [
            (
                FOURMEME_V1,
                FOURMEME_V1_PURCHASE_TOPIC0,
                5,
                SwapVenue::FourMemeV1,
                LaunchpadSide::Buy,
            ),
            (
                FOURMEME_V1,
                FOURMEME_V1_SALE_TOPIC0,
                5,
                SwapVenue::FourMemeV1,
                LaunchpadSide::Sell,
            ),
            (
                FOURMEME_V2,
                FOURMEME_V2_PURCHASE_TOPIC0,
                8,
                SwapVenue::FourMemeV2,
                LaunchpadSide::Buy,
            ),
            (
                FOURMEME_V2,
                FOURMEME_V2_SALE_TOPIC0,
                8,
                SwapVenue::FourMemeV2,
                LaunchpadSide::Sell,
            ),
        ] {
            let l = fourmeme_log(manager, topic, words, acct);
            match gate.classify(&l) {
                GateOutcome::Verified(v) => {
                    assert_eq!((v.venue, v.emitter, v.pool_id), (venue, manager, None));
                    // V2: FixtureVerified (evm_bsc_venues.rs, amendment 7); V1: no samples.
                    let want = if venue == SwapVenue::FourMemeV2 {
                        VenueVerification::FixtureVerified
                    } else {
                        VenueVerification::IdlOnly
                    };
                    assert_eq!(v.verification, want);
                    assert_eq!(
                        v.launchpad,
                        Some(LaunchpadEvidence {
                            token: Address::repeat_byte(0x70),
                            account: acct,
                            side,
                            recipient: None,
                        })
                    );
                }
                other => panic!("{other:?}"),
            }
            // Never pending: no pool lookup for a singleton.
            assert!(gate.pending_pool_emitters([&l]).is_empty());
            // Same event at another address or on another chain: not evidence.
            let elsewhere = fourmeme_log(Address::repeat_byte(9), topic, words, acct);
            assert!(matches!(
                gate.classify(&elsewhere),
                GateOutcome::UngatedEmitter { .. }
            ));
            assert!(matches!(
                SwapVenueGate::new(8453).classify(&l),
                GateOutcome::UngatedEmitter { .. }
            ));
        }
        // A V2-shaped event at the V1 manager (and vice versa) is ungated.
        let crossed = fourmeme_log(FOURMEME_V1, FOURMEME_V2_PURCHASE_TOPIC0, 8, acct);
        assert!(matches!(
            gate.classify(&crossed),
            GateOutcome::UngatedEmitter { .. }
        ));
        // Broken shape at a gated manager surfaces.
        let broken = fourmeme_log(FOURMEME_V2, FOURMEME_V2_SALE_TOPIC0, 5, acct);
        assert!(matches!(gate.classify(&broken), GateOutcome::Malformed(_)));
    }

    const PONS_FACTORY: Address = address!("7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e");
    const BAGS_FACTORY: Address = address!("e8Cc4431adF8b5A847C113EF0c6af9043219Cb37");

    fn curve_log(
        curve: Address,
        topic: B256,
        who: Address,
        to: Address,
        words: usize,
    ) -> RawEvmLog {
        log(
            curve,
            vec![topic, who.into_word(), to.into_word()],
            vec![0u8; words * 32],
        )
    }

    #[test]
    fn launchpad_curves_are_admitted_only_through_the_factorys_own_record() {
        use crate::curves::{BAGS_TOKENS_BOUGHT_TOPIC0, PONS_V2_CURVE_BUY_TOPIC0};
        let (curve, token) = (Address::repeat_byte(0xc1), Address::repeat_byte(0x70));
        let acct = Address::repeat_byte(0x71);
        let mut gate = SwapVenueGate::new(RH);
        let l = curve_log(curve, PONS_V2_CURVE_BUY_TOPIC0, acct, acct, 4);
        // Before admission: swap-shaped, pending, never evidence.
        assert!(matches!(
            gate.classify(&l),
            GateOutcome::UngatedEmitter {
                venue: SwapVenue::PonsV2Curve,
                ..
            }
        ));
        assert_eq!(
            gate.pending_pool_emitters([&l]),
            vec![(SwapVenue::PonsV2Curve, curve)]
        );
        let good = CurveMetadata {
            factory: Some(PONS_FACTORY),
            token: Some(token),
            quote: Some(Address::ZERO),
            registered_curve: Some(curve),
        };
        // Refusals: no factory, unpinned factory, no token, factory record is another curve / absent.
        let refuse = |meta: CurveMetadata| {
            let mut g = SwapVenueGate::new(RH);
            g.admit_curve(SwapVenue::PonsV2Curve, curve, &meta)
                .unwrap_err()
        };
        assert_eq!(
            refuse(CurveMetadata {
                factory: None,
                ..good
            }),
            PoolRejection::NoFactory
        );
        assert_eq!(
            refuse(CurveMetadata {
                factory: Some(Address::repeat_byte(5)),
                ..good
            }),
            PoolRejection::UnpinnedFactory(Address::repeat_byte(5))
        );
        // The Bags factory is not a Pons V2 factory.
        assert_eq!(
            refuse(CurveMetadata {
                factory: Some(BAGS_FACTORY),
                ..good
            }),
            PoolRejection::UnpinnedFactory(BAGS_FACTORY)
        );
        assert_eq!(
            refuse(CurveMetadata {
                token: None,
                ..good
            }),
            PoolRejection::IncompleteIdentity
        );
        assert_eq!(
            refuse(CurveMetadata {
                registered_curve: Some(Address::repeat_byte(6)),
                ..good
            }),
            PoolRejection::NotRegisteredByFactory {
                record: Some(Address::repeat_byte(6))
            }
        );
        assert_eq!(
            refuse(CurveMetadata {
                registered_curve: None,
                ..good
            }),
            PoolRejection::NotRegisteredByFactory { record: None }
        );
        // A curve on another chain is refused (the factory is pinned per chain).
        assert!(
            SwapVenueGate::new(8453)
                .admit_curve(SwapVenue::PonsV2Curve, curve, &good)
                .is_err()
        );
        // Admission: FixtureVerified (Pons V2, n = 200), the event names token (the curve's), account and recipient.
        assert_eq!(
            gate.admit_curve(SwapVenue::PonsV2Curve, curve, &good),
            Ok(VenueVerification::FixtureVerified)
        );
        assert_eq!(gate.curve_identity(curve).map(|c| c.token), Some(token));
        let to = Address::repeat_byte(0x72);
        match gate.classify(&curve_log(curve, PONS_V2_CURVE_BUY_TOPIC0, acct, to, 4)) {
            GateOutcome::Verified(v) => {
                assert_eq!(
                    (v.venue, v.emitter, v.verification),
                    (
                        SwapVenue::PonsV2Curve,
                        curve,
                        VenueVerification::FixtureVerified
                    )
                );
                assert_eq!(
                    v.launchpad,
                    Some(LaunchpadEvidence {
                        token,
                        account: acct,
                        side: LaunchpadSide::Buy,
                        recipient: Some(to),
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(gate.pending_pool_emitters([&l]).is_empty());
        // A Bags event at an admitted PONS curve is another family's: ungated.
        assert!(matches!(
            gate.classify(&curve_log(curve, BAGS_TOKENS_BOUGHT_TOPIC0, acct, acct, 10)),
            GateOutcome::UngatedEmitter { .. }
        ));
        // A broken shape at an admitted curve surfaces.
        assert!(matches!(
            gate.classify(&curve_log(curve, PONS_V2_CURVE_BUY_TOPIC0, acct, acct, 3)),
            GateOutcome::Malformed(_)
        ));
        // Bags: its own factory.
        let (bcurve, btoken) = (Address::repeat_byte(0xb1), Address::repeat_byte(0xb2));
        let bmeta = CurveMetadata {
            factory: Some(BAGS_FACTORY),
            token: Some(btoken),
            quote: None,
            registered_curve: Some(bcurve),
        };
        assert!(
            gate.admit_curve(SwapVenue::PonsV2Curve, bcurve, &bmeta)
                .is_err()
        );
        assert_eq!(
            gate.admit_curve(SwapVenue::BagsCurve, bcurve, &bmeta),
            Ok(VenueVerification::IdlOnly)
        );
        assert!(matches!(
            gate.classify(&curve_log(bcurve, BAGS_TOKENS_BOUGHT_TOPIC0, acct, acct, 10)),
            GateOutcome::Verified(v) if v.launchpad.map(|l| l.token) == Some(btoken)
        ));
        // A later successful admission clears the earlier refusal.
        assert!(gate.rejections().all(|(a, _)| *a != bcurve));
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

    const AERO_FACTORY: Address = address!("420DD381b31aEf6683db6B902084cB0FFECe40Da");
    const SLIP_FACTORIES: [Address; 3] = [
        address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A"),
        address!("aDe65c38CD4849aDBA595a4323a8C7DdfE89716a"),
        address!("f8f2eB4940CFE7d13603DDDD87f123820Fc061Ef"),
    ];

    fn aero_swap_at(pool: Address) -> RawEvmLog {
        log(
            pool,
            vec![
                AERODROME_V2_SWAP_TOPIC0,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 128],
        )
    }

    #[test]
    fn aerodrome_v2_pool_is_admitted_by_factory_record_only() {
        let pool = Address::repeat_byte(0xa1);
        let mut gate = SwapVenueGate::new(8453);
        let swap = aero_swap_at(pool);
        assert!(matches!(
            gate.classify(&swap),
            GateOutcome::UngatedEmitter {
                venue: SwapVenue::AerodromeV2,
                ..
            }
        ));
        assert_eq!(
            gate.pending_pool_emitters([&swap]),
            vec![(SwapVenue::AerodromeV2, pool)]
        );
        assert!(gate.needs_registry_record(SwapVenue::AerodromeV2, AERO_FACTORY));
        let meta = PoolMetadata {
            factory: Some(AERO_FACTORY),
            token0: Some(T0),
            token1: Some(T1),
            stable: Some(true),
            registered_pool: Some(pool),
            ..PoolMetadata::default()
        };
        // Missing stable(), missing/foreign record: refused.
        assert_eq!(
            gate.admit_pool(
                SwapVenue::AerodromeV2,
                pool,
                &PoolMetadata {
                    stable: None,
                    ..meta
                }
            ),
            Err(PoolRejection::IncompleteIdentity)
        );
        assert!(matches!(
            gate.admit_pool(
                SwapVenue::AerodromeV2,
                pool,
                &PoolMetadata {
                    registered_pool: None,
                    ..meta
                }
            ),
            Err(PoolRejection::NotRegisteredByFactory { record: None })
        ));
        // A Uniswap v2 factory pinned for another chain is not Aerodrome's.
        assert_eq!(
            gate.admit_pool(
                SwapVenue::AerodromeV2,
                pool,
                &PoolMetadata {
                    factory: Some(Address::repeat_byte(7)),
                    ..meta
                }
            ),
            Err(PoolRejection::UnpinnedFactory(Address::repeat_byte(7)))
        );
        assert_eq!(
            gate.admit_pool(SwapVenue::AerodromeV2, pool, &meta),
            Ok(VenueVerification::FixtureVerified)
        );
        assert!(matches!(
            gate.classify(&swap),
            GateOutcome::Verified(v) if v.venue == SwapVenue::AerodromeV2
        ));
        // The Uniswap v2 topic from the same admitted pool is accepted too.
        let uni_topic = log(
            pool,
            vec![
                V2_SWAP_EVENT_SIGNATURE,
                Address::ZERO.into_word(),
                Address::ZERO.into_word(),
            ],
            vec![0u8; 128],
        );
        assert!(matches!(
            gate.classify(&uni_topic),
            GateOutcome::Verified(v) if v.venue == SwapVenue::AerodromeV2
        ));
        // Another chain does not know the pool.
        assert!(matches!(
            SwapVenueGate::new(56).classify(&swap),
            GateOutcome::UngatedEmitter { .. }
        ));
    }

    #[test]
    fn slipstream_pools_of_every_factory_generation_are_admitted() {
        let mut gate = SwapVenueGate::new(8453);
        for (i, f) in SLIP_FACTORIES.iter().enumerate() {
            let pool = Address::repeat_byte(0xb0 + u8::try_from(i).unwrap());
            let swap = v3_swap_at(pool);
            // v3 topic: Base also pins a Uniswap v3 factory, so the pending
            // pair is asked as v3 and resolved by factory.
            assert_eq!(gate.pending_pool_emitters([&swap]).len(), 1);
            assert_eq!(
                gate.factory_venue(SwapVenue::UniswapV3, *f),
                Some(SwapVenue::AerodromeSlipstream)
            );
            let meta = PoolMetadata {
                factory: Some(*f),
                token0: Some(T0),
                token1: Some(T1),
                tick_spacing: Some(100),
                registered_pool: Some(pool),
                ..PoolMetadata::default()
            };
            assert_eq!(
                gate.admit_pool(
                    SwapVenue::AerodromeSlipstream,
                    pool,
                    &PoolMetadata {
                        tick_spacing: None,
                        ..meta
                    }
                ),
                Err(PoolRejection::IncompleteIdentity)
            );
            assert_eq!(
                gate.admit_pool(SwapVenue::AerodromeSlipstream, pool, &meta),
                Ok(VenueVerification::FixtureVerified)
            );
            assert!(matches!(
                gate.classify(&swap),
                GateOutcome::Verified(v) if v.venue == SwapVenue::AerodromeSlipstream
            ));
        }
        // The Uniswap v3 factory is not a Slipstream factory.
        let uni = address!("33128a8fC17869897dcE68Ed026d694621f6FDfD");
        assert_eq!(
            gate.factory_venue(SwapVenue::UniswapV3, uni),
            Some(SwapVenue::UniswapV3)
        );
        assert_eq!(
            gate.admit_pool(
                SwapVenue::AerodromeSlipstream,
                Address::repeat_byte(0xcc),
                &PoolMetadata {
                    factory: Some(uni),
                    token0: Some(T0),
                    token1: Some(T1),
                    tick_spacing: Some(1),
                    ..PoolMetadata::default()
                }
            ),
            Err(PoolRejection::UnpinnedFactory(uni))
        );
    }

    #[test]
    fn base_aerodrome_rows_are_pinned_without_init_code_hash() {
        let on_base = |v: SwapVenue| -> Vec<Address> {
            VENUE_DEPLOYMENTS
                .iter()
                .filter(|d| d.chain_id == 8453 && d.venue == v)
                .map(|d| d.anchor)
                .collect()
        };
        assert_eq!(on_base(SwapVenue::AerodromeV2), vec![AERO_FACTORY]);
        assert_eq!(
            on_base(SwapVenue::AerodromeSlipstream),
            SLIP_FACTORIES.to_vec()
        );
        for d in VENUE_DEPLOYMENTS.iter().filter(|d| d.chain_id == 8453) {
            if matches!(
                d.venue,
                SwapVenue::AerodromeV2 | SwapVenue::AerodromeSlipstream
            ) {
                assert_eq!(d.init_code_hash, None, "{d:?}");
            }
        }
    }

    #[test]
    fn non_swap_logs_are_not_swaps() {
        let l = log(Address::ZERO, vec![B256::ZERO], vec![]);
        assert_eq!(SwapVenueGate::new(RH).classify(&l), GateOutcome::NotSwap);
        let none = log(Address::ZERO, vec![], vec![]);
        assert_eq!(SwapVenueGate::new(RH).classify(&none), GateOutcome::NotSwap);
    }
}
