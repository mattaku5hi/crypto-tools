//! End-to-end engine runs over the committed live BASE fixture through the
//! real RPC client/scanner against a wiremock replay (ADR-020 amendment 4):
//! wallet-stats listed through `alchemy_getAssetTransfers`.
#![allow(
    clippy::as_conversions,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::collections::BTreeMap;

use alloy_primitives::Address;
use scout_engine::{
    AnalysisWindow, EvmExtractionConfig, EvmStatsSources, QuoteUnit, TradeSide, WalletScanStatus,
    run_evm_wallet_stats,
};
use scout_evm::BASE;
use scout_providers::evm_replay::AlchemyInternalMode;
use scout_providers::{
    AlchemyConfig, AlchemyTransfersSource, EvmHistoryScanner, NativeLegPolicy, NativeLegResolver,
    ScanLimits, WalletIndexer,
};
use tokio_util::sync::CancellationToken;

#[path = "support/evm_base.rs"]
mod evm_base;

fn window() -> AnalysisWindow {
    AnalysisWindow::resolve(
        None,
        Some("2026-10-04T03:06:51Z"),
        Some("2026-10-04T03:07:07Z"),
        evm_base::AS_OF,
    )
    .unwrap()
}

/// Independent per-token net of ERC-20 `Transfer` deltas of `wallet` over all
/// recorded block receipts: token -> signed raw amount.
fn net_flows(fixture: &serde_json::Value, wallet: &str) -> BTreeMap<String, i128> {
    const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
    let pad = format!("0x{:0>64}", wallet.trim_start_matches("0x"));
    let mut m: BTreeMap<String, i128> = BTreeMap::new();
    for c in fixture["calls"].as_array().unwrap() {
        if c["method"] != "eth_getBlockReceipts" {
            continue;
        }
        for r in c["result"].as_array().unwrap() {
            for l in r["logs"].as_array().unwrap() {
                let t = l["topics"].as_array().unwrap();
                if t.len() != 3 || t[0] != TRANSFER {
                    continue;
                }
                let amt = i128::from_str_radix(&l["data"].as_str().unwrap()[34..66], 16).unwrap();
                let tok = l["address"].as_str().unwrap().to_ascii_lowercase();
                if t[2].as_str().unwrap().eq_ignore_ascii_case(&pad) {
                    *m.entry(tok.clone()).or_default() += amt;
                }
                if t[1].as_str().unwrap().eq_ignore_ascii_case(&pad) {
                    *m.entry(tok).or_default() -= amt;
                }
            }
        }
    }
    m
}

const USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
/// USDC-quoted buy + sell of one token (Uniswap v3), all ERC-20 legs.
const W_USDC: &str = "0xb0b21cef6df3cc3716193fd94880b58c2adb90b7";
/// ETH-quoted Uniswap v3 buy (tx.value exact).
const W_BUY: &str = "0x81ef037c407f0a4076a5db10700d04659a127a2b";
/// Native-ETH sell on Uniswap v2 style: proceeds arrive as an internal
/// transfer.
const W_SELL: &str = "0x3484978c2680823516c6f409ff736180ccf62dfc";
const SELL_TX: &str = "0xde0004f147dd18c56d7f45975c2277132029973f9d774e8888752a76ab7e4e71";
/// Uniswap v4 on Base is IdlOnly: buy + sell are booked but the wallet is
/// Incomplete.
const W_V4: &str = "0x321b36daf6a6001e07415200149caf906f28f34b";

struct Run {
    report: scout_engine::SolanaWalletStatsReport,
    calls: BTreeMap<String, u64>,
    alchemy_calls: usize,
}

async fn run(internal: AlchemyInternalMode, page: Option<usize>, wallets: &[&str]) -> Run {
    run_with(internal, page, false, wallets).await
}

async fn run_with(
    internal: AlchemyInternalMode,
    page: Option<usize>,
    drop_zero_external: bool,
    wallets: &[&str],
) -> Run {
    let fixture = evm_base::fixture();
    let mut replay = evm_base::replay(&fixture, internal);
    if let Some(p) = page {
        replay = replay.with_alchemy_page_size(p);
    }
    if drop_zero_external {
        replay = replay.with_alchemy_zero_external_dropped();
    }
    let server = evm_base::serve(replay).await;
    let rpc = evm_base::rpc_client(&server);
    let chain = rpc.preflight().await.unwrap();
    let scanner = EvmHistoryScanner::new(rpc.clone(), chain, ScanLimits::default());
    let indexer: WalletIndexer =
        AlchemyTransfersSource::new(rpc.clone(), AlchemyConfig::default()).into();
    let resolver = NativeLegResolver::new(rpc, NativeLegPolicy::default());
    let cfg = EvmExtractionConfig::for_profile(BASE);
    let wallets: Vec<Address> = wallets.iter().map(|w| w.parse().unwrap()).collect();
    let report = run_evm_wallet_stats(
        &cfg,
        &EvmStatsSources {
            scanner: &scanner,
            explorer: &indexer,
            max_requests: None,
            notice: None,
            resolver: Some(&resolver),
        },
        &wallets,
        &window(),
        2,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let alchemy_calls = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("alchemy_getAssetTransfers"))
        .count();
    Run {
        report,
        calls: scanner.rpc().calls_by_method(),
        alchemy_calls,
    }
}

#[tokio::test]
async fn base_wallet_stats_over_the_fixture_usdc_and_eth_wallets() {
    let fixture = evm_base::fixture();
    let r = run(
        AlchemyInternalMode::Unsupported,
        None,
        &[W_USDC, W_BUY, W_SELL, W_V4],
    )
    .await;
    let info = r.report.evm.as_ref().unwrap();
    assert_eq!(info.chain.name, "base");
    // Only the v4 wallet (IdlOnly on Base) is flagged.
    assert_eq!(r.report.wallets.len(), 4);
    assert_eq!(
        r.report
            .wallets
            .iter()
            .filter(|w| !w.coverage_complete())
            .count(),
        1
    );
    // Request accounting: per wallet 2 transfer calls (external+erc20 from,
    // to) = 8, plus the internal call(s) answered "unsupported": the first
    // one is cached for the chain, concurrent wallets may race it (1..=4).
    assert_eq!(
        u64::try_from(r.alchemy_calls).unwrap(),
        r.calls["alchemy_getAssetTransfers"]
    );
    assert!((9..=12).contains(&r.alchemy_calls), "{:?}", r.calls);
    assert_eq!(r.calls.get("eth_getLogs"), None);

    // --- Wallet 1: USDC-quoted buy then sell of one token.
    let w = &r.report.wallets[0];
    assert_eq!(w.status, WalletScanStatus::Ok);
    let l = w.ledger.as_ref().unwrap();
    assert_eq!((l.trades.buys, l.trades.sells), (1, 1));
    let ev = l.evm.as_ref().unwrap();
    assert_eq!(ev.trades.len(), 2);
    for t in &ev.trades {
        assert_eq!(t.unit, QuoteUnit::UsdcUnits);
        assert_eq!(t.native_leg, "not_involved");
        assert_eq!(t.venue, "uniswap_v3");
    }
    let (buy, sell) = (&ev.trades[0], &ev.trades[1]);
    assert_eq!((buy.side, sell.side), (TradeSide::Buy, TradeSide::Sell));
    assert_eq!(buy.quote_amount, Some(706_071_300));
    assert_eq!(sell.quote_amount, Some(461_676_345));
    // Independent check from the raw receipts: net flows of the wallet.
    let net = net_flows(&fixture, W_USDC);
    assert_eq!(net[USDC], -(706_071_300 - 461_676_345));
    let token = format!("{:#x}", buy.token);
    assert_eq!(net[&token], 12_189_221_686_084_042_750_894);
    assert_eq!(l.open_positions.len(), 1);
    assert_eq!(
        l.open_positions[0].open_amount_raw,
        12_189_221_686_084_042_750_894
    );
    // Fees (gas incl. the Base l1Fee) of USDC-quoted trades are not mixed
    // into the USDC basis: recorded, uncapitalized.
    assert_eq!(buy.fee_wei, Some(1_004_180_309_453));
    assert_eq!(
        ev.uncapitalized_gas_wei,
        1_004_180_309_453 + 934_458_753_195
    );
    let b = l.unit_block(QuoteUnit::UsdcUnits).unwrap();
    // FIFO proration floors: consumed basis = floor(706071300 * sold /
    // bought) = 461_767_164 (independent big-integer computation), so the
    // known disposal inside the still-open episode is 461_676_345 -
    // 461_767_164 = -90_819 USDC units (6 dp). Nothing is closed.
    assert_eq!(b.open_episode_known_disposal_pnl_raw, -90_819);
    assert_eq!((b.closed_episodes_known, b.realized_trade_pnl_raw), (0, 0));
    assert!(
        l.unit_block(QuoteUnit::Wei)
            .unwrap()
            .consumed_acquisition_basis_raw
            == 0
    );

    // --- Wallet 2: ETH-quoted buy: exact tx.value, gas incl. l1Fee capitalized.
    let w = &r.report.wallets[1];
    assert_eq!(w.status, WalletScanStatus::Ok);
    let l = w.ledger.as_ref().unwrap();
    let ev = l.evm.as_ref().unwrap();
    assert_eq!((l.trades.buys, l.trades.sells), (1, 0));
    let t = &ev.trades[0];
    assert_eq!((t.unit, t.venue), (QuoteUnit::Wei, "uniswap_v3"));
    assert_eq!(t.quote_amount, Some(123_000_000_000));
    assert_eq!(t.fee_wei, Some(826_679_604_549));
    assert_eq!(t.token_amount, 2_624_647_480_136_581_162);
    assert_eq!(t.native_leg, "logs_and_value_only");
    let b = l.unit_block(QuoteUnit::Wei).unwrap();
    assert_eq!(
        l.open_positions[0].open_amount_raw,
        2_624_647_480_136_581_162
    );
    let _ = b;

    // --- Wallet 3: native sell, proceeds not observed (no trace, no archive
    // state, no internal category): Unknown, never zero.
    let w = &r.report.wallets[2];
    assert_eq!(w.status, WalletScanStatus::Ok);
    let l = w.ledger.as_ref().unwrap();
    let ev = l.evm.as_ref().unwrap();
    assert_eq!((l.trades.buys, l.trades.sells), (0, 1));
    assert_eq!(ev.trades[0].quote_amount, None);
    assert_eq!(l.trades.native_leg_not_observed, 1);
    assert_eq!(format!("{:#x}", ev.trades[0].tx_hash), SELL_TX);
    assert!(info.trace.starts_with("unsupported"), "{}", info.trace);
    assert_eq!(info.listing_kind, "alchemy_transfers");
    assert_eq!(info.coverage_notes.len(), 1);
    assert!(info.coverage_notes[0].contains("failed-transaction"));

    // --- Wallet 4: v4 is IdlOnly on Base => booked but flagged Incomplete.
    let w = &r.report.wallets[3];
    assert_eq!(w.status, WalletScanStatus::Incomplete);
    assert!(w.incomplete_reasons.iter().any(|x| x.contains("IdlOnly")));
    assert_eq!(w.ledger.as_ref().unwrap().trades.idl_only_variant, 2);
    println!(
        "base replay: wallets=4 alchemy_calls={} calls={:?} native_legs={:?}",
        r.alchemy_calls, r.calls, info.native_leg_counts
    );
}

#[tokio::test]
async fn paginated_listing_gives_identical_cards() {
    let big = run(
        AlchemyInternalMode::Unsupported,
        None,
        &[W_USDC, W_BUY, W_SELL, W_V4],
    )
    .await;
    // Pages of 1 row force pageKey pagination in every stream.
    let small = run(
        AlchemyInternalMode::Unsupported,
        Some(1),
        &[W_USDC, W_BUY, W_SELL, W_V4],
    )
    .await;
    assert!(small.alchemy_calls > big.alchemy_calls);
    for (a, b) in big.report.wallets.iter().zip(&small.report.wallets) {
        let (la, lb) = (a.ledger.as_ref().unwrap(), b.ledger.as_ref().unwrap());
        assert_eq!(
            la.evm.as_ref().unwrap().trades,
            lb.evm.as_ref().unwrap().trades
        );
        assert_eq!(la.open_positions.len(), lb.open_positions.len());
        assert_eq!(a.status, b.status);
    }
}

/// SYNTHETIC internal transfer on the real Base data: Alchemy's `internal`
/// category is supported (as on the real Base endpoint) and the router
/// forwards a scripted amount to the seller. The figure is conditional on
/// that assumption; the point proven is the pipeline: the native sell turns
/// exact, labelled `alchemy_internal`, with no archive/trace request.
#[tokio::test]
async fn base_internal_transfers_from_alchemy_make_the_native_sell_exact() {
    let fixture = evm_base::fixture();
    // The sell tx of W_ETH, from the earlier run's audit trail.
    let probe = run(AlchemyInternalMode::Unsupported, None, &[W_SELL]).await;
    let sell = probe.report.wallets[0]
        .ledger
        .as_ref()
        .unwrap()
        .evm
        .as_ref()
        .unwrap()
        .trades
        .iter()
        .find(|t| t.side == TradeSide::Sell)
        .unwrap()
        .clone();
    assert_eq!(format!("{:#x}", sell.tx_hash), SELL_TX);
    let router = "0x6ff5693b99212da76ad316178a184ab56d299b43";
    let row = scout_providers::evm_replay::AlchemyReplayRow {
        category: "internal",
        hash: format!("{:#x}", sell.tx_hash),
        block: sell.block_number,
        time: Some(sell.block_time),
        from: router.to_string(),
        to: Some(W_SELL.to_string()),
        contract: None,
        raw_value: "0x2386f26fc10000".to_string(), // 0.01 ETH exactly
        unique_id: format!("{:#x}:internal:0", sell.tx_hash),
    };
    let r = run(AlchemyInternalMode::Supported(vec![row]), None, &[W_SELL]).await;
    let l = r.report.wallets[0].ledger.as_ref().unwrap();
    let ev = l.evm.as_ref().unwrap();
    let s = ev
        .trades
        .iter()
        .find(|t| t.side == TradeSide::Sell)
        .unwrap();
    assert_eq!(s.native_leg, "alchemy_internal");
    assert_eq!(s.quote_amount, Some(10_000_000_000_000_000));
    assert_eq!(l.trades.native_leg_not_observed, 0);
    // Internal supported: 2 transfer calls + 2 internal calls, no probes of
    // trace/archive state needed (nothing left unobserved).
    assert_eq!(r.calls.get("alchemy_getAssetTransfers"), Some(&4));
    assert_eq!(r.calls.get("debug_traceTransaction"), None);
    assert_eq!(r.calls.get("eth_getBalance"), None);
    assert_eq!(ev.native_leg_counts.get("alchemy_internal"), Some(&1));
    let _ = fixture;
}

/// A signed zero-ETH sell has no `external` row (an indexer that skips
/// zero-value rows): the signer is then learned from the receipt, and
/// `tx.value`/gas price from ONE `eth_getTransactionByHash`, counted in the
/// plan. Same card as with the external row.
#[tokio::test]
async fn a_signed_sell_without_an_external_row_costs_one_transaction_lookup() {
    let with = run(AlchemyInternalMode::Unsupported, None, &[W_SELL]).await;
    assert_eq!(with.calls.get("eth_getTransactionByHash"), None);
    let without = run_with(AlchemyInternalMode::Unsupported, None, true, &[W_SELL]).await;
    assert_eq!(without.calls.get("eth_getTransactionByHash"), Some(&1));
    let tr = |r: &Run| {
        r.report.wallets[0]
            .ledger
            .as_ref()
            .unwrap()
            .evm
            .as_ref()
            .unwrap()
            .trades
            .clone()
    };
    assert_eq!(tr(&with), tr(&without));
    assert_eq!(without.report.wallets[0].status, WalletScanStatus::Ok);
}

mod intersect {
    use super::*;
    use scout_core::{AddressBytes, AssetKey};
    use scout_engine::{EvmRunInfo, SideFilter, TokenScanStatus, run_evm_buyer_intersect};

    /// Tokens with many Transfers in the 8 receipt blocks (not USDC/WETH).
    const TOKEN_A: &str = "0x07b3d902783c3c12b077508c3b5c00113d1291d0";
    const TOKEN_B: &str = "0xacfe6019ed1a7dc6f7b508c02d1b04ec88cc21bf";

    /// Window = the 8 receipt blocks +-20 blocks of margin: 48 blocks, so a
    /// 10-block logs cap needs ceil(48 / 10) = 5 requests per token.
    fn wide_window() -> AnalysisWindow {
        AnalysisWindow::resolve(
            None,
            Some("2026-10-04T03:06:11Z"),
            Some("2026-10-04T03:07:47Z"),
            evm_base::AS_OF,
        )
        .unwrap()
    }

    fn asset(chain: &scout_core::ChainKey, a: &str) -> AssetKey {
        AssetKey::Token(
            chain.clone(),
            AddressBytes::Evm(a.parse::<Address>().unwrap().into_array()),
        )
    }

    /// Independent counts from the raw receipts: (Transfer logs, distinct txs).
    fn raw_counts(fixture: &serde_json::Value, token: &str) -> (usize, usize) {
        const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
        let (mut logs, mut txs) = (0usize, std::collections::BTreeSet::new());
        for c in fixture["calls"].as_array().unwrap() {
            if c["method"] != "eth_getBlockReceipts" {
                continue;
            }
            for r in c["result"].as_array().unwrap() {
                for l in r["logs"].as_array().unwrap() {
                    if l["address"].as_str().unwrap() == token
                        && l["topics"][0] == TRANSFER
                        && l["topics"].as_array().unwrap().len() == 3
                    {
                        logs += 1;
                        txs.insert(r["transactionHash"].as_str().unwrap().to_string());
                    }
                }
            }
        }
        (logs, txs.len())
    }

    #[tokio::test]
    async fn buyer_intersect_on_base_with_a_10_block_logs_cap() {
        let fixture = evm_base::fixture();
        let replay = evm_base::replay_capped(&fixture, AlchemyInternalMode::Unsupported, Some(10));
        let server = evm_base::serve(replay).await;
        let rpc = evm_base::rpc_client(&server);
        let chain = rpc.preflight().await.unwrap();
        let scanner = EvmHistoryScanner::new(rpc.clone(), chain.clone(), ScanLimits::default());
        let cfg = EvmExtractionConfig::for_profile(BASE);
        let info = EvmRunInfo::from_config(&cfg, "eth_getLogs token scan + RPC receipts");
        let tokens = [asset(&chain, TOKEN_A), asset(&chain, TOKEN_B)];
        let report = run_evm_buyer_intersect(
            &cfg,
            &scanner,
            &tokens,
            1,
            SideFilter::Any,
            &wide_window(),
            info,
        )
        .await
        .unwrap();
        let calls = scanner.rpc().calls_by_method();
        for (i, tok) in [TOKEN_A, TOKEN_B].iter().enumerate() {
            let t = &report.per_token[i];
            assert!(matches!(t.status, TokenScanStatus::Ok), "{:?}", t.status);
            let (logs, txs) = raw_counts(&fixture, tok);
            assert_eq!(
                (t.transfer_logs, t.transactions_scanned),
                (Some(logs as u64), Some(txs as u64)),
                "{tok}"
            );
        }
        // Every eth_getLogs request obeyed the 10-block cap: the replay
        // answers wider ranges with the provider's error, which the scanner
        // follows via the suggested range. 2 tokens x 5 spans (48 blocks) plus
        // at most one rejected first attempt per token.
        let n_logs = calls.get("eth_getLogs").copied().unwrap();
        assert!((10..=12).contains(&n_logs), "eth_getLogs calls {n_logs}");
        let a = &report.per_token[0];
        println!(
            "base intersect: logs_calls={n_logs} A: logs={:?} txs={:?} buyers={} sellers={} \
             wallets_k1={} | calls={calls:?}",
            a.transfer_logs,
            a.transactions_scanned,
            a.qualified_buyers,
            a.qualified_sellers,
            report.base.matches.len()
        );
        // Recorded replay numbers (token A): 42 signers bought, 41 sold.
        assert_eq!((a.qualified_buyers, a.qualified_sellers), (42, 41));
        assert_eq!(report.base.matches.len(), 70);
        assert_eq!(a.ungated_swap_logs, 0, "all pools of the fixture admitted");
    }
}
