//! scout-api: extension-point traits for external providers and
//! decoders. See ADR-008 for the full contract.
//!
//! Deliberately minimal dependencies — `scout-core` + `async-trait` +
//! `futures` + `tokio-util`, no `tokio` runtime dependency. This keeps
//! AGENTS.md invariant #15 (no hidden runtime) true of the API surface
//! itself, not just of our implementations, and keeps this crate cheap
//! enough to sit in every consumer's default dependency path (ADR-007's
//! scanner-only-build proof depends on this staying true).
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

mod error;
mod history_provider;

pub use error::ProviderError;
pub use history_provider::{
    CapabilityStatus, HistoryProvider, ScanEnvelope, ScanPlan, ScanRequest, ScanTask,
    SourceCapabilities,
};
