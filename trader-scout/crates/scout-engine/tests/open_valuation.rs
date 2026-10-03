//! ADR-019 end-to-end: ledger -> open-position valuation through the real
//! `HeliusProvider` (wiremock `getMultipleAccounts` with a fake curve, fake
//! pools and fake vaults) -> USD at `as_of` -> rank. Formulas are checked
//! against hand-computed literals and against the verified swap math;
//! the live fixture wallet supplies real pool addresses and event fee bps.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use futures::StreamExt as _;
use scout_api::{HistoryProvider, ScanRequest, ScanTask};
use scout_core::{
    AddressBytes, AssetKey, RawPayload, RawSolanaInstruction, RawSolanaTransaction,
    SolanaExecutionStatus, SolanaNativeBalanceChange, SolanaPubkey, SolanaTokenBalanceChange,
};
use scout_dex_solana::{
    BONDING_CURVE_ACCOUNT_DISCRIMINATOR, BUY_INSTRUCTION_DISCRIMINATOR, EVENT_CPI_DISCRIMINATOR,
    POOL_ACCOUNT_DISCRIMINATOR, PUMP_AMM_PROGRAM_ID_BYTES, PUMP_PROGRAM_ID_BYTES,
    SPL_TOKEN_PROGRAM_ID_BYTES, TRADE_EVENT_DISCRIMINATOR, WRAPPED_SOL_MINT, amm_sell_quote,
    curve_sell_quote, effective_quote_reserve,
};
use scout_engine::{
    AnalysisWindow, ExclusionReason, LedgerDecoders, LedgerOptions, PUMP_BONDING_CURVE_PROGRAM_ID,
    RankBy, RankPolicy, RankProfile, SolanaWalletStats, UnvaluedReason, Venue, WalletScanStatus,
    apply_open_valuation, apply_usd_pricing, build_solana_wallet_ledger,
    build_solana_wallet_ledger_venues, pump_amm_decoder, pump_bonding_curve_decoder,
    rank_solana_wallets, solana_mainnet_chain,
};
use scout_pricing::{Candle, DecimalPrice, InMemoryPriceSource, QuoteAsset};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

fn b58(k: &SolanaPubkey) -> String {
    bs58::encode(k).into_string()
}

const T0: i64 = 1_790_942_400; // 2026-10-02T12:00:00Z
const W: u8 = 1;
const M1: u8 = 10;

// ---------------------------------------------------------------------
// Fake chain state served over `getMultipleAccounts`
// ---------------------------------------------------------------------

struct FakeAccount {
    owner: SolanaPubkey,
    data: Vec<u8>,
}

fn curve_account(complete: bool, vsol: u64, vtok: u64) -> FakeAccount {
    let mut d = BONDING_CURVE_ACCOUNT_DISCRIMINATOR.to_vec();
    for v in [vtok, vsol, 0, 0, 1_000_000_000_000_000u64] {
        d.extend(v.to_le_bytes());
    }
    d.push(u8::from(complete));
    d.extend(pk(0xAA));
    FakeAccount {
        owner: PUMP_PROGRAM_ID_BYTES,
        data: d,
    }
}

#[allow(clippy::too_many_arguments)]
fn pool_account(
    base_mint: SolanaPubkey,
    quote_mint: SolanaPubkey,
    base_vault: SolanaPubkey,
    quote_vault: SolanaPubkey,
    vq: i128,
) -> FakeAccount {
    let mut d = POOL_ACCOUNT_DISCRIMINATOR.to_vec();
    d.push(255);
    d.extend(0u16.to_le_bytes());
    d.extend(pk(1));
    d.extend(base_mint);
    d.extend(quote_mint);
    d.extend(pk(4));
    d.extend(base_vault);
    d.extend(quote_vault);
    d.extend(0u64.to_le_bytes());
    d.extend(pk(8));
    d.extend([0, 0]);
    d.extend(vq.to_le_bytes());
    FakeAccount {
        owner: PUMP_AMM_PROGRAM_ID_BYTES,
        data: d,
    }
}

fn token_account(mint: SolanaPubkey, amount: u64) -> FakeAccount {
    let mut d = Vec::new();
    d.extend(mint);
    d.extend(pk(2));
    d.extend(amount.to_le_bytes());
    d.extend([0u8; 36]);
    d.push(1);
    d.resize(165, 0);
    FakeAccount {
        owner: SPL_TOKEN_PROGRAM_ID_BYTES,
        data: d,
    }
}

/// Serve `accounts`; the context slot is `7000 + calls so far`.
async fn chain_server(accounts: BTreeMap<String, FakeAccount>) -> MockServer {
    let calls = Arc::new(AtomicU64::new(0));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |req: &Request| {
            let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(v["method"], "getMultipleAccounts");
            assert_eq!(v["params"][1]["encoding"], "base64");
            let slot = 7000 + calls.fetch_add(1, Ordering::SeqCst);
            let value: Vec<serde_json::Value> = v["params"][0]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| match accounts.get(a.as_str().unwrap()) {
                    None => serde_json::Value::Null,
                    Some(acc) => serde_json::json!({
                        "data": [base64::engine::general_purpose::STANDARD.encode(&acc.data), "base64"],
                        "executable": false, "lamports": 1, "owner": b58(&acc.owner),
                        "rentEpoch": 0, "space": acc.data.len()
                    }),
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"context": {"slot": slot}, "value": value}
            }))
        })
        .mount(&server)
        .await;
    server
}

fn provider(server: &MockServer) -> HeliusProvider {
    HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1).unwrap()
}

async fn account_requests(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

fn card(ledger: scout_engine::SolanaWalletLedgerReport, wallet: u8) -> SolanaWalletStats {
    SolanaWalletStats {
        chain: scout_engine::SOLANA_DISPLAY,
        wallet: pk(wallet),
        status: WalletScanStatus::Ok,
        transactions_scanned: Some(1),
        transactions_in_window: None,
        truncated: false,
        unexpected_payloads: 0,
        error: None,
        ledger: Some(ledger),
        incomplete_reasons: vec![],
        failure: None,
        not_scanned: None,
    }
}

// ---------------------------------------------------------------------
// Synthetic bonding-curve wallet (hand-computed golden)
// ---------------------------------------------------------------------

fn pump() -> SolanaPubkey {
    pubkey(PUMP_BONDING_CURVE_PROGRAM_ID)
}

fn buy_ix(user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
    let mut accounts: Vec<SolanaPubkey> = (0..16u8).map(|i| pk(100 + i)).collect();
    accounts[2] = pk(mint);
    accounts[6] = pk(user);
    let mut data = BUY_INSTRUCTION_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&2u64.to_le_bytes());
    data.push(1);
    RawSolanaInstruction {
        program_id: pump(),
        accounts,
        data,
        instruction_index: idx,
    }
}

fn buy_event_ix(
    user: u8,
    mint: u8,
    sol: u64,
    tokens: u64,
    fee: u64,
    cfee: u64,
    idx: u32,
) -> RawSolanaInstruction {
    let le = |v: u64| v.to_le_bytes();
    let mut p = Vec::new();
    p.extend(pk(mint));
    p.extend(le(sol));
    p.extend(le(tokens));
    p.push(1);
    p.extend(pk(user));
    p.extend(1_790_942_000i64.to_le_bytes());
    for v in [11, 12, 13, 14] {
        p.extend(le(v));
    }
    p.extend(pk(0xfe));
    p.extend(le(95));
    p.extend(le(fee));
    p.extend(pk(0xcc));
    p.extend(le(30));
    p.extend(le(cfee));
    let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
    data.extend(TRADE_EVENT_DISCRIMINATOR);
    data.extend(p);
    RawSolanaInstruction {
        program_id: pump(),
        accounts: vec![pk(0xea)],
        data,
        instruction_index: idx,
    }
}

/// Wallet `user` buys `tokens` of `mint` for `sol` + fees; `with_event`
/// false = trade instruction without its event (no fee observation).
fn curve_buy_tx(user: u8, mint: u8, sig: u8, with_event: bool) -> RawSolanaTransaction {
    let (sol, tokens, fee, cfee, tx_fee) = (1_000_000u64, 1000u64, 10_000u64, 5_000u64, 5_000u64);
    let mut ixs = vec![buy_ix(user, mint, 0)];
    if with_event {
        ixs.push(buy_event_ix(user, mint, sol, tokens, fee, cfee, 1));
    }
    RawSolanaTransaction {
        log_messages: None,
        block_time: Some(T0 - 600),
        signature: [sig; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot: 100,
        transaction_index: 0,
        instructions: ixs,
        token_balance_changes: vec![SolanaTokenBalanceChange {
            mint: pk(mint),
            owner: Some(pk(user)),
            decimals: 6,
            pre_amount: Some(0),
            post_amount: tokens,
            closed: false,
        }],
        fee_lamports: tx_fee,
        fee_payer: pk(user),
        signers: vec![pk(user)],
        native_balance_changes: vec![SolanaNativeBalanceChange {
            account: pk(user),
            pre_lamports: 10_000_000_000,
            post_lamports: 10_000_000_000 - (sol + fee + cfee + tx_fee),
        }],
    }
}

fn curve_ledger(user: u8, mint: u8, with_event: bool) -> scout_engine::SolanaWalletLedgerReport {
    build_solana_wallet_ledger(
        &pk(user),
        &[curve_buy_tx(user, mint, 1, with_event)],
        &pump_bonding_curve_decoder().unwrap(),
    )
    .unwrap()
}

fn flat_candle(time: i64, close: &str) -> Candle {
    let p = DecimalPrice::parse(close).unwrap();
    Candle {
        time,
        low: p,
        high: p,
        open: p,
        close: p,
        volume: DecimalPrice::ONE,
    }
}

const AS_OF: i64 = T0 + 100; // minute T0 + 60

#[tokio::test]
async fn curve_position_is_valued_with_event_fees_slot_basis_and_usd() {
    let ledger = curve_ledger(W, M1, true);
    let info = ledger.open_venues[0].clone();
    assert_eq!(info.last_venue, Some(Venue::BondingCurve));
    let curve = info.curve.unwrap();
    // Fee bps come from the wallet's own paired event (95 + 30 bps).
    assert_eq!(curve.fee_observed_at, Some(1_790_942_000));
    // Basis: 1_000_000 + 10_000 + 5_000 + tx fee 5_000.
    assert!(info.basis.fully_known_sol);

    let accounts = BTreeMap::from([(
        b58(&curve.address),
        curve_account(false, 2_000_000_000, 1_000_000),
    )]);
    let server = chain_server(accounts).await;
    let p = provider(&server);
    let mut cards = vec![card(ledger, W)];
    let window = AnalysisWindow::none(AS_OF);
    let run = apply_open_valuation(&mut cards, &p, &window).await;

    assert!(run.ran && !run.historical && !run.budget_exhausted);
    assert_eq!((run.account_calls, run.state_slot), (1, Some(7000)));
    assert_eq!(p.total_requests_made(), 1); // counted in the request budget
    assert_eq!((run.totals.positions, run.totals.valued), (1, 1));

    let view = cards[0]
        .ledger
        .as_ref()
        .unwrap()
        .open_valuation
        .clone()
        .unwrap();
    let v = view.positions[0].valued().unwrap();
    // gross = floor(1000 * 2e9 / 1_001_000) = 1_998_001
    assert_eq!(v.gross_lamports, 1_998_001);
    assert_eq!(v.fee_lamports, 18_982 + 5_995);
    assert_eq!(v.realizable_lamports, 1_973_024);
    assert_eq!(v.marginal_lamports, 2_000_000); // 1000 * 2e9 / 1e6
    assert_eq!(v.price_impact_bps, Some(9)); // (2_000_000-1_998_001)*1e4/2e6 = 9.99
    assert_eq!(v.account_slot, 7000);
    assert_eq!(v.label, "realizable_cp_quote");
    // Cross-check with the verified formula.
    let q = curve_sell_quote(1000, 2_000_000_000, 1_000_000, 95, 30).unwrap();
    assert_eq!(q.net, v.realizable_lamports);
    // Unrealized = realizable - known remaining basis (1_020_000).
    assert_eq!(
        v.unrealized_pnl.unwrap(),
        scout_engine::lamports_to_money(1_973_024 - 1_020_000).unwrap()
    );

    // USD at as_of (minute T0+60), one half-even conversion.
    let src = InMemoryPriceSource::new().with_candles(
        QuoteAsset::Sol,
        &[
            flat_candle(T0 - 600, "90.00"),
            flat_candle(T0 + 60, "100.00"),
        ],
    );
    let upr = apply_usd_pricing(&mut cards, &src).await;
    // as_of minute + the lot's acquisition minute.
    assert_eq!(upr.minutes_requested[&QuoteAsset::Sol], 2);
    let v = cards[0]
        .ledger
        .as_ref()
        .unwrap()
        .open_valuation
        .as_ref()
        .unwrap()
        .positions[0]
        .valued()
        .unwrap()
        .clone();
    // 1_973_024 lamports * 100 USD/SOL = 0.1973024 USD = 19_730_240 (1e-8 USD).
    let usd = v.usd.unwrap();
    assert_eq!(usd.value.scaled_units(), 19_730_240);
    assert_eq!(usd.price_label, "cex_reference_1m");
    // USD unrealized = 19_730_240 - basis 1_020_000 lamports * 90 USD/SOL
    // (0.0918 USD = 9_180_000) = 10_550_240 (1e-8 USD).
    assert_eq!(v.usd_unrealized.unwrap().scaled_units(), 10_550_240);
    assert_eq!(v.usd_unrealized_reason, None);
    let totals = cards[0]
        .ledger
        .as_ref()
        .unwrap()
        .open_valuation
        .as_ref()
        .unwrap()
        .totals();
    assert_eq!(
        (
            totals.usd_unrealized_known_scaled,
            totals.usd_unrealized_known_positions,
            totals.usd_unrealized_unknown_positions
        ),
        (10_550_240, 1, 0)
    );
    // A realized figure never reads the valuation.
    assert!(cards[0].ledger.as_ref().unwrap().usd.is_some());
}

#[tokio::test]
async fn usd_price_labels_stale_and_unknown_never_zero() {
    for (candles, expect_label, expect_reason) in [
        // as_of minute T0+60 has no candle; T0 is 1 minute before: stale_1m.
        (vec![flat_candle(T0, "100.00")], Some("stale_1m"), None),
        (vec![], None, Some("no_candle_within_5m")),
    ] {
        let ledger = curve_ledger(W, M1, true);
        let curve = ledger.open_venues[0].curve.unwrap();
        let accounts = BTreeMap::from([(
            b58(&curve.address),
            curve_account(false, 2_000_000_000, 1_000_000),
        )]);
        let server = chain_server(accounts).await;
        let mut cards = vec![card(ledger, W)];
        apply_open_valuation(&mut cards, &provider(&server), &AnalysisWindow::none(AS_OF)).await;
        let src = InMemoryPriceSource::new().with_candles(QuoteAsset::Sol, &candles);
        apply_usd_pricing(&mut cards, &src).await;
        let v = cards[0]
            .ledger
            .as_ref()
            .unwrap()
            .open_valuation
            .as_ref()
            .unwrap()
            .positions[0]
            .valued()
            .unwrap()
            .clone();
        assert_eq!(v.usd.as_ref().map(|u| u.price_label.as_str()), expect_label);
        assert_eq!(v.usd_unpriced_reason.as_deref(), expect_reason);
        // The SOL value is still there; USD is absent, never zero.
        assert_eq!(v.realizable_lamports, 1_973_024);
    }
}

#[tokio::test]
async fn usd_unrealized_is_unknown_when_basis_or_value_is_unpriced() {
    // (candles, expected reason): the lot's acquisition minute has no
    // candle -> basis unpriced; the as_of minute has none -> value unpriced.
    let cases = [
        (
            vec![flat_candle(T0 + 60, "100.00")],
            "basis_no_candle_within_5m",
        ),
        (vec![flat_candle(T0 - 600, "90.00")], "no_candle_within_5m"),
    ];
    for (candles, reason) in cases {
        let ledger = curve_ledger(W, M1, true);
        let curve = ledger.open_venues[0].curve.unwrap();
        let accounts = BTreeMap::from([(
            b58(&curve.address),
            curve_account(false, 2_000_000_000, 1_000_000),
        )]);
        let server = chain_server(accounts).await;
        let mut cards = vec![card(ledger, W)];
        apply_open_valuation(&mut cards, &provider(&server), &AnalysisWindow::none(AS_OF)).await;
        let src = InMemoryPriceSource::new().with_candles(QuoteAsset::Sol, &candles);
        apply_usd_pricing(&mut cards, &src).await;
        let view = cards[0]
            .ledger
            .as_ref()
            .unwrap()
            .open_valuation
            .as_ref()
            .unwrap();
        let v = view.positions[0].valued().unwrap();
        assert_eq!(v.usd_unrealized, None, "{reason}");
        assert_eq!(v.usd_unrealized_reason.as_deref(), Some(reason));
        let t = view.totals();
        assert_eq!(
            (
                t.usd_unrealized_known_positions,
                t.usd_unrealized_unknown_positions
            ),
            (0, 1)
        );
    }
}

#[tokio::test]
async fn historical_window_is_unvalued_and_reads_nothing() {
    let ledger = curve_ledger(W, M1, true);
    let server = chain_server(BTreeMap::new()).await;
    let p = provider(&server);
    let mut cards = vec![card(ledger, W)];
    let window =
        AnalysisWindow::resolve(Some("30d"), None, Some("2026-09-01T00:00:00Z"), T0).unwrap();
    assert!(window.until < window.as_of);
    let run = apply_open_valuation(&mut cards, &p, &window).await;
    assert!(run.historical);
    assert_eq!(account_requests(&server).await, 0);
    assert_eq!(p.total_requests_made(), 0);
    let view = cards[0]
        .ledger
        .as_ref()
        .unwrap()
        .open_valuation
        .as_ref()
        .unwrap();
    assert_eq!(
        view.positions[0].unvalued_reason(),
        Some(UnvaluedReason::HistoricalWindow)
    );
    assert_eq!(
        run.totals.unvalued_by_reason.get("historical_window"),
        Some(&1)
    );
}

#[tokio::test]
async fn no_venue_migrated_and_missing_account_reasons() {
    // A trade without its paired event is booked as a route swap (ADR-013)
    // from the wallet's deltas: no direct venue account is known.
    let no_event = curve_ledger(2, 20, false);
    assert!(no_event.open_venues[0].curve.is_none());
    assert_eq!(no_event.open_venues[0].last_venue, None);
    let migrated = curve_ledger(3, 30, true);
    let missing = curve_ledger(4, 40, true);
    let key =
        |l: &scout_engine::SolanaWalletLedgerReport| b58(&l.open_venues[0].curve.unwrap().address);
    let cases: Vec<(
        scout_engine::SolanaWalletLedgerReport,
        BTreeMap<String, FakeAccount>,
        UnvaluedReason,
        u64,
    )> = vec![
        (
            no_event,
            BTreeMap::new(),
            UnvaluedReason::NoVenueObserved,
            0,
        ),
        (
            migrated.clone(),
            BTreeMap::from([(key(&migrated), curve_account(true, 1, 1))]),
            UnvaluedReason::MigratedPoolUnknown,
            1,
        ),
        (missing, BTreeMap::new(), UnvaluedReason::AccountMissing, 1),
    ];
    for (ledger, accounts, want, calls) in cases {
        let server = chain_server(accounts).await;
        let w = ledger.wallet;
        let p = provider(&server);
        let mut cards = vec![card(ledger, w[0])];
        let run = apply_open_valuation(&mut cards, &p, &AnalysisWindow::none(AS_OF)).await;
        assert_eq!(run.account_calls, calls, "{want:?}");
        let view = cards[0]
            .ledger
            .as_ref()
            .unwrap()
            .open_valuation
            .as_ref()
            .unwrap();
        assert_eq!(view.positions[0].unvalued_reason(), Some(want));
    }
}

#[tokio::test]
async fn budget_exhaustion_leaves_positions_unvalued_and_is_flagged() {
    let ledger = curve_ledger(W, M1, true);
    let server = chain_server(BTreeMap::new()).await;
    // The scan already spent the whole budget.
    let p = provider(&server).with_max_total_requests(Some(1));
    p.get_multiple_accounts(&[pk(1)], "confirmed")
        .await
        .unwrap();
    let mut cards = vec![card(ledger, W)];
    let run = apply_open_valuation(&mut cards, &p, &AnalysisWindow::none(AS_OF)).await;
    assert!(run.budget_exhausted);
    assert!(run.fetch_error.is_some());
    assert_eq!(p.total_requests_made(), 1, "no attempt beyond the budget");
    let view = cards[0]
        .ledger
        .as_ref()
        .unwrap()
        .open_valuation
        .as_ref()
        .unwrap();
    assert_eq!(
        view.positions[0].unvalued_reason(),
        Some(UnvaluedReason::RequestBudgetExhausted)
    );
}

// ---------------------------------------------------------------------
// Live PumpSwap fixture wallet: real pools and real event fee bps
// ---------------------------------------------------------------------

const PUMPSWAP_WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";

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
    let provider = provider(&server);
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::TokenMarketActivity {
                asset: AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pk(M1))),
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

fn fixture_ledger(txs: &[RawSolanaTransaction]) -> scout_engine::SolanaWalletLedgerReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    build_solana_wallet_ledger_venues(
        &pubkey(PUMPSWAP_WALLET),
        txs,
        &LedgerDecoders {
            curve: &curve,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        },
        LedgerOptions {
            left_censoring: true,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn live_pumpswap_wallet_positions_are_valued_on_fake_pool_state() {
    let txs = fixture_txs("pumpswap_wallet_page_2026-10-02.json").await;
    let ledger = fixture_ledger(&txs);
    assert!(ledger.open_venues.len() >= 10);
    // Fee bps were observed for every real pool of the fixture wallet.
    let mut accounts: BTreeMap<String, FakeAccount> = BTreeMap::new();
    let mut plan = Vec::new();
    for (i, info) in ledger.open_venues.iter().enumerate() {
        assert_eq!(info.last_venue, Some(Venue::PumpAmm));
        let pool = info.pool.unwrap();
        assert!(pool.fee.is_some(), "event fee bps observed for every pool");
        let i64_ = i64::try_from(i).unwrap();
        let (bv, qv) = (
            pk(0xB0u8.wrapping_add(u8::try_from(2 * i).unwrap())),
            pk(0xB1u8.wrapping_add(u8::try_from(2 * i).unwrap())),
        );
        let base_balance =
            u64::try_from(info.open_amount_raw).unwrap() * (3 + u64::try_from(i).unwrap());
        let quote_balance = 1_000_000_000u64 * (u64::try_from(i).unwrap() + 1);
        let vq = i128::from(i64_) * 17;
        accounts.insert(
            b58(&pool.address),
            pool_account(info.mint, WRAPPED_SOL_MINT, bv, qv, vq),
        );
        accounts.insert(b58(&bv), token_account(info.mint, base_balance));
        accounts.insert(b58(&qv), token_account(WRAPPED_SOL_MINT, quote_balance));
        plan.push((base_balance, quote_balance, vq));
    }
    let server = chain_server(accounts).await;
    let p = provider(&server);
    let mut cards = vec![card(ledger, 77)];
    cards[0].wallet = pubkey(PUMPSWAP_WALLET);
    let run = apply_open_valuation(&mut cards, &p, &AnalysisWindow::none(AS_OF)).await;
    // One account pass + one vault pass, counted in the budget.
    assert_eq!(run.account_calls, 2);
    assert_eq!(p.total_requests_made(), 2);
    assert_eq!(
        (run.state_slot, run.state_slot_min),
        (Some(7001), Some(7000))
    );
    assert_eq!(run.totals.valued, run.totals.positions);

    let l = cards[0].ledger.as_ref().unwrap();
    let view = l.open_valuation.as_ref().unwrap();
    for ((info, pos), (base_balance, quote_balance, vq)) in
        l.open_venues.iter().zip(&view.positions).zip(plan)
    {
        let v = pos.valued().unwrap();
        let fee = match info.pool.unwrap().fee.unwrap() {
            scout_engine::FeeObservation::Amm {
                lp_bps,
                protocol_bps,
                creator_bps,
            } => (lp_bps, protocol_bps, creator_bps),
            scout_engine::FeeObservation::Curve { .. } => panic!("curve fee on a pool"),
        };
        let q = amm_sell_quote(
            u64::try_from(info.open_amount_raw).unwrap(),
            base_balance,
            effective_quote_reserve(quote_balance, vq).unwrap(),
            fee.0,
            fee.1,
            fee.2,
        )
        .unwrap();
        assert_eq!(v.realizable_lamports, q.net);
        assert_eq!(v.gross_lamports, q.raw_out);
        assert_eq!((v.account_slot, v.vault_slot), (7000, Some(7001)));
        assert_eq!(v.venue, Venue::PumpAmm);
        // Open amount is sold in full into a pool of 3x+ the amount: a
        // visible, non-zero price impact, always below the spot value.
        assert!(v.gross_lamports <= v.marginal_lamports);
        assert!(v.price_impact_bps.is_some());
        assert_eq!(v.unrealized_pnl.is_some(), info.basis.fully_known_sol);
    }
}

#[tokio::test]
async fn ranking_keys_never_read_unrealized_values_and_require_valued_open_excludes() {
    // Wallet 1: valued open position. Wallet 2: unvalued (no fee observation).
    let l1 = curve_ledger(1, 10, true);
    let l2 = curve_ledger(2, 20, false);
    let curve1 = b58(&l1.open_venues[0].curve.unwrap().address);
    let accounts = BTreeMap::from([(curve1, curve_account(false, 2_000_000_000, 1_000_000))]);
    let server = chain_server(accounts).await;
    let mut cards = vec![card(l1, 1), card(l2, 2)];
    // Rank keys are realized-only: build the reference BEFORE valuation.
    let mut policy = RankPolicy::for_profile(RankProfile::None, RankBy::RealizedNetPnl, 10);
    let before = rank_solana_wallets(&cards, &policy);
    apply_open_valuation(&mut cards, &provider(&server), &AnalysisWindow::none(AS_OF)).await;
    let after = rank_solana_wallets(&cards, &policy);
    let keys = |r: &scout_engine::WalletRankReport| {
        let mut v: Vec<_> = r
            .ranked
            .iter()
            .map(|x| (x.observation.wallet, x.observation.key_net, x.rank))
            .collect();
        v.extend(
            r.excluded
                .iter()
                .map(|x| (x.observation.wallet, x.observation.key_net, 0)),
        );
        v.sort();
        v
    };
    assert_eq!(keys(&before), keys(&after));
    // Exposure labels: wallet 1 valued, wallet 2 unvalued (reason in totals).
    let label = |r: &scout_engine::WalletRankReport, w: u8| {
        r.ranked
            .iter()
            .map(|x| &x.observation)
            .chain(r.excluded.iter().map(|x| &x.observation))
            .find(|o| o.wallet == pk(w))
            .unwrap()
            .open_exposure
            .label()
    };
    assert_eq!(label(&before, 1), "unvalued");
    assert_eq!(label(&after, 1), "valued");
    assert_eq!(label(&after, 2), "unvalued");

    policy.require_valued_open = true;
    let strict = rank_solana_wallets(&cards, &policy);
    let reasons = |w: u8| {
        strict
            .excluded
            .iter()
            .find(|e| e.observation.wallet == pk(w))
            .map(|e| e.reasons.clone())
    };
    // Both wallets lack closed episodes (MetricUnknown under profile none);
    // only the unvalued one gets the open-exposure reason.
    assert_eq!(reasons(1), Some(vec![ExclusionReason::MetricUnknown]));
    assert_eq!(
        reasons(2),
        Some(vec![
            ExclusionReason::MetricUnknown,
            ExclusionReason::OpenExposureUnvalued
        ])
    );
    // Documented order: open_exposure_unvalued sits right after open_exposure.
    assert!(ExclusionReason::OpenExposure < ExclusionReason::OpenExposureUnvalued);
    assert!(ExclusionReason::OpenExposureUnvalued < ExclusionReason::BelowTopN);
    assert_eq!(
        ExclusionReason::OpenExposureUnvalued.label(),
        "open_exposure_unvalued"
    );
}
