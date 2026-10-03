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
use scout_dex_solana::TradeSide;
use scout_dex_solana::{
    AmmAttribution, AmmTradeEventPairing, PumpAmmDecoder, PumpAmmTradeVariant, TrackVolumeEncoding,
    WRAPPED_SOL_MINT, reconcile_pump_amm_transaction,
};
use scout_engine::{Venue, run_solana_buyer_intersect, solana_mainnet_chain};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, method};
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

const LIVE_26: &str = "pumpswap_buy_26byte_live_2026-10-03.json";
const LIVE_26_MINTS: [&str; 3] = [
    "FFrRBPP9yWtSqnKjXXM9C77MBdLztfdgSnRE858zpump",
    "458eEcXufhsLoS1NWa6donw9Yh9USfWMCtBmyFTypump",
    "9pJWJdpPebyANys45eetpemLJo8yTz4n5B9zbpYw9ZMr",
];

/// ADR-009 amendment 2026-10-03: nine live txs, every one with a 26-byte
/// (`0x01 0x01`) buy / buy_exact_quote_in. Per sig: encoding, pairing,
/// attribution and base-leg residual through the provider decode path.
#[tokio::test]
async fn live_26_byte_track_volume_buys_pair_and_reconcile() {
    let d = PumpAmmDecoder::mainnet();
    let txs = fixture_txs(LIVE_26).await;
    assert_eq!(txs.len(), 9);
    // (sig prefix, succeeded, variant, quote residual) in fixture order.
    let expected: [(&str, bool, &str, i128); 9] = [
        ("4A5TdxV6", true, "buy", -1_713_840),
        ("4mcVs7X1", false, "buy", 0),
        ("4gcSwEbV", true, "buy_exact_quote_in", -700_000),
        ("2i6zrXCL", true, "buy_exact_quote_in", -800_000),
        ("2McC4KPu", true, "buy", -6_960_040),
        ("47UYcoGM", false, "buy", 0),
        ("4yQRCGeA", true, "buy", -1_713_840),
        ("3keov4Gp", true, "buy", -1_713_840),
        ("3mshLbL7", true, "buy", -3_513_840),
    ];
    for (tx, (prefix, ok, variant, quote_residual)) in txs.iter().zip(expected) {
        assert_eq!(sig_prefix(tx), prefix);
        let r = reconcile_pump_amm_transaction(&d, tx);
        assert_eq!(r.succeeded, ok, "{prefix}");
        let p = &r.pairing;
        assert_eq!(p.trades.len(), 1, "{prefix}");
        assert_eq!(
            (p.malformed_trades, p.orphan_events.len(), p.mismatched()),
            (0, 0, 0),
            "{prefix}"
        );
        let t = &p.trades[0].trade;
        assert_eq!(t.variant.name(), variant, "{prefix}");
        assert_eq!(t.track_volume_encoding, TrackVolumeEncoding::TwoByteOption);
        assert_eq!(t.trailing_arg_bytes, [1, 1]);
        assert_eq!(t.track_volume, Some(1));
        if ok {
            assert_eq!((p.paired(), p.missing()), (1, 0), "{prefix}");
            assert_eq!(r.users.len(), 1, "{prefix}");
            let u = &r.users[0];
            assert!(u.user_is_signer, "{prefix}");
            // Base leg exact, quote leg off by rent/platform fees only.
            assert_eq!(u.attribution, AmmAttribution::QuoteResidual, "{prefix}");
            let base: Vec<_> = u.legs.iter().filter(|l| l.is_base).collect();
            assert_eq!(base.len(), 1);
            assert_eq!(base[0].residual(), 0, "{prefix}");
            assert!(base[0].actual > 0);
            let quote: Vec<_> = u.legs.iter().filter(|l| !l.is_base).collect();
            assert_eq!(quote[0].residual(), quote_residual, "{prefix}");
        } else {
            // Failed tx: instruction decodes, no events, nothing attributed.
            assert!(matches!(tx.execution, SolanaExecutionStatus::Failed { .. }));
            assert_eq!((p.paired(), p.missing()), (0, 1), "{prefix}");
            assert!(r.users.is_empty(), "{prefix}");
        }
    }
}

/// The same fixture as a `buyer-intersect` token scan: each mint is served
/// only the txs that mention it; no malformed trade, no orphan event, and
/// every successful 26-byte buy qualifies as a PumpSwap buy.
#[tokio::test]
async fn live_26_byte_buys_qualify_in_buyer_intersect_without_gaps() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(LIVE_26);
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data: Vec<serde_json::Value> = fixture["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["data"].as_array().unwrap().clone())
        .collect();
    let server = MockServer::start().await;
    let mut served = 0;
    for mint in LIVE_26_MINTS {
        let subset: Vec<_> = data
            .iter()
            .filter(|t| t.to_string().contains(mint))
            .cloned()
            .collect();
        served += subset.len();
        Mock::given(method("POST"))
            .and(body_string_contains(mint))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": { "data": subset, "paginationToken": null }
            })))
            .mount(&server)
            .await;
    }
    assert_eq!(served, 9);
    let provider =
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap();
    let tokens: Vec<AssetKey> = LIVE_26_MINTS
        .iter()
        .map(|m| {
            let mint: SolanaPubkey = bs58::decode(m).into_vec().unwrap().try_into().unwrap();
            AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(mint))
        })
        .collect();
    let report = run_solana_buyer_intersect(&provider, &tokens, 1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.trade.malformed_trades, 0);
    assert_eq!(report.trade.orphan_events, 0);
    assert_eq!(report.diagnostics.malformed_instructions, 0);
    assert_eq!(report.trade.ops(Venue::PumpAmm, TradeSide::Buy), 7);
    // 7 successful buys by 4 distinct signers.
    assert_eq!(report.base.matches.len(), 4);
    assert!(
        !report.is_coverage_incomplete(),
        "{:?}",
        report.incomplete_reasons()
    );
}
