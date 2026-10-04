//! scout-dex-evm: EVM DEX protocol decoders. See workspace
//! docs/ARCHITECTURE.md §5 ("Матрица DEX") — no decoder here claims
//! support for a real deployment; `docs/p0/deployment-registry.md` is
//! empty (P0.2 not yet done), so this crate currently only proves the
//! decode *mechanism* against a synthetic fixture (AGENTS.md invariant
//! #16: a decoder is not "support" without a confirmed deployment,
//! ABI/IDL, and golden fixtures — this is the mechanism half only).
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod fourmeme;
mod gate;
mod pancake;
mod uniswap;
mod v2_swap;

pub use fourmeme::{
    FOURMEME_V1_PURCHASE_TOPIC0, FOURMEME_V1_SALE_TOPIC0, FOURMEME_V2_PURCHASE_TOPIC0,
    FOURMEME_V2_SALE_TOPIC0, FourMemeTrade, FourMemeVersion, LaunchpadSide, decode_fourmeme_trade,
    fourmeme_topic_kind,
};
pub use gate::{
    AnchorRole, GateOutcome, LaunchpadEvidence, PANCAKE_V2_INIT_CODE_HASH,
    PANCAKE_V3_INIT_CODE_HASH, PoolMetadata, PoolRejection, SwapVenue, SwapVenueGate,
    UNISWAP_V2_CANONICAL_INIT_CODE_HASH, UNISWAP_V3_CANONICAL_INIT_CODE_HASH, VENUE_DEPLOYMENTS,
    VenueDeployment, VenueVerification, VerifiedSwap,
};
pub use pancake::{PANCAKE_V3_SWAP_TOPIC0, PancakeV3Swap, decode_pancake_v3_swap};
pub use uniswap::{
    V2_PAIR_CREATED_TOPIC0, V2PairCreated, V3_POOL_CREATED_TOPIC0, V3_SWAP_TOPIC0, V3PoolCreated,
    V3Swap, V4_INITIALIZE_TOPIC0, V4_SWAP_TOPIC0, V4Initialize, V4Swap, decode_v2_pair_created,
    decode_v3_pool_created, decode_v3_swap, decode_v4_initialize, decode_v4_swap,
    v2_pair_address_create2, v3_pool_address_create2,
};
pub use v2_swap::{
    AERODROME_V2_SWAP_TOPIC0, DecodedSwap, V2_SWAP_EVENT_SIGNATURE, V2SwapDecoder,
    decode_aerodrome_v2_swap, decode_v2_style_swap,
};
