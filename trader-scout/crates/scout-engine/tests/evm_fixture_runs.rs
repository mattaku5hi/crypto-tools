//! End-to-end engine runs over the committed live Robinhood fixture
//! (`evm_robinhood_token_aiden_v4_2026-10-03.json`) through the real RPC
//! client/scanner against a wiremock replay: token-centric buyer-intersect
//! and wallet-stats for two wallets of the fixture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use alloy_primitives::{Address, address};
use scout_core::{AddressBytes, AssetKey};
use scout_engine::{
    AnalysisWindow, EvmExtractionConfig, EvmRunInfo, SideFilter, TokenScanStatus,
    run_evm_buyer_intersect,
};
use scout_evm::ROBINHOOD;
use scout_providers::evm_replay::{EvmFixtureReplay, ReplayReply};
use scout_providers::{EvmHistoryScanner, ScanLimits};

#[path = "support/evm_aiden.rs"]
mod evm_aiden;
use evm_aiden::AIDEN;

const OTHER_TOKEN: Address = address!("00000000000000000000000000000000000000c9");

fn window() -> AnalysisWindow {
    AnalysisWindow::resolve(
        None,
        Some("2026-10-03T17:54:46Z"),
        Some("2026-10-03T18:04:46Z"),
        1_791_060_000,
    )
    .unwrap()
}

/// The recorded fixture plus: any `eth_getLogs` of another token is empty.
fn replay_with_empty_other_token() -> EvmFixtureReplay {
    evm_aiden::replay().with_handler(Box::new(|method, params| {
        if method == "eth_getLogs" {
            let addr = params[0]["address"].as_str().unwrap_or("");
            if addr.eq_ignore_ascii_case(&format!("{OTHER_TOKEN:#x}")) {
                return Some(ReplayReply::Result(serde_json::json!([])));
            }
        }
        None
    }))
}

fn token_asset(chain: &scout_core::ChainKey, t: Address) -> AssetKey {
    AssetKey::Token(chain.clone(), AddressBytes::Evm(t.into_array()))
}

#[tokio::test]
async fn buyer_intersect_over_the_aiden_window_reports_fixture_numbers() {
    let server = evm_aiden::serve(replay_with_empty_other_token()).await;
    let rpc = evm_aiden::rpc_client(&server);
    let chain = rpc.preflight().await.unwrap();
    let scanner = EvmHistoryScanner::new(rpc, chain.clone(), ScanLimits::default());
    let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
    let info = EvmRunInfo::from_config(&cfg, "eth_getLogs token scan + RPC receipts");
    let tokens = [token_asset(&chain, AIDEN), token_asset(&chain, OTHER_TOKEN)];
    let report =
        run_evm_buyer_intersect(&cfg, &scanner, &tokens, 1, SideFilter::Any, &window(), info)
            .await
            .unwrap();
    let t = &report.per_token[0];
    assert!(matches!(t.status, TokenScanStatus::Ok));
    // 278 Transfer logs in 48 transactions; 45 v4 swaps, 44 booked trades.
    assert_eq!(
        (t.transfer_logs, t.transactions_scanned),
        (Some(278), Some(48))
    );
    let ex = t.extraction.as_ref().unwrap();
    assert_eq!(
        (ex.transactions, ex.trades, ex.unknown_consideration),
        (48, 44, 19)
    );
    assert_eq!(ex.nft_transfer_logs, 2);
    // 22 distinct signers bought, 10 of them (also) sold; every seller bought.
    assert_eq!(
        (t.qualified_buyers, t.qualified_sellers, t.qualified_wallets),
        (22, 10, 22)
    );
    assert_eq!((t.idl_only_trades, t.ungated_swap_logs), (0, 0));
    // The second input token has no Transfer in the window: scanned, empty.
    let t1 = &report.per_token[1];
    assert!(matches!(t1.status, TokenScanStatus::Ok));
    assert_eq!(
        (t1.transfer_logs, t1.transactions_scanned),
        (Some(0), Some(0))
    );
    // K = 1 over `any`: the 22 signers match, sorted by hit_count then wallet.
    assert_eq!(report.base.matches.len(), 22);
    assert!(report.base.matches.iter().all(|m| m.hit_count == 1));
    assert!(
        report.incomplete_reasons().is_empty(),
        "complete within scope"
    );
    // Side evidence: lowest (block, tx index) per side, scan-order independent.
    let first = report.base.matches.first().unwrap();
    let hits = &report.side_hits[&first.wallet][&tokens[0]];
    assert!(hits.buy.is_some());
    assert_eq!(hits.buy.as_ref().unwrap().venue, "uniswap_v4");
    // `--side sell` keeps exactly the 10 sellers; `--side buy` the 22 buyers.
    for (side, expect) in [(SideFilter::Sell, 10usize), (SideFilter::Buy, 22)] {
        let info = EvmRunInfo::from_config(&cfg, "x");
        let r = run_evm_buyer_intersect(&cfg, &scanner, &tokens, 1, side, &window(), info)
            .await
            .unwrap();
        assert_eq!(r.base.matches.len(), expect, "{side:?}");
    }
}

mod stats {
    use super::*;
    use scout_engine::{EvmStatsSources, QuoteUnit, WalletScanStatus, run_evm_wallet_stats};
    use scout_providers::evm_replay::ExplorerReplay;
    use scout_providers::{
        BlockscoutApiKey, BlockscoutEvmConfig, BlockscoutEvmSource, NativeLegPolicy,
        NativeLegResolver, WalletIndexer,
    };
    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer};

    pub async fn explorer(server_fixture: &serde_json::Value) -> (MockServer, WalletIndexer) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ExplorerReplay::from_fixture(server_fixture))
            .mount(&s)
            .await;
        let mut cfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("test-key"));
        cfg.base_url = s.uri();
        cfg.base_delay_ms = 1;
        (s, BlockscoutEvmSource::new(cfg).unwrap().into())
    }

    #[tokio::test]
    async fn two_wallets_of_the_fixture_through_wallet_stats() {
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evm_aiden::fixture_path()).unwrap())
                .unwrap();
        let senders = ExplorerReplay::from_fixture(&fixture).senders();
        let server = evm_aiden::serve(evm_aiden::replay()).await;
        let rpc = evm_aiden::rpc_client(&server);
        let chain = rpc.preflight().await.unwrap();
        let scanner = EvmHistoryScanner::new(rpc.clone(), chain, ScanLimits::default());
        let (_ex_server, explorer) = explorer(&fixture).await;
        let resolver = NativeLegResolver::new(rpc, NativeLegPolicy::default());
        let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
        let wallets: Vec<Address> = senders
            .iter()
            .take(2)
            .map(|(a, _)| a.parse().unwrap())
            .collect();
        let report = run_evm_wallet_stats(
            &cfg,
            &EvmStatsSources {
                scanner: &scanner,
                explorer: &explorer,
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
        let info = report.evm.as_ref().unwrap();
        assert!(info.trace.starts_with("unsupported"), "{}", info.trace);
        assert!(
            info.archive_state.starts_with("unsupported"),
            "{}",
            info.archive_state
        );
        assert_eq!(info.native_leg_counts.get("logs_and_value_only"), Some(&9));
        // Request counts BY METHOD (logical calls) for the two wallets: the
        // explorer rows describe every signed tx, so no transaction is
        // fetched; at most one receipt request per transaction; the only
        // native-leg calls are the one-time capability probes (trace 1,
        // archive balance 1) because both sources are unsupported here.
        let n_txs: usize = senders.iter().take(2).map(|(_, n)| *n).sum();
        let calls = scanner.rpc().calls_by_method();
        let c = |m: &str| calls.get(m).copied().unwrap_or(0);
        assert_eq!(c("eth_getTransactionByHash"), 0, "{calls:?}");
        let receipt_requests = c("eth_getTransactionReceipt") + c("eth_getBlockReceipts");
        assert!(
            receipt_requests >= 1 && receipt_requests <= u64::try_from(n_txs).unwrap(),
            "{calls:?} for {n_txs} txs"
        );
        assert_eq!((c("debug_traceTransaction"), c("eth_getBalance")), (1, 1));
        assert_eq!(c("eth_getLogs"), 0);
        assert_eq!(report.wallets.len(), 2);
        assert!(!report.is_coverage_incomplete());

        // (wallet, buys, sells, open amount raw): the open amount is the
        // independent sum of the wallet's own Aiden Transfer deltas in the
        // fixture (18-decimal amounts, > u64).
        let expect = [
            (wallets[0], 2u64, 3u64, 46_472_032_518_329_289_554_142u128),
            (wallets[1], 1u64, 3u64, 15_160_018_389_976_913_268_698u128),
        ];
        for (w, (addr, buys, sells, open)) in report.wallets.iter().zip(expect) {
            assert_eq!(w.chain.address(&w.wallet), format!("{addr:#x}"));
            assert_eq!(w.status, WalletScanStatus::Ok);
            let l = w.ledger.as_ref().unwrap();
            assert_eq!((l.trades.buys, l.trades.sells), (buys, sells));
            // Every sell is a native-ETH sell with unobserved proceeds: Unknown,
            // never zero. Nothing closed, one open episode, no known PnL.
            assert_eq!(l.trades.native_leg_not_observed, sells);
            assert_eq!(l.diagnostics.unknown_disposals, sells);
            assert_eq!((l.closed_episodes_known, l.open_episodes), (0, 1));
            assert_eq!(l.open_positions.len(), 1);
            assert_eq!(l.open_positions[0].open_amount_raw, open);
            let b = l.unit_block(QuoteUnit::Wei).unwrap();
            assert_eq!(b.realized_trade_pnl_raw, 0);
            assert_eq!(b.closed_episodes_known, 0, "no PnL is claimed");
            let ev = l.evm.as_ref().unwrap();
            assert_eq!(ev.trades.len(), usize::try_from(buys + sells).unwrap());
            assert!(
                ev.trades
                    .iter()
                    .all(|t| t.native_leg == "logs_and_value_only")
            );
            for t in &ev.trades {
                match t.side {
                    scout_engine::TradeSide::Sell => assert_eq!(t.quote_amount, None),
                    scout_engine::TradeSide::Buy => assert!(t.quote_amount.is_some()),
                }
            }
        }
        // Hand-checked first buy of wallet 0x4e40ce...: pays tx.value
        // 0.07425 ETH exactly and 3_285_566_928_000 wei of gas.
        let first = &report.wallets[0]
            .ledger
            .as_ref()
            .unwrap()
            .evm
            .as_ref()
            .unwrap()
            .trades[0];
        assert_eq!(first.quote_amount, Some(74_250_000_000_000_000));
        assert_eq!(first.fee_wei, Some(3_285_566_928_000));
        assert_eq!(first.token_amount, 4_446_129_077_508_853_808_783_222);
    }

    /// SYNTHETIC trace source on the real data: a node that serves
    /// `debug_traceTransaction` where the router forwards exactly the pool
    /// event's `amount0` to the signer. (The real Robinhood RPC has no trace
    /// source; this proves the pipeline, the figures are conditional on that
    /// assumption and are NOT claims about the real proceeds.) Expected
    /// numbers are an independent big-integer FIFO over the fixture.
    #[tokio::test]
    async fn a_trace_capable_node_turns_native_sells_exact_end_to_end() {
        use alloy_primitives::B256;
        use std::collections::HashMap;

        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evm_aiden::fixture_path()).unwrap())
                .unwrap();
        // tx hash -> (signer, amount0 of its v4 swap when positive).
        let mut proceeds: HashMap<String, (String, u128)> = HashMap::new();
        for c in fixture["calls"].as_array().unwrap() {
            if c["method"] == "eth_getBlockReceipts" {
                for r in c["result"].as_array().into_iter().flatten() {
                    for l in r["logs"].as_array().unwrap() {
                        if l["topics"][0]
                            == "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f"
                        {
                            let d = l["data"].as_str().unwrap();
                            let w0 = &d[2..66];
                            // amount0 > 0 (swapper receives ETH): top bit clear, non-zero.
                            if !w0.starts_with('f') && !w0.trim_start_matches('0').is_empty() {
                                let v = u128::from_str_radix(&w0[w0.len() - 32..], 16).unwrap();
                                proceeds.insert(
                                    r["transactionHash"].as_str().unwrap().to_string(),
                                    (r["from"].as_str().unwrap().to_string(), v),
                                );
                            }
                        }
                    }
                }
            }
        }
        let replay = evm_aiden::replay().with_handler(Box::new(move |method, params| {
            if method != "debug_traceTransaction" {
                return None;
            }
            let hash = params[0].as_str()?.to_string();
            let Some((signer, v)) = proceeds.get(&hash).cloned() else {
                // A buy: no internal value transfer (no refund).
                return Some(ReplayReply::Result(serde_json::json!({
                    "type": "CALL", "from": "0x0000000000000000000000000000000000000001",
                    "to": "0x0000000000000000000000000000000000000abc", "value": "0x0"
                })));
            };
            Some(ReplayReply::Result(serde_json::json!({
                "type": "CALL", "from": signer, "to": "0x0000000000000000000000000000000000000abc",
                "value": "0x0",
                "calls": [{"type": "CALL", "from": "0x0000000000000000000000000000000000000abc",
                           "to": signer, "value": format!("{v:#x}")}]
            })))
        }));
        let server = evm_aiden::serve(replay).await;
        let rpc = evm_aiden::rpc_client(&server);
        let chain = rpc.preflight().await.unwrap();
        let scanner = EvmHistoryScanner::new(rpc.clone(), chain, ScanLimits::default());
        let (_ex, explorer) = explorer(&fixture).await;
        let resolver = NativeLegResolver::new(rpc, NativeLegPolicy::default());
        let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
        let wallets: Vec<Address> = vec![
            "0x4e40ceacc9d16dad54f90daffd3a7291cacc0884"
                .parse()
                .unwrap(),
            "0x41deeacdcdfebc9f0f549b7cbb8269a6b21d805d"
                .parse()
                .unwrap(),
        ];
        let report = run_evm_wallet_stats(
            &cfg,
            &EvmStatsSources {
                scanner: &scanner,
                explorer: &explorer,
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
        let info = report.evm.as_ref().unwrap();
        assert_eq!(info.trace, "supported");
        assert!(info.archive_state.starts_with("unsupported"));
        // Only native-quoted SELLS are resolved (6 of the 9 native trades);
        // the 3 buys book tx.value and never cost a trace call.
        assert_eq!(info.native_leg_counts.get("trace"), Some(&6));
        assert_eq!(info.native_leg_counts.get("logs_and_value_only"), Some(&3));
        // (raw SUM B pnl of the known disposals inside the still-open episode,
        // remaining basis scaled): independent FIFO, see the module docs.
        let expect = [
            (
                16_276_621_058_357_939i128,
                77_879_179_939_541_613_702_348i128,
            ),
            (
                8_092_373_456_799_373i128,
                23_400_511_614_038_208_000_002i128,
            ),
        ];
        for (w, (pnl, basis_scaled)) in report.wallets.iter().zip(expect) {
            assert_eq!(w.status, WalletScanStatus::Ok);
            let l = w.ledger.as_ref().unwrap();
            assert_eq!(l.trades.native_leg_not_observed, 0);
            assert_eq!(l.diagnostics.known_disposals, 3);
            assert_eq!(l.diagnostics.unknown_disposals, 0);
            assert_eq!(l.open_episode_known_disposals, 3);
            assert_eq!(l.open_episode_known_disposal_pnl_lamports, pnl);
            assert_eq!(
                l.open_venues[0].basis.known_sol_basis.scaled_units(),
                basis_scaled
            );
            assert!(!l.has_unknown_basis_inventory);
            let ev = l.evm.as_ref().unwrap();
            // sells go through the trace; buys never needed it
            assert!(ev.trades.iter().all(|t| matches!(
                (format!("{:?}", t.side).as_str(), t.native_leg),
                ("Sell", "trace") | ("Buy", "logs_and_value_only")
            )));
            assert!(ev.trades.iter().all(|t| t.quote_amount.is_some()));
        }
        let _ = B256::ZERO;
    }

    #[tokio::test]
    async fn a_wallet_that_cannot_fit_the_budget_is_refused_before_any_receipt_request() {
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evm_aiden::fixture_path()).unwrap())
                .unwrap();
        let senders = ExplorerReplay::from_fixture(&fixture).senders();
        let server = evm_aiden::serve(evm_aiden::replay()).await;
        let rpc = evm_aiden::rpc_client(&server);
        let chain = rpc.preflight().await.unwrap();
        let scanner = EvmHistoryScanner::new(rpc.clone(), chain, ScanLimits::default());
        let (_ex_server, explorer) = explorer(&fixture).await;
        let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
        let wallets: Vec<Address> = senders
            .iter()
            .take(2)
            .map(|(a, _)| a.parse().unwrap())
            .collect();
        let notes = std::sync::Mutex::new(Vec::<String>::new());
        let say = |m: &str| notes.lock().unwrap().push(m.to_string());
        let report = run_evm_wallet_stats(
            &cfg,
            &EvmStatsSources {
                scanner: &scanner,
                explorer: &explorer,
                // the window resolution alone already used more than this
                max_requests: Some(1),
                notice: Some(&say),
                resolver: None,
            },
            &wallets,
            &window(),
            2,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(report.all_failed_for_budget());
        for w in &report.wallets {
            assert_eq!(w.status, WalletScanStatus::NotScanned);
            assert!(matches!(
                w.not_scanned,
                Some(scout_engine::ScanStop::BudgetExhausted { limit: 1 })
            ));
            assert!(w.incomplete_reasons[0].contains("refused before scanning"));
        }
        let calls = scanner.rpc().calls_by_method();
        for m in [
            "eth_getTransactionReceipt",
            "eth_getBlockReceipts",
            "eth_getTransactionByHash",
            "eth_getBalance",
        ] {
            assert_eq!(calls.get(m), None, "{m} must not be called: {calls:?}");
        }
        let notes = notes.lock().unwrap();
        assert!(
            notes
                .iter()
                .any(|n| n.contains("planned RPC requests: eth_getBalance(<=)=")
                    && n.contains("eth_getTransactionReceipt=")),
            "{notes:?}"
        );
    }
}
