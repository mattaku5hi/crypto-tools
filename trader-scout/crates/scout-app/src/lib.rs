//! scout-app: shared input/config/output layer for the three CLI binaries.
//! See workspace docs/CLI.md for the full contract this crate implements.
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

mod input;

pub use input::{
    IdentityKind, IdentityRecord, InputError, InputFormat, ParsedInput, parse_input, resolve_chain,
};
