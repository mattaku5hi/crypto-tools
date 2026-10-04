//! ADR-020 amendment 6: Binance-Peg USDT/USDC (18 dp, bridged) are pinned BSC
//! quote assets. Their trades are booked in their OWN units (never the 6-dp
//! `UsdtUnits`/`UsdcUnits`), exact in raw base units, and valued in USD with
//! the peg visible in the coverage. Synthetic transactions (hand-computed).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions
)]

use alloy_primitives::{Address, B256, Bytes, U256, address};
use scout_core::{EvmTxStatus, Money, RawEvmLog, RawEvmTransaction};
use scout_dex_evm::V4_SWAP_TOPIC0;
use scout_engine::{
    EvmExtractionConfig, LedgerOptions, QuoteUnit, UsdOutcome, build_evm_wallet_ledger,
    quote_asset_on, quote_unit_decimals, quote_unit_label,
};
use scout_evm::{BASE, BASE_USDC, BSC, BSC_USDC, BSC_USDT, TRANSFER_TOPIC0};
use scout_pricing::{Candle, DecimalPrice, InMemoryPriceSource, QuoteAsset};

/// BSC Uniswap v4 PoolManager (pinned deployment; any gated emitter works).
const PM: Address = address!("28e2ea090877bf75740558f6bfb36a5ffee9e9df");
const W: Address = address!("00000000000000000000000000000000000000a1");
const ROUTER: Address = address!("00000000000000000000000000000000000000b2");
const TOKEN_A: Address = address!("00000000000000000000000000000000000000c1");
const TOKEN_B: Address = address!("00000000000000000000000000000000000000c2");
const E18: u128 = 1_000_000_000_000_000_000;
/// Minute starts of the two fixed block times used below.
const M_BUY: i64 = 1_789_999_980;
const M_SELL: i64 = 1_790_000_040;

fn transfer_log(token: Address, from: Address, to: Address, v: u128, idx: u64) -> RawEvmLog {
    RawEvmLog {
        address: token,
        topics: vec![TRANSFER_TOPIC0, from.into_word(), to.into_word()],
        data: Bytes::from(U256::from(v).to_be_bytes::<32>().to_vec()),
        block_number: 1,
        transaction_index: 1,
        log_index: idx,
    }
}

/// `n` = 1 (buy minute) or 7 (sell minute): block time `1_790_000_000 + 10 n`.
fn tx(
    n: u64,
    quote: Address,
    token: Address,
    quote_out: bool,
    quote_amt: u128,
) -> RawEvmTransaction {
    let (q_from, q_to, t_from, t_to) = if quote_out {
        (W, PM, PM, W) // buy: wallet pays quote, receives token
    } else {
        (PM, W, W, PM) // sell
    };
    let swap = RawEvmLog {
        address: PM,
        topics: vec![V4_SWAP_TOPIC0, B256::repeat_byte(1), ROUTER.into_word()],
        data: Bytes::from(vec![0u8; 192]),
        block_number: n,
        transaction_index: 1,
        log_index: 2,
    };
    RawEvmTransaction {
        chain: BSC.verified_chain_key(),
        hash: B256::repeat_byte(u8::try_from(n).unwrap() + if quote_out { 0 } else { 100 }),
        from: W,
        to: Some(ROUTER),
        block_number: n,
        transaction_index: 1,
        block_time: 1_790_000_000 + n * 10,
        value: U256::ZERO,
        status: EvmTxStatus::Success,
        gas_used: 100,
        effective_gas_price: U256::from(3u8),
        l1_fee: None,
        logs: vec![
            transfer_log(quote, q_from, q_to, quote_amt, 0),
            transfer_log(token, t_from, t_to, 100, 1),
            swap,
        ],
        native_source: None,
        internal_transfers: None,
        native_balance_diff: None,
    }
}

fn candle(time: i64, close: &str) -> Candle {
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

fn run(txs: &[RawEvmTransaction]) -> scout_engine::SolanaWalletLedgerReport {
    let cfg = EvmExtractionConfig::for_profile(BSC);
    let mut r = build_evm_wallet_ledger(&cfg, W, txs, LedgerOptions::default()).unwrap();
    let src = InMemoryPriceSource::new().with_candles(
        QuoteAsset::Usdt,
        &[candle(M_BUY, "0.9998"), candle(M_SELL, "1.0003")],
    );
    r.apply_usd_prices(&src).unwrap();
    r
}

#[test]
fn bsc_pins_binance_peg_stables_as_distinct_18dp_units() {
    // Same ticker, different chains, different decimals: distinct units.
    assert_eq!((BASE_USDC.decimals, BSC_USDC.decimals), (6, 18));
    assert_ne!(QuoteUnit::UsdcUnits, QuoteUnit::BinancePegUsdcUnits);
    assert_ne!(QuoteUnit::UsdtUnits, QuoteUnit::BinancePegUsdtUnits);
    assert_eq!(quote_unit_decimals(QuoteUnit::UsdcUnits), Some(6));
    assert_eq!(
        quote_unit_decimals(QuoteUnit::BinancePegUsdcUnits),
        Some(18)
    );
    assert_eq!(
        quote_unit_decimals(QuoteUnit::BinancePegUsdtUnits),
        Some(18)
    );
    assert_eq!(quote_unit_label(QuoteUnit::BinancePegUsdtUnits), "usdt_peg");
    assert_eq!(quote_unit_label(QuoteUnit::BinancePegUsdcUnits), "usdc_peg");
    assert_eq!(BSC_USDT.origin, "binance_peg");
    assert_eq!(BSC_USDC.origin, "binance_peg");
    assert_eq!(
        BSC_USDT.address,
        address!("55d398326f99059fF775485246999027B3197955")
    );
    assert_eq!(
        BSC_USDC.address,
        address!("8AC76a51cc950d9822D68b83fE1ad97B32Cd580d")
    );
    // A report block set per chain: BSC carries the peg units only; Base its
    // own 6-dp USDC. They never share a unit.
    let bsc = scout_engine::ChainDisplay::evm(&BSC);
    let base = scout_engine::ChainDisplay::evm(&BASE);
    assert_eq!(
        bsc.quote_units(),
        &[
            QuoteUnit::Wei,
            QuoteUnit::BinancePegUsdtUnits,
            QuoteUnit::BinancePegUsdcUnits
        ]
    );
    assert_eq!(base.quote_units(), &[QuoteUnit::Wei, QuoteUnit::UsdcUnits]);
    assert_eq!(
        quote_asset_on(&bsc, QuoteUnit::BinancePegUsdtUnits),
        Some(QuoteAsset::Usdt)
    );
    assert_eq!(
        quote_asset_on(&bsc, QuoteUnit::BinancePegUsdcUnits),
        Some(QuoteAsset::Usdc)
    );
}

#[test]
fn usdt_peg_trades_are_exact_in_18dp_and_priced_through_usdt_usd() {
    // Buy 5 USDT-peg, sell for 7.25: pnl exactly 2.25e18 raw. USD: buy at
    // 0.9998 -> 4.999, sell at 1.0003 -> 7.252175, pnl 2.253175.
    let r = run(&[
        tx(1, BSC_USDT.address, TOKEN_A, true, 5 * E18),
        tx(
            7,
            BSC_USDT.address,
            TOKEN_A,
            false,
            7_250_000_000_000_000_000,
        ),
    ]);
    assert_eq!(r.chain.name, "bsc");
    assert_eq!(r.trades.route_swaps_by_quote.usdt_peg, 2);
    assert_eq!(r.trades.route_swaps_by_quote.usdt, 0, "never the 6-dp unit");
    assert_eq!(r.diagnostics.route_rejected.multi_asset, 0);
    let b = r.unit_block(QuoteUnit::BinancePegUsdtUnits).unwrap();
    assert_eq!(b.closed_episodes_known, 1);
    assert_eq!(b.realized_trade_pnl_raw, 2_250_000_000_000_000_000);
    assert_eq!(b.consumed_acquisition_basis_raw, 5_000_000_000_000_000_000);
    assert!(r.unit_block(QuoteUnit::UsdtUnits).is_none());
    assert_eq!(
        r.unit_block(QuoteUnit::Wei).unwrap().closed_episodes_known,
        0
    );
    let u = r.usd.as_ref().unwrap();
    assert_eq!(
        u.episodes[0].outcome,
        UsdOutcome::ClosedKnown {
            pnl: Money::from_scaled_units(225_317_500),
            consumed_basis: Money::from_scaled_units(499_900_000),
        }
    );
    assert_eq!(u.coverage.binance_peg_legs, 2);
    assert_eq!(u.coverage.usdc_par_legs, 0);
    assert_eq!(
        u.coverage.by_label.get("cex_reference_1m+binance_peg"),
        Some(&2)
    );
}

#[test]
fn usdc_peg_trades_are_par_assumed_with_the_peg_visible() {
    let r = run(&[
        tx(1, BSC_USDC.address, TOKEN_B, true, 3 * E18),
        tx(7, BSC_USDC.address, TOKEN_B, false, 4 * E18),
    ]);
    assert_eq!(r.trades.route_swaps_by_quote.usdc_peg, 2);
    assert_eq!(r.trades.route_swaps_by_quote.usdc, 0);
    let b = r.unit_block(QuoteUnit::BinancePegUsdcUnits).unwrap();
    assert_eq!(b.realized_trade_pnl_raw, 1_000_000_000_000_000_000);
    assert!(r.unit_block(QuoteUnit::UsdcUnits).is_none());
    let u = r.usd.as_ref().unwrap();
    assert_eq!(
        u.episodes[0].outcome,
        UsdOutcome::ClosedKnown {
            pnl: Money::from_scaled_units(100_000_000),
            consumed_basis: Money::from_scaled_units(300_000_000),
        }
    );
    assert_eq!(u.coverage.usdc_par_legs, 2);
    assert_eq!(u.coverage.binance_peg_legs, 2);
    assert_eq!(
        u.coverage.by_label.get("usdc_par_assumed+binance_peg"),
        Some(&2)
    );
}

#[test]
fn a_stable_quote_still_unpinned_on_the_chain_is_a_counted_gap() {
    // Base USDC is NOT a BSC quote asset: such a transfer is just another
    // token, so the trade has two traded tokens and is rejected, not booked.
    let r = run(&[tx(1, BASE_USDC.address, TOKEN_A, true, 5 * E18)]);
    assert_eq!(r.trades.route_swaps_by_quote.usdc, 0);
    assert_eq!(r.trades.route_swaps_by_quote.usdc_peg, 0);
    assert_eq!(r.closed_episodes_known + r.closed_episodes_unknown, 0);
    assert_eq!(r.diagnostics.route_rejected.multi_asset, 1);
}
