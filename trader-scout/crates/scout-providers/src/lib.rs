//! scout-providers: reference implementations of `scout-api`'s
//! `HistoryProvider` trait (ADR-008). This crate is no longer the sole
//! home of the trait — third parties implement `scout_api::HistoryProvider`
//! directly and never need to depend on this crate at all.
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

mod fixture;
mod unconfigured;

pub use fixture::{Fixture, FixtureProvenance, FixtureProvider};
pub use scout_api::{
    CapabilityStatus, HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest,
    ScanTask, SourceCapabilities,
};
pub use unconfigured::UnconfiguredProvider;
