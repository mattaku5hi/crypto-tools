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

mod config;
mod input;
mod output;
mod time;

pub use config::{
    AnalysisConfig, ChainConfig, ConfigError, OutputConfig, ProviderConfig, QualityConfig,
    ScanConfig, ScoutConfig, StorageConfig,
};
pub use input::{
    IdentityKind, IdentityRecord, InputError, InputFormat, ParsedInput, chain_profile_name,
    parse_input, resolve_chain, resolve_token_assets, resolve_wallet_keys,
};
pub use output::{
    JsonlRecord, RunStatus, SCHEMA_VERSION, Window, WriteOutcome, write_lines_to_stdout,
};
pub use time::{format_unix_utc, now_utc_rfc3339};
