//! `HeliusProvider`: `scout_api::HistoryProvider` backed by Helius's
//! `getTransactionsForAddress` (Solana). See
//! `docs/p0/measurements/2026-09-27-helius-blockscout.md` for the live
//! probes this implementation is built against — both `signatures` and
//! `full` detail modes are confirmed working on the free tier, and
//! `full` mode is what this provider uses: 10 credits per 100 returned
//! transactions, a 10x reduction over calling `getTransaction` once per
//! signature (1 credit each).
//!
//! Scope: this provider satisfies `ScanRequest::WalletActivity` only.
//! `TokenMarketActivity` (token -> historical buyers) is not something
//! `getTransactionsForAddress` can answer directly — it is address-
//! centric, not mint-centric — so that request variant returns
//! `ProviderError::Unsupported` here rather than a wrong or partial
//! answer (AGENTS.md invariant #18: an unfamiliar/unsupported shape is
//! surfaced, never silently degraded).

use futures::stream::{self, BoxStream, StreamExt};
use scout_api::{
    CapabilityStatus, HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest,
    ScanTask, SourceCapabilities,
};
use scout_core::{RawPayload, RawSolanaInstruction, RawSolanaTransaction, WalletKey};
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
    #[allow(dead_code)] // kept for a future paginated scan(), not read yet
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

    /// Fetch up to `limit` full transactions for a wallet, oldest-first
    /// (`sortOrder: "asc"`) so canonical ordering (ADR-002: slot +
    /// transaction_index, never fetch order) is easy for a caller to
    /// preserve downstream — this method does not itself sort, it
    /// relies on Helius honoring the requested order and passes
    /// `(slot, transaction_index)` through unchanged either way.
    async fn fetch_transactions(
        &self,
        wallet: &WalletKey,
        limit: u32,
    ) -> Result<Vec<RawSolanaTransaction>, ProviderError> {
        let address = wallet.address.to_string();
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

        result
            .data
            .into_iter()
            .map(decode_full_transaction_record)
            .collect()
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
    let account_keys = record
        .transaction
        .message
        .account_keys
        .iter()
        .map(|key| decode_pubkey(key))
        .collect::<Result<Vec<_>, _>>()?;

    let mut inner_by_top_level_index: std::collections::BTreeMap<u32, Vec<InnerInstruction>> =
        std::collections::BTreeMap::new();
    if let Some(meta) = record.meta {
        for group in meta.inner_instructions {
            inner_by_top_level_index.insert(group.index, group.instructions);
        }
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
    })
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
        // LiveVerified per docs/p0/measurements/2026-09-27-helius-blockscout.md
        // (dated, actual successful call — ADR-006). TokenMarketActivity
        // is Unsupported, not Unknown: we have checked, and
        // getTransactionsForAddress cannot answer a mint-centric query.
        let mut caps = SourceCapabilities::empty();
        caps.by_capability.insert(
            "wallet_activity".to_string(),
            CapabilityStatus::LiveVerified,
        );
        caps.by_capability.insert(
            "token_market_activity".to_string(),
            CapabilityStatus::Unsupported,
        );
        caps
    }

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        match request {
            ScanRequest::WalletActivity { .. } => Ok(ScanPlan {
                request_echo: format!("{request:?}"),
                capabilities: self.capabilities(),
            }),
            ScanRequest::TokenMarketActivity { .. } => Err(ProviderError::Unsupported {
                capability: "token_market_activity".to_string(),
            }),
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
        // Wallet address is threaded through ScanTask.description today
        // (ScanTask is deliberately minimal per scout-api's own docs,
        // pending the real scheduler) — parsed back out here rather
        // than widening ScanTask's shape for one provider's needs.
        let Some(address) = task.description.strip_prefix("wallet:") else {
            let error = ProviderError::Other(Box::new(std::io::Error::other(
                "HeliusProvider::scan requires a ScanTask::description of the form \
                 'wallet:<base58-address>'",
            )));
            return Box::pin(stream::once(async move { Err(error) }));
        };
        let address = address.to_string();

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
    ) -> Result<Vec<RawSolanaTransaction>, ProviderError> {
        let wallet = parse_solana_wallet(&address)?;
        self.fetch_transactions(&wallet, MAX_TRANSACTIONS_PER_SCAN)
            .await
    }
}

fn stream_results(
    result: Result<Vec<RawSolanaTransaction>, ProviderError>,
) -> BoxStream<'static, Result<ScanEnvelope, ProviderError>> {
    match result {
        Ok(transactions) => {
            let envelopes: Vec<_> = transactions
                .into_iter()
                .map(|tx| {
                    Ok(ScanEnvelope {
                        payload: RawPayload::SolanaTransaction(tx),
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

fn parse_solana_wallet(address: &str) -> Result<WalletKey, ProviderError> {
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
    Ok(WalletKey {
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
    async fn plan_rejects_token_market_activity_but_accepts_wallet_activity() {
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
}
