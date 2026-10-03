//! History assembly for EVM chains (ADR-020 §3): from tx hashes (token-centric
//! via `eth_getLogs`, wallet-centric via Blockscout) to `RawEvmTransaction`s
//! with receipts, fees, block times and — only when a complete source
//! provided them — internal transfers.
//!
//! Everything is bounded (tx cap, log cap, concurrency, request budget) and
//! the output is in canonical `(block, transaction_index)` order regardless
//! of async completion order (invariant #12).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use alloy_primitives::{Address, B256, U256};
use scout_core::{ChainKey, InternalTransfer, RawEvmTransaction};
use scout_evm::TRANSFER_TOPIC0;

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
}

#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    /// Max distinct transactions assembled per call.
    pub max_transactions: usize,
    pub receipt_mode: ReceiptMode,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_transactions: 20_000,
            receipt_mode: ReceiptMode::BlockReceipts,
        }
    }
}

/// Native internal transfers keyed by transaction, with their provenance.
#[derive(Debug, Clone, Default)]
pub struct InternalIndex {
    by_tx: HashMap<B256, Vec<InternalTransfer>>,
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
        Some(Self { by_tx })
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
    /// `false`: internal transfers were not (completely) available; every
    /// transaction then carries `internal_transfers = None`.
    pub internal_complete: bool,
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
        let mut hashes: Vec<(u64, u64, B256)> = Vec::new();
        let mut seen = BTreeSet::new();
        for l in &logs.logs {
            // RawEvmLog does not carry the tx hash; resolve (block, index)
            // to hashes through block receipts below.
            seen.insert((l.block_number, l.transaction_index));
        }
        let blocks: Vec<u64> = seen
            .iter()
            .map(|(b, _)| *b)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // Map (block, tx_index) -> hash via the block's receipts.
        let receipts = self.rpc.receipts_by_blocks(&blocks).await?;
        let mut receipt_by_pos: HashMap<(u64, u64), EvmReceiptInfo> = HashMap::new();
        for (_, rs) in receipts {
            for r in rs {
                receipt_by_pos.insert((r.block_number, r.transaction_index), r);
            }
        }
        // Keep only the receipts of transactions that have a token log
        // (whole-block receipts are dropped here, bounding memory).
        let mut needed: Vec<EvmReceiptInfo> = Vec::with_capacity(seen.len());
        for key in &seen {
            let r = receipt_by_pos.remove(key).ok_or_else(|| {
                malformed(
                    "eth_getBlockReceipts",
                    format!("no receipt at block {} index {}", key.0, key.1),
                )
            })?;
            hashes.push((key.0, key.1, r.tx_hash));
            needed.push(r);
        }
        if hashes.len() > self.limits.max_transactions {
            return Err(EvmSourceError::TooManyTransactions {
                cap: self.limits.max_transactions,
            });
        }
        let hash_list: Vec<B256> = hashes.iter().map(|(_, _, h)| *h).collect();
        let transactions = self
            .build(
                &hash_list,
                None,
                Some(receipt_by_pos.into_values().collect()),
            )
            .await?;
        Ok(TokenScanOutput {
            transactions,
            transfer_logs: logs.logs.len(),
            log_requests: logs.requests,
            log_splits: logs.splits,
        })
    }

    /// Wallet-centric scan via Blockscout: the wallet's own (`from ==
    /// wallet`) transactions in `[from, to]`, plus its internal transfers.
    pub async fn scan_wallet(
        &self,
        source: &BlockscoutEvmSource,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<WalletScanOutput, EvmSourceError> {
        let txlist = source.txlist(wallet, from_block, to_block).await?;
        let internal = source
            .internal_transfers(wallet, from_block, to_block)
            .await?;
        let index = InternalIndex::from_complete_listing(&internal);
        let mut hashes: Vec<B256> = txlist
            .rows
            .iter()
            .filter(|t| t.from == wallet)
            .map(|t| t.hash)
            .collect();
        hashes.sort();
        hashes.dedup();
        if hashes.len() > self.limits.max_transactions {
            return Err(EvmSourceError::TooManyTransactions {
                cap: self.limits.max_transactions,
            });
        }
        let transactions = self.build(&hashes, index.as_ref(), None).await?;
        Ok(WalletScanOutput {
            transactions,
            txlist_complete: txlist.complete,
            internal_complete: index.is_some(),
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
        self.build(hashes, internals, None).await
    }

    async fn build(
        &self,
        hashes: &[B256],
        internals: Option<&InternalIndex>,
        prefetched_receipts: Option<Vec<EvmReceiptInfo>>,
    ) -> Result<Vec<RawEvmTransaction>, EvmSourceError> {
        let txs = self.rpc.transactions_by_hashes(hashes).await?;
        let mut receipts: HashMap<B256, EvmReceiptInfo> = prefetched_receipts
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.tx_hash, r))
            .collect();
        let missing: Vec<&crate::evm_wire::EvmTxInfo> = txs
            .iter()
            .filter(|t| !receipts.contains_key(&t.hash))
            .collect();
        match self.limits.receipt_mode {
            ReceiptMode::PerTransaction => {
                let hs: Vec<B256> = missing.iter().map(|t| t.hash).collect();
                for r in self.rpc.receipts_by_hashes(&hs).await? {
                    receipts.insert(r.tx_hash, r);
                }
            }
            ReceiptMode::BlockReceipts => {
                let blocks: Vec<u64> = missing
                    .iter()
                    .map(|t| t.block_number)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                for (_, rs) in self.rpc.receipts_by_blocks(&blocks).await? {
                    for r in rs {
                        receipts.insert(r.tx_hash, r);
                    }
                }
            }
        }
        let blocks: Vec<u64> = txs
            .iter()
            .map(|t| t.block_number)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let times: BTreeMap<u64, u64> = self.rpc.block_timestamps(&blocks).await?;

        let mut out = Vec::with_capacity(txs.len());
        for t in txs {
            let r = receipts
                .remove(&t.hash)
                .ok_or_else(|| EvmSourceError::NotFound {
                    what: format!("receipt {:#x}", t.hash),
                })?;
            if r.block_number != t.block_number || r.transaction_index != t.transaction_index {
                return Err(malformed(
                    "receipt",
                    format!("receipt position differs from transaction {:#x}", t.hash),
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
                hash: t.hash,
                from: t.from,
                to: t.to,
                block_number: t.block_number,
                transaction_index: t.transaction_index,
                block_time,
                value: t.value,
                status: r.status,
                gas_used: r.gas_used,
                effective_gas_price,
                l1_fee: r.l1_fee,
                logs: r.logs,
                internal_transfers: internals.map(|i| i.for_tx(&t.hash)),
            });
        }
        out.sort_by_key(|t| (t.block_number, t.transaction_index));
        Ok(out)
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
                    let (block, idx) = if hs == format!("{:#x}", h(0xa1)) {
                        ("0x7", "0x1")
                    } else {
                        ("0x5", "0x0")
                    };
                    json!({"hash":hs,"from":WALLET,"to":null,"value":"0x9","blockNumber":block,"transactionIndex":idx})
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
            let src = BlockscoutEvmSource::new(cfg).unwrap();
            let out = sc
                .scan_wallet(&src, WALLET.parse().unwrap(), 0, 100)
                .await
                .unwrap();
            // Only the wallet-signed tx is assembled.
            assert_eq!(out.transactions.len(), 1);
            assert!(out.txlist_complete);
            assert_eq!(out.internal_complete, complete);
            let it = &out.transactions[0].internal_transfers;
            if complete {
                assert_eq!(it.as_ref().unwrap()[0].value, U256::from(77u8));
            } else {
                assert_eq!(*it, None);
            }
        }
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
