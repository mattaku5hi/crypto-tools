//! History assembly for EVM chains (ADR-020 §3): from tx hashes (token-centric
//! via `eth_getLogs`, wallet-centric via Blockscout) to `RawEvmTransaction`s
//! with receipts, fees, block times and — only when a complete source
//! provided them — internal transfers.
//!
//! Everything is bounded (tx cap, log cap, concurrency, request budget) and
//! the output is in canonical `(block, transaction_index)` order regardless
//! of async completion order (invariant #12).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use alloy_primitives::{Address, B256, U256};
use scout_core::{ChainKey, InternalTransfer, NativeSource, RawEvmTransaction};
use scout_evm::TRANSFER_TOPIC0;

use crate::evm_alchemy::{
    ALCHEMY_COVERAGE_NOTE, ALCHEMY_LISTING_KIND, AlchemyCategory, AlchemyListing, AlchemyTransfer,
    AlchemyTransfersSource,
};
use crate::evm_blockscout::{BlockscoutEvmSource, BlockscoutInternal, Listing};
use crate::evm_rpc::{EvmRpcClient, EvmSourceError, LogFilter};
use crate::evm_wire::{EvmReceiptInfo, malformed};

/// How receipts are fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptMode {
    /// One `eth_getBlockReceipts` per distinct block (few requests, big
    /// bodies).
    BlockReceipts,
    /// One `eth_getTransactionReceipt` per transaction.
    PerTransaction,
    /// Per block: `eth_getBlockReceipts` when at least
    /// `ScanLimits::block_receipts_min_txs` of the needed transactions share
    /// it, else one `eth_getTransactionReceipt` each. A block's receipts are
    /// one big body (and a heavy call on metered providers), so isolated
    /// transactions of a wallet are cheaper one by one.
    Auto,
}

#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    /// Max distinct transactions assembled per call.
    pub max_transactions: usize,
    pub receipt_mode: ReceiptMode,
    /// `ReceiptMode::Auto`: blocks with at least this many needed
    /// transactions are fetched with `eth_getBlockReceipts`.
    pub block_receipts_min_txs: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_transactions: 20_000,
            receipt_mode: ReceiptMode::Auto,
            block_receipts_min_txs: 4,
        }
    }
}

/// Result of [`EvmHistoryScanner::scan_swap_logs`].
#[derive(Debug, Clone)]
pub struct SwapScanOutput {
    /// The newest `max_txs` distinct transactions, canonical order.
    pub transactions: Vec<RawEvmTransaction>,
    /// Matching logs in the window (all of them, before the cap).
    pub swap_logs: usize,
    /// Distinct transactions with a matching log, before the cap.
    pub txs_before_cap: usize,
    pub log_requests: u32,
    pub log_splits: u32,
}

/// Native internal transfers keyed by transaction, with their provenance.
#[derive(Debug, Clone, Default)]
pub struct InternalIndex {
    by_tx: HashMap<B256, Vec<InternalTransfer>>,
    source: Option<NativeSource>,
}

impl InternalIndex {
    /// Build from a **complete** Blockscout listing. Failed internal calls
    /// revert their value and are dropped. Returns `None` when the listing
    /// was incomplete (`internal_transfers` must then stay `None`).
    #[must_use]
    pub fn from_complete_listing(listing: &Listing<BlockscoutInternal>) -> Option<Self> {
        if !listing.complete {
            return None;
        }
        let mut by_tx: HashMap<B256, Vec<InternalTransfer>> = HashMap::new();
        for r in listing
            .rows
            .iter()
            .filter(|r| !r.failed && !r.value.is_zero())
        {
            by_tx.entry(r.hash).or_default().push(InternalTransfer {
                from: r.from,
                to: r.to,
                value: r.value,
            });
        }
        Some(Self {
            by_tx,
            source: Some(NativeSource::Explorer),
        })
    }

    /// Build from a COMPLETE Alchemy `internal` listing (category supported
    /// and pagination finished). `None` when the chain does not support the
    /// category or the listing is incomplete.
    #[must_use]
    pub fn from_alchemy(listing: &AlchemyListing) -> Option<Self> {
        let rows = listing
            .internal
            .as_ref()
            .filter(|_| listing.internal_complete)?;
        let mut by_tx: HashMap<B256, Vec<InternalTransfer>> = HashMap::new();
        for r in rows.iter().filter(|r| !r.value.is_zero() && r.to.is_some()) {
            if let Some(to) = r.to {
                by_tx.entry(r.hash).or_default().push(InternalTransfer {
                    from: r.from,
                    to,
                    value: r.value,
                });
            }
        }
        Some(Self {
            by_tx,
            source: Some(NativeSource::AlchemyInternal),
        })
    }

    fn for_tx(&self, hash: &B256) -> Vec<InternalTransfer> {
        self.by_tx.get(hash).cloned().unwrap_or_default()
    }
}

/// Result of a token-centric scan.
#[derive(Debug, Clone)]
pub struct TokenScanOutput {
    pub transactions: Vec<RawEvmTransaction>,
    /// `Transfer` logs of the token inside the window.
    pub transfer_logs: usize,
    pub log_requests: u32,
    pub log_splits: u32,
}

/// Result of a wallet-centric scan.
#[derive(Debug, Clone)]
pub struct WalletScanOutput {
    pub transactions: Vec<RawEvmTransaction>,
    /// `false`: the wallet's `txlist` was truncated by the page cap.
    pub txlist_complete: bool,
    /// `false`: the wallet's `tokentx` listing was truncated by the page cap.
    pub tokentx_complete: bool,
    /// Distinct transactions the listings named (before the transaction cap).
    pub listed_transactions: usize,
    /// How many of them only appeared in `tokentx` (W did not sign them).
    pub token_only_transactions: usize,
    /// `false`: internal transfers were not (completely) available; every
    /// transaction then carries `internal_transfers = None`.
    pub internal_complete: bool,
    /// Which indexer listed the wallet (`blockscout`, `alchemy_transfers`).
    pub listing_kind: &'static str,
    /// Known blind spot of that indexer, `None` when there is none.
    pub coverage_note: Option<&'static str>,
}

/// Normalized phase-1 result of a wallet listing indexer.
#[derive(Debug, Clone)]
pub struct IndexedWallet {
    pub kind: &'static str,
    /// Transactions with a known signer = the wallet (full tx fields).
    pub signed: Vec<IndexedSigned>,
    /// Token transfers to/from the wallet (any signer).
    pub transfers: Vec<IndexedTransfer>,
    /// Complete internal transfers, `None` when not (completely) available.
    pub internal: Option<InternalIndex>,
    pub signed_complete: bool,
    pub transfers_complete: bool,
    pub internal_complete: bool,
    /// `true`: `signed` names EVERY transaction the wallet signed in the
    /// range (Blockscout `txlist`); `false`: a token transfer from the wallet
    /// may belong to a transaction the wallet signed that `signed` lacks.
    pub signer_known: bool,
    pub coverage_note: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct IndexedSigned {
    pub hash: B256,
    pub block_number: u64,
    pub time_stamp: Option<u64>,
    pub from: Address,
    pub to: Option<Address>,
    pub value: U256,
    pub gas_price: Option<U256>,
}

#[derive(Debug, Clone)]
pub struct IndexedTransfer {
    pub hash: B256,
    pub block_number: u64,
    pub time_stamp: Option<u64>,
    pub from: Address,
}

/// The wallet-history indexer of a run (ADR-020 amendment 4).
#[derive(Debug, Clone)]
pub enum WalletIndexer {
    Blockscout(BlockscoutEvmSource),
    /// Boxed: the source carries an RPC client clone.
    AlchemyTransfers(Box<AlchemyTransfersSource>),
}

impl From<BlockscoutEvmSource> for WalletIndexer {
    fn from(s: BlockscoutEvmSource) -> Self {
        Self::Blockscout(s)
    }
}

impl From<AlchemyTransfersSource> for WalletIndexer {
    fn from(s: AlchemyTransfersSource) -> Self {
        Self::AlchemyTransfers(Box::new(s))
    }
}

impl WalletIndexer {
    /// `blockscout` / `alchemy_transfers` (the report's `listing_kind`).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Blockscout(_) => "blockscout",
            Self::AlchemyTransfers(_) => ALCHEMY_LISTING_KIND,
        }
    }

    /// The indexer's known blind spot for reports, if any.
    #[must_use]
    pub fn coverage_note(&self) -> Option<&'static str> {
        match self {
            Self::Blockscout(_) => None,
            Self::AlchemyTransfers(_) => Some(ALCHEMY_COVERAGE_NOTE),
        }
    }

    /// Indexer-side HTTP attempts not already counted by the shared RPC
    /// client (Blockscout is a separate HTTP service; Alchemy rides the RPC).
    #[must_use]
    pub fn own_requests_made(&self) -> u64 {
        match self {
            Self::Blockscout(b) => b.total_requests_made(),
            Self::AlchemyTransfers(_) => 0,
        }
    }

    async fn list(
        &self,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<IndexedWallet, EvmSourceError> {
        match self {
            Self::Blockscout(src) => {
                let txlist = src.txlist(wallet, from_block, to_block).await?;
                let tokentx = src
                    .token_transfers(wallet, None, from_block, to_block)
                    .await?;
                let internal = src.internal_transfers(wallet, from_block, to_block).await?;
                Ok(IndexedWallet {
                    kind: self.kind(),
                    signed: txlist
                        .rows
                        .iter()
                        .map(|t| IndexedSigned {
                            hash: t.hash,
                            block_number: t.block_number,
                            time_stamp: Some(t.time_stamp),
                            from: t.from,
                            to: t.to,
                            value: t.value,
                            gas_price: Some(t.gas_price),
                        })
                        .collect(),
                    transfers: tokentx
                        .rows
                        .iter()
                        .map(|t| IndexedTransfer {
                            hash: t.hash,
                            block_number: t.block_number,
                            time_stamp: Some(t.time_stamp),
                            from: t.from,
                        })
                        .collect(),
                    internal: InternalIndex::from_complete_listing(&internal),
                    signed_complete: txlist.complete,
                    transfers_complete: tokentx.complete,
                    internal_complete: internal.complete,
                    signer_known: true,
                    coverage_note: None,
                })
            }
            Self::AlchemyTransfers(src) => {
                let l = src.list(wallet, from_block, to_block).await?;
                let internal = InternalIndex::from_alchemy(&l);
                let mut signed = Vec::new();
                let mut transfers = Vec::new();
                for t in &l.transfers {
                    transfers_or_signed(t, wallet, &mut signed, &mut transfers);
                }
                Ok(IndexedWallet {
                    kind: self.kind(),
                    signed,
                    transfers,
                    internal,
                    signed_complete: l.transfers_complete,
                    transfers_complete: l.transfers_complete,
                    internal_complete: l.internal_complete,
                    signer_known: false,
                    coverage_note: Some(ALCHEMY_COVERAGE_NOTE),
                })
            }
        }
    }
}

/// An `external` row sent by the wallet is a transaction it signed (top-level
/// value transfers have `from == tx.from`); every other row only names the
/// transaction (`erc20` rows keep their sender for the sell-candidate test).
fn transfers_or_signed(
    t: &AlchemyTransfer,
    wallet: Address,
    signed: &mut Vec<IndexedSigned>,
    transfers: &mut Vec<IndexedTransfer>,
) {
    match t.category {
        AlchemyCategory::External if t.from == wallet => signed.push(IndexedSigned {
            hash: t.hash,
            block_number: t.block_number,
            time_stamp: t.time_stamp,
            from: t.from,
            to: t.to,
            value: t.value,
            gas_price: None,
        }),
        AlchemyCategory::External | AlchemyCategory::Erc20 => transfers.push(IndexedTransfer {
            hash: t.hash,
            block_number: t.block_number,
            time_stamp: t.time_stamp,
            from: t.from,
        }),
        AlchemyCategory::Internal => {}
    }
}

/// Assembles raw transactions for one verified chain.
#[derive(Debug, Clone)]
pub struct EvmHistoryScanner {
    rpc: EvmRpcClient,
    chain: ChainKey,
    limits: ScanLimits,
}

impl EvmHistoryScanner {
    /// `chain` should come from `EvmRpcClient::preflight`.
    #[must_use]
    pub fn new(rpc: EvmRpcClient, chain: ChainKey, limits: ScanLimits) -> Self {
        Self { rpc, chain, limits }
    }

    #[must_use]
    pub fn limits(&self) -> &ScanLimits {
        &self.limits
    }

    #[must_use]
    pub fn rpc(&self) -> &EvmRpcClient {
        &self.rpc
    }

    /// Block range for a time window: `since` inclusive, `until` exclusive
    /// (unix seconds). `None` = open end. Returns `None` when the window
    /// contains no block.
    pub async fn resolve_window(
        &self,
        since: Option<u64>,
        until: Option<u64>,
    ) -> Result<Option<(u64, u64)>, EvmSourceError> {
        let latest = self.rpc.block_number().await?;
        let from = match since {
            Some(ts) => self.rpc.first_block_at_or_after(ts, latest).await?,
            None => 0,
        };
        let end_exclusive = match until {
            Some(ts) => self.rpc.first_block_at_or_after(ts, latest).await?,
            None => latest.saturating_add(1),
        };
        Ok((from < end_exclusive).then(|| (from, end_exclusive - 1)))
    }

    /// Token-centric scan: every `Transfer` log of `token` in `[from, to]`,
    /// then receipts/transactions of the distinct tx hashes. Internal
    /// transfers are `None` (no trace source here).
    pub async fn scan_token(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<TokenScanOutput, EvmSourceError> {
        let filter = LogFilter {
            addresses: vec![token],
            topics: [Some(vec![TRANSFER_TOPIC0]), None, None, None],
        };
        let logs = self.rpc.get_logs(&filter, from_block, to_block).await?;
        // The topic filter also matches ERC-721 `Transfer`; assembly keeps
        // all logs of the tx and extraction rejects malformed ones.
        // RawEvmLog does not carry the tx hash; the logs' `transactionHash`
        // (when present) lets isolated transactions skip block receipts.
        let seen: BTreeSet<(u64, u64)> = logs
            .logs
            .iter()
            .map(|l| (l.block_number, l.transaction_index))
            .collect();
        if seen.len() > self.limits.max_transactions {
            // Cheap early exit: no receipt request for an oversized scan.
            return Err(EvmSourceError::TooManyTransactions {
                cap: self.limits.max_transactions,
            });
        }
        let transactions = self.transactions_at(&seen, &logs.tx_hashes).await?;
        Ok(TokenScanOutput {
            transactions,
            transfer_logs: logs.logs.len(),
            log_requests: logs.requests,
            log_splits: logs.splits,
        })
    }

    /// Swap-topic scan: every log matching `filters` (each an `eth_getLogs`
    /// filter, results unioned) in `[from_block, to_block]`, then receipts and
    /// transactions of the NEWEST `max_txs` distinct transactions. Older
    /// transactions are dropped and reported (`txs_before_cap`), never an
    /// error: the caller asked for a bounded sample.
    pub async fn scan_swap_logs(
        &self,
        filters: &[LogFilter],
        from_block: u64,
        to_block: u64,
        max_txs: usize,
    ) -> Result<SwapScanOutput, EvmSourceError> {
        let max_txs = max_txs.min(self.limits.max_transactions);
        let mut seen: BTreeSet<(u64, u64)> = BTreeSet::new();
        let mut tx_hashes: BTreeMap<(u64, u64), B256> = BTreeMap::new();
        let (mut swap_logs, mut log_requests, mut log_splits) = (0usize, 0u32, 0u32);
        for f in filters {
            let logs = self.rpc.get_logs(f, from_block, to_block).await?;
            swap_logs = swap_logs.saturating_add(logs.logs.len());
            log_requests = log_requests.saturating_add(logs.requests);
            log_splits = log_splits.saturating_add(logs.splits);
            tx_hashes.extend(logs.tx_hashes.iter().map(|(k, h)| (*k, *h)));
            seen.extend(
                logs.logs
                    .iter()
                    .map(|l| (l.block_number, l.transaction_index)),
            );
        }
        let txs_before_cap = seen.len();
        while seen.len() > max_txs {
            seen.pop_first();
        }
        let transactions = self.transactions_at(&seen, &tx_hashes).await?;
        Ok(SwapScanOutput {
            transactions,
            swap_logs,
            txs_before_cap,
            log_requests,
            log_splits,
        })
    }

    /// Receipts + transactions of the transactions at `(block, index)`
    /// positions, canonical order. Receipts follow [`ReceiptMode`]: in
    /// `Auto`, a block with fewer than `block_receipts_min_txs` wanted
    /// transactions whose hashes are all known (`hashes`, from the logs) is
    /// fetched one `eth_getTransactionReceipt` each (15 CU vs 500 CU for
    /// `eth_getBlockReceipts` on Alchemy; most token-scan blocks hold one
    /// wanted transaction). A per-hash receipt must sit at the wanted
    /// position, else the scan fails (reorg / provider inconsistency).
    async fn transactions_at(
        &self,
        seen: &BTreeSet<(u64, u64)>,
        hashes: &BTreeMap<(u64, u64), B256>,
    ) -> Result<Vec<RawEvmTransaction>, EvmSourceError> {
        let mut per_block: BTreeMap<u64, Vec<(u64, u64)>> = BTreeMap::new();
        for key in seen {
            per_block.entry(key.0).or_default().push(*key);
        }
        let mut blocks: Vec<u64> = Vec::new();
        let mut singles: Vec<((u64, u64), B256)> = Vec::new();
        for (b, keys) in &per_block {
            let all_hashed = keys.iter().all(|k| hashes.contains_key(k));
            let whole = match self.limits.receipt_mode {
                ReceiptMode::BlockReceipts => true,
                ReceiptMode::PerTransaction => !all_hashed,
                ReceiptMode::Auto => {
                    !all_hashed || keys.len() >= self.limits.block_receipts_min_txs
                }
            };
            if whole {
                blocks.push(*b);
            } else {
                singles.extend(keys.iter().filter_map(|k| hashes.get(k).map(|h| (*k, *h))));
            }
        }
        let mut receipt_by_pos: HashMap<(u64, u64), EvmReceiptInfo> = HashMap::new();
        for (_, rs) in self.rpc.receipts_by_blocks(&blocks).await? {
            for r in rs {
                receipt_by_pos.insert((r.block_number, r.transaction_index), r);
            }
        }
        let single_hashes: Vec<B256> = singles.iter().map(|(_, h)| *h).collect();
        let single_receipts = self.rpc.receipts_by_hashes(&single_hashes).await?;
        for ((key, h), r) in singles.iter().zip(single_receipts) {
            if (r.block_number, r.transaction_index) != *key || r.tx_hash != *h {
                return Err(malformed(
                    "eth_getTransactionReceipt",
                    format!(
                        "receipt of {h:#x} is at block {} index {}, the log said block {} \
                         index {}",
                        r.block_number, r.transaction_index, key.0, key.1
                    ),
                ));
            }
            receipt_by_pos.insert(*key, r);
        }
        // Keep only the receipts of the wanted transactions (whole-block
        // receipts are dropped here, bounding memory).
        let mut hashes: Vec<B256> = Vec::with_capacity(seen.len());
        let mut needed: Vec<EvmReceiptInfo> = Vec::with_capacity(seen.len());
        for key in seen {
            let r = receipt_by_pos.remove(key).ok_or_else(|| {
                malformed(
                    "eth_getBlockReceipts",
                    format!("no receipt at block {} index {}", key.0, key.1),
                )
            })?;
            hashes.push(r.tx_hash);
            needed.push(r);
        }
        // The needed receipts are already in hand: no second fetch.
        self.build(&hashes, None, Some(needed), None).await
    }

    /// Wallet-centric scan through the indexer listings (never a window-wide
    /// `eth_getLogs`, which is infeasible on 0.1 s-block chains with
    /// range-capped providers): [`Self::list_wallet`] then
    /// [`Self::assemble_listing`]. See those for the request costs.
    pub async fn scan_wallet(
        &self,
        source: &WalletIndexer,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<WalletScanOutput, EvmSourceError> {
        let listing = self
            .list_wallet(source, wallet, from_block, to_block)
            .await?;
        self.assemble_listing(listing).await
    }

    /// Phase 1 (indexer requests only): over the block range `[from, to]`
    ///
    /// - Blockscout: `txlist` (the wallet's own signed transactions, failed
    ///   included; these rows carry from/to/value/gas price/block, so NO
    ///   `eth_getTransactionByHash` is made for them), `tokentx` (every ERC-20
    ///   transfer to/from the wallet, which also names transactions the wallet
    ///   did NOT sign) and `txlistinternal` (only when the explorer reports
    ///   complete processing);
    /// - Alchemy transfers: `external` + `erc20` (+ `internal` where the chain
    ///   supports it) from/to the wallet. The signer of a transaction is known
    ///   only from an `external` row sent by the wallet; a hash with token
    ///   transfers FROM the wallet and no such row may still be signed by it
    ///   (a token sell with zero ETH value), which the receipt decides at the
    ///   cost of one `eth_getTransactionByHash` (counted in the plan).
    ///
    /// Hashes are deduplicated and the indexer's block timestamps seed the
    /// block-time cache. Nothing is fetched over RPC yet, so the cost of
    /// phase 2 can be planned ([`WalletListing::plan`]) and refused first.
    pub async fn list_wallet(
        &self,
        source: &WalletIndexer,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<WalletListing, EvmSourceError> {
        let indexed = source.list(wallet, from_block, to_block).await?;
        let mut described: HashMap<B256, Described> = HashMap::new();
        for t in indexed.signed.iter().filter(|t| t.from == wallet) {
            described.insert(
                t.hash,
                Described {
                    block_number: t.block_number,
                    kind: DescKind::Signed {
                        from: t.from,
                        to: t.to,
                        value: t.value,
                        gas_price: t.gas_price,
                    },
                },
            );
        }
        let own: HashSet<B256> = described.keys().copied().collect();
        let mut prefer_block: HashSet<B256> = HashSet::new();
        let mut signer_lookups: HashSet<B256> = HashSet::new();
        // Complete internal transfers are authoritative for native legs: no
        // sole-touch block receipts and no archive balance reads are needed.
        let internals_complete = indexed.internal.is_some();
        for t in &indexed.transfers {
            described.entry(t.hash).or_insert(Described {
                block_number: t.block_number,
                kind: DescKind::TokenOnly,
            });
            if t.from != wallet {
                continue;
            }
            if own.contains(&t.hash) {
                if internals_complete {
                    continue;
                }
                // A signed transaction in which the wallet SENDS a token may
                // be a sell: its block's receipts are fetched whole (once) so
                // the native-leg sole-touch check needs no second request.
                prefer_block.insert(t.hash);
            } else if !indexed.signer_known {
                // Maybe signed by the wallet (receipt decides): same block
                // policy, plus a possible transaction lookup for `value`.
                if !internals_complete {
                    prefer_block.insert(t.hash);
                }
                signer_lookups.insert(t.hash);
            }
        }
        let mut hashes: Vec<B256> = described.keys().copied().collect();
        hashes.sort();
        let token_only = hashes.len().saturating_sub(own.len());
        self.rpc.seed_block_timestamps(
            indexed
                .signed
                .iter()
                .map(|t| (t.block_number, t.time_stamp))
                .chain(
                    indexed
                        .transfers
                        .iter()
                        .map(|t| (t.block_number, t.time_stamp)),
                )
                .filter_map(|(b, ts)| ts.map(|ts| (b, ts))),
        );
        Ok(WalletListing {
            wallet,
            hashes,
            described,
            prefer_block,
            index: indexed.internal,
            txlist_complete: indexed.signed_complete,
            tokentx_complete: indexed.transfers_complete,
            internal_complete: indexed.internal_complete,
            token_only,
            signer_lookups: signer_lookups.len(),
            listing_kind: indexed.kind,
            coverage_note: indexed.coverage_note,
        })
    }

    /// Phase 2: receipts (one request per transaction, or one per crowded /
    /// possible-sell block) and the few transactions the explorer did not
    /// describe, then the canonical `RawEvmTransaction`s.
    pub async fn assemble_listing(
        &self,
        listing: WalletListing,
    ) -> Result<WalletScanOutput, EvmSourceError> {
        if listing.hashes.len() > self.limits.max_transactions {
            return Err(EvmSourceError::TooManyTransactions {
                cap: self.limits.max_transactions,
            });
        }
        let transactions = self
            .build(
                &listing.hashes,
                listing.index.as_ref(),
                None,
                Some(&listing),
            )
            .await?;
        Ok(WalletScanOutput {
            transactions,
            txlist_complete: listing.txlist_complete,
            tokentx_complete: listing.tokentx_complete,
            listed_transactions: listing.hashes.len(),
            token_only_transactions: listing.token_only,
            internal_complete: listing.index.is_some(),
            listing_kind: listing.listing_kind,
            coverage_note: listing.coverage_note,
        })
    }

    /// Assemble transactions for explicit hashes (no internal transfers
    /// unless `internals` is given).
    pub async fn assemble(
        &self,
        hashes: &[B256],
        internals: Option<&InternalIndex>,
    ) -> Result<Vec<RawEvmTransaction>, EvmSourceError> {
        if hashes.len() > self.limits.max_transactions {
            return Err(EvmSourceError::TooManyTransactions {
                cap: self.limits.max_transactions,
            });
        }
        self.build(hashes, internals, None, None).await
    }

    async fn build(
        &self,
        hashes: &[B256],
        internals: Option<&InternalIndex>,
        prefetched_receipts: Option<Vec<EvmReceiptInfo>>,
        listing: Option<&WalletListing>,
    ) -> Result<Vec<RawEvmTransaction>, EvmSourceError> {
        let describe = |h: &B256| listing.and_then(|l| l.described.get(h));
        // 1. Transactions: only hashes no explorer row describes cost a call.
        let undescribed: Vec<B256> = hashes
            .iter()
            .filter(|h| describe(h).is_none())
            .copied()
            .collect();
        let mut cores: HashMap<B256, TxCore> = HashMap::with_capacity(hashes.len());
        for t in self.rpc.transactions_by_hashes(&undescribed).await? {
            cores.insert(
                t.hash,
                TxCore {
                    from: t.from,
                    to: t.to,
                    value: t.value,
                    block_number: t.block_number,
                    index: Some(t.transaction_index),
                    gas_price: t.gas_price,
                },
            );
        }
        let mut token_only: Vec<B256> = Vec::new();
        for h in hashes {
            match describe(h) {
                Some(Described {
                    block_number,
                    kind:
                        DescKind::Signed {
                            from,
                            to,
                            value,
                            gas_price,
                        },
                }) => {
                    cores.insert(
                        *h,
                        TxCore {
                            from: *from,
                            to: *to,
                            value: *value,
                            block_number: *block_number,
                            index: None,
                            gas_price: *gas_price,
                        },
                    );
                }
                Some(Described {
                    kind: DescKind::TokenOnly,
                    ..
                }) => token_only.push(*h),
                None => {}
            }
        }
        // 2. Receipts: exactly one request per transaction, or one per
        // block when crowded / flagged (see `ReceiptMode::Auto`).
        let mut receipts: HashMap<B256, EvmReceiptInfo> = prefetched_receipts
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.tx_hash, r))
            .collect();
        let block_of = |h: &B256| {
            cores
                .get(h)
                .map(|c| c.block_number)
                .or_else(|| describe(h).map(|d| d.block_number))
        };
        let missing: Vec<(B256, u64)> = hashes
            .iter()
            .filter(|h| !receipts.contains_key(*h))
            .filter_map(|h| block_of(h).map(|b| (*h, b)))
            .collect();
        let prefer = |h: &B256| listing.is_some_and(|l| l.prefer_block.contains(h));
        let mut per_block: BTreeMap<u64, (usize, bool)> = BTreeMap::new();
        for (h, b) in &missing {
            let e = per_block.entry(*b).or_insert((0, false));
            e.0 += 1;
            e.1 |= prefer(h);
        }
        let use_block = |block: u64| match self.limits.receipt_mode {
            ReceiptMode::BlockReceipts => true,
            ReceiptMode::PerTransaction => false,
            ReceiptMode::Auto => per_block
                .get(&block)
                .is_some_and(|(n, flagged)| *flagged || *n >= self.limits.block_receipts_min_txs),
        };
        let single: Vec<B256> = missing
            .iter()
            .filter(|(_, b)| !use_block(*b))
            .map(|(h, _)| *h)
            .collect();
        let whole_blocks: Vec<u64> = per_block
            .keys()
            .copied()
            .filter(|b| use_block(*b))
            .collect();
        if !single.is_empty() {
            for r in self.rpc.receipts_by_hashes(&single).await? {
                receipts.insert(r.tx_hash, r);
            }
        }
        if !whole_blocks.is_empty() {
            for (_, rs) in self.rpc.receipts_by_blocks(&whole_blocks).await? {
                for r in rs {
                    receipts.insert(r.tx_hash, r);
                }
            }
        }
        // 3. Transactions the wallet did not sign: sender/recipient from the
        // receipt. `value` is never read for a non-signer (extraction books
        // only the signer's native value). A receipt that names the wallet
        // as sender, or has no `from`, falls back to the RPC transaction.
        let wallet = listing.map(|l| l.wallet);
        let mut fallback: Vec<B256> = Vec::new();
        for h in &token_only {
            let r = receipts.get(h).ok_or_else(|| EvmSourceError::NotFound {
                what: format!("receipt {h:#x}"),
            })?;
            match (r.from, wallet) {
                (Some(from), Some(w)) if from != w => {
                    cores.insert(
                        *h,
                        TxCore {
                            from,
                            to: r.to,
                            value: U256::ZERO,
                            block_number: r.block_number,
                            index: None,
                            gas_price: None,
                        },
                    );
                }
                _ => fallback.push(*h),
            }
        }
        for t in self.rpc.transactions_by_hashes(&fallback).await? {
            cores.insert(
                t.hash,
                TxCore {
                    from: t.from,
                    to: t.to,
                    value: t.value,
                    block_number: t.block_number,
                    index: Some(t.transaction_index),
                    gas_price: t.gas_price,
                },
            );
        }
        let blocks: Vec<u64> = cores
            .values()
            .map(|c| c.block_number)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let times: BTreeMap<u64, u64> = self.rpc.block_timestamps(&blocks).await?;

        let mut out = Vec::with_capacity(cores.len());
        for h in hashes {
            let t = cores.remove(h).ok_or_else(|| EvmSourceError::NotFound {
                what: format!("transaction {h:#x}"),
            })?;
            let r = receipts.remove(h).ok_or_else(|| EvmSourceError::NotFound {
                what: format!("receipt {h:#x}"),
            })?;
            if r.block_number != t.block_number || t.index.is_some_and(|i| i != r.transaction_index)
            {
                return Err(malformed(
                    "receipt",
                    format!("receipt position differs from transaction {h:#x}"),
                ));
            }
            let effective_gas_price: U256 = r
                .effective_gas_price
                .or(t.gas_price)
                .ok_or_else(|| malformed("receipt", "no effectiveGasPrice or gasPrice"))?;
            let block_time = *times.get(&t.block_number).ok_or_else(|| {
                malformed(
                    "block",
                    format!("no timestamp for block {}", t.block_number),
                )
            })?;
            out.push(RawEvmTransaction {
                chain: self.chain.clone(),
                hash: *h,
                from: t.from,
                to: t.to,
                block_number: t.block_number,
                transaction_index: r.transaction_index,
                block_time,
                value: t.value,
                status: r.status,
                gas_used: r.gas_used,
                effective_gas_price,
                l1_fee: r.l1_fee,
                logs: r.logs,
                internal_transfers: internals.map(|i| i.for_tx(h)),
                native_source: internals.and_then(|i| i.source),
                native_balance_diff: None,
            });
        }
        out.sort_by_key(|t| (t.block_number, t.transaction_index));
        Ok(out)
    }
}

/// Transaction fields assembled from an explorer row, a receipt or an RPC
/// transaction.
struct TxCore {
    from: Address,
    to: Option<Address>,
    value: U256,
    block_number: u64,
    /// Known position (RPC source); `None` = taken from the receipt.
    index: Option<u64>,
    gas_price: Option<U256>,
}

#[derive(Debug, Clone)]
enum DescKind {
    /// A `txlist` row of a transaction the wallet signed.
    Signed {
        from: Address,
        to: Option<Address>,
        value: U256,
        gas_price: Option<U256>,
    },
    /// Named by `tokentx` only (the wallet did not sign it).
    TokenOnly,
}

#[derive(Debug, Clone)]
struct Described {
    block_number: u64,
    kind: DescKind,
}

/// Phase-1 result of a wallet scan: the explorer's view, before any RPC
/// receipt/transaction request.
#[derive(Debug, Clone)]
pub struct WalletListing {
    wallet: Address,
    hashes: Vec<B256>,
    described: HashMap<B256, Described>,
    prefer_block: HashSet<B256>,
    index: Option<InternalIndex>,
    pub txlist_complete: bool,
    pub tokentx_complete: bool,
    pub internal_complete: bool,
    /// Transactions only `tokentx` named (the wallet did not sign them).
    pub token_only: usize,
    /// Token-only hashes with a transfer FROM the wallet whose signer the
    /// indexer could not tell (Alchemy): each may need one
    /// `eth_getTransactionByHash`. Always 0 for Blockscout.
    pub signer_lookups: usize,
    pub listing_kind: &'static str,
    pub coverage_note: Option<&'static str>,
}

impl WalletListing {
    /// Distinct transactions to assemble.
    #[must_use]
    pub fn transactions(&self) -> usize {
        self.hashes.len()
    }

    /// The RPC cost of phase 2 under `limits`.
    #[must_use]
    pub fn plan(&self, limits: &ScanLimits) -> WalletCostPlan {
        let mut per_block: BTreeMap<u64, (usize, bool)> = BTreeMap::new();
        for h in &self.hashes {
            if let Some(d) = self.described.get(h) {
                let e = per_block.entry(d.block_number).or_insert((0, false));
                e.0 += 1;
                e.1 |= self.prefer_block.contains(h);
            }
        }
        let (mut tx_receipt_calls, mut block_receipt_calls) = (0u64, 0u64);
        for (n, flagged) in per_block.values() {
            let whole = match limits.receipt_mode {
                ReceiptMode::BlockReceipts => true,
                ReceiptMode::PerTransaction => false,
                ReceiptMode::Auto => *flagged || *n >= limits.block_receipts_min_txs,
            };
            if whole {
                block_receipt_calls += 1;
            } else {
                tx_receipt_calls += u64::try_from(*n).unwrap_or(u64::MAX);
            }
        }
        WalletCostPlan {
            transactions: u64::try_from(self.hashes.len()).unwrap_or(u64::MAX),
            tx_receipt_calls,
            block_receipt_calls,
            candidate_sells: u64::try_from(self.prefer_block.len()).unwrap_or(u64::MAX),
            tx_lookups: u64::try_from(self.signer_lookups).unwrap_or(u64::MAX),
        }
    }
}

/// Planned RPC requests of assembling one wallet (before native legs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletCostPlan {
    pub transactions: u64,
    pub tx_receipt_calls: u64,
    pub block_receipt_calls: u64,
    /// Signed transactions in which the wallet sent a token: each MAY be a
    /// native-quoted sell needing two archive `eth_getBalance` calls.
    pub candidate_sells: u64,
    /// Upper bound of `eth_getTransactionByHash` for hashes whose signer the
    /// indexer could not name (the wallet signed them: need `tx.value`).
    pub tx_lookups: u64,
}

impl WalletCostPlan {
    /// Receipt requests, certain.
    #[must_use]
    pub fn base_requests(&self) -> u64 {
        self.tx_receipt_calls
            .saturating_add(self.block_receipt_calls)
    }

    /// Upper bound: every candidate sell resolved through the archive
    /// balance diff (2 `eth_getBalance`; its block receipts are already
    /// fetched as the transaction's own receipt).
    #[must_use]
    pub fn max_requests(&self) -> u64 {
        self.base_requests()
            .saturating_add(self.tx_lookups)
            .saturating_add(self.candidate_sells.saturating_mul(2))
    }

    /// Requests by method, upper bound for `eth_getBalance`.
    #[must_use]
    pub fn by_method(&self) -> BTreeMap<&'static str, u64> {
        BTreeMap::from([
            ("eth_getTransactionReceipt", self.tx_receipt_calls),
            ("eth_getBlockReceipts", self.block_receipt_calls),
            ("eth_getTransactionByHash(<=)", self.tx_lookups),
            ("eth_getBalance(<=)", self.candidate_sells.saturating_mul(2)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use scout_core::EvmTxStatus;
    use scout_evm::ROBINHOOD;
    use scout_rpc::{RpcClient, RpcEndpoint};
    use serde_json::{Value, json};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;
    use crate::evm_blockscout::{BlockscoutApiKey, BlockscoutEvmConfig};

    const TOKEN: &str = "0x00000000000000000000000000000000000000c1";
    const WALLET: &str = "0x00000000000000000000000000000000000000aa";

    fn h(n: u8) -> B256 {
        B256::repeat_byte(n)
    }

    const OTHER: &str = "0x00000000000000000000000000000000000000bb";

    /// `(block, index, from)` of the fake chain's transactions by hash.
    fn pos_of(hash: &str) -> (&'static str, &'static str, &'static str) {
        if hash == format!("{:#x}", h(0xa1)) {
            ("0x7", "0x1", WALLET)
        } else if hash == format!("{:#x}", h(0xee)) {
            ("0x8", "0x0", OTHER)
        } else {
            ("0x5", "0x0", WALLET)
        }
    }

    async fn methods(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .filter_map(|b| b["method"].as_str().map(str::to_string))
            .collect()
    }

    /// Fake chain: tx 0xa1 in block 7 idx 1 (wallet sends), tx 0xa2 in block 5 idx 0.
    struct Chain;
    impl Respond for Chain {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let result = match body["method"].as_str().unwrap() {
                "eth_getLogs" => json!([
                    {"address":TOKEN,"topics":[format!("{TRANSFER_TOPIC0:#x}")],"data":"0x",
                     "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x3"},
                    {"address":TOKEN,"topics":[format!("{TRANSFER_TOPIC0:#x}")],"data":"0x",
                     "blockNumber":"0x5","transactionIndex":"0x0","logIndex":"0x0"},
                    {"address":TOKEN,"topics":[format!("{TRANSFER_TOPIC0:#x}")],"data":"0x",
                     "blockNumber":"0x7","transactionIndex":"0x1","logIndex":"0x4"}
                ]),
                "eth_getBlockReceipts" => {
                    let b = body["params"][0].as_str().unwrap();
                    let (hash, idx) = if b == "0x7" {
                        (h(0xa1), "0x1")
                    } else {
                        (h(0xa2), "0x0")
                    };
                    json!([{"transactionHash":format!("{hash:#x}"),"blockNumber":b,
                        "transactionIndex":idx,"status":"0x1","gasUsed":"0x64",
                        "effectiveGasPrice":"0x2","logs":[]}])
                }
                "eth_getTransactionByHash" => {
                    let hs = body["params"][0].as_str().unwrap().to_string();
                    let (block, idx, from) = pos_of(&hs);
                    json!({"hash":hs,"from":from,"to":null,"value":"0x9","blockNumber":block,"transactionIndex":idx})
                }
                "eth_getTransactionReceipt" => {
                    let hs = body["params"][0].as_str().unwrap().to_string();
                    let (block, idx, from) = pos_of(&hs);
                    json!({"transactionHash":hs,"blockNumber":block,"from":from,
                        "transactionIndex":idx,"status":"0x1","gasUsed":"0x64",
                        "effectiveGasPrice":"0x2","logs":[]})
                }
                "eth_getBlockByNumber" => json!({"timestamp":"0x3e8"}),
                other => panic!("unexpected {other}"),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
        }
    }

    async fn scanner(server: &MockServer, limits: ScanLimits) -> EvmHistoryScanner {
        let rpc = RpcClient::new(RpcEndpoint::new(server.uri()), 5_000, 1).unwrap();
        EvmHistoryScanner::new(
            EvmRpcClient::new(rpc, ROBINHOOD),
            ROBINHOOD.verified_chain_key(),
            limits,
        )
    }

    #[tokio::test]
    async fn token_scan_dedups_hashes_orders_canonically_and_leaves_internals_unobserved() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&s)
            .await;
        let sc = scanner(&s, ScanLimits::default()).await;
        let out = sc.scan_token(TOKEN.parse().unwrap(), 0, 10).await.unwrap();
        assert_eq!(out.transfer_logs, 3);
        assert_eq!(out.transactions.len(), 2);
        let order: Vec<_> = out
            .transactions
            .iter()
            .map(|t| (t.block_number, t.transaction_index))
            .collect();
        assert_eq!(order, vec![(5, 0), (7, 1)]);
        let t = &out.transactions[1];
        assert_eq!(t.hash, h(0xa1));
        assert_eq!(
            (t.block_time, t.status, t.gas_used),
            (1000, EvmTxStatus::Success, 100)
        );
        assert_eq!(t.internal_transfers, None);
        assert_eq!(t.chain, ROBINHOOD.verified_chain_key());
    }

    /// [`Chain`] whose logs carry `transactionHash`: block 7's log names
    /// `hash7`, block 5's log has none.
    struct HashedChain {
        hash7: B256,
    }
    impl Respond for HashedChain {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            if body["method"] != "eth_getLogs" {
                return Chain.respond(req);
            }
            let t = format!("{TRANSFER_TOPIC0:#x}");
            let result = json!([
                {"address":TOKEN,"topics":[t],"data":"0x","blockNumber":"0x7",
                 "transactionIndex":"0x1","logIndex":"0x3",
                 "transactionHash":format!("{:#x}", self.hash7)},
                {"address":TOKEN,"topics":[t],"data":"0x","blockNumber":"0x5",
                 "transactionIndex":"0x0","logIndex":"0x0"}
            ]);
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
        }
    }

    #[tokio::test]
    async fn token_scan_fetches_isolated_hashed_transactions_one_by_one() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(HashedChain { hash7: h(0xa1) })
            .mount(&s)
            .await;
        let sc = scanner(&s, ScanLimits::default()).await;
        let out = sc.scan_token(TOKEN.parse().unwrap(), 0, 10).await.unwrap();
        let got: Vec<_> = out.transactions.iter().map(|t| t.hash).collect();
        assert_eq!(got, vec![h(0xa2), h(0xa1)]);
        let m = methods(&s).await;
        let n = |x: &str| m.iter().filter(|y| *y == x).count();
        // block 7 (hash known, 1 wanted tx): 15-CU receipt; block 5 (no hash
        // in the log): block receipts
        assert_eq!(n("eth_getTransactionReceipt"), 1);
        assert_eq!(n("eth_getBlockReceipts"), 1);
        // BlockReceipts mode keeps the old behaviour
        let s2 = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(HashedChain { hash7: h(0xa1) })
            .mount(&s2)
            .await;
        let limits = ScanLimits {
            receipt_mode: ReceiptMode::BlockReceipts,
            ..ScanLimits::default()
        };
        let sc = scanner(&s2, limits).await;
        sc.scan_token(TOKEN.parse().unwrap(), 0, 10).await.unwrap();
        let m = methods(&s2).await;
        assert_eq!(m.iter().filter(|y| *y == "eth_getBlockReceipts").count(), 2);
        assert!(!m.iter().any(|y| y == "eth_getTransactionReceipt"));
    }

    #[tokio::test]
    async fn token_scan_rejects_a_receipt_away_from_the_logged_position() {
        let s = MockServer::start().await;
        // the log at block 7 index 1 names a hash whose receipt is at block 8
        Mock::given(method("POST"))
            .respond_with(HashedChain { hash7: h(0xee) })
            .mount(&s)
            .await;
        let sc = scanner(&s, ScanLimits::default()).await;
        let err = sc
            .scan_token(TOKEN.parse().unwrap(), 0, 10)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("the log said block 7 index 1"), "{err}");
    }

    #[tokio::test]
    async fn swap_scan_keeps_the_newest_transactions_and_reports_the_cap() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&s)
            .await;
        let sc = scanner(&s, ScanLimits::default()).await;
        let filter = LogFilter {
            addresses: vec![],
            topics: [Some(vec![TRANSFER_TOPIC0]), None, None, None],
        };
        let out = sc
            .scan_swap_logs(std::slice::from_ref(&filter), 0, 10, 1)
            .await
            .unwrap();
        assert_eq!((out.swap_logs, out.txs_before_cap), (3, 2));
        // Newest first: the (block 7, index 1) transaction survives.
        assert_eq!(out.transactions.len(), 1);
        assert_eq!(out.transactions[0].hash, h(0xa1));
        let all = sc.scan_swap_logs(&[filter], 0, 10, 10).await.unwrap();
        let order: Vec<_> = all.transactions.iter().map(|t| t.block_number).collect();
        assert_eq!(order, vec![5, 7]);
    }

    #[tokio::test]
    async fn tx_cap_is_a_typed_error() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&s)
            .await;
        let sc = scanner(
            &s,
            ScanLimits {
                max_transactions: 1,
                ..ScanLimits::default()
            },
        )
        .await;
        let e = sc
            .scan_token(TOKEN.parse().unwrap(), 0, 10)
            .await
            .unwrap_err();
        assert!(matches!(e, EvmSourceError::TooManyTransactions { cap: 1 }));
    }

    #[tokio::test]
    async fn assemble_per_transaction_mode_uses_tx_receipts() {
        struct PerTx;
        impl Respond for PerTx {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let result = match body["method"].as_str().unwrap() {
                    "eth_getTransactionReceipt" => {
                        json!({"transactionHash":format!("{:#x}",h(0xa1)),
                        "blockNumber":"0x7","transactionIndex":"0x1","status":"0x0","gasUsed":"0x1",
                        "logs":[]})
                    }
                    "eth_getTransactionByHash" => {
                        json!({"hash":format!("{:#x}",h(0xa1)),"from":WALLET,
                        "to":WALLET,"value":"0x0","blockNumber":"0x7","transactionIndex":"0x1","gasPrice":"0x5"})
                    }
                    "eth_getBlockByNumber" => json!({"timestamp":"0x1"}),
                    other => panic!("unexpected {other}"),
                };
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
            }
        }
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(PerTx)
            .mount(&s)
            .await;
        let sc = scanner(
            &s,
            ScanLimits {
                receipt_mode: ReceiptMode::PerTransaction,
                ..ScanLimits::default()
            },
        )
        .await;
        let txs = sc.assemble(&[h(0xa1)], None).await.unwrap();
        // effectiveGasPrice absent -> falls back to tx gasPrice; failed status kept.
        assert_eq!(txs[0].effective_gas_price, U256::from(5u8));
        assert_eq!(txs[0].status, EvmTxStatus::Failed);
    }

    #[tokio::test]
    async fn wallet_scan_uses_blockscout_hashes_and_only_complete_internals() {
        let rpc_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&rpc_server)
            .await;
        let sc = scanner(&rpc_server, ScanLimits::default()).await;

        let bs = MockServer::start().await;
        struct Api(bool);
        impl Respond for Api {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let q: HashMap<String, String> = req.url.query_pairs().into_owned().collect();
                let body = match q["action"].as_str() {
                    "txlist" => json!({"status":"1","message":"OK","result":[
                        {"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1","from":WALLET,
                         "to":"","value":"9","gasUsed":"100","gasPrice":"2","isError":"0","txreceipt_status":"1"},
                        {"hash":format!("{:#x}",h(0xee)),"blockNumber":"8","timeStamp":"1",
                         "from":"0x00000000000000000000000000000000000000bb","to":WALLET,"value":"1",
                         "gasUsed":"1","gasPrice":"1","isError":"0"}]}),
                    "tokentx" => json!({"status":"1","message":"OK","result":[
                        {"hash":format!("{:#x}",h(0xee)),"blockNumber":"8","timeStamp":"1","from":OTHER,
                         "to":WALLET,"contractAddress":TOKEN,"value":"5"},
                        {"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1","from":WALLET,
                         "to":OTHER,"contractAddress":TOKEN,"value":"5"}]}),
                    "txlistinternal" if self.0 => json!({"status":"1","message":"OK","result":[
                        {"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1",
                         "from":"0x00000000000000000000000000000000000000bb","to":WALLET,"value":"77","isError":"0"}]}),
                    _ => json!({"status":"2","message":"not yet processed","result":[]}),
                };
                ResponseTemplate::new(200).set_body_json(body)
            }
        }
        for complete in [true, false] {
            bs.reset().await;
            Mock::given(method("GET"))
                .respond_with(Api(complete))
                .mount(&bs)
                .await;
            let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("k"));
            cfg.base_url = bs.uri();
            let src: WalletIndexer = BlockscoutEvmSource::new(cfg).unwrap().into();
            let out = sc
                .scan_wallet(&src, WALLET.parse().unwrap(), 0, 100)
                .await
                .unwrap();
            // The signed tx plus the token-only tx the wallet did not sign
            // (tokentx names it); the plain incoming txlist row (0xee as an
            // ETH transfer) is deduplicated against it.
            assert_eq!(out.transactions.len(), 2);
            assert_eq!(
                (out.listed_transactions, out.token_only_transactions),
                (2, 1)
            );
            assert!(out.txlist_complete && out.tokentx_complete);
            assert_eq!(out.internal_complete, complete);
            let a1 = out.transactions.iter().find(|t| t.hash == h(0xa1)).unwrap();
            assert_eq!(a1.from.to_string().to_lowercase(), WALLET);
            let it = &a1.internal_transfers;
            if complete {
                assert_eq!(it.as_ref().unwrap()[0].value, U256::from(77u8));
            } else {
                assert_eq!(*it, None);
            }
        }
    }

    /// Explorer fake: txlist 1 signed tx; tokentx pages as configured.
    struct Listings {
        tokentx_rows: Vec<Value>,
    }
    impl Respond for Listings {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let q: HashMap<String, String> = req.url.query_pairs().into_owned().collect();
            let body = match q["action"].as_str() {
                "txlist" => json!({"status":"1","message":"OK","result":[
                    {"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1","from":WALLET,
                     "to":"","value":"9","gasUsed":"100","gasPrice":"2","isError":"1","txreceipt_status":"0"}]}),
                "tokentx" => json!({"status":"1","message":"OK","result":self.tokentx_rows}),
                // Internals not indexed yet: incomplete, so sole-touch policy applies.
                "txlistinternal" => {
                    json!({"status":"2","message":"not yet processed","result":[]})
                }
                _ => json!({"status":"0","message":"No transactions found","result":[]}),
            };
            ResponseTemplate::new(200).set_body_json(body)
        }
    }

    fn token_row(hash: B256, block: u64) -> Value {
        json!({"hash":format!("{hash:#x}"),"blockNumber":block.to_string(),"timeStamp":"1",
            "from":OTHER,"to":WALLET,"contractAddress":TOKEN,"value":"5"})
    }

    #[tokio::test]
    async fn complete_internals_need_no_sole_touch_receipts_or_balances() {
        struct Api;
        impl Respond for Api {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let q: HashMap<String, String> = req.url.query_pairs().into_owned().collect();
                let sent = json!({"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1",
                    "from":WALLET,"to":OTHER,"contractAddress":TOKEN,"value":"5"});
                let body = match q["action"].as_str() {
                    "txlist" => json!({"status":"1","message":"OK","result":[
                        {"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1","from":WALLET,
                         "to":"","value":"9","gasUsed":"100","gasPrice":"2","isError":"0","txreceipt_status":"1"}]}),
                    "tokentx" => json!({"status":"1","message":"OK","result":[sent]}),
                    "txlistinternal" => json!({"status":"1","message":"OK","result":[
                        {"hash":format!("{:#x}",h(0xa1)),"blockNumber":"7","timeStamp":"1",
                         "from":OTHER,"to":WALLET,"value":"77","isError":"0"}]}),
                    _ => json!({"status":"2","message":"not yet processed","result":[]}),
                };
                ResponseTemplate::new(200).set_body_json(body)
            }
        }
        let rpc = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&rpc)
            .await;
        let sc = scanner(&rpc, ScanLimits::default()).await;
        let bs = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(Api)
            .mount(&bs)
            .await;
        let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("k"));
        cfg.base_url = bs.uri();
        let src: WalletIndexer = BlockscoutEvmSource::new(cfg).unwrap().into();
        let listing = sc
            .list_wallet(&src, WALLET.parse().unwrap(), 0, 100)
            .await
            .unwrap();
        assert!(listing.internal_complete);
        let plan = listing.plan(&ScanLimits::default());
        // One per-transaction receipt, no whole-block fetch, no balance reads.
        assert_eq!(
            (
                plan.tx_receipt_calls,
                plan.block_receipt_calls,
                plan.candidate_sells
            ),
            (1, 0, 0)
        );
        assert_eq!((plan.base_requests(), plan.max_requests()), (1, 1));
        assert_eq!(plan.by_method()["eth_getBalance(<=)"], 0);
        sc.assemble_listing(listing).await.unwrap();
        let ms = methods(&rpc).await;
        assert!(
            ms.iter()
                .all(|m| m != "eth_getBlockReceipts" && m != "eth_getBalance")
        );
    }

    #[tokio::test]
    async fn wallet_scan_never_calls_window_get_logs_and_skips_timestamp_calls() {
        let rpc = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&rpc)
            .await;
        let sc = scanner(&rpc, ScanLimits::default()).await;
        let bs = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(Listings {
                tokentx_rows: vec![token_row(h(0xee), 8)],
            })
            .mount(&bs)
            .await;
        let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("k"));
        cfg.base_url = bs.uri();
        let src: WalletIndexer = BlockscoutEvmSource::new(cfg).unwrap().into();
        // a 864,000-block window: the scan must not care
        let out = sc
            .scan_wallet(&src, WALLET.parse().unwrap(), 1_000, 865_000)
            .await
            .unwrap();
        assert_eq!(out.transactions.len(), 2);
        // the FAILED signed tx is kept (fee is real)
        let ms = methods(&rpc).await;
        assert!(!ms.iter().any(|m| m == "eth_getLogs"), "{ms:?}");
        // timestamps come from the listings, receipts are per transaction
        assert!(!ms.iter().any(|m| m == "eth_getBlockByNumber"), "{ms:?}");
        assert!(!ms.iter().any(|m| m == "eth_getBlockReceipts"), "{ms:?}");
        assert_eq!(
            ms.iter()
                .filter(|m| *m == "eth_getTransactionReceipt")
                .count(),
            2
        );
        assert!(out.transactions.iter().all(|t| t.block_time == 1));
        // listing requests carry the block range
        let q: Vec<String> = bs
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.query().unwrap_or("").to_string())
            .collect();
        assert!(
            q.iter()
                .all(|u| !u.contains("startblock") || u.contains("startblock=1000"))
        );
        assert!(
            q.iter()
                .any(|u| u.contains("action=tokentx") && u.contains("endblock=865000"))
        );
    }

    #[tokio::test]
    async fn possible_sell_blocks_are_fetched_whole_once_and_shared_with_the_resolver() {
        let rpc = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&rpc)
            .await;
        let sc = scanner(&rpc, ScanLimits::default()).await;
        let bs = MockServer::start().await;
        // W SENDS a token in its own tx 0xa1 (possible sell); 0xee is incoming only
        let mut sent = token_row(h(0xa1), 7);
        sent["from"] = json!(WALLET);
        sent["to"] = json!(OTHER);
        Mock::given(method("GET"))
            .respond_with(Listings {
                tokentx_rows: vec![sent, token_row(h(0xee), 8)],
            })
            .mount(&bs)
            .await;
        let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("k"));
        cfg.base_url = bs.uri();
        let src: WalletIndexer = BlockscoutEvmSource::new(cfg).unwrap().into();
        let listing = sc
            .list_wallet(&src, WALLET.parse().unwrap(), 0, 100)
            .await
            .unwrap();
        let plan = listing.plan(&ScanLimits::default());
        assert_eq!(
            (
                plan.transactions,
                plan.tx_receipt_calls,
                plan.block_receipt_calls,
                plan.candidate_sells
            ),
            (2, 1, 1, 1)
        );
        // 2 receipts + 2 balances at most
        assert_eq!((plan.base_requests(), plan.max_requests()), (2, 4));
        assert!(
            methods(&rpc).await.is_empty(),
            "planning makes no RPC request"
        );
        sc.assemble_listing(listing).await.unwrap();
        let ms = methods(&rpc).await;
        assert_eq!(
            ms.iter().filter(|m| *m == "eth_getBlockReceipts").count(),
            1
        );
        assert_eq!(
            ms.iter()
                .filter(|m| *m == "eth_getTransactionReceipt")
                .count(),
            1
        );
        // the cached block comes back for free (the native-leg check)
        let again = sc.rpc().block_receipts_shared(7).await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(
            methods(&rpc)
                .await
                .iter()
                .filter(|m| *m == "eth_getBlockReceipts")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn truncated_token_listing_is_reported_incomplete() {
        let rpc = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&rpc)
            .await;
        let sc = scanner(&rpc, ScanLimits::default()).await;
        let bs = MockServer::start().await;
        // every page is full (page_size 1) -> page cap reached
        Mock::given(method("GET"))
            .respond_with(Listings {
                tokentx_rows: vec![token_row(h(0xee), 8)],
            })
            .mount(&bs)
            .await;
        let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("k"));
        cfg.base_url = bs.uri();
        cfg.page_size = 1;
        cfg.max_pages = 2;
        let src: WalletIndexer = BlockscoutEvmSource::new(cfg).unwrap().into();
        let out = sc
            .scan_wallet(&src, WALLET.parse().unwrap(), 0, 100)
            .await
            .unwrap();
        assert!(!out.tokentx_complete && !out.txlist_complete);
        // duplicates across pages collapse to distinct transactions
        assert_eq!(out.listed_transactions, 2);
    }

    #[tokio::test]
    async fn auto_receipts_use_block_receipts_only_for_crowded_blocks() {
        struct Crowded;
        impl Respond for Crowded {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let hash_of = |i: u8| format!("{:#x}", h(i));
                let result = match body["method"].as_str().unwrap() {
                    "eth_getTransactionByHash" => {
                        let hs = body["params"][0].as_str().unwrap().to_string();
                        // 0x01, 0x02 in block 3 (idx 0, 1); 0x03 alone in block 9
                        let (b, i) = match hs.as_str() {
                            x if x == hash_of(1) => ("0x3", "0x0"),
                            x if x == hash_of(2) => ("0x3", "0x1"),
                            _ => ("0x9", "0x0"),
                        };
                        json!({"hash":hs,"from":WALLET,"to":null,"value":"0x0","blockNumber":b,"transactionIndex":i})
                    }
                    "eth_getBlockReceipts" => json!([
                        {"transactionHash":hash_of(1),"blockNumber":"0x3","transactionIndex":"0x0","status":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x1","logs":[]},
                        {"transactionHash":hash_of(2),"blockNumber":"0x3","transactionIndex":"0x1","status":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x1","logs":[]}]),
                    "eth_getTransactionReceipt" => json!({"transactionHash":hash_of(3),
                        "blockNumber":"0x9","transactionIndex":"0x0","status":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x1","logs":[]}),
                    "eth_getBlockByNumber" => json!({"timestamp":"0x5"}),
                    other => panic!("unexpected {other}"),
                };
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
            }
        }
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Crowded)
            .mount(&s)
            .await;
        let sc = scanner(
            &s,
            ScanLimits {
                block_receipts_min_txs: 2,
                ..ScanLimits::default()
            },
        )
        .await;
        let txs = sc.assemble(&[h(1), h(2), h(3)], None).await.unwrap();
        assert_eq!(txs.len(), 3);
        let ms = methods(&s).await;
        assert_eq!(
            ms.iter().filter(|m| *m == "eth_getBlockReceipts").count(),
            1
        );
        assert_eq!(
            ms.iter()
                .filter(|m| *m == "eth_getTransactionReceipt")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn token_scan_fetches_each_block_receipts_once() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Chain)
            .mount(&s)
            .await;
        let sc = scanner(&s, ScanLimits::default()).await;
        sc.scan_token(TOKEN.parse().unwrap(), 0, 10).await.unwrap();
        let ms = methods(&s).await;
        // blocks 5 and 7, once each (no second fetch inside the assembly)
        assert_eq!(
            ms.iter().filter(|m| *m == "eth_getBlockReceipts").count(),
            2
        );
    }

    #[tokio::test]
    async fn resolve_window_maps_times_to_blocks() {
        struct Blocks;
        impl Respond for Blocks {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let result = if body["method"] == "eth_blockNumber" {
                    json!("0x64") // latest = 100
                } else {
                    let n = u64::from_str_radix(
                        body["params"][0].as_str().unwrap().trim_start_matches("0x"),
                        16,
                    )
                    .unwrap();
                    json!({"timestamp": format!("{:#x}", n * 2)})
                };
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
            }
        }
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Blocks)
            .mount(&s)
            .await;
        let sc = scanner(&s, ScanLimits::default()).await;
        // blocks with ts in [20, 40): 10..=19
        assert_eq!(
            sc.resolve_window(Some(20), Some(40)).await.unwrap(),
            Some((10, 19))
        );
        assert_eq!(
            sc.resolve_window(Some(20), None).await.unwrap(),
            Some((10, 100))
        );
        assert_eq!(
            sc.resolve_window(None, Some(1)).await.unwrap(),
            Some((0, 0))
        );
        assert_eq!(sc.resolve_window(Some(10_000), None).await.unwrap(), None);
    }
}
