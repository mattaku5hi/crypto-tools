//! Offline tests for the Solana wallet-stats engine: committed live
//! fixtures (decoded by the real `HeliusProvider` via wiremock) served by
//! a scripted stub provider keyed by wallet.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures::StreamExt as _;
use futures::stream::{self, BoxStream};
use scout_api::{
    HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest, ScanTask,
    SourceCapabilities,
};
use scout_core::{AddressBytes, RawPayload, RawSolanaTransaction, SolanaPubkey, WalletKey};
use scout_dex_solana::{TradeEventPairing, pair_trades_with_events};
use scout_engine::{
    WalletScanStatus, build_solana_wallet_ledger, pump_bonding_curve_decoder,
    run_solana_wallet_stats, solana_mainnet_chain,
};
use scout_providers::{HeliusProvider, ScanOrder};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fixture_txs(name: &str) -> Vec<RawSolanaTransaction> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data: Vec<serde_json::Value> = if let Some(pages) = fixture["pages"].as_array() {
        pages
            .iter()
            .flat_map(|p| p["data"].as_array().unwrap().clone())
            .collect()
    } else {
        fixture["result"]["data"].as_array().unwrap().clone()
    };
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "data": data, "paginationToken": null }
    });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let provider =
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
            .with_scan_order(ScanOrder::NewestFirst);
    let wallet = WalletKey {
        chain: solana_mainnet_chain(),
        address: AddressBytes::Solana([3; 32]),
    };
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::WalletActivity { wallet },
            description: "test".to_string(),
        },
        CancellationToken::new(),
    );
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        if let RawPayload::SolanaTransaction(tx) = item.unwrap().payload {
            out.push(tx);
        }
    }
    assert_eq!(out.len(), data.len());
    out
}

/// Distinct paired-trade users of the txs, in first-seen order.
fn traders(txs: &[RawSolanaTransaction]) -> Vec<SolanaPubkey> {
    let decoder = pump_bonding_curve_decoder().unwrap();
    let mut out = Vec::new();
    for tx in txs {
        let rep =
            pair_trades_with_events(&decoder, &tx.instructions, tx.slot, tx.transaction_index);
        for p in &rep.trades {
            if matches!(p.pairing, TradeEventPairing::Paired(_)) && !out.contains(&p.trade.user) {
                out.push(p.trade.user);
            }
        }
    }
    out
}

#[derive(Clone)]
enum Script {
    Txs {
        txs: Vec<RawSolanaTransaction>,
        truncated: bool,
    },
    Fail,
    Config,
    NonTx(scout_core::RawSolanaInstruction),
}

struct Stub {
    by_wallet: BTreeMap<SolanaPubkey, Script>,
    seen: std::sync::Mutex<Vec<SolanaPubkey>>,
}

impl Stub {
    fn new(by_wallet: BTreeMap<SolanaPubkey, Script>) -> Self {
        Self {
            by_wallet,
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl HistoryProvider for Stub {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::empty()
    }
    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        Ok(ScanPlan {
            request_echo: format!("{request:?}"),
            capabilities: SourceCapabilities::empty(),
        })
    }
    fn scan(
        &self,
        task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        let ScanRequest::WalletActivity { wallet } = &task.request else {
            panic!("expected wallet request");
        };
        let AddressBytes::Solana(addr) = wallet.address else {
            panic!("expected solana");
        };
        self.seen.lock().unwrap().push(addr);
        let items: Vec<Result<ScanEnvelope, ProviderError>> = match self.by_wallet.get(&addr) {
            None => vec![],
            Some(Script::Txs { txs, truncated }) => {
                let n = txs.len();
                txs.iter()
                    .enumerate()
                    .map(|(i, tx)| {
                        Ok(ScanEnvelope {
                            payload: RawPayload::SolanaTransaction(tx.clone()),
                            truncated: *truncated && i + 1 == n,
                        })
                    })
                    .collect()
            }
            Some(Script::Fail) => vec![Err(ProviderError::Transport(Box::new(
                std::io::Error::other("boom https://h/?api-key=SECRET99"),
            )))],
            Some(Script::Config) => vec![Err(ProviderError::ConfigurationRequired {
                port: "solana_history".to_string(),
                detail: "set SCOUT_HELIUS_API_KEY".to_string(),
            })],
            Some(Script::NonTx(ix)) => vec![Ok(ScanEnvelope {
                payload: RawPayload::SolanaInstruction(ix.clone()),
                truncated: false,
            })],
        };
        Box::pin(stream::iter(items))
    }
}

async fn all_fixture_txs() -> Vec<RawSolanaTransaction> {
    let mut txs = fixture_txs("pump_variants_live_2026-10-02.json").await;
    txs.extend(fixture_txs("pump_bonding_curve_buy_probe.json").await);
    txs
}

const EMPTY_WALLET: SolanaPubkey = [77; 32];

#[tokio::test]
async fn order_dedup_status_and_ledger_equivalence() {
    let txs = all_fixture_txs().await;
    let users = traders(&txs);
    assert!(users.len() >= 2);
    let (a, b) = (users[0], users[1]);
    let script = Script::Txs {
        txs: txs.clone(),
        truncated: false,
    };
    let stub = Stub::new(BTreeMap::from([(a, script.clone()), (b, script)]));
    let decoder = pump_bonding_curve_decoder().unwrap();
    // Input: b, empty, a, b (dup), empty (dup).
    let input = [b, EMPTY_WALLET, a, b, EMPTY_WALLET];
    let report = run_solana_wallet_stats(&stub, &input, &decoder, CancellationToken::new())
        .await
        .unwrap();
    let order: Vec<_> = report.wallets.iter().map(|w| w.wallet).collect();
    assert_eq!(order, vec![b, EMPTY_WALLET, a]);
    assert_eq!(
        *stub.seen.lock().unwrap(),
        vec![b, EMPTY_WALLET, a],
        "each distinct wallet scanned once"
    );
    assert_eq!(report.wallets[1].status, WalletScanStatus::NoActivity);
    assert_eq!(report.wallets[1].transactions_scanned, Some(0));
    for (i, w) in [(0usize, b), (2, a)] {
        let card = &report.wallets[i];
        let direct = build_solana_wallet_ledger(&w, &txs, &decoder).unwrap();
        assert_eq!(card.ledger.as_ref().unwrap(), &direct);
        assert!(direct.trades.buys + direct.trades.sells >= 1);
        assert_eq!(
            card.transactions_scanned,
            Some(u64::try_from(txs.len()).unwrap())
        );
        assert!(
            matches!(
                card.status,
                WalletScanStatus::Ok | WalletScanStatus::Incomplete
            ),
            "{:?}",
            card.status
        );
        if card.status == WalletScanStatus::Incomplete {
            assert!(!card.incomplete_reasons.is_empty());
        }
    }
}

#[tokio::test]
async fn truncated_scan_is_incomplete_and_marks_report() {
    let txs = all_fixture_txs().await;
    let user = traders(&txs)[0];
    let stub = Stub::new(BTreeMap::from([(
        user,
        Script::Txs {
            txs,
            truncated: true,
        },
    )]));
    let decoder = pump_bonding_curve_decoder().unwrap();
    let report = run_solana_wallet_stats(&stub, &[user], &decoder, CancellationToken::new())
        .await
        .unwrap();
    let card = &report.wallets[0];
    assert_eq!(card.status, WalletScanStatus::Incomplete);
    assert!(card.truncated);
    assert!(card.ledger.is_some());
    assert!(
        card.incomplete_reasons
            .iter()
            .any(|r| r.contains("older transactions not seen"))
    );
    assert!(report.is_coverage_incomplete());
    assert!(!report.all_failed());
}

#[tokio::test]
async fn provider_error_for_one_wallet_is_an_error_card_not_an_abort() {
    let txs = all_fixture_txs().await;
    let users = traders(&txs);
    let (good, bad) = (users[0], [55u8; 32]);
    let stub = Stub::new(BTreeMap::from([
        (
            good,
            Script::Txs {
                txs,
                truncated: false,
            },
        ),
        (bad, Script::Fail),
    ]));
    let decoder = pump_bonding_curve_decoder().unwrap();
    let report = run_solana_wallet_stats(&stub, &[bad, good], &decoder, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.wallets.len(), 2);
    let card = &report.wallets[0];
    assert_eq!(card.status, WalletScanStatus::Error);
    assert!(card.ledger.is_none());
    assert!(card.transactions_scanned.is_none());
    let err = card.error.as_deref().unwrap();
    assert!(!err.contains("SECRET99"), "{err}");
    assert_ne!(report.wallets[1].status, WalletScanStatus::Error);
    assert!(report.is_coverage_incomplete());
    assert!(!report.all_failed());

    let only_bad = run_solana_wallet_stats(&stub, &[bad], &decoder, CancellationToken::new())
        .await
        .unwrap();
    assert!(only_bad.all_failed());
}

#[tokio::test]
async fn configuration_required_aborts_the_run() {
    let user = [9u8; 32];
    let stub = Stub::new(BTreeMap::from([(user, Script::Config)]));
    let decoder = pump_bonding_curve_decoder().unwrap();
    let err = run_solana_wallet_stats(&stub, &[user], &decoder, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ConfigurationRequired { .. }));
}

#[tokio::test]
async fn unrelated_transactions_are_no_pump_activity_and_non_tx_payload_is_a_gap() {
    let txs = all_fixture_txs().await;
    let stranger = [5u8; 32];
    let weird = [6u8; 32];
    let ix = txs
        .iter()
        .find_map(|t| t.instructions.first().cloned())
        .unwrap();
    let stub = Stub::new(BTreeMap::from([
        (
            stranger,
            Script::Txs {
                txs,
                truncated: false,
            },
        ),
        (weird, Script::NonTx(ix)),
    ]));
    let decoder = pump_bonding_curve_decoder().unwrap();
    let report = run_solana_wallet_stats(
        &stub,
        &[stranger, weird],
        &decoder,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.wallets[0].status, WalletScanStatus::NoPumpActivity);
    assert!(report.wallets[0].transactions_scanned.unwrap() > 0);
    assert_eq!(report.wallets[1].status, WalletScanStatus::Incomplete);
    assert_eq!(report.wallets[1].unexpected_payloads, 1);
}

#[tokio::test]
async fn cancelled_run_keeps_every_wallet_as_a_card() {
    let decoder = pump_bonding_curve_decoder().unwrap();
    let stub = Stub::new(BTreeMap::new());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let report = run_solana_wallet_stats(&stub, &[[1; 32], [2; 32]], &decoder, cancel)
        .await
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.wallets.len(), 2);
    assert!(
        report
            .wallets
            .iter()
            .all(|w| w.status == WalletScanStatus::Error)
    );
}

#[tokio::test]
async fn real_helius_provider_wallet_scan_matches_direct_ledger() {
    // The same fixture served through HeliusProvider (WalletActivity,
    // newest-first) end to end.
    let fixture = "pump_variants_live_2026-10-02.json";
    let txs = fixture_txs(fixture).await;
    let user = traders(&txs)[0];
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(fixture);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data: Vec<serde_json::Value> = if let Some(pages) = json["pages"].as_array() {
        pages
            .iter()
            .flat_map(|p| p["data"].as_array().unwrap().clone())
            .collect()
    } else {
        json["result"]["data"].as_array().unwrap().clone()
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "data": data, "paginationToken": null }
        })))
        .mount(&server)
        .await;
    let provider =
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
            .with_scan_order(ScanOrder::NewestFirst);
    let decoder = pump_bonding_curve_decoder().unwrap();
    let report = run_solana_wallet_stats(&provider, &[user], &decoder, CancellationToken::new())
        .await
        .unwrap();
    let card = &report.wallets[0];
    let direct = build_solana_wallet_ledger(&user, &txs, &decoder).unwrap();
    assert_eq!(card.ledger.as_ref().unwrap(), &direct);
}
