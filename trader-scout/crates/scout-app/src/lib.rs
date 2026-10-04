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
    BLOCKSCOUT_URL_OVERRIDE_ENV, CostFn, DEFAULT_LOG_SCAN_REQUEST_LIMIT, EvmSetup, EvmSetupError,
    EvmStatsError, EvmStatsRun, LimiterNotice, PUBLIC_RPC_OVERRIDE_ENV, RunFamily,
    check_log_scan_feasible, collect_evm_stats, make_limiter, rate_limit_line, run_family,
    setup_evm,
};
pub use evm_source::{
    BLOCKSCOUT_KEY_ENV, EvmNetOptions, EvmRpcUrl, KEYED_RPC_RPS, PUBLIC_RPC_RPS,
    ROBINHOOD_PUBLIC_RPC, evm_rpc_url_from_env, logs_rpc_env_name, rpc_env_name,
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
pub use price_source::{
    COINBASE_ENDPOINT_ENV, build_coinbase_source, build_coinbase_source_evm,
    build_coinbase_source_evm_bsc,
};
pub use time::{format_unix_utc, now_utc_rfc3339};
