//! scout-evm: minimal EVM raw data types (logs, transactions) needed by
//! decoders. See workspace docs/ARCHITECTURE.md §5 for scope: this crate
//! knows nothing about DEX protocol semantics — that lives in
//! scout-dex-evm.
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

mod chain;
mod events;
mod raw;

pub use chain::{
    BASE, BASE_USDC, BSC, BSC_USDC, BSC_USDT, EvmChainProfile, QuoteAssetSpec, QuoteUsdPolicy,
    ROBINHOOD, ROBINHOOD_USDG,
};
pub use events::*;
pub use raw::{
    EvmTxStatus, InternalTransfer, NativeBalanceDiff, NativeSource, RawEvmLog, RawEvmTransaction,
};
