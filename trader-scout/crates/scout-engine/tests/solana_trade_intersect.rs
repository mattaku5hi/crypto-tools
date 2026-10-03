//! ADR-014 offline tests: trade-side matching (`--side buy|sell|any`) over
//! the committed real fixtures (served per mint through the real
//! `HeliusProvider` via wiremock) plus scripted synthetic transactions for
//! the cases no fixture holds (K semantics, window boundary, transfers,
//! IdlOnly).
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
use scout_core::{
    AddressBytes, AssetKey, RawPayload, RawSolanaInstruction, RawSolanaTransaction,
    SolanaExecutionStatus, SolanaPubkey, SolanaTokenBalanceChange, WalletKey,
};
use scout_dex_solana::{
    AmmAttribution, BUY_INSTRUCTION_DISCRIMINATOR, EVENT_CPI_DISCRIMINATOR,
    JUPITER_EVENT_AUTHORITY_BYTES, JUPITER_SWAPS_EVENT_DISCRIMINATOR, JUPITER_V6_PROGRAM_ID_BYTES,
    PumpTradeVariant, SELL_INSTRUCTION_DISCRIMINATOR, VariantVerification, WRAPPED_SOL_MINT,
    reconcile_pump_amm_transaction,
};
use scout_engine::{
    AnalysisWindow, IntersectOptions, PUMP_BONDING_CURVE_PROGRAM_ID, SideFilter,
    SolanaBuyerIntersectReport, TradeSide, USDC_MINT, Venue, WindowSource, pump_amm_decoder,
    run_solana_trade_intersect, run_solana_trade_intersect_with_policy, solana_mainnet_chain,
};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- real fixtures ------------------------------------------------------

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

fn asset_b58(s: &str) -> AssetKey {
    AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pubkey(s)))
}

/// All `data` rows of a committed capture (wallet-page style `pages[]` or a
/// single `result.data` page), as a complete, non-truncated response.
fn fixture_response(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data: Vec<serde_json::Value> = if fixture.get("pages").is_some() {
        fixture["pages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["data"].as_array().unwrap().clone())
            .collect()
    } else {
        fixture["result"]["data"].as_array().unwrap().clone()
    };
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "data": data, "paginationToken": null }
    })
}

/// Serve `fixture` as the history of every listed mint (matched on the mint
/// in the request body).
async fn serve(mints: &[&str], fixture: &str) -> (MockServer, HeliusProvider) {
    let server = MockServer::start().await;
    for mint in mints {
        Mock::given(method("POST"))
            .and(body_string_contains(*mint))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture_response(fixture)))
            .mount(&server)
            .await;
    }
    let provider =
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap();
    (server, provider)
}

async fn fixture_txs(fixture: &str) -> Vec<RawSolanaTransaction> {
    // A catch-all server: any requested address gets the whole capture.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture_response(fixture)))
        .mount(&server)
        .await;
    let provider =
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap();
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::TokenMarketActivity {
                asset: AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana([10; 32])),
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
    out
}

fn opts(side: SideFilter) -> IntersectOptions {
    IntersectOptions {
        side,
        window: AnalysisWindow::none(0),
    }
}

async fn run_fixture(
    mints: &[&str],
    fixture: &str,
    k: usize,
    side: SideFilter,
) -> SolanaBuyerIntersectReport {
    let (_server, provider) = serve(mints, fixture).await;
    let tokens: Vec<AssetKey> = mints.iter().map(|m| asset_b58(m)).collect();
    run_solana_trade_intersect(&provider, &tokens, k, opts(side), CancellationToken::new())
        .await
        .unwrap()
}

fn wallet_of(key: SolanaPubkey) -> WalletKey {
    WalletKey {
        chain: solana_mainnet_chain(),
        address: AddressBytes::Solana(key),
    }
}

fn wallet_b58(report: &SolanaBuyerIntersectReport, prefix: &str) -> WalletKey {
    report
        .side_hits
        .keys()
        .find(|w| w.address.to_string().starts_with(prefix))
        .unwrap_or_else(|| panic!("no wallet {prefix}* in hits"))
        .clone()
}

const PUMPSWAP_VARIANTS: &str = "pumpswap_variants_live_2026-10-02.json";
const PUMPSWAP_PAGE: &str = "pumpswap_wallet_page_2026-10-02.json";
const ROUTER_9OC3: &str = "router_wallet_9oC3_page_2026-10-02.json";
const ROUTER_TAWV: &str = "router_wallet_tAwv_page_2026-10-02.json";

// Mints with a known hand-counted shape in the captures (see the report of
// this change: per-mint qualifying operations).
const SELL_ONLY_AMM: &str = "8QwL8QeCUcJAmzD3xFuhdCTkzKyAjys2Pmt6rXAZkTGH";
const HEJ91: &str = "Hej91Bhp9jdNHLoWbWuR6ob5Xj7wgMBwvepi7yUtpump";
const NINE_CBD: &str = "9cbDbgUNQbBjWDoVSL4SdV9LFBjNQ6ZriEfYUGUqpump";

#[tokio::test]
async fn seller_only_wallet_hits_under_sell_and_any_but_not_buy() {
    // pumpswap_variants: exactly one qualifying operation on this mint, a
    // PumpSwap sell.
    let buy = run_fixture(&[SELL_ONLY_AMM], PUMPSWAP_VARIANTS, 1, SideFilter::Buy).await;
    assert!(buy.base.matches.is_empty());
    assert_eq!(buy.per_token[0].qualified_wallets, 0);
    assert!(
        !buy.is_coverage_incomplete(),
        "{:?}",
        buy.incomplete_reasons()
    );

    for side in [SideFilter::Sell, SideFilter::Any] {
        let r = run_fixture(&[SELL_ONLY_AMM], PUMPSWAP_VARIANTS, 1, side).await;
        assert_eq!(r.base.matches.len(), 1, "{side:?}");
        let wallet = &r.base.matches[0].wallet;
        let hits = &r.side_hits[wallet][&asset_b58(SELL_ONLY_AMM)];
        assert!(hits.buy.is_none());
        let sell = hits.sell.as_ref().unwrap();
        assert_eq!(sell.count, 1);
        assert_eq!(sell.venue, Venue::PumpAmm);
        assert_eq!(r.per_token[0].qualified_sellers, 1);
        assert_eq!(r.per_token[0].qualified_buyers, 0);
        assert_eq!(r.trade.ops(Venue::PumpAmm, TradeSide::Sell), 1);
        assert_eq!(r.trade.ops(Venue::PumpAmm, TradeSide::Buy), 0);
        assert!(!r.is_coverage_incomplete(), "{:?}", r.incomplete_reasons());
    }
}

#[tokio::test]
async fn pumpswap_wallet_shows_both_sides_with_counts_and_k_semantics() {
    // pumpswap_wallet_page: one wallet trades both tokens on PumpSwap.
    // Hand count: HEJ91 12 buys / 12 sells, 9cbD 9 buys / 4 sells.
    let mints = [HEJ91, NINE_CBD];
    let any = run_fixture(&mints, PUMPSWAP_PAGE, 2, SideFilter::Any).await;
    assert_eq!(any.base.matches.len(), 1);
    let m = &any.base.matches[0];
    assert_eq!(m.hit_count, 2);
    let hits = &any.side_hits[&m.wallet];
    let (h1, h2) = (&hits[&asset_b58(HEJ91)], &hits[&asset_b58(NINE_CBD)]);
    assert_eq!(
        (
            h1.buy.as_ref().unwrap().count,
            h1.sell.as_ref().unwrap().count
        ),
        (12, 12)
    );
    assert_eq!(
        (
            h2.buy.as_ref().unwrap().count,
            h2.sell.as_ref().unwrap().count
        ),
        (9, 4)
    );
    assert_eq!(h1.buy.as_ref().unwrap().venue, Venue::PumpAmm);
    assert_ne!(h1.buy.as_ref().unwrap().signature, [0; 64]);
    assert_eq!(any.trade.ops(Venue::PumpAmm, TradeSide::Buy), 21);
    assert_eq!(any.trade.ops(Venue::PumpAmm, TradeSide::Sell), 16);

    // Side filter narrows what is tracked: buy-only run has no sells.
    let buy = run_fixture(&mints, PUMPSWAP_PAGE, 2, SideFilter::Buy).await;
    assert_eq!(buy.base.matches.len(), 1);
    let bh = &buy.side_hits[&buy.base.matches[0].wallet][&asset_b58(HEJ91)];
    assert!(bh.sell.is_none());
    assert_eq!(bh.buy.as_ref().unwrap().count, 12);
    assert_eq!(buy.trade.ops(Venue::PumpAmm, TradeSide::Sell), 0);
    let sell = run_fixture(&mints, PUMPSWAP_PAGE, 2, SideFilter::Sell).await;
    assert_eq!(sell.base.matches.len(), 1);
    assert!(
        sell.side_hits[&sell.base.matches[0].wallet][&asset_b58(HEJ91)]
            .buy
            .is_none()
    );
}

#[tokio::test]
async fn router_forwarded_pumpswap_user_is_not_attributed() {
    // Find a real router-forward (decoded user is a non-signer that nets
    // zero on every leg) in the capture, then run the engine on its mint.
    let txs = fixture_txs(PUMPSWAP_VARIANTS).await;
    let decoder = pump_amm_decoder();
    let mut forwards: Vec<(SolanaPubkey, SolanaPubkey)> = Vec::new(); // (user, mint)
    for tx in &txs {
        let rec = reconcile_pump_amm_transaction(&decoder, tx);
        for u in &rec.users {
            if u.attribution == AmmAttribution::NoUserDelta && !u.user_is_signer {
                for &i in &u.trade_indices {
                    let t = &rec.pairing.trades[i].trade;
                    let mint = if t.base_mint == WRAPPED_SOL_MINT {
                        t.quote_mint
                    } else {
                        t.base_mint
                    };
                    forwards.push((u.user, mint));
                }
            }
        }
    }
    assert!(!forwards.is_empty(), "capture holds router-forwards");
    for (user, mint) in forwards {
        let mint_b58 = bs58::encode(mint).into_string();
        let r = run_fixture(&[&mint_b58], PUMPSWAP_VARIANTS, 1, SideFilter::Any).await;
        assert!(r.trade.router_forwards_not_attributed >= 1, "{mint_b58}");
        let forwarder = WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana(user),
        };
        assert!(
            !r.side_hits.contains_key(&forwarder),
            "forwarder {} was attributed",
            bs58::encode(user).into_string()
        );
    }
}

#[tokio::test]
async fn route_swap_signer_is_attributed_on_both_sides() {
    // router_wallet_9oC3: the wallet signs ADR-013 route swaps whose decoded
    // pump legs belong to pass-through users. Hand counts (route venue):
    // GAwhc 12 buys / 2 sells, 8dBn 5 / 8.
    let mints = [
        "GAwhcphCqCv5bKHmCiN4VDdNWfbXJL4npmkc8L3Q9S9H",
        "8dBnKHwNYH3hz2fFTJczVwzBTpJFAMtc53k3uA4zpump",
    ];
    let r = run_fixture(&mints, ROUTER_9OC3, 2, SideFilter::Any).await;
    assert_eq!(r.base.matches.len(), 1);
    let wallet = wallet_b58(&r, "9oC3");
    assert_eq!(r.base.matches[0].wallet, wallet);
    let hits = &r.side_hits[&wallet];
    let g = &hits[&asset_b58(mints[0])];
    assert_eq!(g.buy.as_ref().unwrap().venue, Venue::Route);
    assert_eq!(g.buy.as_ref().unwrap().variant, "route_swap");
    assert_eq!(
        (
            g.buy.as_ref().unwrap().count,
            g.sell.as_ref().unwrap().count
        ),
        (12, 2)
    );
    let d = &hits[&asset_b58(mints[1])];
    assert_eq!(
        (
            d.buy.as_ref().unwrap().count,
            d.sell.as_ref().unwrap().count
        ),
        (5, 8)
    );
    assert_eq!(r.trade.ops(Venue::Route, TradeSide::Buy), 17);
    assert_eq!(r.trade.ops(Venue::Route, TradeSide::Sell), 10);
    // Only the signer wallet is a match: no pass-through leg user, relayer
    // or fee payer.
    assert_eq!(r.side_hits.len(), 1);

    // The second route wallet (tAwv): 7Vert 4/4, 7cYa 5/1, CNoh 3/3.
    let mints2 = [
        "7VertkgF9KLhxxJXHX6uaWuoYZTP9LdGj2bWmVXVpump",
        "7cYaQc21w9dzKkGP6LgkqtmL5yzoK5UJE1GeamTRHwny",
        "CNohWHNTurS7PpB55czspwUJfpk9yy92uf1wXdweEGg9",
    ];
    let r2 = run_fixture(&mints2, ROUTER_TAWV, 3, SideFilter::Any).await;
    assert_eq!(r2.base.matches.len(), 1);
    let w2 = wallet_b58(&r2, "tAwv");
    assert_eq!(r2.base.matches[0].wallet, w2);
    assert_eq!(r2.side_hits.len(), 1);
    assert_eq!(r2.trade.ops(Venue::Route, TradeSide::Buy), 12);
    assert_eq!(r2.trade.ops(Venue::Route, TradeSide::Sell), 8);
    // `--side sell` keeps the route sells only.
    let r3 = run_fixture(&mints2, ROUTER_TAWV, 3, SideFilter::Sell).await;
    assert_eq!(r3.base.matches.len(), 1);
    assert_eq!(r3.trade.ops(Venue::Route, TradeSide::Buy), 0);
    assert_eq!(r3.trade.ops(Venue::Route, TradeSide::Sell), 8);
}

#[tokio::test]
async fn curve_fixture_sides_and_venue() {
    // pump_bonding_curve_buy_probe: AB48 has a curve BUY, 67266... a curve
    // SELL (different wallets).
    let buy_mint = "AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
    let sell_mint = "67266Ha2icrdCKHyrYKyG4oJyJ7RqheaGbuGd6vwXbLD";
    let any = run_fixture(
        &[buy_mint, sell_mint],
        "pump_bonding_curve_buy_probe.json",
        1,
        SideFilter::Any,
    )
    .await;
    assert_eq!(any.base.matches.len(), 2);
    assert_eq!(any.trade.ops(Venue::BondingCurve, TradeSide::Buy), 1);
    assert_eq!(any.trade.ops(Venue::BondingCurve, TradeSide::Sell), 1);
    let buy = run_fixture(
        &[buy_mint, sell_mint],
        "pump_bonding_curve_buy_probe.json",
        1,
        SideFilter::Buy,
    )
    .await;
    assert_eq!(buy.base.matches.len(), 1);
    let sell = run_fixture(
        &[buy_mint, sell_mint],
        "pump_bonding_curve_buy_probe.json",
        1,
        SideFilter::Sell,
    )
    .await;
    assert_eq!(sell.base.matches.len(), 1);
    assert_ne!(buy.base.matches[0].wallet, sell.base.matches[0].wallet);
}

// ---- remapped real transactions ----------------------------------------

fn remap_tx(
    tx: &RawSolanaTransaction,
    map: &[(SolanaPubkey, SolanaPubkey)],
) -> RawSolanaTransaction {
    let m = |k: SolanaPubkey| map.iter().find(|(a, _)| *a == k).map_or(k, |(_, b)| *b);
    let mut out = tx.clone();
    for ix in &mut out.instructions {
        for a in &mut ix.accounts {
            *a = m(*a);
        }
        // Event-CPI payloads embed pubkeys (the event `user`): substitute them
        // too so the trade event still pairs with its instruction.
        for (from, to) in map {
            let mut i = 0;
            while i + 32 <= ix.data.len() {
                if ix.data[i..i + 32] == from[..] {
                    ix.data[i..i + 32].copy_from_slice(to);
                    i += 32;
                } else {
                    i += 1;
                }
            }
        }
    }
    for c in &mut out.token_balance_changes {
        c.mint = m(c.mint);
        c.owner = c.owner.map(m);
    }
    for c in &mut out.native_balance_changes {
        c.account = m(c.account);
    }
    for s in &mut out.signers {
        *s = m(*s);
    }
    out.fee_payer = m(out.fee_payer);
    out
}

struct Scripted {
    by_mint: BTreeMap<SolanaPubkey, (Vec<RawSolanaTransaction>, bool)>,
}

#[async_trait::async_trait]
impl HistoryProvider for Scripted {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::empty()
    }

    async fn plan(&self, _request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        Ok(ScanPlan {
            request_echo: String::new(),
            capabilities: SourceCapabilities::empty(),
        })
    }

    fn scan(
        &self,
        task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        let ScanRequest::TokenMarketActivity {
            asset: AssetKey::Token(_, AddressBytes::Solana(mint)),
        } = &task.request
        else {
            return Box::pin(stream::empty());
        };
        let (txs, truncated) = self.by_mint.get(mint).cloned().unwrap_or_default();
        Box::pin(stream::iter(txs.into_iter().map(move |tx| {
            Ok(ScanEnvelope {
                payload: RawPayload::SolanaTransaction(tx),
                truncated,
            })
        })))
    }
}

#[tokio::test]
async fn curve_buy_then_amm_sell_wallet_shows_both_sides() {
    // Real curve buy (probe: HgwB buys AB48) and a REAL PumpSwap sell moved
    // onto the same wallet and mint by consistent pubkey substitution (the
    // decoders read accounts and owner-keyed deltas, not PDAs).
    let probe_buyer = pubkey("HgwBZM6kQE8qpYBdM2aXDxaEs5GTpDryNxuREeVP8f8B");
    let ab48 = pubkey("AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump");
    let probe = run_fixture(
        &["AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump"],
        "pump_bonding_curve_buy_probe.json",
        1,
        SideFilter::Buy,
    )
    .await;
    assert_eq!(probe.base.matches[0].wallet, wallet_of(probe_buyer));
    let buy_sig = probe.side_hits[&probe.base.matches[0].wallet]
        [&asset_b58("AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump")]
        .buy
        .as_ref()
        .unwrap()
        .signature;
    let buy_tx = fixture_txs("pump_bonding_curve_buy_probe.json")
        .await
        .into_iter()
        .find(|tx| tx.signature == buy_sig)
        .unwrap();

    // Locate a qualifying AMM sell in the PumpSwap capture through the engine.
    let report = run_fixture(&[SELL_ONLY_AMM], PUMPSWAP_VARIANTS, 1, SideFilter::Sell).await;
    let seller = match &report.base.matches[0].wallet.address {
        AddressBytes::Solana(k) => *k,
        AddressBytes::Evm(_) => panic!("solana expected"),
    };
    let sig = report.side_hits[&report.base.matches[0].wallet][&asset_b58(SELL_ONLY_AMM)]
        .sell
        .as_ref()
        .unwrap()
        .signature;
    let amm_tx = fixture_txs(PUMPSWAP_VARIANTS)
        .await
        .into_iter()
        .find(|tx| tx.signature == sig)
        .unwrap();
    let mut sell_tx = remap_tx(
        &amm_tx,
        &[(pubkey(SELL_ONLY_AMM), ab48), (seller, probe_buyer)],
    );
    sell_tx.signature = [77; 64];
    sell_tx.slot = buy_tx.slot + 10;

    let provider = Scripted {
        by_mint: BTreeMap::from([(ab48, (vec![buy_tx, sell_tx], false))]),
    };
    let r = run_solana_trade_intersect(
        &provider,
        &[asset_b58("AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump")],
        1,
        opts(SideFilter::Any),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let wallet = WalletKey {
        chain: solana_mainnet_chain(),
        address: AddressBytes::Solana(probe_buyer),
    };
    let hits = &r.side_hits[&wallet][&asset_b58("AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump")];
    assert_eq!(hits.buy.as_ref().unwrap().venue, Venue::BondingCurve);
    assert_eq!(hits.sell.as_ref().unwrap().venue, Venue::PumpAmm);
    assert_eq!(hits.sell.as_ref().unwrap().signature, [77; 64]);
    assert_eq!(r.per_token[0].qualified_wallets, 1);
    assert_eq!(r.per_token[0].qualified_buyers, 1);
    assert_eq!(r.per_token[0].qualified_sellers, 1);
}

// ---- synthetic curve transactions ---------------------------------------

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn token(b: u8) -> AssetKey {
    AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pk(b)))
}

fn wallet(b: u8) -> WalletKey {
    WalletKey {
        chain: solana_mainnet_chain(),
        address: AddressBytes::Solana(pk(b)),
    }
}

fn curve_ix(buy: bool, user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
    let n = if buy { 16u8 } else { 14u8 };
    let mut accounts: Vec<SolanaPubkey> = (0..n).map(|i| pk(100 + i)).collect();
    accounts[2] = pk(mint);
    accounts[6] = pk(user);
    let mut data = if buy {
        BUY_INSTRUCTION_DISCRIMINATOR.to_vec()
    } else {
        SELL_INSTRUCTION_DISCRIMINATOR.to_vec()
    };
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&if buy { 2u64 } else { 0u64 }.to_le_bytes());
    if buy {
        data.push(1);
    }
    RawSolanaInstruction {
        program_id: pubkey(PUMP_BONDING_CURVE_PROGRAM_ID),
        accounts,
        data,
        instruction_index: idx,
    }
}

fn bal(mint: u8, owner: u8, pre: Option<u64>, post: u64) -> SolanaTokenBalanceChange {
    SolanaTokenBalanceChange {
        mint: pk(mint),
        owner: Some(pk(owner)),
        decimals: 6,
        pre_amount: pre,
        post_amount: post,
        closed: false,
    }
}

fn curve_tx(buy: bool, user: u8, mint: u8, sig: u8, slot: u64) -> RawSolanaTransaction {
    RawSolanaTransaction {
        block_time: None,
        signature: [sig; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot,
        transaction_index: 0,
        instructions: vec![curve_ix(buy, user, mint, 0)],
        token_balance_changes: vec![if buy {
            bal(mint, user, None, 10)
        } else {
            bal(mint, user, Some(10), 0)
        }],
        fee_lamports: 5_000,
        fee_payer: pk(user),
        signers: vec![pk(user)],
        native_balance_changes: vec![],
    }
}

fn at(mut tx: RawSolanaTransaction, block_time: Option<i64>) -> RawSolanaTransaction {
    tx.block_time = block_time;
    tx
}

fn scripted(entries: Vec<(u8, Vec<RawSolanaTransaction>, bool)>) -> Scripted {
    Scripted {
        by_mint: entries
            .into_iter()
            .map(|(m, t, trunc)| (pk(m), (t, trunc)))
            .collect(),
    }
}

async fn run(
    provider: &Scripted,
    tokens: &[u8],
    k: usize,
    o: IntersectOptions,
) -> SolanaBuyerIntersectReport {
    let assets: Vec<AssetKey> = tokens.iter().map(|t| token(*t)).collect();
    run_solana_trade_intersect(provider, &assets, k, o, CancellationToken::new())
        .await
        .unwrap()
}

#[tokio::test]
async fn k_counts_distinct_tokens_hit_under_the_selected_side() {
    // W (1) buys token 10 and sells token 11; V (2) buys both; U (3) sells both.
    let provider = scripted(vec![
        (
            10,
            vec![
                curve_tx(true, 1, 10, 1, 100),
                curve_tx(true, 2, 10, 2, 101),
                curve_tx(false, 3, 10, 3, 102),
            ],
            false,
        ),
        (
            11,
            vec![
                curve_tx(false, 1, 11, 4, 103),
                curve_tx(true, 2, 11, 5, 104),
                curve_tx(false, 3, 11, 6, 105),
            ],
            false,
        ),
    ]);
    let wallets = |r: &SolanaBuyerIntersectReport| -> Vec<WalletKey> {
        r.base.matches.iter().map(|m| m.wallet.clone()).collect()
    };
    let any = run(&provider, &[10, 11], 2, opts(SideFilter::Any)).await;
    assert_eq!(any.base.matches.len(), 3);
    assert!(any.base.matches.iter().all(|m| m.hit_count == 2));
    let buy = run(&provider, &[10, 11], 2, opts(SideFilter::Buy)).await;
    assert_eq!(wallets(&buy), vec![wallet(2)]);
    let sell = run(&provider, &[10, 11], 2, opts(SideFilter::Sell)).await;
    assert_eq!(wallets(&sell), vec![wallet(3)]);
    // K=1 under buy: everyone who bought at least one token.
    let buy1 = run(&provider, &[10, 11], 1, opts(SideFilter::Buy)).await;
    assert_eq!(buy1.base.matches.len(), 2);
    // W shows buy on 10 and sell on 11 under any.
    let w = &any.side_hits[&wallet(1)];
    assert!(w[&token(10)].buy.is_some() && w[&token(10)].sell.is_none());
    assert!(w[&token(11)].sell.is_some() && w[&token(11)].buy.is_none());
    // Two buys of one token by one wallet are ONE hit (count 2).
    let provider2 = scripted(vec![(
        10,
        vec![curve_tx(true, 1, 10, 1, 100), curve_tx(true, 1, 10, 2, 101)],
        false,
    )]);
    let r = run(&provider2, &[10], 1, opts(SideFilter::Buy)).await;
    assert_eq!(r.base.matches[0].hit_count, 1);
    let e = r.side_hits[&wallet(1)][&token(10)].buy.clone().unwrap();
    assert_eq!(e.count, 2);
    // First evidence = lowest slot, regardless of scan order.
    assert_eq!((e.slot, e.signature), (100, [1; 64]));
}

#[tokio::test]
async fn transfer_only_recipient_never_hits() {
    // The wallet RECEIVES token 10 through a plain transfer (positive owner
    // delta, no decoded trade) and sends token 11 away the same way.
    let transfer = |mint: u8, pre: Option<u64>, post: u64, sig: u8| RawSolanaTransaction {
        block_time: None,
        signature: [sig; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot: 500,
        transaction_index: 0,
        instructions: vec![RawSolanaInstruction {
            program_id: pubkey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
            accounts: vec![pk(50), pk(51), pk(52)],
            data: vec![3, 1, 0, 0, 0, 0, 0, 0, 0],
            instruction_index: 0,
        }],
        token_balance_changes: vec![bal(mint, 7, pre, post)],
        fee_lamports: 5_000,
        fee_payer: pk(52),
        signers: vec![pk(52)],
        native_balance_changes: vec![],
    };
    let provider = scripted(vec![
        (10, vec![transfer(10, None, 10, 1)], false),
        (11, vec![transfer(11, Some(10), 0, 2)], false),
    ]);
    for side in [SideFilter::Any, SideFilter::Buy, SideFilter::Sell] {
        let r = run(&provider, &[10, 11], 1, opts(side)).await;
        assert!(r.base.matches.is_empty(), "{side:?}");
        assert!(r.side_hits.is_empty());
        assert_eq!(r.per_token[0].qualified_wallets, 0);
        assert!(!r.is_coverage_incomplete());
    }
    let r = run(&provider, &[10, 11], 1, opts(SideFilter::Any)).await;
    assert_eq!(r.per_token[0].positive_delta_without_instruction, 1);
}

fn sell_variant_idl_only(v: PumpTradeVariant) -> VariantVerification {
    if v == PumpTradeVariant::Sell {
        VariantVerification::IdlOnly
    } else {
        v.verification()
    }
}

#[tokio::test]
async fn idl_only_trade_never_qualifies_is_counted_and_forces_incomplete() {
    let provider = scripted(vec![(
        10,
        vec![
            curve_tx(false, 1, 10, 1, 100),
            curve_tx(true, 2, 10, 2, 101),
        ],
        false,
    )]);
    let assets = [token(10)];
    for side in [SideFilter::Sell, SideFilter::Any] {
        let r = run_solana_trade_intersect_with_policy(
            &provider,
            &assets,
            1,
            opts(side),
            CancellationToken::new(),
            sell_variant_idl_only,
        )
        .await
        .unwrap();
        // The IdlOnly sell never qualifies; under any the verified buy does.
        assert_eq!(r.base.matches.len(), usize::from(side == SideFilter::Any));
        assert_eq!(r.trade.idl_only_trades, 1, "{side:?}");
        assert_eq!(
            r.diagnostics.unverified_variant_buys[PumpTradeVariant::Sell.index()],
            1
        );
        assert!(r.is_coverage_incomplete());
        assert!(
            r.incomplete_reasons()
                .iter()
                .any(|s| s.contains("IdlOnly") && s.contains("sell")),
            "{:?}",
            r.incomplete_reasons()
        );
    }
    // Side buy never looks at the sell: complete.
    let r = run_solana_trade_intersect_with_policy(
        &provider,
        &assets,
        1,
        opts(SideFilter::Buy),
        CancellationToken::new(),
        sell_variant_idl_only,
    )
    .await
    .unwrap();
    assert_eq!(r.trade.idl_only_trades, 0);
    assert!(!r.is_coverage_incomplete());
}

fn window(since: i64, until: i64) -> AnalysisWindow {
    AnalysisWindow {
        since,
        until,
        as_of: until + 1_000,
        source: WindowSource::Explicit,
    }
}

#[tokio::test]
async fn window_boundary_is_half_open_and_reaching_it_completes_the_token() {
    // Window [1000, 3000): t=1000 in (inclusive start), t=2999 in, t=3000
    // out (exclusive end), t=500 before the start (boundary reached).
    let txs = vec![
        at(curve_tx(true, 4, 10, 4, 400), Some(3000)),
        at(curve_tx(true, 3, 10, 3, 300), Some(2999)),
        at(curve_tx(false, 2, 10, 2, 200), Some(1000)),
        at(curve_tx(true, 1, 10, 1, 100), Some(500)),
    ];
    // The provider reports an unconsumed cursor, but the boundary tx proves
    // the window was fully covered.
    let provider = scripted(vec![(10, txs.clone(), true)]);
    let o = IntersectOptions {
        side: SideFilter::Any,
        window: window(1000, 3000),
    };
    let r = run(&provider, &[10], 1, o).await;
    let hit: Vec<WalletKey> = r.base.matches.iter().map(|m| m.wallet.clone()).collect();
    assert_eq!(hit.len(), 2);
    assert!(hit.contains(&wallet(2)) && hit.contains(&wallet(3)));
    assert!(!r.side_hits.contains_key(&wallet(1)), "before the window");
    assert!(!r.side_hits.contains_key(&wallet(4)), "at/after until");
    assert_eq!(r.per_token[0].transactions_scanned, 4);
    assert_eq!(r.per_token[0].transactions_in_window, Some(2));
    assert!(r.per_token[0].boundary_reached);
    assert!(!r.per_token[0].truncated);
    assert!(!r.is_coverage_incomplete(), "{:?}", r.incomplete_reasons());

    // Same without the boundary tx and with a cursor left: the page budget
    // ended before the window start -> incomplete, with the window text.
    let provider = scripted(vec![(10, txs[..3].to_vec(), true)]);
    let r = run(&provider, &[10], 1, o).await;
    assert!(r.per_token[0].truncated);
    assert!(r.is_coverage_incomplete());
    assert!(
        r.incomplete_reasons()
            .iter()
            .any(|s| s.contains("before the window start was reached")),
        "{:?}",
        r.incomplete_reasons()
    );

    // No window: the old truncation semantics.
    let provider = scripted(vec![(10, txs[..3].to_vec(), true)]);
    let r = run(&provider, &[10], 1, opts(SideFilter::Any)).await;
    assert!(
        r.incomplete_reasons()
            .iter()
            .any(|s| s.contains("unconsumed pagination"))
    );
    assert_eq!(r.per_token[0].transactions_in_window, None);
    assert_eq!(r.base.matches.len(), 3);
}

#[tokio::test]
async fn window_transaction_without_block_time_is_a_coverage_gap() {
    let txs = vec![
        at(curve_tx(true, 1, 10, 1, 100), None),
        at(curve_tx(true, 2, 10, 2, 90), Some(2000)),
    ];
    let provider = scripted(vec![(10, txs, false)]);
    let o = IntersectOptions {
        side: SideFilter::Buy,
        window: window(1000, 3000),
    };
    let r = run(&provider, &[10], 1, o).await;
    assert_eq!(r.base.matches.len(), 1, "only the timed tx is placed");
    assert_eq!(r.per_token[0].missing_block_time, 1);
    assert!(r.is_coverage_incomplete());
}

#[tokio::test]
async fn duplicate_delivery_of_a_transaction_counts_once() {
    let tx = curve_tx(true, 1, 10, 1, 100);
    let provider = scripted(vec![(10, vec![tx.clone(), tx], false)]);
    let r = run(&provider, &[10], 1, opts(SideFilter::Any)).await;
    assert_eq!(r.per_token[0].transactions_scanned, 1);
    assert_eq!(
        r.side_hits[&wallet(1)][&token(10)]
            .buy
            .as_ref()
            .unwrap()
            .count,
        1
    );
}

// ---- ADR-015: Jupiter legs through the shared attribution -----------------

/// Signer wallet 1 (relayer 88 pays) swaps USDC for / into token 10; the only
/// decoded evidence is a Jupiter `SwapsEvent` whose hop trades token 10.
fn jupiter_route_tx(
    buy: bool,
    trade_mint: u8,
    sig: u8,
    slot: u64,
    with_event: bool,
) -> RawSolanaTransaction {
    let usdc = pubkey(USDC_MINT);
    let (input, in_amt, output, out_amt) = if buy {
        (usdc, 5_000_000u64, pk(trade_mint), 1_000u64)
    } else {
        (pk(trade_mint), 1_000u64, usdc, 5_000_000u64)
    };
    let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
    data.extend(JUPITER_SWAPS_EVENT_DISCRIMINATOR);
    data.extend(1u32.to_le_bytes());
    data.extend(input);
    data.extend(in_amt.to_le_bytes());
    data.extend(output);
    data.extend(out_amt.to_le_bytes());
    data.extend(pk(0x77));
    let ix = RawSolanaInstruction {
        program_id: JUPITER_V6_PROGRAM_ID_BYTES,
        accounts: vec![JUPITER_EVENT_AUTHORITY_BYTES],
        data,
        instruction_index: 1,
    };
    let usdc_bal = |pre: u64, post: u64| SolanaTokenBalanceChange {
        mint: usdc,
        owner: Some(pk(1)),
        decimals: 6,
        pre_amount: Some(pre),
        post_amount: post,
        closed: false,
    };
    RawSolanaTransaction {
        block_time: None,
        signature: [sig; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot,
        transaction_index: 0,
        instructions: if with_event { vec![ix] } else { vec![] },
        token_balance_changes: if buy {
            vec![bal(10, 1, None, 1_000), usdc_bal(5_000_000, 0)]
        } else {
            vec![bal(10, 1, Some(1_000), 0), usdc_bal(0, 5_000_000)]
        },
        fee_lamports: 5_000,
        fee_payer: pk(88),
        signers: vec![pk(88), pk(1)],
        native_balance_changes: vec![],
    }
}

#[tokio::test]
async fn jupiter_leg_makes_a_signer_route_swap_a_hit_in_buyer_intersect() {
    let provider = scripted(vec![(
        10,
        vec![
            jupiter_route_tx(true, 10, 1, 100, true),
            jupiter_route_tx(false, 10, 2, 101, true),
            // Same movements with no decoded evidence: a transfer-shaped tx.
            jupiter_route_tx(true, 10, 3, 102, false),
            // A Jupiter hop that does not trade token 10 is not evidence for it.
            jupiter_route_tx(true, 11, 4, 103, true),
        ],
        false,
    )]);
    let r = run(&provider, &[10], 1, opts(SideFilter::Any)).await;
    assert_eq!(r.base.matches.len(), 1);
    let hits = &r.side_hits[&wallet(1)][&token(10)];
    let (b, s) = (hits.buy.clone().unwrap(), hits.sell.clone().unwrap());
    assert_eq!(
        (b.venue, b.variant, b.count),
        (Venue::Route, "route_swap", 1)
    );
    assert_eq!((s.venue, s.count), (Venue::Route, 1));
    assert_eq!(r.trade.ops(Venue::Route, TradeSide::Buy), 1);
    assert_eq!(r.trade.ops(Venue::Route, TradeSide::Sell), 1);
    // Relayer (fee payer) and the Jupiter venue are never attributed.
    assert_eq!(r.side_hits.len(), 1);
}

#[tokio::test]
async fn buyer_intersect_keeps_bounded_evidence_for_malformed_items() {
    let mut txs = Vec::new();
    for i in 0..7u8 {
        let mut tx = curve_tx(true, 1, 10, 20 + i, 100 + u64::from(i));
        tx.instructions[0].data.truncate(12);
        txs.push(tx);
    }
    let provider = scripted(vec![(10, txs, false)]);
    let r = run(&provider, &[10], 1, opts(SideFilter::Any)).await;
    assert_eq!(r.trade.malformed_trades, 7);
    let ev = &r.trade.evidence_samples;
    assert_eq!(ev.len(), 5);
    assert_eq!(ev[0].signature, [20; 64]);
    assert_eq!(ev[0].slot, 100);
    assert_eq!(ev[0].variant_or_discriminator, "buy");
    assert_eq!((ev[0].data_len, ev[0].accounts_len), (12, 16));
    assert_eq!(ev[0].kind.label(), "malformed_trade_instruction");
    assert_eq!(ev[0].program_name(), "pump_curve");
    // Per-token diagnostics carry the same samples.
    assert_eq!(r.per_token[0].trade.evidence_samples, *ev);
}
