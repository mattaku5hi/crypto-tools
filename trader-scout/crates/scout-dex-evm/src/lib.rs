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

mod curves;
mod flap;
mod fourmeme;
mod gate;
mod pancake;
mod quote;
mod uniswap;
mod v2_swap;

pub use curves::{
    BAGS_TOKENS_BOUGHT_TOPIC0, BAGS_TOKENS_SOLD_TOPIC0, CurveFamily, CurveTrade,
    PONS_V2_CURVE_BUY_REFUNDED_TOPIC0, PONS_V2_CURVE_BUY_TOPIC0, PONS_V2_CURVE_COMPLETED_TOPIC0,
    PONS_V2_CURVE_SELL_TOPIC0, PonsBuyRefunded, PonsCurveCompleted, curve_topic_kind,
    decode_curve_trade, decode_pons_buy_refunded, decode_pons_curve_completed,
};
pub use flap::{
    FLAP_LAUNCHED_TO_DEX_TOPIC0, FLAP_PORTAL_BSC, FLAP_TOKEN_BOUGHT_TOPIC0,
    FLAP_TOKEN_CREATED_TOPIC0, FLAP_TOKEN_SOLD_TOPIC0, FlapLaunchedToDex, FlapTokenCreated,
    FlapTrade, decode_flap_launched_to_dex, decode_flap_token_created, decode_flap_trade,
    flap_topic_side,
};
pub use fourmeme::{
    FOURMEME_TOKEN_CREATE_TOPIC0, FOURMEME_V1_PURCHASE_TOPIC0, FOURMEME_V1_SALE_TOPIC0,
    FOURMEME_V2_PURCHASE_TOPIC0, FOURMEME_V2_SALE_TOPIC0, FourMemeTokenCreate, FourMemeTrade,
    FourMemeVersion, LaunchpadSide, decode_fourmeme_token_create, decode_fourmeme_trade,
    fourmeme_topic_kind,
};
pub use gate::{
    AnchorRole, CurveIdentity, CurveMetadata, GateOutcome, LaunchpadEvidence,
    PANCAKE_V2_INIT_CODE_HASH, PANCAKE_V3_INIT_CODE_HASH, PoolMetadata, PoolRejection, SwapVenue,
    SwapVenueGate, UNISWAP_V2_CANONICAL_INIT_CODE_HASH, UNISWAP_V3_CANONICAL_INIT_CODE_HASH,
    VENUE_DEPLOYMENTS, VenueDeployment, VenueVerification, VerifiedSwap,
};
pub use pancake::{PANCAKE_V3_SWAP_TOPIC0, PancakeV3Swap, decode_pancake_v3_swap};
pub use quote::{
    PANCAKE_V2_BSC_FACTORY, PANCAKE_V2_FEE, QuoterFamily, QuoterRevert, UNISWAP_V2_FEE, V2Fee,
    V4PoolKey, aerodrome_amount_out_calldata, decode_amount_out, decode_liquidity,
    decode_quoter_revert, decode_reserves, decode_v4_pool_keys, pinned_quoter,
    pinned_slipstream_quoter, pinned_v4_position_manager, pinned_v4_state_view, price_impact_bps,
    selector, v2_amount_out, v2_fee_for_factory, v2_reserves_calldata, v3_liquidity_calldata,
    v3_quote_calldata, v4_liquidity_calldata, v4_pool_keys_calldata, v4_quote_calldata,
};
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
