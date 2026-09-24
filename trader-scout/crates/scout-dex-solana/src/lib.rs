//! scout-dex-solana: Solana DEX protocol decoders. See workspace
//! docs/ARCHITECTURE.md §5 ("Матрица DEX") — no decoder here claims
//! support for a real deployment; `docs/p0/deployment-registry.md` is
//! empty (P0.2 not yet done). This crate currently only proves the
//! decode *mechanism* against a synthetic fixture (AGENTS.md invariant
//! #16: a decoder is not "support" without a confirmed deployment,
//! IDL, and golden fixtures).
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

mod bonding_curve_buy;

pub use bonding_curve_buy::{
    BondingCurveBuyDecoder, DecodedBondingCurveBuy, decode_bonding_curve_buy,
};
