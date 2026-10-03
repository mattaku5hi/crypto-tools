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
mod evm_run;
mod evm_source;
mod input;
mod output;
mod price_source;
mod time;

pub use config::{
    AnalysisConfig, ChainConfig, ConfigError, OutputConfig, ProviderConfig, QualityConfig,
    ScanConfig, ScoutConfig, StorageConfig,
};
pub use evm_run::{
    BLOCKSCOUT_URL_OVERRIDE_ENV, EvmSetup, EvmSetupError, EvmStatsError, EvmStatsRun, RunFamily,
    collect_evm_stats, run_family, setup_evm,
};
pub use evm_source::{
    BLOCKSCOUT_KEY_ENV, EvmRpcUrl, ROBINHOOD_PUBLIC_RPC, evm_rpc_url_from_env, rpc_env_name,
};
pub use input::{
    IdentityKind, IdentityRecord, InputError, InputFormat, ParsedInput, UpstreamInfo,
    chain_profile_name, parse_input, parse_jsonl_with_upstream, resolve_chain,
    resolve_token_assets, resolve_wallet_keys,
};
pub use output::{
    DecodeEvidenceDto, FeeBpsDto, JsonlRecord, OpenPositionDto, OpenValuationMetaDto,
    OpenValuationTotalsDto, PrefetchDto, PriceCoverageDto, PricingMetaDto, PricingMetaInput,
    RunStatus, SCHEMA_VERSION, VenueDiagDto, VenueEventsDto, Window, WriteOutcome, evidence_dtos,
    evidence_line, evm_spelling, ledger_totals_dto, open_positions_dto, open_valuation_line,
    open_valuation_meta, pricing_line, pricing_meta, totals_dto, write_lines_to_stdout,
};
pub use price_source::{COINBASE_ENDPOINT_ENV, build_coinbase_source, build_coinbase_source_evm};
pub use time::{format_unix_utc, now_utc_rfc3339};
