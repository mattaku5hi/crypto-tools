//! Workspace error type. See ADR-005 for the coverage/exit-code contract
//! this feeds into, and ADR-006 for `ConfigurationRequired`'s role in the
//! provider-port contract.

use crate::identity::ChainKey;

/// Errors shared across scout-core and any crate that depends on it.
///
/// This is intentionally a single flat enum for the domain-fundamental
/// error cases. Higher-level crates (scout-scan, scout-providers,
/// scout-ledger, ...) define their own error types and wrap `ScoutError`
/// as a source where appropriate, per AGENTS.md's "library errors are
/// typed; the application layer may add context."
#[derive(Debug, thiserror::Error)]
pub enum ScoutError {
    /// A provider port has no configured backend. Per ADR-006, this must
    /// be returned instead of an empty successful result.
    #[error("configuration required for port `{port}` (set {env_var})")]
    ConfigurationRequired {
        port: &'static str,
        env_var: &'static str,
    },

    /// An address string is not valid for the given chain family.
    #[error("invalid address for {family:?}: {reason}")]
    InvalidAddress {
        family: crate::identity::ChainFamily,
        reason: String,
    },

    /// A bare EVM address resolved to more than one enabled chain.
    #[error("ambiguous chain: address matches {candidates:?}")]
    AmbiguousChain { candidates: Vec<ChainKey> },

    /// An asset's decimals are unknown, or exceed what the checked
    /// arithmetic in this workspace supports. Per AGENTS.md invariant #7,
    /// this must never be silently defaulted (e.g. to 18).
    #[error("unsupported or unknown decimals for asset (decimals={decimals:?})")]
    UnsupportedDecimals { decimals: Option<u8> },

    /// Checked arithmetic overflowed. Per ADR-001, this is a typed error,
    /// never a silent wraparound or panic.
    #[error("arithmetic overflow: {context}")]
    ArithmeticOverflow { context: &'static str },

    /// A strict ledger operation was asked to record Tier 2
    /// (`TrustLevel::ExternalUnverified`) data. Per ADR-008, the strict
    /// path never silently downgrades or accepts unverified data —
    /// callers who intend to accept it must use the ledger's explicit
    /// `apply_unverified` entry point, which requires the same
    /// `ExternalDataOptIn` token and marks the resulting lot's basis
    /// `Unknown` rather than computing a PnL from unverified numbers.
    #[error("strict ledger path rejected unverified (Tier 2) data for port `{port}`")]
    UnverifiedDataRejected { port: String },
}
