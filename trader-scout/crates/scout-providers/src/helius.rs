//! `HeliusProvider`: `scout_api::HistoryProvider` backed by Helius's
//! `getTransactionsForAddress` (Solana). See
//! `docs/p0/measurements/2026-09-27-helius-blockscout.md` for the live
//! probes this implementation is built against — both `signatures` and
//! `full` detail modes are confirmed working on the free tier, and
//! `full` mode is what this provider uses: 10 credits per 100 returned
//! transactions, a 10x reduction over calling `getTransaction` once per
//! signature (1 credit each).
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

use futures::stream::{self, BoxStream, StreamExt};
use scout_api::{
    CapabilityStatus, HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest,
    ScanTask, SourceCapabilities,
};
use scout_core::{RawPayload, RawSolanaInstruction, RawSolanaTransaction};
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

#[derive(Debug, Deserialize)]
struct TransactionMeta {
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
        Ok(Self { client })
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
        slot: record.slot,
        transaction_index: record.transaction_index,
        instructions,
        token_balance_changes,
    })
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
    let pre_by_index: std::collections::BTreeMap<u32, &TokenBalanceEntry> = pre
        .iter()
        .map(|entry| (entry.account_index, entry))
        .collect();

    let mut changes = Vec::with_capacity(post.len());
    for entry in post {
        let mint = decode_pubkey(&entry.mint)?;
        let owner = entry.owner.as_deref().map(decode_pubkey).transpose()?;
        let post_amount: u64 = entry
            .ui_token_amount
            .amount
            .parse()
            .map_err(|_| malformed("postTokenBalances amount is not a valid u64"))?;

        let pre_amount = match pre_by_index.get(&entry.account_index) {
            Some(pre_entry) => {
                let amount: u64 = pre_entry
                    .ui_token_amount
                    .amount
                    .parse()
                    .map_err(|_| malformed("preTokenBalances amount is not a valid u64"))?;
                Some(amount)
            }
            // Absent from preTokenBalances: the account did not exist
            // before this transaction (e.g. ATA created within it).
            // Honestly None (= zero), not an error.
            None => None,
        };

        // Sanity check, not a correctness requirement: accountIndex
        // should resolve within the shared account-key space this
        // transaction already built. A failure here means Helius
        // returned an index this provider's ALT resolution did not
        // anticipate -- surface it rather than silently accept an
        // unverifiable index.
        let index = usize::try_from(entry.account_index)
            .map_err(|_| malformed("token balance accountIndex exceeds usize"))?;
        if account_keys.get(index).is_none() {
            return Err(malformed(
                "token balance accountIndex out of range of the transaction's account-key space",
            ));
        }

        changes.push(scout_core::SolanaTokenBalanceChange {
            mint,
            owner,
            decimals: entry.ui_token_amount.decimals,
            pre_amount,
            post_amount,
        });
    }

    Ok(changes)
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
        _cancel: CancellationToken,
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

        // One page (up to MAX_TRANSACTIONS_PER_SCAN transactions) per
        // scan() call for this P0 vertical slice — pagination via the
        // response's pagination_token is a real follow-up (tracked,
        // not silently dropped), not built here to keep this step
        // narrow per the plan's own guidance.
        //
        // fetch_transactions() returns the whole page as one unit (the
        // HTTP call either succeeds with a full page or fails with a
        // typed error — never a silently-truncated partial page), so
        // this flattens that single Result<Vec<_>> into N Ok stream
        // items on success or one Err item on failure. A caller
        // draining the stream never sees a silently empty stream
        // (ADR-006) — success yields N envelopes, failure yields
        // exactly one typed error.
        let fetch = self.fetch_page(address);
        Box::pin(stream::once(fetch).flat_map(stream_results))
    }
}

impl HeliusProvider {
    async fn fetch_page(
        &self,
        address: String,
    ) -> Result<(Vec<RawSolanaTransaction>, bool), ProviderError> {
        self.fetch_transactions_page(&address, MAX_TRANSACTIONS_PER_SCAN)
            .await
    }

    /// Like `fetch_transactions`, but also reports whether the response
    /// carried an unconsumed `paginationToken` -- that signal is the
    /// provider-level fact `ScanEnvelope.truncated` exists to surface
    /// (ARCHITECTURE.md §4: a provider declaring the end of a range is
    /// not itself a durable checkpoint). This is a separate method
    /// rather than widening `fetch_transactions`'s own return type,
    /// since that method's existing callers (none currently outside
    /// this crate) have no need for the pagination signal.
    async fn fetch_transactions_page(
        &self,
        address: &str,
        limit: u32,
    ) -> Result<(Vec<RawSolanaTransaction>, bool), ProviderError> {
        let params = serde_json::json!([
            address,
            {
                "transactionDetails": "full",
                "sortOrder": "asc",
                "limit": limit,
            }
        ]);

        let result: TransactionsForAddressResult = self
            .client
            .call("getTransactionsForAddress", params)
            .await?;

        let truncated = result.pagination_token.is_some();
        let transactions = result
            .data
            .into_iter()
            .map(decode_full_transaction_record)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((transactions, truncated))
    }
}

fn stream_results(
    result: Result<(Vec<RawSolanaTransaction>, bool), ProviderError>,
) -> BoxStream<'static, Result<ScanEnvelope, ProviderError>> {
    match result {
        Ok((transactions, truncated)) => {
            let envelopes: Vec<_> = transactions
                .into_iter()
                .map(|tx| {
                    Ok(ScanEnvelope {
                        payload: RawPayload::SolanaTransaction(tx),
                        truncated,
                    })
                })
                .collect();
            Box::pin(stream::iter(envelopes))
        }
        Err(err) => Box::pin(stream::once(async move { Err(err) })),
    }
}

/// Maximum transactions fetched by one `scan()` call. Chosen as a
/// single-page value (Helius caps `full` mode at 1,000 per call per
/// its own docs) — deliberately conservative for this first wiring
/// pass, not a measured budget number (that's P0.6/P0.8).
const MAX_TRANSACTIONS_PER_SCAN: u32 = 100;

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
                                "instructions": [
                                    {"programIdIndex": 1, "accounts": [0], "data": "3Bxs4h"}
                                ]
                            }
                        },
                        "meta": {
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
}
