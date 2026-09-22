//! scout-normalize: net-delta buy classification and owner attribution.
//! See ADR-003 for the full contract this crate implements.
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

mod attribution;
mod classify;

pub use attribution::{AmbiguityReason, AttributionEvidence, AttributionStatus};
pub use classify::{ActionKind, AssetFlow, NetDeltaInput, classify_buy};
