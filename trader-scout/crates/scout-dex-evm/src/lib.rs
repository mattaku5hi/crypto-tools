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

mod v2_swap;

pub use v2_swap::{DecodedSwap, DexDecodeError, decode_v2_style_swap};
