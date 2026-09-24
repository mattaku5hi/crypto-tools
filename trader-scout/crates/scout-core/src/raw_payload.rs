//! Raw provider payload shapes shared across chain families.
//!
//! Moved here from `scout-evm`/`scout-solana` (ADR-008 S1) so
//! `scout-api`'s `ScanEnvelope::payload` can be a real typed enum
//! without pulling either chain crate's full dependency footprint.
//! `scout-evm`/`scout-solana` re-export these names unchanged — this
//! move is mechanical, not a behavior or API change for existing
//! callers.

use alloy_primitives::{Address, B256, Bytes, U256};

/// One decoded-from-JSON-RPC log entry, prior to any protocol-specific
/// interpretation. `topics[0]` is the event signature hash when present;
/// callers must not assume a fixed topic count without checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvmLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
    pub block_number: u64,
    pub transaction_index: u64,
    pub log_index: u64,
}

/// Minimal transaction context a decoder needs: who sent it, and its
/// canonical position (block_number, tx_index) per ADR-002's ordering
/// contract — never wall-clock/fetch order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvmTransaction {
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub block_number: u64,
    pub transaction_index: u64,
    pub value: U256,
}

/// A 32-byte Solana address (program id, mint, or account pubkey). Kept
/// as a raw byte array rather than `scout_core::AddressBytes` — this
/// module operates on positional account-key lists as Solana's
/// transaction format actually encodes them; converting to a canonical
/// `AddressBytes`/`WalletKey` happens in scout-normalize once ownership
/// is resolved, not here.
pub type SolanaPubkey = [u8; 32];

/// One instruction within a transaction, prior to any protocol-specific
/// interpretation. `program_id` and `accounts` are resolved from the
/// transaction's account-keys table by the caller; this type holds the
/// already-resolved pubkeys, not raw indices, so a decoder never needs
/// its own copy of the account-keys table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSolanaInstruction {
    pub program_id: SolanaPubkey,
    /// Accounts referenced by this instruction, in the order the
    /// instruction defines them (protocol-specific meaning; a decoder
    /// for one program knows what account index means what).
    pub accounts: Vec<SolanaPubkey>,
    pub data: Vec<u8>,
    /// Position within the transaction's flattened top-level +
    /// inner-instruction list — the finer-grained "instruction path"
    /// ARCHITECTURE.md §6 requires for canonical ordering (analogous to
    /// EVM's `log_index` within a transaction).
    pub instruction_index: u32,
}

/// Minimal transaction context a decoder needs: canonical position
/// (slot, transaction index — ADR-002's ordering contract, never
/// wall-clock/fetch order) plus the instructions it contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSolanaTransaction {
    pub signature: [u8; 64],
    pub slot: u64,
    pub transaction_index: u64,
    pub instructions: Vec<RawSolanaInstruction>,
}

/// The full set of raw payload shapes a `HistoryProvider` (Tier 1) may
/// return. Introduced by ADR-008 to replace `ScanEnvelope`'s previous
/// `raw_payload_description: String` field with something a decoder can
/// actually act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawPayload {
    EvmLog(RawEvmLog),
    EvmTransaction(RawEvmTransaction),
    SolanaInstruction(RawSolanaInstruction),
    SolanaTransaction(RawSolanaTransaction),
}
