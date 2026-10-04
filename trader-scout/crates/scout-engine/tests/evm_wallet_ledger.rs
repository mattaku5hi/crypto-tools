//! Goldens of the EVM wallet ledger (ADR-020 step 2): the shared FIFO /
//! episode core fed with EVM trades. Hand-computed exact figures; Unknown is
//! never zero.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions
)]

use alloy_primitives::{Address, B256, Bytes, I256, U256, address};
use scout_core::{
    EvmTxStatus, InternalTransfer, NativeBalanceDiff, NativeSource, RawEvmLog, RawEvmTransaction,
};
use scout_dex_evm::V4_SWAP_TOPIC0;
use scout_engine::{
    EpisodeOutcome, EvmExtractionConfig, LedgerOptions, QuoteUnit, UnknownReason,
    build_evm_wallet_ledger,
};
use scout_evm::{
    BASE, ROBINHOOD, ROBINHOOD_USDG, TRANSFER_TOPIC0, WETH_DEPOSIT_TOPIC0, WETH_WITHDRAWAL_TOPIC0,
};

const PM: Address = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
const W: Address = address!("00000000000000000000000000000000000000a1");
const ROUTER: Address = address!("00000000000000000000000000000000000000b2");
const TOKEN: Address = address!("00000000000000000000000000000000000000c1");
const OTHER: Address = address!("00000000000000000000000000000000000000c2");
const FEE: u64 = 300; // gas_used 100 * price 3

fn weth() -> Address {
    ROBINHOOD.wrapped_native
}

fn usdg() -> Address {
    ROBINHOOD_USDG.address
}

fn amt(v: u128) -> Vec<u8> {
    U256::from(v).to_be_bytes::<32>().to_vec()
}

struct Tx {
    n: u64,
    logs: Vec<RawEvmLog>,
    value: u128,
    internal: Option<Vec<InternalTransfer>>,
    diff: Option<NativeBalanceDiff>,
}

impl Tx {
    fn new(n: u64) -> Self {
        Self {
            n,
            logs: Vec::new(),
            value: 0,
            internal: None,
            diff: None,
        }
    }

    fn log(mut self, address: Address, topics: Vec<B256>, data: Vec<u8>) -> Self {
        let idx = u64::try_from(self.logs.len()).unwrap();
        self.logs.push(RawEvmLog {
            address,
            topics,
            data: Bytes::from(data),
            block_number: self.n,
            transaction_index: 1,
            log_index: idx,
        });
        self
    }

    fn transfer(self, token: Address, from: Address, to: Address, v: u128) -> Self {
        self.log(
            token,
            vec![TRANSFER_TOPIC0, from.into_word(), to.into_word()],
            amt(v),
        )
    }

    fn deposit(self, account: Address, v: u128) -> Self {
        self.log(
            weth(),
            vec![WETH_DEPOSIT_TOPIC0, account.into_word()],
            amt(v),
        )
    }

    fn withdrawal(self, account: Address, v: u128) -> Self {
        self.log(
            weth(),
            vec![WETH_WITHDRAWAL_TOPIC0, account.into_word()],
            amt(v),
        )
    }

    fn swap(self) -> Self {
        self.log(
            PM,
            vec![V4_SWAP_TOPIC0, B256::repeat_byte(1), ROUTER.into_word()],
            vec![0u8; 192],
        )
    }

    fn value(mut self, v: u128) -> Self {
        self.value = v;
        self
    }

    fn build(self) -> RawEvmTransaction {
        RawEvmTransaction {
            chain: ROBINHOOD.verified_chain_key(),
            hash: B256::repeat_byte(u8::try_from(self.n).unwrap()),
            from: W,
            to: Some(ROUTER),
            block_number: self.n,
            transaction_index: 1,
            block_time: 1_790_000_000 + self.n * 10,
            value: U256::from(self.value),
            status: EvmTxStatus::Success,
            gas_used: 100,
            effective_gas_price: U256::from(3u8),
            l1_fee: None,
            logs: self.logs,
            native_source: self.internal.as_ref().map(|_| NativeSource::Trace),
            internal_transfers: self.internal,
            native_balance_diff: self.diff,
        }
    }
}

fn cfg() -> EvmExtractionConfig {
    EvmExtractionConfig::for_profile(ROBINHOOD)
}

fn ledger(txs: &[RawEvmTransaction]) -> scout_engine::SolanaWalletLedgerReport {
    build_evm_wallet_ledger(&cfg(), W, txs, LedgerOptions::default()).unwrap()
}

/// WETH-quoted buy: pays 5_000_000 WETH for 100 TOKEN.
fn weth_buy(n: u64) -> RawEvmTransaction {
    Tx::new(n)
        .transfer(weth(), W, PM, 5_000_000)
        .transfer(TOKEN, PM, W, 100)
        .swap()
        .build()
}

#[test]
fn weth_buy_then_sell_is_exact_with_gas_capitalized_once() {
    let sell = Tx::new(2)
        .transfer(TOKEN, W, PM, 100)
        .transfer(weth(), PM, W, 9_000_000)
        .swap()
        .build();
    let r = ledger(&[sell, weth_buy(1)]); // input order is irrelevant
    assert_eq!(r.chain.name, "robinhood");
    assert_eq!(r.quote_unit, QuoteUnit::Wei);
    assert_eq!((r.trades.buys, r.trades.sells), (1, 1));
    assert_eq!(r.trades.priced, 2);
    // basis = 5_000_000 + fee 300; net proceeds = 9_000_000 - fee 300.
    let pnl = 9_000_000 - i128::from(FEE) - (5_000_000 + i128::from(FEE));
    assert_eq!(pnl, 3_999_400);
    assert_eq!(r.closed_episodes_known, 1);
    assert_eq!(
        r.realized_trade_pnl_lamports, pnl,
        "legacy figure = native block (wei)"
    );
    let b = r.unit_block(QuoteUnit::Wei).unwrap();
    assert_eq!(b.realized_trade_pnl_raw, pnl);
    assert_eq!(
        b.consumed_acquisition_basis_raw,
        5_000_000 + i128::from(FEE)
    );
    assert_eq!((b.wins, b.losses, b.breakeven), (1, 0, 0));
    assert!(matches!(
        r.episodes[0].outcome,
        EpisodeOutcome::ClosedKnown { .. }
    ));
    // The second (USDG) block exists and is empty: never mixed.
    assert_eq!(
        r.unit_block(QuoteUnit::UsdgUnits)
            .unwrap()
            .closed_episodes_known,
        0
    );
    assert!(r.open_positions.is_empty());
    let ev = r.evm.as_ref().unwrap();
    assert_eq!(ev.trades.len(), 2);
    assert_eq!(ev.trades[0].fee_wei, Some(u128::from(FEE)));
    assert_eq!(ev.extraction.trades, 2);
}

#[test]
fn usdg_trades_are_their_own_unit_and_gas_is_not_mixed_into_usdg() {
    let buy = Tx::new(1)
        .transfer(usdg(), W, PM, 2_000_000)
        .transfer(TOKEN, PM, W, 50)
        .swap()
        .build();
    let sell = Tx::new(2)
        .transfer(TOKEN, W, PM, 50)
        .transfer(usdg(), PM, W, 3_000_000)
        .swap()
        .build();
    let r = ledger(&[buy, sell]);
    let b = r.unit_block(QuoteUnit::UsdgUnits).unwrap();
    assert_eq!(b.closed_episodes_known, 1);
    assert_eq!(b.realized_trade_pnl_raw, 1_000_000, "exactly 1 USDG");
    assert_eq!(b.consumed_acquisition_basis_raw, 2_000_000);
    assert_eq!(
        r.unit_block(QuoteUnit::Wei).unwrap().closed_episodes_known,
        0
    );
    assert_eq!(r.trades.route_swaps_by_quote.usdg, 2);
    // Gas of both trades is native, charged once, recorded as uncapitalized.
    let ev = r.evm.as_ref().unwrap();
    assert_eq!(ev.uncapitalized_gas_wei, 2 * u128::from(FEE));
    assert_eq!(
        r.diagnostics.unexplained_native_flow_lamports,
        -2 * i128::from(FEE)
    );
    assert_eq!(
        format!(
            "{:?}",
            scout_engine::quote_unit_decimals(QuoteUnit::UsdgUnits)
        ),
        "Some(6)"
    );
}

/// Native-ETH buy (tx.value) then a native sell through the unwrap path.
fn native_buy(n: u64) -> RawEvmTransaction {
    Tx::new(n)
        .value(7_000_000)
        .deposit(ROUTER, 7_000_000)
        .transfer(weth(), ROUTER, PM, 7_000_000)
        .transfer(TOKEN, PM, W, 100)
        .swap()
        .build()
}

fn native_sell_tx(n: u64) -> Tx {
    Tx::new(n)
        .transfer(TOKEN, W, PM, 100)
        .transfer(weth(), PM, ROUTER, 11_000_000)
        .withdrawal(ROUTER, 11_000_000)
        .swap()
}

#[test]
fn native_buy_is_exact_and_native_sell_is_unknown_never_zero() {
    let r = ledger(&[native_buy(1), native_sell_tx(2).build()]);
    assert_eq!(r.trades.native_leg_not_observed, 1);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert!(matches!(
        r.episodes[0].outcome,
        EpisodeOutcome::ClosedUnknown
    ));
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::NativeLegNotObserved)
    );
    // ADR-016 worst case: PnL >= -(known basis consumed) = -(7_000_000 + fee).
    let b = r.unit_block(QuoteUnit::Wei).unwrap();
    assert_eq!(
        b.realized_pnl_lower_bound_raw().bounded().copied(),
        Some(-(7_000_000 + i128::from(FEE)))
    );
    assert_eq!(
        b.realized_trade_pnl_raw, 0,
        "no KNOWN episode: nothing is claimed"
    );
    assert!(r.has_unknown_basis_inventory);
    let ev = r.evm.as_ref().unwrap();
    assert_eq!(ev.native_leg_counts.get("logs_and_value_only"), Some(&2));
}

#[test]
fn native_sell_is_exact_with_trace_internal_transfers_and_with_balance_diff_alike() {
    let sell_value = 11_000_000u128;
    // (a) trace: the router sends the unwrapped ETH to W.
    let mut traced = native_sell_tx(2);
    traced.internal = Some(vec![InternalTransfer {
        from: ROUTER,
        to: W,
        value: U256::from(sell_value),
    }]);
    let a = ledger(&[native_buy(1), traced.build()]);
    // (b) archive: W's balance moved by +sell_value - fee (value 0, fee 300).
    let mut diffed = native_sell_tx(2);
    diffed.diff = Some(NativeBalanceDiff {
        account: W,
        net_excl_fee: I256::try_from(sell_value).unwrap(),
    });
    let b = ledger(&[native_buy(1), diffed.build()]);
    let pnl = i128::try_from(sell_value).unwrap() - i128::from(FEE) - (7_000_000 + i128::from(FEE));
    for r in [&a, &b] {
        assert_eq!(r.closed_episodes_known, 1);
        assert_eq!(r.closed_episodes_unknown, 0);
        assert_eq!(
            r.unit_block(QuoteUnit::Wei).unwrap().realized_trade_pnl_raw,
            pnl
        );
        assert_eq!(r.trades.native_leg_not_observed, 0);
    }
    assert_eq!(a.realized_trade_pnl_lamports, b.realized_trade_pnl_lamports);
    // The source of each native leg is recorded per trade.
    let sa = a.evm.as_ref().unwrap();
    let sb = b.evm.as_ref().unwrap();
    assert_eq!(sa.trades[1].native_leg, "trace");
    assert_eq!(sb.trades[1].native_leg, "balance_diff");
    assert_eq!(sa.native_leg_counts.get("trace"), Some(&1));
    assert_eq!(sb.native_leg_counts.get("balance_diff"), Some(&1));
}

#[test]
fn balance_diff_buy_supersedes_tx_value() {
    // The router refunded 1_000_000 of the 7_000_000 sent: only 6_000_000
    // was spent. tx.value alone would overstate the cost; the archive diff
    // (value out, refund in) is the exact movement.
    let mut buy = Tx::new(1)
        .value(7_000_000)
        .deposit(ROUTER, 6_000_000)
        .transfer(weth(), ROUTER, PM, 6_000_000)
        .transfer(TOKEN, PM, W, 100);
    buy = buy.swap();
    buy.diff = Some(NativeBalanceDiff {
        account: W,
        net_excl_fee: -I256::try_from(6_000_000u64).unwrap(),
    });
    let with_diff = ledger(&[buy.build()]);
    let ev = with_diff.evm.as_ref().unwrap();
    assert_eq!(ev.trades[0].quote_amount, Some(6_000_000));
    assert_eq!(ev.trades[0].native_leg, "balance_diff");
    assert_eq!(
        with_diff
            .unit_block(QuoteUnit::Wei)
            .unwrap()
            .consumed_acquisition_basis_raw,
        0,
        "open episode: nothing consumed yet"
    );
    assert_eq!(with_diff.open_positions.len(), 1);
}

#[test]
fn eighteen_decimal_amounts_do_not_overflow_u64() {
    let huge_tokens: u128 = 1_000_000_000_000_000_000_000_000_000; // 1e27
    let buy = Tx::new(1)
        .transfer(weth(), W, PM, 3_000_000_000_000_000_000)
        .transfer(TOKEN, PM, W, huge_tokens)
        .swap()
        .build();
    let sell = Tx::new(2)
        .transfer(TOKEN, W, PM, huge_tokens)
        .transfer(weth(), PM, W, 4_000_000_000_000_000_000)
        .swap()
        .build();
    let r = ledger(&[buy, sell]);
    let fee = i128::from(FEE);
    let pnl = 4_000_000_000_000_000_000i128 - fee - (3_000_000_000_000_000_000i128 + fee);
    assert_eq!(
        r.unit_block(QuoteUnit::Wei).unwrap().realized_trade_pnl_raw,
        pnl
    );
    assert!(r.open_positions.is_empty());
}

#[test]
fn untraded_inventory_movements_break_continuity_into_unknown() {
    // Buy 100, then transfer 40 out (no swap): the transfer is an unexplained
    // outbound movement -> Unknown disposal; the later full sale of the
    // remaining 60 is exact but the episode is Unknown.
    let transfer_out = Tx::new(2).transfer(TOKEN, W, OTHER, 40).build();
    let sell = Tx::new(3)
        .transfer(TOKEN, W, PM, 60)
        .transfer(weth(), PM, W, 8_000_000)
        .swap()
        .build();
    let r = ledger(&[weth_buy(1), transfer_out, sell]);
    assert_eq!(r.diagnostics.continuity_breaks, 1);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.closed_episodes_known, 0);
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::UnexplainedOutboundTokenMovement)
    );
}

#[test]
fn window_left_censors_a_sale_of_inventory_bought_before_it() {
    let sell = Tx::new(3)
        .transfer(TOKEN, W, PM, 100)
        .transfer(weth(), PM, W, 9_000_000)
        .swap()
        .build();
    let r = build_evm_wallet_ledger(
        &cfg(),
        W,
        &[sell],
        LedgerOptions {
            left_censoring: true,
        },
    )
    .unwrap();
    assert_eq!(r.left_censored_episodes, 1);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.closed_episodes_unknown, 0);
    assert!(matches!(
        r.episodes[0].outcome,
        EpisodeOutcome::LeftCensored
    ));
}

#[test]
fn base_without_l1_fee_makes_the_basis_unknown_not_partial() {
    // Quote-free check on the fee rule directly: a Base buy with a receipt lacking l1Fee.
    let base_cfg = {
        let mut c =
            EvmExtractionConfig::new(BASE, scout_dex_evm::SwapVenueGate::new(BASE.chain_id));
        c.quote_tokens.clear();
        c
    };
    let base_pm = address!("498581ff718922c3f8e6a244956af099b2652b2b");
    let mut tx = Tx::new(1)
        .value(1_000)
        .transfer(BASE.wrapped_native, ROUTER, base_pm, 1_000)
        .transfer(TOKEN, base_pm, W, 10)
        .build();
    tx.chain = BASE.verified_chain_key();
    // Gated swap at Base's PoolManager.
    tx.logs.push(RawEvmLog {
        address: base_pm,
        topics: vec![V4_SWAP_TOPIC0, B256::repeat_byte(1), ROUTER.into_word()],
        data: Bytes::from(vec![0u8; 192]),
        block_number: 1,
        transaction_index: 1,
        log_index: 9,
    });
    let r = build_evm_wallet_ledger(&base_cfg, W, &[tx], LedgerOptions::default()).unwrap();
    assert_eq!(r.trades.fee_not_observed, 1);
    assert_eq!(
        r.trades.idl_only_variant, 0,
        "Base v4 is fixture-verified (evm_uniswap_v4_base.rs)"
    );
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::FeeNotObserved)
    );
    assert_eq!(r.chain.name, "base");
}

#[test]
fn duplicates_and_input_order_never_change_the_result() {
    let sell = Tx::new(2)
        .transfer(TOKEN, W, PM, 100)
        .transfer(weth(), PM, W, 9_000_000)
        .swap()
        .build();
    let a = ledger(&[weth_buy(1), sell.clone()]);
    let b = ledger(&[sell.clone(), weth_buy(1), sell, weth_buy(1)]);
    assert_eq!(b.diagnostics.duplicate_transactions_ignored, 2);
    assert_eq!(a.realized_trade_pnl_exact, b.realized_trade_pnl_exact);
    assert_eq!(a.episodes, b.episodes);
}
