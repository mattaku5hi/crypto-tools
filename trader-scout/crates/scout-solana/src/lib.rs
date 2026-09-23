//! scout-solana: minimal raw Solana instruction/transaction types needed
//! by decoders. See workspace docs/ARCHITECTURE.md §5 for scope — this
//! crate knows nothing about DEX protocol semantics; that lives in
//! scout-dex-solana.
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

mod raw;

pub use raw::{RawSolanaInstruction, RawSolanaTransaction, SolanaPubkey};
