//! scout-ledger: FIFO cost basis, fee allocation, lot lineage.
//! See ADR-004 for the full contract this crate implements.
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

mod fifo;
mod lot;

pub use fifo::{DisposalResult, Ledger};
pub use lot::{BasisStatus, Lot, LotProvenance};
