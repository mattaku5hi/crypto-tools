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

mod accounts;
mod evm_blockscout;
mod evm_cost;
mod evm_native;
#[cfg(feature = "test-support")]
pub mod evm_replay;
mod evm_rpc;
mod evm_scan;
mod evm_wire;
mod fixture;
mod helius;
mod unconfigured;

pub use accounts::{AccountRead, AccountsRead, GET_MULTIPLE_ACCOUNTS_MAX, MAX_ACCOUNT_DATA_BYTES};
pub use evm_blockscout::{
    BlockscoutApiKey, BlockscoutEvmConfig, BlockscoutEvmSource, BlockscoutInternal,
    BlockscoutTokenTransfer, BlockscoutTx, Closest, Listing,
};
pub use evm_cost::{DEFAULT_METHOD_CU, approx_method_cu};
pub use evm_native::{
    Capability, NativeLegCapabilities, NativeLegOutcome, NativeLegPolicy, NativeLegResolver,
    NativeLegRun, TraceParseError, Unobserved, parse_call_tree,
};
pub use evm_rpc::{
    CallRecorder, EvmRpcClient, EvmRpcConfig, EvmSourceError, LogFilter, LogsResult, PoolKind,
    PoolOnchainMetadata, capped_span_from_error, is_range_or_cap_error, suggested_range,
};
pub use evm_scan::{
    EvmHistoryScanner, InternalIndex, ReceiptMode, ScanLimits, SwapScanOutput, TokenScanOutput,
    WalletCostPlan, WalletListing, WalletScanOutput,
};
pub use evm_wire::{EvmReceiptInfo, EvmTxInfo};
pub use fixture::{Fixture, FixtureProvenance, FixtureProvider};
pub use helius::{
    DEFAULT_PAGE_LIMIT, HeliusProvider, HeliusRequestOptions, MAX_DERIVED_RESPONSE_BYTES,
    MAX_PAGE_LIMIT, ScanOrder, StatusFilter, TokenAccountsFilter,
};
pub use scout_api::{
    CapabilityStatus, HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest,
    ScanTask, SourceCapabilities,
};
pub use unconfigured::UnconfiguredProvider;
