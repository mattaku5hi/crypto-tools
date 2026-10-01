//! scout-dex-solana: Solana DEX protocol decoders. See workspace
//! docs/ARCHITECTURE.md §5 ("Матрица DEX").
//!
//! `bonding_curve_buy` decodes pump.fun's `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`
//! `buy`/`sell` instructions -- a **confirmed** deployment per
//! `docs/p0/deployment-registry.md`'s "2026-10-01 confirmation" (on-chain
//! `executable` state + official IDL from `pump-fun/pump-public-docs`,
//! commit `e0687ae9b7e064a0f54efc7297c65eecfbba3a8f`, cross-validated
//! against live balance-delta data). See that module's own doc comment
//! for the full confirmation trail and argument/account layout.
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
    BUY_INSTRUCTION_DISCRIMINATOR, BondingCurveBuyDecoder, DecodedBondingCurveBuy,
    DecodedBondingCurveSell, DecodedBondingCurveTrade, SELL_INSTRUCTION_DISCRIMINATOR,
    decode_bonding_curve_instruction,
};
