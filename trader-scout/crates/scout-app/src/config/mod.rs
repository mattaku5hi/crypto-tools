//! Config loader for `config/scout.example.toml`'s schema. See
//! docs/ARCHITECTURE.md and the example file's own comments for the
//! full contract — this module only owns parsing and the typed
//! representation, not policy decisions about what a value should be.
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

mod schema;

pub use schema::{
    AnalysisConfig, ChainConfig, ConfigError, OutputConfig, ProviderConfig, QualityConfig,
    ScanConfig, ScoutConfig, StorageConfig,
};
