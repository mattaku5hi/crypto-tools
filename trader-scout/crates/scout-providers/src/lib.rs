//! scout-providers: HistoryProvider port and its baseline implementations.
//! See ADR-006 for the taxonomy and contract this crate implements.
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

mod capability;
mod fixture;
mod port;
mod unconfigured;

pub use capability::{CapabilityStatus, SourceCapabilities};
pub use fixture::{Fixture, FixtureProvenance, FixtureProvider};
pub use port::{HistoryProvider, ScanEnvelope, ScanPlan, ScanRequest, ScanTask};
pub use unconfigured::UnconfiguredProvider;
