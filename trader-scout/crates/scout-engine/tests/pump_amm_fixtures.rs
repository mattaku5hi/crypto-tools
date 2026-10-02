//! PumpSwap AMM decoder over the committed live fixtures, loaded through
//! the real `HeliusProvider` decode path (wiremock), asserting the same
//! pairing and reconciliation totals as `scout-dex-solana`'s raw-JSON test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ScanRequest, ScanTask};
use scout_core::{
    AddressBytes, AssetKey, RawPayload, RawSolanaTransaction, SolanaExecutionStatus, SolanaPubkey,
};
use scout_dex_solana::{
    AmmAttribution, AmmTradeEventPairing, PumpAmmDecoder, PumpAmmTradeVariant, WRAPPED_SOL_MINT,
    reconcile_pump_amm_transaction,
};
use scout_engine::solana_mainnet_chain;
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fixture_txs(name: &str) -> Vec<RawSolanaTransaction> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data: Vec<serde_json::Value> = fixture["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["data"].as_array().unwrap().clone())
        .collect();
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
            .unwrap();
    let mint: SolanaPubkey = [10; 32];
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::TokenMarketActivity {
                asset: AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(mint)),
            },
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

fn sig_prefix(tx: &RawSolanaTransaction) -> String {
    bs58::encode(tx.signature).into_string()[..8].to_string()
}

#[tokio::test]
async fn provider_path_pairing_and_reconciliation_totals() {
    let d = PumpAmmDecoder::mainnet();
    let mut all = fixture_txs("pumpswap_variants_live_2026-10-02.json").await;
    assert_eq!(all.len(), 28);
    let wallet = fixture_txs("pumpswap_wallet_page_2026-10-02.json").await;
    assert_eq!(wallet.len(), 100);
    all.extend(wallet);

    let (mut trades, mut paired, mut missing, mut other_bad) = (0, 0, 0, 0);
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_variant: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failed = 0;
    let mut quote_mints = std::collections::BTreeSet::new();
    for tx in &all {
        let r = reconcile_pump_amm_transaction(&d, tx);
        if matches!(tx.execution, SolanaExecutionStatus::Failed { .. }) {
            failed += 1;
            assert!(r.users.is_empty());
        }
        trades += r.pairing.trades.len();
        paired += r.pairing.paired();
        missing += r.pairing.missing();
        other_bad += r.pairing.mismatched()
            + r.pairing.orphan_events.len()
            + r.pairing.malformed_trades
            + r.pairing.malformed_events
            + r.pairing.unknown_events
            + r.pairing.unknown_instructions;
        for p in &r.pairing.trades {
            *per_variant.entry(p.trade.variant.name()).or_default() += 1;
            quote_mints.insert(p.trade.quote_mint);
            if matches!(p.pairing, AmmTradeEventPairing::Mismatch { .. }) {
                panic!("mismatch in {}", sig_prefix(tx));
            }
        }
        for u in &r.users {
            *groups.entry(format!("{:?}", u.attribution)).or_default() += 1;
            // Provider-built token/native deltas reconcile base legs exactly
            // for every attributed user.
            if u.attribution != AmmAttribution::NoUserDelta {
                assert!(u.user_is_signer, "{}", sig_prefix(tx));
                for l in u.legs.iter().filter(|l| l.is_base) {
                    assert_eq!(l.residual(), 0, "{}", sig_prefix(tx));
                }
            }
        }
    }
    assert_eq!(
        (trades, paired, missing, other_bad, failed),
        (128, 127, 1, 0, 1)
    );
    assert_eq!(per_variant.get("buy"), Some(&64));
    assert_eq!(per_variant.get("buy_exact_quote_in"), Some(&9));
    assert_eq!(per_variant.get("sell"), Some(&55));
    assert_eq!(groups.get("Exact"), Some(&116));
    assert_eq!(groups.get("QuoteResidual"), Some(&4));
    assert_eq!(groups.get("QuoteFundedElsewhere"), Some(&3));
    assert_eq!(groups.get("NoUserDelta"), Some(&3));
    assert_eq!(groups.values().sum::<usize>(), 126);
    assert_eq!(quote_mints.len(), 7);
    assert!(quote_mints.contains(&WRAPPED_SOL_MINT));
    let _ = PumpAmmTradeVariant::Buy;
}
