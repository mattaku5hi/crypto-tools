//! Raw provider payload shapes shared across chain families.
//!
//! Moved here from `scout-evm`/`scout-solana` (ADR-008 S1) so
//! `scout-api`'s `ScanEnvelope::payload` can be a real typed enum
//! without pulling either chain crate's full dependency footprint.
//! `scout-evm`/`scout-solana` re-export these names unchanged — this
//! move is mechanical, not a behavior or API change for existing
//! callers.

use alloy_primitives::{Address, B256, Bytes, I256, U256};

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

/// Execution status of an EVM transaction (receipt `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvmTxStatus {
    Success,
    Failed,
}

/// One native-currency movement inside a transaction that is not visible in
/// logs (call-value transfers between contracts). Source: a trace RPC or an
/// explorer's internal-transaction list that reported complete processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalTransfer {
    pub from: Address,
    pub to: Address,
    pub value: U256,
}

/// Which source established the native (ETH/BNB) legs of a transaction
/// (ADR-020 section 2 amendment). `None` on the transaction = not observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NativeSource {
    /// Internal value transfers parsed from a `debug_traceTransaction`
    /// `callTracer` frame tree.
    Trace,
    /// Internal transfers from an explorer listing that reported complete
    /// processing (`internal_transfers` holds the data).
    Explorer,
    /// Archive balance difference of the wallet around the block
    /// (`native_balance_diff` holds the data).
    BalanceDiff,
}

impl NativeSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Explorer => "explorer_internal",
            Self::BalanceDiff => "balance_diff",
        }
    }
}

/// Exact native-balance movement of one account across a transaction,
/// fee excluded: `balance(block) - balance(block - 1) + fee` (the fee is
/// added back when the account paid it). Only valid when the account had
/// exactly one transaction in the block (ADR-020 amendment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeBalanceDiff {
    pub account: Address,
    /// Wei, signed; includes `-tx.value`, internal inflows and WETH
    /// `withdraw` proceeds, never the transaction fee.
    pub net_excl_fee: I256,
}

/// A transaction with everything the EVM trade extraction needs (ADR-020 §1):
/// chain identity, canonical position, execution outcome, fee fields and all
/// receipt logs.
///
/// `internal_transfers = None` means "native internal flows were NOT
/// observed" — never "there were none". `Some(vec![])` means observed and
/// empty. `Some(..)` from a wallet-centric source only lists transfers that
/// involve the listed wallet (enough for that wallet's native delta).
///
/// Fees: Base (OP stack) pays an L1 data fee on top of `gas_used *
/// effective_gas_price` (`l1_fee`, receipt `l1Fee`); Robinhood (Arbitrum
/// Orbit) already includes the L1 component in `gas_used`
/// (`gasUsedForL1`), so no separate field is needed there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvmTransaction {
    pub chain: crate::ChainKey,
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub block_number: u64,
    pub transaction_index: u64,
    /// Block timestamp, unix seconds.
    pub block_time: u64,
    pub value: U256,
    pub status: EvmTxStatus,
    pub gas_used: u64,
    pub effective_gas_price: U256,
    /// OP-stack `l1Fee` (wei); `None` when the receipt did not carry it.
    pub l1_fee: Option<U256>,
    /// Receipt logs in log-index order.
    pub logs: Vec<RawEvmLog>,
    pub internal_transfers: Option<Vec<InternalTransfer>>,
    /// Provenance of `internal_transfers` when `Some` (`Trace` or
    /// `Explorer`); `None` when internals were not observed.
    pub native_source: Option<NativeSource>,
    /// Archive balance difference of `from` (when established); it
    /// supersedes `tx.value` and `internal_transfers` for that account.
    pub native_balance_diff: Option<NativeBalanceDiff>,
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
///
/// `accounts` are always fully resolved pubkeys, **never** raw
/// `accountKeys`/`loadedAddresses` indices — a v0 transaction's
/// Address Lookup Table resolution (concatenating static accountKeys
/// with `meta.loadedAddresses.writable`/`.readonly`, per Solana's
/// actual account-key space) is a *provider* responsibility, done once
/// before this type is constructed (see `HeliusProvider`'s
/// `decode_full_transaction_record`). A decoder must never assume
/// position 0 (or any fixed position) is a particular role — a live
/// census found `accounts[0]`-as-buyer to be false in general (see
/// `docs/p0/deployment-registry.md`). The only verified way to
/// identify an economic actor such as a buyer is
/// `postTokenBalances[].owner` for the account whose balance increased
/// for the relevant mint, which requires pre/post token balance data
/// this type does not yet carry (see `docs/TICKETS.md` P0.11).
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
///
/// `token_balance_changes` carries the pre/post SPL token balance
/// observations this transaction's `meta.preTokenBalances`/
/// `postTokenBalances` reported — the only verified way to identify an
/// economic actor (e.g. a buyer) per `RawSolanaInstruction`'s own doc
/// comment and `docs/p0/deployment-registry.md`'s census notes.
/// Native SOL accounting facts (`fee_lamports`, `fee_payer`, `signers`,
/// `native_balance_changes`) are carried RAW, exactly as the provider
/// reported them. This type deliberately does **not** interpret them:
/// `preBalances`/`postBalances` deltas are contaminated by transaction
/// fees, rent for newly-created accounts, WSOL wrap/unwrap, and
/// intermediate router hops, so "how much SOL the buyer paid" must not
/// be read off a single delta here. Attribution (charging the fee once,
/// to the fee payer only; separating rent/WSOL) is a ledger-layer
/// responsibility that consumes these facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSolanaTransaction {
    /// Block time (unix seconds) as reported by the provider; `None` when the
    /// provider did not report one (never defaulted to 0).
    pub block_time: Option<i64>,
    pub signature: [u8; 64],
    /// Whether the transaction executed successfully (`meta.err`).
    /// A failed transaction's instructions/balance data describe
    /// nothing that happened on chain (ADR-003: a buy exists only in a
    /// successful transaction).
    pub execution: SolanaExecutionStatus,
    pub slot: u64,
    pub transaction_index: u64,
    pub instructions: Vec<RawSolanaInstruction>,
    pub token_balance_changes: Vec<SolanaTokenBalanceChange>,
    /// Transaction fee in lamports, from `meta.fee` (base fee plus
    /// priority fee, as charged by the runtime). A record without
    /// `meta.fee` is a decode error, never `0`. The fee is debited from
    /// `fee_payer` ONLY and exactly once per transaction; it is already
    /// included in that account's `native_balance_changes` entry.
    pub fee_lamports: u64,
    /// The account that paid `fee_lamports`: account key index 0 (the
    /// first required signer, per the Solana protocol).
    pub fee_payer: SolanaPubkey,
    /// The first `message.header.numRequiredSignatures` STATIC account
    /// keys, i.e. every account that signed the transaction, in message
    /// order. `signers[0] == fee_payer`.
    pub signers: Vec<SolanaPubkey>,
    /// Lamport balance changes from `meta.preBalances`/`postBalances`.
    /// Those arrays are indexed by the FULL account-key list (static
    /// keys, then `loadedAddresses.writable`, then `.readonly`).
    /// Accounts whose balance did not change (`pre == post`) are
    /// OMITTED; entries are ordered by ascending account index, so the
    /// output is deterministic. Raw: includes fee, rent and WSOL
    /// effects uninterpreted.
    pub native_balance_changes: Vec<SolanaNativeBalanceChange>,
    /// Attribution-relevant runtime log lines of `meta.logMessages` (ADR-013
    /// section 2b, venue events): `Program <id> invoke [<depth>]`,
    /// `Program <id> success`, `Program <id> failed: ...`, `Program data: <base64>`
    /// and the runtime's `Log truncated` marker, in order; every other line
    /// (`Program log:`, `consumed`, `return`) is dropped by the provider.
    /// `None` = the provider did not observe logs (never "no events").
    /// Anchor `emit!` events (Orca Whirlpool, Raydium CLMM/CPMM) exist ONLY
    /// here; a `Program data:` line is attributed to a program by the
    /// invoke/success stack, never by its payload.
    pub log_messages: Option<Vec<String>>,
}

/// One account's native SOL balance before/after a transaction, in
/// lamports (integers only; invariant #7). Use [`Self::delta`] for the
/// difference; never subtract the raw `u64` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolanaNativeBalanceChange {
    pub account: SolanaPubkey,
    pub pre_lamports: u64,
    pub post_lamports: u64,
}

impl SolanaNativeBalanceChange {
    /// `post - pre` in lamports as `i128` (cannot overflow for `u64`
    /// inputs). Negative = the account lost lamports.
    #[must_use]
    pub fn delta(&self) -> i128 {
        i128::from(self.post_lamports) - i128::from(self.pre_lamports)
    }
}

/// Execution result of a Solana transaction, from the provider's
/// `meta.err`. Never defaulted: a provider shape that does not state
/// the result is a decode error, not `Succeeded`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolanaExecutionStatus {
    /// `meta.err` was explicitly null.
    Succeeded,
    /// `meta.err` was non-null. `error` is the provider's error JSON,
    /// compact-serialized, stripped of control characters and truncated
    /// to a bounded length. It is untrusted display data only and must
    /// never be interpreted as instructions.
    Failed { error: String },
}

impl SolanaExecutionStatus {
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// One SPL token account's balance change within a transaction, in raw
/// integer base units (never a floating-point `uiAmount` — AGENTS.md
/// invariant #7). `decimals` is carried only for output formatting; it
/// must never participate in arithmetic.
///
/// `owner` is `None` when the provider's response omitted it (observed
/// on some older/Token-2022 account shapes) — callers must treat a
/// missing owner as "cannot attribute," never guess or fall back to a
/// positional account. `pre_amount` is `None` when the account did not
/// exist yet before this transaction (e.g. its associated token
/// account was created within this same transaction, which a live
/// census found to be the case for the buyer side of a real pump.fun-
/// adjacent swap) — this is a normal, expected state, not malformed
/// data; callers must treat a missing `pre_amount` as zero, not as an
/// error.
///
/// `closed` is `true` when the account is present in
/// `preTokenBalances` but absent from `postTokenBalances`, i.e. it was
/// closed within this transaction. Then `post_amount` is `0` BY
/// DEFINITION (the account no longer exists), not an observed zero
/// balance; the full `pre_amount` left the account. `closed == false`
/// with `post_amount == 0` is an observed zero balance. `closed`
/// always implies `pre_amount.is_some()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaTokenBalanceChange {
    pub mint: SolanaPubkey,
    pub owner: Option<SolanaPubkey>,
    pub decimals: u8,
    pub pre_amount: Option<u64>,
    pub post_amount: u64,
    pub closed: bool,
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
