//! `HeliusProvider`: `scout_api::HistoryProvider` backed by Helius's
//! `getTransactionsForAddress` (Solana). See
//! `docs/p0/measurements/2026-09-27-helius-blockscout.md` for the live
//! probes this implementation is built against — both `signatures` and
//! `full` detail modes are confirmed working on the free tier, and
//! `full` mode is what this provider uses: 10 credits per 100 returned
//! transactions, a 10x reduction over calling `getTransaction` once per
//! signature (1 credit each).
//!
//! Pagination: `scan()` follows `paginationToken` sequentially up to a
//! page budget (`DEFAULT_MAX_PAGES_PER_SCAN`, an unmeasured
//! conservative value; see `with_max_pages`). `ScanEnvelope.truncated`
//! means the scan stopped with an unconsumed cursor.
//!
//! Scan order: `with_scan_order` selects `sortOrder` for
//! `getTransactionsForAddress` (`ScanOrder::OldestFirst` = `"asc"`,
//! the default; `ScanOrder::NewestFirst` = `"desc"`). Mechanism of
//! pagination/truncation is identical for both; only the meaning of
//! `truncated` differs: OldestFirst -> NEWER history was not seen;
//! NewestFirst -> OLDER history was not seen (so a consumer's opening
//! inventory/position before the first seen transaction is unknown).
//! Envelopes are yielded in provider order and are NOT re-sorted here.
//! Consumers MUST sort by canonical `(slot, transaction_index)` before
//! any ledger/FIFO use (AGENTS.md invariant 12); with `NewestFirst`
//! the raw stream order is reverse-chronological.
//!
//! Scope: `ScanRequest::WalletActivity` and
//! `ScanRequest::TokenMarketActivity { asset: AssetKey::Token(..) }`.
//! Per `docs/p0/measurements/2026-10-01-helius-mint-centric-query.md`,
//! `getTransactionsForAddress` was confirmed live to also accept an SPL
//! mint address (not just a wallet) and return transactions touching
//! it — this was measured on 2 mints x 5 transactions each, not a
//! systematic completeness/pagination check, so `capabilities()` notes
//! that caveat rather than claiming a fully general guarantee.
//! `AssetKey::Native(..)` (a chain's native currency, not an SPL mint)
//! has no mint address to query and returns `ProviderError::Unsupported`
//! (AGENTS.md invariant #18: an unfamiliar/unsupported shape is
//! surfaced, never silently degraded).

use std::num::NonZeroU32;

use futures::stream::{self, BoxStream, StreamExt};
use scout_api::{
    CapabilityStatus, HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest,
    ScanTask, SourceCapabilities,
};
use scout_core::{
    RawPayload, RawSolanaInstruction, RawSolanaTransaction, SolanaExecutionStatus,
    SolanaNativeBalanceChange,
};
use scout_rpc::RpcClient;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

/// Helius `getTransactionsForAddress` response shape, `transactionDetails: "full"`.
/// Only the fields this provider actually consumes are modeled — Helius
/// returns a much larger envelope (block metadata, rewards, etc.) that
/// this provider does not need and does not want to silently depend on
/// the exact shape of.
#[derive(Debug, Deserialize)]
struct TransactionsForAddressResult {
    data: Vec<FullTransactionRecord>,
    #[serde(default, rename = "paginationToken")]
    pagination_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FullTransactionRecord {
    slot: u64,
    #[serde(rename = "transactionIndex")]
    transaction_index: u64,
    transaction: InnerTransaction,
    #[serde(default)]
    meta: Option<TransactionMeta>,
}

fn present_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// Max chars of provider `meta.err` JSON retained in
/// `SolanaExecutionStatus::Failed`.
const MAX_EXECUTION_ERROR_LEN: usize = 200;

/// Compact, control-free, length-bounded rendering of provider error
/// JSON. Untrusted display text only.
fn bounded_error_text(err: &serde_json::Value) -> String {
    err.to_string()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_EXECUTION_ERROR_LEN)
        .collect()
}

#[derive(Debug, Deserialize)]
struct TransactionMeta {
    // Execution result. `None` = the key was ABSENT (unsupported shape,
    // rejected at decode); `Some(Value::Null)` = explicit success;
    // anything else = failed transaction. `Option<Value>` alone would
    // fold an explicit `null` into absence, hence the custom
    // deserializer.
    #[serde(default, deserialize_with = "present_value")]
    err: Option<serde_json::Value>,
    #[serde(default, rename = "innerInstructions")]
    inner_instructions: Vec<InnerInstructionGroup>,
    // Address Lookup Table (ALT) resolved addresses for a v0
    // transaction. Solana's canonical index space for
    // `programIdIndex`/`accounts` is
    // `message.accountKeys ++ loadedAddresses.writable ++
    // loadedAddresses.readonly` — NOT `message.accountKeys` alone.
    // Omitting this silently rejects most real AMM swaps: Jupiter and
    // most current-generation AMM routes are v0 transactions that push
    // the AMM/pool accounts into a lookup table specifically to fit
    // more accounts than legacy transactions allow. A census of real
    // pump.fun-minted tokens in this session showed ALT-using
    // transactions are the norm, not the exception, for post-migration
    // trading — this is not a rare edge case to defer.
    #[serde(default, rename = "loadedAddresses")]
    loaded_addresses: Option<LoadedAddresses>,
    // Per-account SPL token balance snapshots before/after this
    // transaction executed. `accountIndex` indexes into the SAME
    // concatenated static+ALT account-key space built for
    // instructions -- never message.accountKeys alone. This is the
    // only verified source for identifying an economic actor (e.g. a
    // buyer): see RawSolanaInstruction's doc comment and
    // docs/p0/deployment-registry.md's census notes, which disproved
    // every positional shortcut tried (accounts[0], a fixed index).
    #[serde(default, rename = "preTokenBalances")]
    pre_token_balances: Vec<TokenBalanceEntry>,
    #[serde(default, rename = "postTokenBalances")]
    post_token_balances: Vec<TokenBalanceEntry>,
    // Transaction fee in lamports. `None` = key absent -> typed decode
    // error (never an implicit 0).
    #[serde(default)]
    fee: Option<u64>,
    // Native lamport balances, indexed by the FULL account-key list
    // (static ++ loaded writable ++ loaded readonly). Absent -> typed
    // decode error.
    #[serde(default, rename = "preBalances")]
    pre_balances: Option<Vec<u64>>,
    #[serde(default, rename = "postBalances")]
    post_balances: Option<Vec<u64>>,
}

/// One entry from `preTokenBalances`/`postTokenBalances`. `owner` is
/// `Option` because Helius's response schema does not guarantee it
/// (older account shapes, some Token-2022 cases) -- never defaulted or
/// guessed when absent.
#[derive(Debug, Deserialize)]
struct TokenBalanceEntry {
    #[serde(rename = "accountIndex")]
    account_index: u32,
    mint: String,
    owner: Option<String>,
    #[serde(rename = "uiTokenAmount")]
    ui_token_amount: UiTokenAmount,
}

/// Only `amount` (the raw integer string in base units) and `decimals`
/// are modeled. `uiAmount` (an `f64`) and `uiAmountString` (an
/// already-scaled decimal string) are deliberately NOT fields here --
/// AGENTS.md invariant #7 forbids floats for amounts, and the scaled
/// string is redundant with `amount`/`decimals` plus an extra
/// opportunity to use the wrong one by accident.
#[derive(Debug, Deserialize)]
struct UiTokenAmount {
    amount: String,
    decimals: u8,
}

#[derive(Debug, Deserialize)]
struct LoadedAddresses {
    #[serde(default)]
    writable: Vec<String>,
    #[serde(default)]
    readonly: Vec<String>,
}

/// One CPI group: `index` is the position (0-based) of the top-level
/// instruction that triggered these inner (CPI) calls; `instructions`
/// is that call's nested instruction list, in execution order. This is
/// where the overwhelming majority of real AMM swaps live — anything
/// routed through Jupiter, an aggregator, or any program that CPIs into
/// Raydium/Meteora/Orca appears *only* here, never in the top-level
/// `message.instructions` list.
#[derive(Debug, Deserialize)]
struct InnerInstructionGroup {
    index: u32,
    instructions: Vec<InnerInstruction>,
}

#[derive(Debug, Deserialize)]
struct InnerTransaction {
    signatures: Vec<String>,
    message: InnerMessage,
}

#[derive(Debug, Deserialize)]
struct InnerMessage {
    #[serde(rename = "accountKeys")]
    account_keys: Vec<String>,
    // Absent -> typed decode error: the signer count is needed to
    // identify who signed (and therefore the fee payer's position).
    #[serde(default)]
    header: Option<MessageHeader>,
    instructions: Vec<InnerInstruction>,
    // Non-empty only on v0 transactions using Address Lookup Tables.
    // Its presence (independent of meta.loadedAddresses) is what tells
    // us whether the transaction *needs* ALT resolution at all — a
    // legacy transaction or a v0 transaction with zero lookups never
    // has indices beyond account_keys.len(), so absence of this field
    // safely means "nothing to resolve," not "we don't know."
    #[serde(default, rename = "addressTableLookups")]
    address_table_lookups: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct MessageHeader {
    #[serde(rename = "numRequiredSignatures")]
    num_required_signatures: u32,
}

#[derive(Debug, Deserialize)]
struct InnerInstruction {
    #[serde(rename = "programIdIndex")]
    program_id_index: u32,
    /// Base58 is Helius's default encoding for account indices/data on
    /// this endpoint when `encoding` is left unset; this provider does
    /// not request `jsonParsed`, so instruction `data` arrives as a
    /// base58 string, and `accounts` as indices into `account_keys`.
    accounts: Vec<u32>,
    data: String,
}

/// A `HistoryProvider` backed by Helius. Holds an `RpcClient` (from
/// `scout-rpc`) already pointed at the Helius mainnet endpoint with the
/// API key embedded in the URL — construction fails loudly
/// (`ProviderError::ConfigurationRequired`) if the key is missing,
/// never silently falls back to an unauthenticated call.
#[derive(Debug)]
pub struct HeliusProvider {
    client: RpcClient,
    max_pages: NonZeroU32,
    scan_order: ScanOrder,
}

/// Which end of an address's history `scan()` starts from.
///
/// `truncated` on the last `ScanEnvelope` means the scan stopped with an
/// unconsumed cursor; WHICH history is missing depends on the order:
/// - `OldestFirst` (default, `sortOrder: "asc"`): NEWER history unseen.
/// - `NewestFirst` (`sortOrder: "desc"`): OLDER history unseen, so a
///   consumer's opening inventory before the earliest seen transaction
///   is unknown.
///
/// The provider yields envelopes in provider order and never reorders.
/// Consumers MUST sort by canonical `(slot, transaction_index)` before
/// any ledger use (AGENTS.md invariant 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanOrder {
    /// Earliest history first (`"asc"`). Current/default behavior.
    #[default]
    OldestFirst,
    /// Most recent history first (`"desc"`).
    NewestFirst,
}

impl ScanOrder {
    fn as_sort_order(self) -> &'static str {
        match self {
            Self::OldestFirst => "asc",
            Self::NewestFirst => "desc",
        }
    }
}

impl HeliusProvider {
    /// `api_key` is the raw Helius API key (not a full URL) — this
    /// constructor owns building the endpoint URL so callers never
    /// construct a Helius URL by hand and risk a typo'd host.
    pub fn new(
        api_key: impl AsRef<str>,
        request_timeout_ms: u64,
        max_attempts: u32,
    ) -> Result<Self, ProviderError> {
        let key = api_key.as_ref();
        if key.trim().is_empty() {
            return Err(ProviderError::ConfigurationRequired {
                port: "solana_history".to_string(),
                detail: "set SCOUT_HELIUS_API_KEY".to_string(),
            });
        }
        let endpoint =
            scout_rpc::RpcEndpoint::new(format!("https://mainnet.helius-rpc.com/?api-key={key}"));
        Self::new_with_endpoint(endpoint, request_timeout_ms, max_attempts)
    }

    /// Construct against an arbitrary endpoint — the real entry point
    /// `new()` wraps for production use, and the one tests use to
    /// point this provider at a `wiremock` server instead of the real
    /// Helius host.
    pub fn new_with_endpoint(
        endpoint: scout_rpc::RpcEndpoint,
        request_timeout_ms: u64,
        max_attempts: u32,
    ) -> Result<Self, ProviderError> {
        let client = RpcClient::new(endpoint, request_timeout_ms, max_attempts)?;
        Ok(Self {
            client,
            max_pages: DEFAULT_MAX_PAGES_PER_SCAN,
            scan_order: ScanOrder::default(),
        })
    }

    /// Sets the per-`scan()` page budget (explicit, non-zero). When the
    /// budget is exhausted while a `paginationToken` is still
    /// outstanding, the last yielded envelope carries
    /// `truncated = true`.
    #[must_use]
    pub fn with_max_pages(mut self, max_pages: NonZeroU32) -> Self {
        self.max_pages = max_pages;
        self
    }

    /// Sets the per-response body cap in bytes (default
    /// `scout_rpc::DEFAULT_MAX_RESPONSE_BYTES`).
    #[must_use]
    pub fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.client = self.client.with_max_response_bytes(max_response_bytes);
        self
    }

    /// Sets the total HTTP-attempt budget for this provider instance
    /// (`None` = unlimited, the default). Every attempt counts,
    /// including retries, across all `scan()` calls and tokens. When
    /// spent, calls fail terminally with a `scout_rpc::RequestBudgetExhausted`
    /// inside `ProviderError::Other`.
    #[must_use]
    pub fn with_max_total_requests(mut self, limit: Option<u64>) -> Self {
        self.client = self.client.with_max_total_requests(limit);
        self
    }

    /// HTTP attempts started so far (including retries), whether or not
    /// a budget is set.
    #[must_use]
    pub fn total_requests_made(&self) -> u64 {
        self.client.total_requests_made()
    }

    /// Sets the history direction (default `ScanOrder::OldestFirst`).
    /// See `ScanOrder` for what `truncated` means per order and for the
    /// requirement that consumers sort canonically before ledger use.
    #[must_use]
    pub fn with_scan_order(mut self, order: ScanOrder) -> Self {
        self.scan_order = order;
        self
    }
}

/// Converts one Helius `full`-mode transaction record into our own
/// `RawSolanaTransaction`. Returns `ProviderError::Other` (not a panic,
/// not a silently-skipped record) for any record whose shape does not
/// match what this provider expects — per AGENTS.md invariant #18, an
/// unfamiliar transaction format is surfaced, not dropped.
///
/// `instructions` on the result is the **flattened top-level +
/// inner-instruction list**, per `scout_core::RawSolanaInstruction`'s
/// own documented contract — not top-level only. Top-level instructions
/// come first in transaction order; each top-level instruction's CPI
/// calls (from `meta.innerInstructions`, when that top-level index has
/// a matching group) are appended immediately after it, preserving the
/// nesting order Solana itself executed them in. Skipping this would
/// make every AMM swap routed through an aggregator or CPI invisible —
/// exactly the failure mode that looks like "no swaps exist" rather
/// than "the parser only reads half the instructions."
fn decode_full_transaction_record(
    record: FullTransactionRecord,
) -> Result<RawSolanaTransaction, ProviderError> {
    let signature = decode_signature(&record.transaction.signatures)?;

    // Canonical index space per Solana's own resolution order: static
    // accountKeys first, then ALT writable, then ALT readonly. Any
    // programIdIndex/account index in this transaction's instructions
    // refers into this concatenated list, never into accountKeys alone
    // once addressTableLookups is non-empty.
    let has_alt_lookups = !record.transaction.message.address_table_lookups.is_empty();
    let mut account_keys = record
        .transaction
        .message
        .account_keys
        .iter()
        .map(|key| decode_pubkey(key))
        .collect::<Result<Vec<_>, _>>()?;
    let static_key_count = account_keys.len();

    match (&record.meta, has_alt_lookups) {
        (Some(meta), _) => {
            if let Some(loaded) = &meta.loaded_addresses {
                for key in &loaded.writable {
                    account_keys.push(decode_pubkey(key)?);
                }
                for key in &loaded.readonly {
                    account_keys.push(decode_pubkey(key)?);
                }
            } else if has_alt_lookups {
                // v0 transaction that declares lookups but meta carries
                // no resolved addresses for them -- cannot honestly
                // resolve any index that lands in the ALT-loaded range.
                // Surfacing this explicitly, not guessing and not
                // silently truncating account_keys to the static set.
                return Err(malformed(
                    "transaction uses address lookup tables but meta.loadedAddresses is absent",
                ));
            }
        }
        (None, true) => {
            return Err(malformed(
                "transaction uses address lookup tables but no meta was returned to resolve them",
            ));
        }
        (None, false) => {}
    }

    let mut inner_by_top_level_index: std::collections::BTreeMap<u32, Vec<InnerInstruction>> =
        std::collections::BTreeMap::new();
    let mut token_balance_changes = Vec::new();
    // `meta` and `meta.err` are required: a record that does not state
    // its execution result is an unsupported shape (invariant 18), never
    // an implicit success.
    let execution = match record.meta.as_ref().map(|meta| &meta.err) {
        None => return Err(malformed("meta is absent; execution status unknown")),
        Some(None) => {
            return Err(malformed(
                "meta.err key is absent; execution status unknown",
            ));
        }
        Some(Some(serde_json::Value::Null)) => SolanaExecutionStatus::Succeeded,
        Some(Some(err)) => SolanaExecutionStatus::Failed {
            error: bounded_error_text(err),
        },
    };
    let (fee_lamports, fee_payer, signers, native_balance_changes) = decode_native_accounting(
        record.meta.as_ref(),
        record.transaction.message.header.as_ref(),
        &account_keys,
        static_key_count,
    )?;
    if let Some(meta) = record.meta {
        for group in meta.inner_instructions {
            inner_by_top_level_index.insert(group.index, group.instructions);
        }
        token_balance_changes = decode_token_balance_changes(
            &meta.pre_token_balances,
            &meta.post_token_balances,
            &account_keys,
        )?;
    }

    let mut instructions = Vec::new();
    let mut next_instruction_index: u32 = 0;
    for (top_level_index, instruction) in record
        .transaction
        .message
        .instructions
        .into_iter()
        .enumerate()
    {
        let top_level_index = u32::try_from(top_level_index)
            .map_err(|_| malformed("instruction count exceeds u32::MAX"))?;
        instructions.push(decode_instruction(
            instruction,
            &account_keys,
            next_instruction_index,
        )?);
        next_instruction_index = next_instruction_index
            .checked_add(1)
            .ok_or_else(|| malformed("instruction_index overflow"))?;

        if let Some(inner) = inner_by_top_level_index.remove(&top_level_index) {
            for cpi_instruction in inner {
                instructions.push(decode_instruction(
                    cpi_instruction,
                    &account_keys,
                    next_instruction_index,
                )?);
                next_instruction_index = next_instruction_index
                    .checked_add(1)
                    .ok_or_else(|| malformed("instruction_index overflow"))?;
            }
        }
    }

    Ok(RawSolanaTransaction {
        signature,
        execution,
        slot: record.slot,
        transaction_index: record.transaction_index,
        instructions,
        token_balance_changes,
        fee_lamports,
        fee_payer,
        signers,
        native_balance_changes,
    })
}

/// Extracts the raw native-SOL accounting facts: fee, fee payer,
/// signers and lamport balance changes. `account_keys` is the full
/// static+ALT key space (the index space of `preBalances`/
/// `postBalances`); `static_key_count` is the number of static keys
/// (the only ones that can be signers). No interpretation (rent, fee,
/// WSOL) happens here.
#[allow(clippy::type_complexity)]
fn decode_native_accounting(
    meta: Option<&TransactionMeta>,
    header: Option<&MessageHeader>,
    account_keys: &[[u8; 32]],
    static_key_count: usize,
) -> Result<(u64, [u8; 32], Vec<[u8; 32]>, Vec<SolanaNativeBalanceChange>), ProviderError> {
    let meta = meta.ok_or_else(|| malformed("meta is absent; fee and balances unknown"))?;
    let fee = meta.fee.ok_or_else(|| malformed("meta.fee is absent"))?;
    let header = header.ok_or_else(|| malformed("message.header is absent"))?;
    let pre = meta
        .pre_balances
        .as_ref()
        .ok_or_else(|| malformed("meta.preBalances is absent"))?;
    let post = meta
        .post_balances
        .as_ref()
        .ok_or_else(|| malformed("meta.postBalances is absent"))?;
    if pre.len() != post.len() || pre.len() != account_keys.len() {
        return Err(malformed(
            "preBalances/postBalances length does not match the resolved account-key count",
        ));
    }
    let signer_count = usize::try_from(header.num_required_signatures)
        .map_err(|_| malformed("numRequiredSignatures exceeds usize"))?;
    let signers: Vec<[u8; 32]> = if signer_count <= static_key_count {
        account_keys.iter().take(signer_count).copied().collect()
    } else {
        Vec::new()
    };
    let Some(fee_payer) = signers.first().copied() else {
        return Err(malformed(
            "header.numRequiredSignatures is zero or exceeds the static account-key count",
        ));
    };
    let changes = account_keys
        .iter()
        .zip(pre.iter().zip(post.iter()))
        .filter(|(_, (pre, post))| pre != post)
        .map(|(account, (pre, post))| SolanaNativeBalanceChange {
            account: *account,
            pre_lamports: *pre,
            post_lamports: *post,
        })
        .collect();
    Ok((fee, fee_payer, signers, changes))
}

/// Builds `SolanaTokenBalanceChange` entries from a transaction's
/// `preTokenBalances`/`postTokenBalances`. `account_keys` is the SAME
/// concatenated static+ALT key space `decode_full_transaction_record`
/// already built for instructions -- `accountIndex` here indexes into
/// that identical space, never a second independently-built list.
///
/// A mint/account present in `postTokenBalances` but absent from
/// `preTokenBalances` is NOT an error: it means the token account
/// (commonly an ATA) did not exist before this transaction, which a
/// live census found to be the normal case for a buyer's first
/// purchase of a mint. Its pre-amount is honestly `None` (meaning
/// zero), not a decode failure.
fn decode_token_balance_changes(
    pre: &[TokenBalanceEntry],
    post: &[TokenBalanceEntry],
    account_keys: &[[u8; 32]],
) -> Result<Vec<scout_core::SolanaTokenBalanceChange>, ProviderError> {
    let mut pre_by_index: std::collections::BTreeMap<u32, &TokenBalanceEntry> =
        std::collections::BTreeMap::new();
    for entry in pre {
        if pre_by_index.insert(entry.account_index, entry).is_some() {
            return Err(malformed("duplicate accountIndex in preTokenBalances"));
        }
    }
    let mut post_by_index: std::collections::BTreeMap<u32, &TokenBalanceEntry> =
        std::collections::BTreeMap::new();
    for entry in post {
        if post_by_index.insert(entry.account_index, entry).is_some() {
            return Err(malformed("duplicate accountIndex in postTokenBalances"));
        }
    }

    // accountIndex must resolve within the shared account-key space
    // this transaction already built; an unverifiable index is
    // surfaced, not accepted.
    for index in pre_by_index.keys().chain(post_by_index.keys()) {
        let index = usize::try_from(*index)
            .map_err(|_| malformed("token balance accountIndex exceeds usize"))?;
        if account_keys.get(index).is_none() {
            return Err(malformed(
                "token balance accountIndex out of range of the transaction's account-key space",
            ));
        }
    }

    let mut changes = Vec::with_capacity(pre_by_index.len().max(post_by_index.len()));
    // Post entries first (in provider order), then closed accounts.
    for entry in post {
        let mint = decode_pubkey(&entry.mint)?;
        let owner = entry.owner.as_deref().map(decode_pubkey).transpose()?;
        let post_amount = parse_amount(entry, "postTokenBalances")?;

        let pre_amount = match pre_by_index.get(&entry.account_index) {
            Some(pre_entry) => {
                // Same account index must describe the same mint and
                // owner on both sides; otherwise do not guess.
                if decode_pubkey(&pre_entry.mint)? != mint {
                    return Err(malformed("pre/post token balance mint mismatch"));
                }
                let pre_owner = pre_entry.owner.as_deref().map(decode_pubkey).transpose()?;
                if pre_owner != owner {
                    return Err(malformed("pre/post token balance owner mismatch"));
                }
                Some(parse_amount(pre_entry, "preTokenBalances")?)
            }
            // Absent from preTokenBalances: the account did not exist
            // before this transaction (e.g. ATA created within it).
            None => None,
        };

        changes.push(scout_core::SolanaTokenBalanceChange {
            mint,
            owner,
            decimals: entry.ui_token_amount.decimals,
            pre_amount,
            post_amount,
            closed: false,
        });
    }
    for entry in pre {
        if post_by_index.contains_key(&entry.account_index) {
            continue;
        }
        // Present before, absent after: the token account was closed in
        // this transaction. Its whole pre balance left the account.
        changes.push(scout_core::SolanaTokenBalanceChange {
            mint: decode_pubkey(&entry.mint)?,
            owner: entry.owner.as_deref().map(decode_pubkey).transpose()?,
            decimals: entry.ui_token_amount.decimals,
            pre_amount: Some(parse_amount(entry, "preTokenBalances")?),
            post_amount: 0,
            closed: true,
        });
    }

    Ok(changes)
}

fn parse_amount(entry: &TokenBalanceEntry, side: &str) -> Result<u64, ProviderError> {
    entry
        .ui_token_amount
        .amount
        .parse()
        .map_err(|_| malformed(&format!("{side} amount is not a valid u64")))
}

fn decode_instruction(
    instruction: InnerInstruction,
    account_keys: &[[u8; 32]],
    instruction_index: u32,
) -> Result<RawSolanaInstruction, ProviderError> {
    let program_id_index = usize::try_from(instruction.program_id_index)
        .map_err(|_| malformed("programIdIndex exceeds usize"))?;
    let program_id = *account_keys
        .get(program_id_index)
        .ok_or_else(|| malformed("instruction programIdIndex out of range"))?;

    let accounts = instruction
        .accounts
        .iter()
        .map(|&idx| {
            let idx = usize::try_from(idx).map_err(|_| malformed("account index exceeds usize"))?;
            account_keys
                .get(idx)
                .copied()
                .ok_or_else(|| malformed("instruction account index out of range"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let data = bs58::decode(&instruction.data)
        .into_vec()
        .map_err(|_| malformed("instruction data is not valid base58"))?;

    Ok(RawSolanaInstruction {
        program_id,
        accounts,
        data,
        instruction_index,
    })
}

fn decode_signature(signatures: &[String]) -> Result<[u8; 64], ProviderError> {
    let first = signatures
        .first()
        .ok_or_else(|| malformed("transaction has no signatures"))?;
    let bytes = bs58::decode(first)
        .into_vec()
        .map_err(|_| malformed("signature is not valid base58"))?;
    <[u8; 64]>::try_from(bytes.as_slice()).map_err(|_| malformed("signature is not 64 bytes"))
}

fn decode_pubkey(key: &str) -> Result<[u8; 32], ProviderError> {
    let bytes = bs58::decode(key)
        .into_vec()
        .map_err(|_| malformed("account key is not valid base58"))?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| malformed("account key is not 32 bytes"))
}

fn malformed(detail: &str) -> ProviderError {
    ProviderError::Other(Box::new(std::io::Error::other(format!(
        "Helius getTransactionsForAddress: malformed response ({detail})"
    ))))
}

#[async_trait::async_trait]
impl HistoryProvider for HeliusProvider {
    fn capabilities(&self) -> SourceCapabilities {
        // wallet_activity: LiveVerified per
        // docs/p0/measurements/2026-09-27-helius-blockscout.md (dated,
        // actual successful call — ADR-006).
        //
        // token_market_activity: LiveVerified per
        // docs/p0/measurements/2026-10-01-helius-mint-centric-query.md
        // -- getTransactionsForAddress was confirmed live to accept an
        // SPL mint address and return transactions touching it (2
        // mints x 5 transactions each). This is NOT a claim of full
        // completeness/pagination correctness for arbitrary mints --
        // that measurement's own "What this does NOT establish"
        // section is the honest boundary of what was actually checked.
        let mut caps = SourceCapabilities::empty();
        caps.by_capability.insert(
            "wallet_activity".to_string(),
            CapabilityStatus::LiveVerified,
        );
        caps.by_capability.insert(
            "token_market_activity".to_string(),
            CapabilityStatus::LiveVerified,
        );
        caps
    }

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        match request {
            ScanRequest::WalletActivity { .. } => Ok(ScanPlan {
                request_echo: format!("{request:?}"),
                capabilities: self.capabilities(),
            }),
            ScanRequest::TokenMarketActivity { asset } => match asset {
                scout_core::AssetKey::Token(..) => Ok(ScanPlan {
                    request_echo: format!("{request:?}"),
                    capabilities: self.capabilities(),
                }),
                // A chain's native currency has no SPL mint address to
                // query -- getTransactionsForAddress has nothing to
                // call here, so this is honestly Unsupported, not a
                // silently-empty plan.
                scout_core::AssetKey::Native(_) => Err(ProviderError::Unsupported {
                    capability: "token_market_activity (native asset has no mint address)"
                        .to_string(),
                }),
            },
            // ScanRequest is #[non_exhaustive] (ADR-008: more request
            // variants may be added later without breaking existing
            // providers) — any future variant this provider doesn't
            // know about is honestly Unsupported, never silently
            // matched to the wrong branch.
            _ => Err(ProviderError::Unsupported {
                capability: format!("{request:?}"),
            }),
        }
    }

    fn scan(
        &self,
        task: ScanTask,
        cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        // Dispatch on the typed ScanRequest plan() already validated --
        // never re-parse a string prefix out of `description` (that was
        // this method's old behavior, and exactly the stringly-typed
        // dispatch that silently misroutes on a typo or a new variant).
        let address = match &task.request {
            ScanRequest::WalletActivity { wallet } => wallet.address.to_string(),
            ScanRequest::TokenMarketActivity {
                asset: scout_core::AssetKey::Token(_, mint_address),
            } => mint_address.to_string(),
            ScanRequest::TokenMarketActivity {
                asset: scout_core::AssetKey::Native(_),
            } => {
                let error = ProviderError::Unsupported {
                    capability: "token_market_activity (native asset has no mint address)"
                        .to_string(),
                };
                return Box::pin(stream::once(async move { Err(error) }));
            }
            _ => {
                let error = ProviderError::Unsupported {
                    capability: format!("{:?}", task.request),
                };
                return Box::pin(stream::once(async move { Err(error) }));
            }
        };

        // Sequential, bounded pagination: see `scan_step` for the
        // hold-back protocol that keeps `truncated` honest. Page N+1 is
        // only requested once the consumer has drained everything the
        // previous step produced (`flat_map` over `unfold`), and at most
        // one page of decoded transactions is buffered.
        let state = ScanState {
            address,
            cancel,
            token: None,
            pages_fetched: 0,
            held: None,
            done: false,
        };
        Box::pin(stream::unfold(state, move |state| self.scan_step(state)).flat_map(stream::iter))
    }
}

/// Pagination state for one `scan()` call.
struct ScanState {
    address: String,
    cancel: CancellationToken,
    /// Continuation cursor returned by the most recent page; `Some`
    /// means history continues beyond what has been fetched.
    token: Option<String>,
    pages_fetched: u32,
    /// The last transaction of the most recent page, withheld until we
    /// know whether the scan continues. If it stops with a cursor
    /// outstanding (budget/cancel/anomaly) it is emitted with
    /// `truncated = true`, so the flag is never lost even if later
    /// pages are empty or never fetched.
    held: Option<RawSolanaTransaction>,
    done: bool,
}

type Chunk = Vec<Result<ScanEnvelope, ProviderError>>;

fn envelope(tx: RawSolanaTransaction, truncated: bool) -> Result<ScanEnvelope, ProviderError> {
    Ok(ScanEnvelope {
        payload: RawPayload::SolanaTransaction(tx),
        truncated,
    })
}

fn pagination_error(detail: &str) -> ProviderError {
    ProviderError::Other(Box::new(std::io::Error::other(format!(
        "helius pagination: {detail}"
    ))))
}

impl HeliusProvider {
    /// One unfold step: fetches at most one page and returns the
    /// results to yield for it. Returns `None` once the stream is over.
    ///
    /// `truncated` semantics: `true` iff the scan stopped with an
    /// unconsumed cursor (budget exhausted, cancelled, or a provider
    /// anomaly). To make that true even when the stop decision comes
    /// after a page's envelopes would normally have been yielded, the
    /// last envelope of each page is held back until the next step.
    async fn scan_step(&self, mut state: ScanState) -> Option<(Chunk, ScanState)> {
        loop {
            if state.done {
                return None;
            }
            let mut out: Chunk = Vec::new();

            // Stop conditions are checked before every fetch.
            let stop = if state.cancel.is_cancelled() {
                Some("scan cancelled")
            } else if state.pages_fetched >= self.max_pages.get() {
                // Budget exhausted. Only a truncation if a cursor remains.
                state.token.as_ref().map(|_| "page budget exhausted")
            } else {
                None
            };
            if state.pages_fetched == 0 && state.cancel.is_cancelled() {
                state.done = true;
                out.push(Err(pagination_error(
                    "scan cancelled before the first page",
                )));
                return Some((out, state));
            }
            if state.pages_fetched > 0 && state.token.is_none() {
                // Natural end of history (defensive; handled below too).
                state.done = true;
                if let Some(tx) = state.held.take() {
                    out.push(envelope(tx, false));
                }
                return Some((out, state));
            }
            if let Some(reason) = stop {
                state.done = true;
                match state.held.take() {
                    Some(tx) => out.push(envelope(tx, true)),
                    // Cursor outstanding but nothing to carry the flag:
                    // surface it as a typed error, never silent.
                    None => out.push(Err(pagination_error(&format!(
                        "{reason} with a continuation cursor outstanding and no envelope to mark truncated"
                    )))),
                }
                return Some((out, state));
            }

            let sent_token = state.token.take();
            let page = self
                .fetch_transactions_page(
                    &state.address,
                    MAX_TRANSACTIONS_PER_SCAN,
                    sent_token.as_deref(),
                )
                .await;
            state.pages_fetched += 1;

            let (transactions, next_token) = match page {
                Ok(page) => page,
                Err(err) => {
                    // The held page's cursor was consumed by this
                    // request, so its envelope is not truncated; the
                    // error itself signals incomplete coverage.
                    state.done = true;
                    if let Some(tx) = state.held.take() {
                        out.push(envelope(tx, false));
                    }
                    out.push(Err(err));
                    return Some((out, state));
                }
            };

            if next_token.is_some() && next_token == sent_token {
                // Provider handed back the cursor we just used: would
                // loop forever. Stop; held envelope is truncated.
                state.done = true;
                if let Some(tx) = state.held.take() {
                    out.push(envelope(tx, true));
                }
                out.push(Err(pagination_error(
                    "provider returned a repeated paginationToken",
                )));
                return Some((out, state));
            }

            state.token = next_token;
            let mut transactions = transactions.into_iter();
            let last = transactions.next_back();
            if let Some(last) = last {
                // The previous held envelope's cursor was consumed.
                if let Some(tx) = state.held.take() {
                    out.push(envelope(tx, false));
                }
                out.extend(transactions.map(|tx| envelope(tx, false)));
                if state.token.is_some() {
                    state.held = Some(last);
                } else {
                    out.push(envelope(last, false));
                    state.done = true;
                }
            } else if state.token.is_none() {
                // Empty page, no cursor: end of history.
                state.done = true;
                if let Some(tx) = state.held.take() {
                    out.push(envelope(tx, false));
                }
            }
            // Empty page WITH a cursor keeps `held` and loops: the
            // budget/cancel/repeat checks decide what happens next.

            if !out.is_empty() || state.done {
                return Some((out, state));
            }
        }
    }
}

impl HeliusProvider {
    /// Fetches one page. Returns the decoded transactions and the
    /// response's `paginationToken` (the continuation cursor), if any.
    async fn fetch_transactions_page(
        &self,
        address: &str,
        limit: u32,
        pagination_token: Option<&str>,
    ) -> Result<(Vec<RawSolanaTransaction>, Option<String>), ProviderError> {
        let mut options = serde_json::json!({
            "transactionDetails": "full",
            "sortOrder": self.scan_order.as_sort_order(),
            "limit": limit,
        });
        if let (Some(token), Some(map)) = (pagination_token, options.as_object_mut()) {
            map.insert("paginationToken".to_string(), token.into());
        }
        let params = serde_json::json!([address, options]);

        let result: TransactionsForAddressResult = self
            .client
            .call("getTransactionsForAddress", params)
            .await?;

        let transactions = result
            .data
            .into_iter()
            .map(decode_full_transaction_record)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((transactions, result.pagination_token))
    }
}

/// Maximum transactions requested per page (`limit`). Helius caps `full`
/// mode at 1,000 per call per its own docs; 100 is deliberately
/// conservative, not a measured budget (P0.6/P0.8).
const MAX_TRANSACTIONS_PER_SCAN: u32 = 100;

/// Default page budget per `scan()` call. A conservative, UNMEASURED
/// value -- not a measured budget (that is P0.6/P0.8). Override with
/// `HeliusProvider::with_max_pages`.
pub const DEFAULT_MAX_PAGES_PER_SCAN: NonZeroU32 = match NonZeroU32::new(10) {
    Some(n) => n,
    None => unreachable!(),
};

#[cfg(test)]
fn parse_solana_wallet(address: &str) -> Result<scout_core::WalletKey, ProviderError> {
    let bytes = bs58::decode(address).into_vec().map_err(|_| {
        ProviderError::Other(Box::new(std::io::Error::other(
            "wallet address is not valid base58",
        )))
    })?;
    let address_bytes = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
        ProviderError::Other(Box::new(std::io::Error::other(
            "wallet address is not 32 bytes",
        )))
    })?;
    Ok(scout_core::WalletKey {
        chain: scout_core::ChainKey {
            family: scout_core::ChainFamily::Solana,
            network_id: scout_core::NetworkId::SolanaCluster(scout_core::SolanaCluster::Mainnet),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        },
        address: scout_core::AddressBytes::Solana(address_bytes),
    })
}

// End of file — placeholder function removed; scan() is fully wired via
// fetch_page/stream_results above.

#[cfg(test)]
mod tests {
    use scout_api::ScanRequest;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    // Fixture built from the *verified* live shape captured in
    // docs/p0/measurements/2026-09-27-helius-blockscout.md and this
    // session's own full-mode probe (saved to
    // /tmp/helius_full_probe.json, 14992 bytes, not truncated):
    // slot/transactionIndex are confirmed siblings of `transaction`,
    // meta.innerInstructions is a confirmed sibling of `transaction`
    // too. The signature and account keys below are the REAL base58
    // values from that probe (a genuine 64-byte Ed25519 signature is
    // 87-88 base58 chars, never 44) — a hand-typed short string here
    // would compile fine but panic decode_signature's length check,
    // exactly the "tests pass against fiction" failure mode this
    // fixture exists to avoid.
    fn full_mode_body(pagination_token: Option<&str>) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "data": [
                    {
                        "transaction": {
                            "signatures": ["3GTsKxAZpuMPXtgkFN5j14xvKdJS9w7H5Jhm2JUdQaYmX4Vdm64tmYpmrgLfrX6EBxUgJvuczYocaq77CJPor8Xr"],
                            "message": {
                                "accountKeys": [
                                    "HPPrTuWA9171igRnDgNE5nQ4bwyVxySVWALdvfhXsQ5Z",
                                    "E9BzZER9vhBTPjBpT9QC1NaiinSXonZWgF89HkpKJxGF",
                                    "2qXeC3b9CB1Zd6eLomEq4Jd9g5VqH6o4PT5pBBGvE8jt"
                                ],
                                "header": {
                                    "numRequiredSignatures": 1,
                                    "numReadonlySignedAccounts": 0,
                                    "numReadonlyUnsignedAccounts": 2
                                },
                                "instructions": [
                                    {"programIdIndex": 1, "accounts": [0], "data": "3Bxs4h"}
                                ]
                            }
                        },
                        "meta": {
                            "err": null,
                            "fee": 5000,
                            "preBalances": [1_000_000, 2_000_000, 3_000_000],
                            "postBalances": [995_000, 2_000_000, 3_000_000],
                            "innerInstructions": [
                                {
                                    "index": 0,
                                    "instructions": [
                                        {"programIdIndex": 2, "accounts": [0, 1], "data": "P"}
                                    ]
                                }
                            ]
                        },
                        "version": 0,
                        "slot": 451989539,
                        "transactionIndex": 1231,
                        "blockTime": 1790777970
                    }
                ],
                "paginationToken": pagination_token,
            }
        })
    }

    #[test]
    fn new_rejects_empty_api_key() {
        let err = HeliusProvider::new("", 5_000, 3).unwrap_err();
        assert!(matches!(err, ProviderError::ConfigurationRequired { .. }));
    }

    #[test]
    fn new_rejects_whitespace_only_api_key() {
        let err = HeliusProvider::new("   ", 5_000, 3).unwrap_err();
        assert!(matches!(err, ProviderError::ConfigurationRequired { .. }));
    }

    #[tokio::test]
    async fn plan_rejects_native_asset_token_market_activity_but_accepts_wallet_activity() {
        // Native assets have no SPL mint address to query -- this
        // remains Unsupported even though AssetKey::Token is now
        // supported (see plan_accepts_token_asset_token_market_activity
        // below).
        let provider = HeliusProvider::new_with_endpoint(
            scout_rpc::RpcEndpoint::new("http://127.0.0.1:0"),
            5_000,
            1,
        )
        .unwrap();

        let token_request = ScanRequest::TokenMarketActivity {
            asset: scout_core::AssetKey::Native(scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            }),
        };
        assert!(matches!(
            provider.plan(&token_request).await,
            Err(ProviderError::Unsupported { .. })
        ));

        let wallet_request = ScanRequest::WalletActivity {
            wallet: parse_solana_wallet("5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1").unwrap(),
        };
        assert!(provider.plan(&wallet_request).await.is_ok());
    }

    #[tokio::test]
    async fn plan_accepts_token_asset_token_market_activity() {
        // Per docs/p0/measurements/2026-10-01-helius-mint-centric-query.md:
        // getTransactionsForAddress was confirmed live to accept an SPL
        // mint address, so AssetKey::Token must plan successfully, not
        // Unsupported.
        let provider = HeliusProvider::new_with_endpoint(
            scout_rpc::RpcEndpoint::new("http://127.0.0.1:0"),
            5_000,
            1,
        )
        .unwrap();

        let mint = scout_core::AddressBytes::Solana(
            decode_pubkey("NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump").unwrap(),
        );
        let token_request = ScanRequest::TokenMarketActivity {
            asset: scout_core::AssetKey::Token(
                scout_core::ChainKey {
                    family: scout_core::ChainFamily::Solana,
                    network_id: scout_core::NetworkId::SolanaCluster(
                        scout_core::SolanaCluster::Mainnet,
                    ),
                    genesis_identity: scout_core::GenesisIdentity::Unverified,
                },
                mint,
            ),
        };
        let plan = provider.plan(&token_request).await.unwrap();
        assert_eq!(
            plan.capabilities.status_for("token_market_activity"),
            CapabilityStatus::LiveVerified
        );
    }

    #[test]
    fn decode_full_transaction_record_flattens_top_level_and_inner_instructions() {
        let body = full_mode_body(None);
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        let record = result.data.into_iter().next().unwrap();

        let tx = decode_full_transaction_record(record).unwrap();

        assert_eq!(tx.slot, 451989539);
        assert_eq!(tx.transaction_index, 1231);
        // One top-level instruction + one CPI instruction from
        // meta.innerInstructions[0] (index: 0, matching the sole
        // top-level instruction) -- proves inner instructions are not
        // silently dropped.
        assert_eq!(tx.instructions.len(), 2);
        assert_eq!(tx.instructions[0].instruction_index, 0);
        assert_eq!(tx.instructions[1].instruction_index, 1);
    }

    #[test]
    fn decode_instruction_out_of_range_program_id_is_a_typed_error_not_a_panic() {
        // The path that would have been an unchecked account_keys[idx]
        // panic before .get().ok_or_else() -- proves it degrades to a
        // typed error instead.
        let account_keys = vec![[0u8; 32]];
        let instruction = InnerInstruction {
            program_id_index: 99, // out of range for a 1-element account_keys
            accounts: vec![],
            data: "1".to_string(),
        };
        let result = decode_instruction(instruction, &account_keys, 0);
        assert!(result.is_err());
    }

    // Real transaction (signature 5XpoGEhyuhQPcSMc8qJ6vw83LrGpLkKuEeXZcn48Q7tJsho1c92cgxqhMVNsuGiU51UT3yFGMT5SVKa9YoXjZfiA,
    // slot 452025725, transactionIndex 1105, blockTime 1790787664) captured
    // live via HeliusProvider and saved at
    // docs/p0/measurements/fixtures/pump_mint1_full.json (data[2]).
    // TRIMMED EXCERPT, not a verbatim capture: only 1 of 6 top-level
    // instructions is retained (instruction[4], the one whose accounts
    // reach into the ALT range), and `meta` is reduced to just
    // `loadedAddresses` (the only field this test exercises). The
    // `addressTableLookups` array below IS copied verbatim from the
    // real record -- its indices sum to exactly 15 writable + 22
    // readonly, matching `loadedAddresses` below byte-for-byte; a
    // fabricated lookup table would not satisfy that invariant.
    fn alt_balances(post: bool) -> Vec<u64> {
        let mut balances = vec![1_000_000_000u64; 48];
        if post {
            balances[0] -= 5_000;
            balances[11] += 5_000;
        }
        balances
    }

    fn alt_transaction_body() -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "data": [{
                    "transaction": {
                        "signatures": ["5XpoGEhyuhQPcSMc8qJ6vw83LrGpLkKuEeXZcn48Q7tJsho1c92cgxqhMVNsuGiU51UT3yFGMT5SVKa9YoXjZfiA"],
                        "message": {
                            "accountKeys": [
                                "7JCe3GHwkEr3feHgtLXnmuJ1yB3A7coSeyynxTBgdG8k",
                                "3nGwiYU8foQk1SGWEjC7WW9t2EFhzn5ytik9KL1NmmS4",
                                "AAnzozhdS8oYSfEMfwbX5F9NPtw3EhM9CNCUnKLRf6dW",
                                "AN1wvW6VnjH8A7TKK8LK1BsUsPaBDZ7er9qUEJAUv4e8",
                                "11111111111111111111111111111111",
                                "ComputeBudget111111111111111111111111111111",
                                "58PMEdUAwvLytNNwCbzrYyhLoh3jpsNV4fW9dT9ibuRc",
                                "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
                                "BVsVzWfjxVQc1Zdveoj9HyqoQW2wMRixspbnF7WdGtNa",
                                "DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH",
                                "EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv"
                            ],
                            "header": {
                                "numRequiredSignatures": 1,
                                "numReadonlySignedAccounts": 0,
                                "numReadonlyUnsignedAccounts": 5
                            },
                            "instructions": [
                                {"programIdIndex": 6, "accounts": [0, 0, 8, 29, 30, 14, 13], "data": "3"}
                            ],
                            "addressTableLookups": [
                                {
                                    "accountKey": "3vwxVdZD5vQHxQkRoNvbS4XZbSqwBUmU3GQyuPNysca7",
                                    "writableIndexes": [183, 249, 176, 92, 216],
                                    "readonlyIndexes": [208, 95, 134, 50, 51, 108, 55, 173, 186, 139, 187, 94, 88, 185]
                                },
                                {
                                    "accountKey": "8sfehAX22hJh9tk4c19Jmni8yMJYK7M7XfoWAGzBNstE",
                                    "writableIndexes": [18, 10, 17],
                                    "readonlyIndexes": []
                                },
                                {
                                    "accountKey": "9AKCoNoAGYLW71TwTHY9e7KrZUWWL3c7VtHKb66NT3EV",
                                    "writableIndexes": [223, 23, 219],
                                    "readonlyIndexes": [225, 22]
                                },
                                {
                                    "accountKey": "9rVP9Ly5RC1nix3WDm5QgkoWJbxV7Kteth1KHtYk5hT9",
                                    "writableIndexes": [],
                                    "readonlyIndexes": [173, 10, 172]
                                },
                                {
                                    "accountKey": "FJ9UStVLv75bt3C3NxnVWREkEeqJij4N8bfx6VNv55oj",
                                    "writableIndexes": [160, 157, 168, 164],
                                    "readonlyIndexes": [175, 132, 173]
                                }
                            ]
                        }
                    },
                    "meta": {
                        "err": null,
                        "fee": 5000,
                        // 11 static + 15 writable + 22 readonly = 48 keys.
                        // Index 0 pays the fee; index 11 (first loaded
                        // writable) gains lamports.
                        "preBalances": alt_balances(false),
                        "postBalances": alt_balances(true),
                        "loadedAddresses": {
                            "writable": [
                                "7xQYoUjUJF1Kg6WVczoTAkaNhn5syQYcbvjmFrhjWpx",
                                "3XCBmEGtot44VFAoBXWoDoBDii7tCaVdQXA5pVV26qfo",
                                "CU3RGMeZVagD3Son8ytvbumtvwvHkwwhAbPBVB9cPvQE",
                                "CrSD3RV8CgxQiraRmgpKBjtTXjStdVBVFNQ8DqW4T659",
                                "GAFuhgcd328SkkBYHpfadzmef9hTGAFRCi9QoCnsZQug",
                                "2Y7HATmn9aJBcxCskE5V2U2epmjvkZmB51zTJBbhj4cU",
                                "8FnX3xo2yYw3EUE6w3nQA4GfXGS9wpK6oj3veJpbFzLo",
                                "ATRsNGv2nDw7hSMfkUTBoVUDsFDwN7po7KbecyiGWNB4",
                                "5pVN5XZB8cYBjNLFrsBCPWkCQBan5K5Mq2dWGzwPgGJV",
                                "9t4P5wMwfFkyn92Z7hf463qYKEZf8ERVZsGBEPNp8uJx",
                                "FLckHLGMJy5gEoXWwcE68Nprde1D4araK4TGLw4pQq2n",
                                "2FHsHW8LFBKaw79NFFNs1msdAU4K8xsqBPxw5R2kPmMP",
                                "7DfuFARLQHn6y7bKd928Unz2gcQS6taVYf3UzjxNdK3y",
                                "AfkcFeEqwuQVjNbwfj6kg7CDkcCayfNuBSNv19sY2LMh",
                                "HvAmVQJP1TH75Sfdf5gyXzs3su6z5E5JV7joPEAwRDvG"
                            ],
                            "readonly": [
                                "9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7",
                                "So11111111111111111111111111111111111111112",
                                "TessVdML9pBGgG9yGks7o4HewRaXVAMuoVj4x83GLQH",
                                "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
                                "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
                                "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
                                "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",
                                "5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx",
                                "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw",
                                "BiSoNHVpsVZW2F7rx2eQ59yQwKxzU5NvBcmKshCSUypi",
                                "C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw",
                                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                                "FWsW1xNtWscwNmKv6wVsU1iTzRN6wmmk3MjxRP5tT7hz",
                                "GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR",
                                "Sysvar1nstructions1111111111111111111111111",
                                "8ekCy2jHHUbW2yeNGFWYJT9Hm9FW7SvZcZK66dSZCDiF",
                                "4cG31VNF9TzFinNc7BmnjhFvGjxkY3sCETVMtMgbrhPs",
                                "8xeaWCsJYxRoudEZGJWURdfrtFhLYZz9b4iHJnW5tb3d",
                                "BAT1Ndpu5gbLTp2AZkSXP79LJBZfCH4B3zGhi6LtvdhK",
                                "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump",
                                "3pZTGDAAeBGZBtcGriK4jnz4nxNidXP1J7S1sH4QFcZb",
                                "7u5bUML1gHyFNofNrkHdy9kC4BUhUuvPPfXbcqeRhenB"
                            ]
                        }
                    },
                    "version": 0,
                    "slot": 452025725,
                    "transactionIndex": 1105,
                    "blockTime": 1790787664
                }],
                "paginationToken": null,
            }
        })
    }

    #[test]
    fn decode_resolves_program_id_from_address_lookup_table_range() {
        // account index 29 falls outside static accountKeys (0..10) --
        // only resolvable via loadedAddresses.readonly[3]
        // (TokenkegQfeZ...), proving the concatenated index space is
        // actually used, not just static keys.
        let body = alt_transaction_body();
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        let record = result.data.into_iter().next().unwrap();
        let tx = decode_full_transaction_record(record).unwrap();

        assert_eq!(tx.instructions.len(), 1);
        let resolved_account = tx.instructions[0].accounts[3]; // account[29] -> Token program
        let expected = decode_pubkey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
        assert_eq!(resolved_account, expected);
    }

    #[test]
    fn decode_rejects_alt_lookups_with_no_resolved_addresses() {
        // A v0 transaction declaring addressTableLookups but whose
        // response carries no meta.loadedAddresses cannot be honestly
        // decoded -- any index beyond the static range is unresolvable.
        // Must be a typed error, never a silent truncation to the
        // static-only account list (which would misattribute every
        // ALT-range index to the wrong account).
        let mut body = alt_transaction_body();
        body["result"]["data"][0]
            .as_object_mut()
            .unwrap()
            .remove("meta");
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        let record = result.data.into_iter().next().unwrap();
        assert!(decode_full_transaction_record(record).is_err());
    }

    #[tokio::test]
    async fn scan_yields_one_envelope_per_transaction_in_the_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "data": [
                        full_mode_body(None)["result"]["data"][0].clone(),
                        full_mode_body(None)["result"]["data"][0].clone(),
                    ],
                }
            })))
            .mount(&server)
            .await;

        let provider =
            HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
                .unwrap();

        let mut stream = provider.scan(
            ScanTask {
                request: ScanRequest::WalletActivity {
                    wallet: parse_solana_wallet("5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1")
                        .unwrap(),
                },
                description: "wallet:5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1".to_string(),
            },
            CancellationToken::new(),
        );

        let mut count = 0;
        while let Some(envelope) = stream.next().await {
            let envelope = envelope.unwrap();
            assert!(matches!(envelope.payload, RawPayload::SolanaTransaction(_)));
            count += 1;
        }
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn scan_accepts_a_mint_address_for_token_market_activity() {
        // Per docs/p0/measurements/2026-10-01-helius-mint-centric-query.md:
        // getTransactionsForAddress accepts an SPL mint, not just a
        // wallet. This proves scan() actually sends the mint string
        // (not silently falling back to some wallet-shaped request) --
        // the wiremock server only matches on method/path, so this
        // doesn't independently verify the request BODY contains the
        // mint, but combined with the dispatch match in scan() (which
        // reads ScanRequest::TokenMarketActivity's AssetKey::Token
        // address directly, no re-parsing), this is the integration
        // point that would break if that wiring regressed.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "data": [full_mode_body(None)["result"]["data"][0].clone()],
                }
            })))
            .mount(&server)
            .await;

        let provider =
            HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
                .unwrap();

        let mint = scout_core::AddressBytes::Solana(
            decode_pubkey("NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump").unwrap(),
        );
        let chain = scout_core::ChainKey {
            family: scout_core::ChainFamily::Solana,
            network_id: scout_core::NetworkId::SolanaCluster(scout_core::SolanaCluster::Mainnet),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        };
        let mut stream = provider.scan(
            ScanTask {
                request: ScanRequest::TokenMarketActivity {
                    asset: scout_core::AssetKey::Token(chain, mint),
                },
                description: "token:NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump".to_string(),
            },
            CancellationToken::new(),
        );

        let first = stream.next().await;
        assert!(
            matches!(first, Some(Ok(_))),
            "expected a decoded envelope, got {first:?}"
        );
    }

    #[tokio::test]
    async fn scan_rejects_native_asset_for_token_market_activity() {
        let provider = HeliusProvider::new_with_endpoint(
            scout_rpc::RpcEndpoint::new("http://127.0.0.1:0"),
            5_000,
            1,
        )
        .unwrap();

        let chain = scout_core::ChainKey {
            family: scout_core::ChainFamily::Solana,
            network_id: scout_core::NetworkId::SolanaCluster(scout_core::SolanaCluster::Mainnet),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        };
        let mut stream = provider.scan(
            ScanTask {
                request: ScanRequest::TokenMarketActivity {
                    asset: scout_core::AssetKey::Native(chain),
                },
                description: "native asset has no mint".to_string(),
            },
            CancellationToken::new(),
        );

        let first = stream.next().await;
        assert!(matches!(
            first,
            Some(Err(ProviderError::Unsupported { .. }))
        ));
    }

    /// Returns the SAME real transaction used for the ALT regression
    /// tests above (data[2] of pump_mint1_full.json) -- it conveniently
    /// also has real preTokenBalances/postTokenBalances for the mint
    /// this session's census focused on, making it a genuine one-shot
    /// fixture for both concerns rather than two disconnected ones.
    fn token_balance_transaction_body() -> serde_json::Value {
        let mut body = alt_transaction_body();
        body["result"]["data"][0]["meta"]["preTokenBalances"] = json!([
            {
                "accountIndex": 1,
                "mint": "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump",
                "owner": "EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv",
                "uiTokenAmount": {"amount": "41636451", "decimals": 6, "uiAmount": 41.636451, "uiAmountString": "41.636451"}
            },
            {
                "accountIndex": 22,
                "mint": "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump",
                "owner": "7DfuFARLQHn6y7bKd928Unz2gcQS6taVYf3UzjxNdK3y",
                "uiTokenAmount": {"amount": "729570450161577", "decimals": 6, "uiAmount": 729570450.161577, "uiAmountString": "729570450.161577"}
            }
        ]);
        body["result"]["data"][0]["meta"]["postTokenBalances"] = json!([
            {
                "accountIndex": 1,
                "mint": "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump",
                "owner": "EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv",
                "uiTokenAmount": {"amount": "181714920688", "decimals": 6, "uiAmount": 181714.920688, "uiAmountString": "181714.920688"}
            },
            {
                "accountIndex": 3,
                "mint": "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump",
                "owner": "7JCe3GHwkEr3feHgtLXnmuJ1yB3A7coSeyynxTBgdG8k",
                "uiTokenAmount": {"amount": "0", "decimals": 6, "uiAmount": null, "uiAmountString": "0"}
            },
            {
                "accountIndex": 22,
                "mint": "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump",
                "owner": "7DfuFARLQHn6y7bKd928Unz2gcQS6taVYf3UzjxNdK3y",
                "uiTokenAmount": {"amount": "729388776877340", "decimals": 6, "uiAmount": 729388776.87734, "uiAmountString": "729388776.87734"}
            }
        ]);
        body
    }

    #[test]
    fn token_balance_changes_identify_the_buyer_by_owner_not_position() {
        // Real data: this exact transaction (signature
        // 5XpoGEhyuhQPcSMc8qJ6vw83LrGpLkKuEeXZcn48Q7tJsho1c92cgxqhMVNsuGiU51UT3yFGMT5SVKa9YoXjZfiA,
        // docs/p0/measurements/fixtures/pump_mint1_full.json data[2]).
        // Owner EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv's tracked
        // balance for this mint went from 41,636,451 to
        // 181,714,920,688 base units -- verified directly against the
        // committed fixture, not recalled from a different probe. This
        // is a DIFFERENT real transaction from the one used in
        // scout-dex-solana's event-CPI false-positive regression test
        // (that one's 206321 delta belongs to an unrelated
        // signature/owner from a separate probe) -- no cross-fixture
        // numeric connectivity is claimed here, only this transaction's
        // own numbers.
        let body = token_balance_transaction_body();
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        let record = result.data.into_iter().next().unwrap();
        let tx = decode_full_transaction_record(record).unwrap();

        let target_mint = decode_pubkey("NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump").unwrap();
        let buyer_owner = decode_pubkey("EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv").unwrap();

        let buyer_change = tx
            .token_balance_changes
            .iter()
            .find(|c| c.mint == target_mint && c.owner == Some(buyer_owner))
            .expect("buyer's token balance change must be present");

        assert_eq!(buyer_change.pre_amount, Some(41_636_451));
        assert_eq!(buyer_change.post_amount, 181_714_920_688);
        assert_eq!(buyer_change.decimals, 6);
        assert_eq!(
            buyer_change.post_amount - buyer_change.pre_amount.unwrap(),
            181_673_284_237
        );
    }

    #[test]
    fn pool_account_with_equal_opposite_delta_is_not_mistaken_for_the_buyer() {
        // The pool (7DfuFARLQHn6y7bKd928Unz2gcQS6taVYf3UzjxNdK3y) lost
        // exactly the same 206321 units the buyer gained -- a decoder
        // that picked "the account with the right magnitude" instead
        // of "the account whose balance increased" would misattribute
        // the pool as a second buyer. Confirms both appear as distinct
        // owners with opposite-signed deltas, never merged or deduped.
        let body = token_balance_transaction_body();
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        let record = result.data.into_iter().next().unwrap();
        let tx = decode_full_transaction_record(record).unwrap();

        let target_mint = decode_pubkey("NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump").unwrap();
        let pool_owner = decode_pubkey("7DfuFARLQHn6y7bKd928Unz2gcQS6taVYf3UzjxNdK3y").unwrap();

        let pool_change = tx
            .token_balance_changes
            .iter()
            .find(|c| c.mint == target_mint && c.owner == Some(pool_owner))
            .expect("pool's token balance change must be present");

        assert_eq!(pool_change.pre_amount, Some(729_570_450_161_577));
        assert_eq!(pool_change.post_amount, 729_388_776_877_340);
        assert!(pool_change.post_amount < pool_change.pre_amount.unwrap());
    }

    #[test]
    fn account_absent_from_pre_token_balances_has_none_not_an_error() {
        // accountIndex=3 (7JCe3GHwkEr3feHgtLXnmuJ1yB3A7coSeyynxTBgdG8k)
        // appears only in postTokenBalances in this real fixture -- its
        // associated token account did not exist before this
        // transaction. Must decode to pre_amount=None (meaning zero),
        // never a decode error or a fabricated Some(0) that looks
        // identical to "we observed a zero balance."
        let body = token_balance_transaction_body();
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        let record = result.data.into_iter().next().unwrap();
        let tx = decode_full_transaction_record(record).unwrap();

        let target_mint = decode_pubkey("NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump").unwrap();
        let new_ata_owner = decode_pubkey("7JCe3GHwkEr3feHgtLXnmuJ1yB3A7coSeyynxTBgdG8k").unwrap();

        let new_account_change = tx
            .token_balance_changes
            .iter()
            .find(|c| c.mint == target_mint && c.owner == Some(new_ata_owner))
            .expect("newly-created ATA's token balance change must be present");

        assert_eq!(new_account_change.pre_amount, None);
        assert_eq!(new_account_change.post_amount, 0);
    }

    // ---- pagination ----

    const WALLET: &str = "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1";

    fn task() -> ScanTask {
        ScanTask {
            request: ScanRequest::WalletActivity {
                wallet: parse_solana_wallet(WALLET).unwrap(),
            },
            description: format!("wallet:{WALLET}"),
        }
    }

    /// A page with one transaction per slot in `slots`.
    fn page(slots: &[u64], token: Option<&str>) -> serde_json::Value {
        let template = full_mode_body(None)["result"]["data"][0].clone();
        let data: Vec<_> = slots
            .iter()
            .map(|slot| {
                let mut tx = template.clone();
                tx["slot"] = json!(slot);
                tx
            })
            .collect();
        json!({"jsonrpc": "2.0", "id": 1, "result": {"data": data, "paginationToken": token}})
    }

    /// Serves `pages[token]` where the key is the request's
    /// `paginationToken` option ("" for none). Unknown token -> HTTP 500.
    async fn paged_server(pages: Vec<(&'static str, serde_json::Value)>) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let token = body["params"][1]["paginationToken"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                match pages.iter().find(|(k, _)| *k == token) {
                    Some((_, page)) => ResponseTemplate::new(200).set_body_json(page.clone()),
                    None => ResponseTemplate::new(500),
                }
            })
            .mount(&server)
            .await;
        server
    }

    async fn request_sort_orders(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                body["params"][1]["sortOrder"].as_str().unwrap().to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn default_scan_order_requests_asc() {
        let server = paged_server(vec![("", page(&[1], None))]).await;
        drain(&provider(&server, 10), CancellationToken::new()).await;
        assert_eq!(request_sort_orders(&server).await, vec!["asc"]);
    }

    #[tokio::test]
    async fn newest_first_requests_desc_follows_cursors_and_keeps_provider_order() {
        let server = paged_server(vec![
            ("", page(&[5, 4], Some("t1"))),
            ("t1", page(&[3], Some("t2"))),
            ("t2", page(&[2, 1], None)),
        ])
        .await;
        let p = provider(&server, 10).with_scan_order(ScanOrder::NewestFirst);
        let out = drain(&p, CancellationToken::new()).await;
        let expect: Vec<_> = [5, 4, 3, 2, 1].iter().map(|s| Some((*s, false))).collect();
        assert_eq!(out, expect);
        assert_eq!(
            request_tokens(&server).await,
            vec![None, Some("t1".into()), Some("t2".into())]
        );
        assert_eq!(request_sort_orders(&server).await, vec!["desc"; 3]);
    }

    #[tokio::test]
    async fn newest_first_budget_exhaustion_marks_only_last_envelope_truncated() {
        let server = paged_server(vec![
            ("", page(&[5, 4], Some("t1"))),
            ("t1", page(&[3, 2], Some("t2"))),
            ("t2", page(&[1], None)),
        ])
        .await;
        let p = provider(&server, 2).with_scan_order(ScanOrder::NewestFirst);
        let out = drain(&p, CancellationToken::new()).await;
        assert_eq!(
            out,
            vec![
                Some((5, false)),
                Some((4, false)),
                Some((3, false)),
                Some((2, true))
            ]
        );
        assert_eq!(request_sort_orders(&server).await, vec!["desc"; 2]);
    }

    fn provider(server: &MockServer, max_pages: u32) -> HeliusProvider {
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
            .with_max_pages(NonZeroU32::new(max_pages).unwrap())
    }

    /// (slot, truncated) for Ok items; None for Err items.
    async fn drain(
        provider: &HeliusProvider,
        cancel: CancellationToken,
    ) -> Vec<Option<(u64, bool)>> {
        let mut out = Vec::new();
        let mut stream = provider.scan(task(), cancel);
        while let Some(item) = stream.next().await {
            out.push(item.ok().map(|e| match e.payload {
                RawPayload::SolanaTransaction(tx) => (tx.slot, e.truncated),
                _ => panic!("unexpected payload"),
            }));
        }
        out
    }

    async fn request_tokens(server: &MockServer) -> Vec<Option<String>> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                body["params"][1]["paginationToken"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect()
    }

    #[tokio::test]
    async fn pagination_follows_cursor_to_natural_end_without_truncation() {
        let server = paged_server(vec![
            ("", page(&[1, 2], Some("t1"))),
            ("t1", page(&[3], Some("t2"))),
            ("t2", page(&[4, 5], None)),
        ])
        .await;
        let out = drain(&provider(&server, 10), CancellationToken::new()).await;
        let expect: Vec<_> = (1..=5).map(|s| Some((s, false))).collect();
        assert_eq!(out, expect);
        assert_eq!(
            request_tokens(&server).await,
            vec![None, Some("t1".into()), Some("t2".into())]
        );
    }

    #[tokio::test]
    async fn pagination_budget_exhaustion_marks_only_last_page_truncated() {
        let server = paged_server(vec![
            ("", page(&[1, 2], Some("t1"))),
            ("t1", page(&[3, 4], Some("t2"))),
            ("t2", page(&[5], None)),
        ])
        .await;
        let out = drain(&provider(&server, 2), CancellationToken::new()).await;
        assert_eq!(
            out,
            vec![
                Some((1, false)),
                Some((2, false)),
                Some((3, false)),
                Some((4, true))
            ]
        );
        assert_eq!(request_tokens(&server).await.len(), 2);
    }

    #[tokio::test]
    async fn pagination_budget_exhausted_on_empty_page_still_flags_truncation() {
        let server = paged_server(vec![
            ("", page(&[1], Some("t1"))),
            ("t1", page(&[], Some("t2"))),
        ])
        .await;
        let out = drain(&provider(&server, 2), CancellationToken::new()).await;
        assert_eq!(out, vec![Some((1, true))]);
    }

    #[tokio::test]
    async fn pagination_empty_only_page_with_cursor_at_budget_is_an_error() {
        let server = paged_server(vec![("", page(&[], Some("t1")))]).await;
        let out = drain(&provider(&server, 1), CancellationToken::new()).await;
        assert_eq!(out, vec![None]);
    }

    #[tokio::test]
    async fn pagination_empty_page_without_cursor_is_end_of_history() {
        let server =
            paged_server(vec![("", page(&[1], Some("t1"))), ("t1", page(&[], None))]).await;
        let out = drain(&provider(&server, 10), CancellationToken::new()).await;
        assert_eq!(out, vec![Some((1, false))]);
    }

    #[tokio::test]
    async fn pagination_error_after_first_page_yields_page_then_one_error() {
        // "t1" is not served -> HTTP 500 on page 2.
        let server = paged_server(vec![("", page(&[1, 2], Some("t1")))]).await;
        let out = drain(&provider(&server, 10), CancellationToken::new()).await;
        assert_eq!(out, vec![Some((1, false)), Some((2, false)), None]);
    }

    #[tokio::test]
    async fn pagination_repeated_cursor_stops_with_truncation_and_error() {
        let server = paged_server(vec![
            ("", page(&[1], Some("t1"))),
            ("t1", page(&[2], Some("t1"))),
        ])
        .await;
        let out = drain(&provider(&server, 10), CancellationToken::new()).await;
        assert_eq!(out, vec![Some((1, true)), None]);
        assert_eq!(request_tokens(&server).await.len(), 2);
    }

    #[tokio::test]
    async fn pagination_cancellation_between_pages_marks_truncated() {
        let server = paged_server(vec![
            ("", page(&[1, 2], Some("t1"))),
            ("t1", page(&[3], None)),
        ])
        .await;
        let provider = provider(&server, 10);
        let cancel = CancellationToken::new();
        let mut stream = provider.scan(task(), cancel.clone());
        let first = stream.next().await.unwrap().unwrap();
        assert!(!first.truncated);
        cancel.cancel();
        let mut rest = Vec::new();
        while let Some(item) = stream.next().await {
            rest.push(item.unwrap().truncated);
        }
        // Held-back last envelope of page 1 carries the flag; page 2
        // was never requested.
        assert_eq!(rest, vec![true]);
        assert_eq!(request_tokens(&server).await.len(), 1);
    }

    #[tokio::test]
    async fn cancellation_before_first_page_is_an_error_not_empty_success() {
        let server = paged_server(vec![("", page(&[1], None))]).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let out = drain(&provider(&server, 10), cancel).await;
        assert_eq!(out, vec![None]);
        assert!(request_tokens(&server).await.is_empty());
    }

    // ---- meta.err execution status and closed token accounts ----------

    fn decode_body(body: &serde_json::Value) -> Result<RawSolanaTransaction, ProviderError> {
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        decode_full_transaction_record(result.data.into_iter().next().unwrap())
    }

    fn decode_fixture(name: &str) -> Vec<Result<RawSolanaTransaction, ProviderError>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures")
            .join(name);
        let body: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let result: TransactionsForAddressResult =
            serde_json::from_value(body["result"].clone()).unwrap();
        result
            .data
            .into_iter()
            .map(decode_full_transaction_record)
            .collect()
    }

    #[test]
    fn probe_fixture_execution_status_succeeded_then_failed() {
        let txs = decode_fixture("pump_bonding_curve_buy_probe.json");
        assert_eq!(txs.len(), 5);
        for tx in &txs[..4] {
            assert_eq!(
                tx.as_ref().unwrap().execution,
                SolanaExecutionStatus::Succeeded
            );
        }
        match &txs[4].as_ref().unwrap().execution {
            SolanaExecutionStatus::Failed { error } => {
                assert_eq!(error, r#"{"InstructionError":[4,{"Custom":6042}]}"#);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn missing_err_key_or_meta_is_a_typed_error_not_success() {
        let mut body = alt_transaction_body();
        body["result"]["data"][0]["meta"]
            .as_object_mut()
            .unwrap()
            .remove("err");
        assert!(decode_body(&body).is_err());

        let mut body = full_mode_body(None);
        body["result"]["data"][0]
            .as_object_mut()
            .unwrap()
            .remove("meta");
        assert!(decode_body(&body).is_err());
    }

    #[test]
    fn failed_error_text_is_bounded_and_control_free() {
        let mut body = full_mode_body(None);
        body["result"]["data"][0]["meta"]["err"] =
            json!({"InstructionError": [0, {"Custom": "x\u{1b}[31m\n".repeat(500)}]});
        let tx = decode_body(&body).unwrap();
        match tx.execution {
            SolanaExecutionStatus::Failed { error } => {
                assert!(error.chars().count() <= MAX_EXECUTION_ERROR_LEN);
                assert!(!error.chars().any(char::is_control));
            }
            SolanaExecutionStatus::Succeeded => panic!("must be Failed"),
        }
    }

    // ---- native SOL accounting facts ---------------------------------

    fn fixture_records(body: &serde_json::Value) -> Vec<serde_json::Value> {
        if let Some(data) = body["result"]["data"].as_array() {
            return data.clone();
        }
        body["pages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["data"].as_array().unwrap().clone())
            .collect()
    }

    fn load_fixture_records(name: &str) -> Vec<serde_json::Value> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures")
            .join(name);
        let body: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        fixture_records(&body)
    }

    fn decode_raw(record: &serde_json::Value) -> Result<RawSolanaTransaction, ProviderError> {
        decode_full_transaction_record(serde_json::from_value(record.clone()).unwrap())
    }

    #[test]
    fn native_facts_match_raw_json_on_all_committed_fixtures() {
        let mut total = 0;
        for name in [
            "pump_bonding_curve_buy_probe.json",
            "pump_variants_live_2026-10-02.json",
        ] {
            for record in load_fixture_records(name) {
                let tx = decode_raw(&record).unwrap();
                total += 1;
                let fee = record["meta"]["fee"].as_u64().unwrap();
                assert!(fee > 0);
                assert_eq!(tx.fee_lamports, fee);
                let keys = record["transaction"]["message"]["accountKeys"]
                    .as_array()
                    .unwrap();
                assert_eq!(
                    tx.fee_payer,
                    decode_pubkey(keys[0].as_str().unwrap()).unwrap()
                );
                let n = record["transaction"]["message"]["header"]["numRequiredSignatures"]
                    .as_u64()
                    .unwrap();
                assert_eq!(u64::try_from(tx.signers.len()).unwrap(), n);
                assert_eq!(tx.signers[0], tx.fee_payer);
                for change in &tx.native_balance_changes {
                    assert_ne!(change.pre_lamports, change.post_lamports);
                }
            }
        }
        assert!(total >= 21, "expected all fixture txs, got {total}");
    }

    #[test]
    fn successful_buy_fee_payer_lamport_delta_is_negative() {
        // Known successful pump.fun buy_exact_sol_in from the live
        // variants fixture (fee payer is also the token buyer).
        const BUY_SIG: &str = "ySd9GrKgj9QYcV8Ty2hwuHQJM1V6NdrexMhbD4ByzwnV6GAxX7jVC9WeRrjAJXyvC1o6PRPR7z6AEx1SezU95Pe";
        let records = load_fixture_records("pump_variants_live_2026-10-02.json");
        let record = records
            .iter()
            .find(|r| r["transaction"]["signatures"][0] == BUY_SIG)
            .expect("buy tx in fixture");
        let tx = decode_raw(record).unwrap();
        assert!(tx.execution.is_success());
        let payer = tx
            .native_balance_changes
            .iter()
            .find(|c| c.account == tx.fee_payer)
            .expect("fee payer balance changed");
        assert!(payer.delta() < 0);
        // Order is by ascending account index.
        let keys = record["transaction"]["message"]["accountKeys"]
            .as_array()
            .unwrap();
        let positions: Vec<usize> = tx
            .native_balance_changes
            .iter()
            .map(|c| {
                keys.iter()
                    .position(|k| decode_pubkey(k.as_str().unwrap()).unwrap() == c.account)
                    .unwrap()
            })
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn alt_loaded_accounts_get_their_own_balances() {
        let mut checked = false;
        for record in load_fixture_records("pump_variants_live_2026-10-02.json") {
            let loaded = &record["meta"]["loadedAddresses"];
            let writable = loaded["writable"].as_array().cloned().unwrap_or_default();
            let readonly = loaded["readonly"].as_array().cloned().unwrap_or_default();
            if writable.is_empty() && readonly.is_empty() {
                continue;
            }
            let tx = decode_raw(&record).unwrap();
            let static_len = record["transaction"]["message"]["accountKeys"]
                .as_array()
                .unwrap()
                .len();
            let pre = record["meta"]["preBalances"].as_array().unwrap();
            let post = record["meta"]["postBalances"].as_array().unwrap();
            let loaded_keys: Vec<&str> = writable
                .iter()
                .chain(readonly.iter())
                .map(|k| k.as_str().unwrap())
                .collect();
            for (offset, key) in loaded_keys.iter().enumerate() {
                let i = static_len + offset;
                let (p, q) = (pre[i].as_u64().unwrap(), post[i].as_u64().unwrap());
                let pk = decode_pubkey(key).unwrap();
                let found = tx.native_balance_changes.iter().find(|c| c.account == pk);
                if p == q {
                    assert!(found.is_none());
                } else {
                    let c = found.expect("changed ALT account listed");
                    assert_eq!((c.pre_lamports, c.post_lamports), (p, q));
                    checked = true;
                }
            }
        }
        assert!(checked, "no ALT-loaded account with a balance change found");
    }

    #[test]
    fn synthetic_alt_tx_maps_loaded_balances_and_fee_payer() {
        let tx = decode_body(&alt_transaction_body()).unwrap();
        assert_eq!(tx.fee_lamports, 5000);
        assert_eq!(tx.signers.len(), 1);
        assert_eq!(tx.native_balance_changes.len(), 2);
        assert_eq!(tx.native_balance_changes[0].account, tx.fee_payer);
        assert_eq!(tx.native_balance_changes[0].delta(), -5000);
        // Index 11 is loadedAddresses.writable[0].
        assert_eq!(
            tx.native_balance_changes[1].account,
            decode_pubkey("7xQYoUjUJF1Kg6WVczoTAkaNhn5syQYcbvjmFrhjWpx").unwrap()
        );
        assert_eq!(tx.native_balance_changes[1].delta(), 5000);
    }

    #[test]
    fn missing_fee_header_or_balances_and_length_mismatch_are_typed_errors() {
        let meta_keys = ["fee", "preBalances", "postBalances"];
        for key in meta_keys {
            let mut body = full_mode_body(None);
            body["result"]["data"][0]["meta"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(decode_body(&body).is_err(), "missing meta.{key}");
        }
        let mut body = full_mode_body(None);
        body["result"]["data"][0]["transaction"]["message"]
            .as_object_mut()
            .unwrap()
            .remove("header");
        assert!(decode_body(&body).is_err());

        // pre/post mismatch.
        let mut body = full_mode_body(None);
        body["result"]["data"][0]["meta"]["postBalances"] = json!([1, 2]);
        assert!(decode_body(&body).is_err());
        // Both shorter than the key count.
        let mut body = full_mode_body(None);
        body["result"]["data"][0]["meta"]["preBalances"] = json!([1, 2]);
        body["result"]["data"][0]["meta"]["postBalances"] = json!([1, 2]);
        assert!(decode_body(&body).is_err());
        // Signer count beyond static keys.
        let mut body = full_mode_body(None);
        body["result"]["data"][0]["transaction"]["message"]["header"]["numRequiredSignatures"] =
            json!(9);
        assert!(decode_body(&body).is_err());
    }

    #[test]
    fn native_delta_is_checked_wide_arithmetic() {
        let c = SolanaNativeBalanceChange {
            account: [0; 32],
            pre_lamports: u64::MAX,
            post_lamports: 0,
        };
        assert_eq!(c.delta(), -i128::from(u64::MAX));
    }

    fn tb(index: u32, mint: &str, owner: &str, amount: &str) -> TokenBalanceEntry {
        serde_json::from_value(json!({
            "accountIndex": index,
            "mint": mint,
            "owner": owner,
            "uiTokenAmount": {"amount": amount, "decimals": 6}
        }))
        .unwrap()
    }

    const MINT_A: &str = "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump";
    const MINT_B: &str = "So11111111111111111111111111111111111111112";
    const OWNER_A: &str = "EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv";
    const OWNER_B: &str = "7DfuFARLQHn6y7bKd928Unz2gcQS6taVYf3UzjxNdK3y";

    #[test]
    fn pre_only_token_account_is_a_closed_change_with_pre_amount() {
        let keys = vec![[1u8; 32]; 4];
        let pre = vec![tb(1, MINT_A, OWNER_A, "500"), tb(2, MINT_A, OWNER_B, "7")];
        let post = vec![tb(2, MINT_A, OWNER_B, "9")];
        let changes = decode_token_balance_changes(&pre, &post, &keys).unwrap();
        assert_eq!(changes.len(), 2);
        let open = &changes[0];
        assert_eq!(
            (open.pre_amount, open.post_amount, open.closed),
            (Some(7), 9, false)
        );
        let closed = &changes[1];
        assert_eq!(closed.pre_amount, Some(500));
        assert_eq!(closed.post_amount, 0);
        assert!(closed.closed);
        assert_eq!(closed.mint, decode_pubkey(MINT_A).unwrap());
        assert_eq!(closed.owner, Some(decode_pubkey(OWNER_A).unwrap()));
        assert_eq!(closed.decimals, 6);
    }

    #[test]
    fn observed_zero_post_balance_is_not_closed() {
        let keys = vec![[1u8; 32]; 4];
        let pre = vec![tb(1, MINT_A, OWNER_A, "500")];
        let post = vec![tb(1, MINT_A, OWNER_A, "0")];
        let changes = decode_token_balance_changes(&pre, &post, &keys).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].closed);
    }

    #[test]
    fn pre_post_mint_or_owner_mismatch_is_a_typed_error() {
        let keys = vec![[1u8; 32]; 4];
        let pre = vec![tb(1, MINT_A, OWNER_A, "5")];
        assert!(decode_token_balance_changes(&pre, &[tb(1, MINT_B, OWNER_A, "5")], &keys).is_err());
        assert!(decode_token_balance_changes(&pre, &[tb(1, MINT_A, OWNER_B, "5")], &keys).is_err());
    }

    #[test]
    fn duplicate_account_index_is_a_typed_error() {
        let keys = vec![[1u8; 32]; 4];
        let dup = vec![tb(1, MINT_A, OWNER_A, "5"), tb(1, MINT_A, OWNER_A, "6")];
        assert!(decode_token_balance_changes(&dup, &[], &keys).is_err());
        assert!(decode_token_balance_changes(&[], &dup, &keys).is_err());
    }

    #[test]
    fn pre_only_out_of_range_index_is_a_typed_error() {
        let keys = vec![[1u8; 32]; 2];
        let pre = vec![tb(9, MINT_A, OWNER_A, "5")];
        assert!(decode_token_balance_changes(&pre, &[], &keys).is_err());
    }

    #[test]
    fn probe_fixture_sell_closing_ata_shows_exact_owner_outflow() {
        let txs = decode_fixture("pump_bonding_curve_buy_probe.json");
        let tx1 = txs[1].as_ref().unwrap();
        let mut found = false;
        for change in tx1.token_balance_changes.iter().filter(|c| c.closed) {
            if change.pre_amount == Some(175_202_561_501) {
                found = true;
                assert_eq!(change.post_amount, 0);
                let owner = bs58::encode(change.owner.unwrap()).into_string();
                assert!(owner.starts_with("FoaRt"), "{owner}");
            }
        }
        assert!(found, "closed seller ATA with pre 175202561501 not found");
    }

    #[test]
    fn mint1_fixture_data2_buyer_delta_unchanged_and_all_fixtures_decode() {
        let txs = decode_fixture("pump_mint1_full.json");
        let tx = txs[2].as_ref().unwrap();
        let mint = decode_pubkey("NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump").unwrap();
        let owner = decode_pubkey("EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv").unwrap();
        let delta: i128 = tx
            .token_balance_changes
            .iter()
            .filter(|c| c.mint == mint && c.owner == Some(owner))
            .map(|c| i128::from(c.post_amount) - i128::from(c.pre_amount.unwrap_or(0)))
            .sum();
        assert_eq!(delta, 181_673_284_237);
        for name in [
            "pump_mint1_full.json",
            "pump_mint2_full.json",
            "wallet_full_probe.json",
            "pump_bonding_curve_buy_probe.json",
        ] {
            for tx in decode_fixture(name) {
                tx.unwrap();
            }
        }
    }
}
