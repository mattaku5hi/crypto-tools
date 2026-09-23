//! scout-storage: embedded SQLite WAL store for manifests, checkpoints,
//! and normalized event/ledger references. See ARCHITECTURE.md §12 for
//! the full contract this crate implements.
//!
//! Raw payloads are NOT stored here (they belong in immutable compressed
//! segment files on disk per §12) — this crate owns the metadata/index
//! layer: watermarks, manifests, and the SQLite connection itself.
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

mod connection;
mod watermark;

pub use connection::{StorageError, StoreConnection};
pub use watermark::{Watermark, WatermarkKind, WatermarkStore};
