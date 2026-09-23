//! scout-sdk: convenient public facade over the trader-scout SDK layers.
//! See ADR-007 for the feature-gate design and ARCHITECTURE.md §4 for
//! the layering this facade exposes.
//!
//! Feature-gated re-exports only — enabling a feature adds visibility
//! into that layer, never changes behavior of an already-enabled
//! narrower feature set (ADR-007's additive-features rule). Default
//! (`scan` only) matches ARCHITECTURE.md §4's stated minimum: a
//! consumer wanting only raw chain data pulls `scout-scan` +
//! `scout-evm`/`scout-solana` + `scout-providers` and nothing else —
//! `scout-ledger`/`scout-analytics`/`scout-storage`/`scout-engine` are
//! not even compiled unless explicitly requested.
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

pub use scout_core::*;

#[cfg(feature = "scan")]
pub use scout_evm as evm;
#[cfg(feature = "scan")]
pub use scout_providers as providers;
#[cfg(feature = "scan")]
pub use scout_scan as scan;
#[cfg(feature = "scan")]
pub use scout_solana as solana;

#[cfg(feature = "ledger")]
pub use scout_ledger as ledger;
#[cfg(feature = "ledger")]
pub use scout_normalize as normalize;

#[cfg(feature = "analytics")]
pub use scout_analytics as analytics;

#[cfg(feature = "full")]
pub use scout_engine as engine;
#[cfg(feature = "full")]
pub use scout_storage as storage;
