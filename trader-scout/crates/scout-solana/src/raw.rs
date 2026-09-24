//! Raw Solana instruction/transaction shapes. Moved to `scout-core`
//! (ADR-008 S1) so `scout-api`'s `RawPayload` enum can reference them
//! without this crate's dependency footprint. Re-exported here
//! unchanged so existing `scout_solana::RawSolanaInstruction`/
//! `RawSolanaTransaction`/`SolanaPubkey` call sites keep compiling.

pub use scout_core::{RawSolanaInstruction, RawSolanaTransaction, SolanaPubkey};
