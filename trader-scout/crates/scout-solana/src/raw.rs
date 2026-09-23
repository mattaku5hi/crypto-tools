//! Minimal raw Solana instruction/transaction shapes. Narrow by design —
//! only what a decoder needs to classify one instruction
//! (ARCHITECTURE.md §4's `TxDecoder` contract input, adapted to
//! Solana's account-keys + instruction-data model rather than EVM's
//! topics + data model).

/// A 32-byte Solana address (program id, mint, or account pubkey).
/// Kept as a raw byte array here rather than reusing
/// `scout_core::AddressBytes` — this crate operates on positional
/// account-key lists as Solana's transaction format actually encodes
/// them; converting to a canonical `AddressBytes`/`WalletKey` happens in
/// scout-normalize once ownership is resolved, not here.
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
/// wall-clock/fetch order) plus pre/post token balances needed to
/// compute net deltas without re-deriving them from raw instruction
/// data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSolanaTransaction {
    pub signature: [u8; 64],
    pub slot: u64,
    pub transaction_index: u64,
    pub instructions: Vec<RawSolanaInstruction>,
}
