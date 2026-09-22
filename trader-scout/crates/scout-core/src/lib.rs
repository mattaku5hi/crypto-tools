//! scout-core: see workspace docs/ARCHITECTURE.md for this crate's contract.
//!
//! Domain-fundamental types shared by every other crate: chain/wallet/asset
//! identity (ADR-002), exact monetary types (ADR-001), and the workspace
//! error type. This crate knows nothing about providers, DEX decoders,
//! ledgers, or CLI formatting.
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

mod amount;
mod error;
mod identity;

pub use amount::{MONEY_SCALE, Money, RawAmount, SignedAmount};
pub use error::ScoutError;
pub use identity::{
    AddressBytes, AssetKey, ChainFamily, ChainKey, ChainResolution, GenesisIdentity, NetworkId,
    WalletKey,
};
