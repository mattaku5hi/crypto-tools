//! scout-engine: minimal synchronous orchestration wiring input →
//! provider → normalize → ledger → analytics → report. See
//! ARCHITECTURE.md §11 for the full highload pipeline this is a first
//! offline slice of.
//!
//! Deliberately NOT implemented here yet: bounded async channels,
//! wallet sharding, backpressure, spawn_blocking CPU pool. Those are
//! real concurrency concerns for a live-RPC scanner processing many
//! wallets in parallel; against a `FixtureProvider` with a handful of
//! synthetic fixtures they would be premature complexity that AGENTS.md
//! itself warns against ("Не внедрять custom lock-free/unsafe без
//! профиля, ADR и измеренной необходимости" — the same principle
//! applies to any concurrency machinery: build it when a measured need
//! exists, not speculatively). This crate's job right now is proving
//! the vertical slice wires together correctly end to end.
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

mod buyer_intersect;
mod solana_buy_qualification;
mod solana_buyer_intersect;

pub use buyer_intersect::{BuyerIntersectReport, BuyerMatch, run_buyer_intersect};
pub use scout_dex_solana::{PumpTradeVariant, VariantVerification};
pub use solana_buy_qualification::{
    PUMP_BONDING_CURVE_IDL_COMMIT, PUMP_BONDING_CURVE_PROGRAM_ID, QualifiedBuy,
    SOLANA_BUY_QUALIFICATION_VERSION, ScopeError, TxQualification, TxQualificationDiagnostics,
    UnverifiedVariantBuy, pump_bonding_curve_decoder, pump_bonding_curve_scope,
    qualify_bonding_curve_buys, solana_mainnet_chain,
};
pub use solana_buyer_intersect::{
    SolanaBuyerIntersectReport, SolanaProtocolScope, SolanaTokenScanSummary,
    run_solana_buyer_intersect, sanitize_provider_text,
};
