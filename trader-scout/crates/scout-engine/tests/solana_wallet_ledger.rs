//! Offline tests for the SOL-quoted wallet ledger (ADR-010 / ADR-004).
//! Synthetic golden cases are hand-computed; fixture cases run the real
//! committed live captures through the real `HeliusProvider` decode path.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use futures::StreamExt as _;
use proptest::prelude::*;
use scout_api::{HistoryProvider, ScanRequest, ScanTask};
use scout_core::{
    AddressBytes, AssetKey, Money, RawPayload, RawSolanaInstruction, RawSolanaTransaction,
    SolanaExecutionStatus, SolanaNativeBalanceChange, SolanaPubkey, SolanaTokenBalanceChange,
};
use scout_dex_solana::{
    BUY_INSTRUCTION_DISCRIMINATOR, BUY_V2_INSTRUCTION_DISCRIMINATOR, EVENT_CPI_DISCRIMINATOR,
    SELL_INSTRUCTION_DISCRIMINATOR, TRADE_EVENT_DISCRIMINATOR, TradeEventPairing, TradeSide,
    pair_trades_with_events,
};
use scout_engine::{
    EpisodeOutcome, PUMP_BONDING_CURVE_PROGRAM_ID, QuoteUnit, SolanaWalletLedgerReport,
    UnknownReason, allocate_fee_proportionally, build_solana_wallet_ledger, lamports_to_money,
    pump_bonding_curve_decoder, solana_mainnet_chain,
};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

const W: u8 = 1;
const OTHER: u8 = 99;
const M1: u8 = 10;
const M2: u8 = 11;

fn pump() -> SolanaPubkey {
    pubkey(PUMP_BONDING_CURVE_PROGRAM_ID)
}

fn trade_ix(
    side: TradeSide,
    v2_quote: Option<u8>,
    user: u8,
    mint: u8,
    idx: u32,
) -> RawSolanaInstruction {
    match (side, v2_quote) {
        (TradeSide::Buy, None) => {
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
        (TradeSide::Sell, None) => {
            let mut accounts: Vec<SolanaPubkey> = (0..14u8).map(|i| pk(100 + i)).collect();
            accounts[2] = pk(mint);
            accounts[6] = pk(user);
            let mut data = SELL_INSTRUCTION_DISCRIMINATOR.to_vec();
            data.extend_from_slice(&1u64.to_le_bytes());
            data.extend_from_slice(&0u64.to_le_bytes());
            RawSolanaInstruction {
                program_id: pump(),
                accounts,
                data,
                instruction_index: idx,
            }
        }
        (_, Some(q)) => {
            // buy_v2 layout (only the buy side is needed here).
            let mut accounts: Vec<SolanaPubkey> = (0..27u8).map(|i| pk(100 + i)).collect();
            accounts[1] = pk(mint);
            accounts[2] = pk(q);
            accounts[13] = pk(user);
            let mut data = BUY_V2_INSTRUCTION_DISCRIMINATOR.to_vec();
            data.extend_from_slice(&1u64.to_le_bytes());
            data.extend_from_slice(&2u64.to_le_bytes());
            RawSolanaInstruction {
                program_id: pump(),
                accounts,
                data,
                instruction_index: idx,
            }
        }
    }
}

struct Ev {
    mint: u8,
    user: u8,
    is_buy: bool,
    sol: u64,
    tokens: u64,
    fee: u64,
    creator_fee: u64,
    ts: i64,
}

fn event_ix(e: &Ev, idx: u32) -> RawSolanaInstruction {
    let le = |v: u64| v.to_le_bytes();
    let mut p = Vec::new();
    p.extend(pk(e.mint));
    p.extend(le(e.sol));
    p.extend(le(e.tokens));
    p.push(u8::from(e.is_buy));
    p.extend(pk(e.user));
    p.extend(e.ts.to_le_bytes());
    for v in [11, 12, 13, 14] {
        p.extend(le(v));
    }
    p.extend(pk(0xfe));
    p.extend(le(95));
    p.extend(le(e.fee));
    p.extend(pk(0xcc));
    p.extend(le(30));
    p.extend(le(e.creator_fee));
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

fn bal(mint: u8, owner: u8, pre: u64, post: u64) -> SolanaTokenBalanceChange {
    SolanaTokenBalanceChange {
        mint: pk(mint),
        owner: Some(pk(owner)),
        decimals: 6,
        pre_amount: Some(pre),
        post_amount: post,
        closed: false,
    }
}

fn nat(account: u8, delta: i64) -> SolanaNativeBalanceChange {
    let pre = 10_000_000_000u64;
    SolanaNativeBalanceChange {
        account: pk(account),
        pre_lamports: pre,
        post_lamports: pre.checked_add_signed(delta).unwrap(),
    }
}

struct Tx {
    sig: u8,
    slot: u64,
    index: u64,
    ixs: Vec<RawSolanaInstruction>,
    bals: Vec<SolanaTokenBalanceChange>,
    fee: u64,
    payer: u8,
    native: Vec<SolanaNativeBalanceChange>,
    ok: bool,
}

impl Tx {
    fn build(self) -> RawSolanaTransaction {
        RawSolanaTransaction {
            signature: [self.sig; 64],
            execution: if self.ok {
                SolanaExecutionStatus::Succeeded
            } else {
                SolanaExecutionStatus::Failed {
                    error: "InstructionError".to_string(),
                }
            },
            slot: self.slot,
            transaction_index: self.index,
            instructions: self.ixs,
            token_balance_changes: self.bals,
            fee_lamports: self.fee,
            fee_payer: pk(self.payer),
            signers: vec![pk(self.payer)],
            native_balance_changes: self.native,
        }
    }
}

/// One wallet trade tx: instruction + event, token change, native change
/// computed to be exactly explained (residual 0) unless `native_extra`.
#[allow(clippy::too_many_arguments)]
fn trade_tx(
    sig: u8,
    slot: u64,
    side: TradeSide,
    mint: u8,
    tokens: u64,
    pre_tokens: u64,
    sol: u64,
    fee: u64,
    creator_fee: u64,
    ts: i64,
    tx_fee: u64,
    payer: u8,
) -> RawSolanaTransaction {
    let buy = side == TradeSide::Buy;
    let ev = Ev {
        mint,
        user: W,
        is_buy: buy,
        sol,
        tokens,
        fee,
        creator_fee,
        ts,
    };
    let post = if buy {
        pre_tokens + tokens
    } else {
        pre_tokens - tokens
    };
    let flow = i64::try_from(sol).unwrap()
        + if buy {
            i64::try_from(fee + creator_fee).unwrap()
        } else {
            -i64::try_from(fee + creator_fee).unwrap()
        };
    let wallet_native = (if buy { -flow } else { flow })
        - if payer == W {
            i64::try_from(tx_fee).unwrap()
        } else {
            0
        };
    Tx {
        sig,
        slot,
        index: 0,
        ixs: vec![trade_ix(side, None, W, mint, 0), event_ix(&ev, 1)],
        bals: vec![bal(mint, W, pre_tokens, post)],
        fee: tx_fee,
        payer,
        native: vec![nat(W, wallet_native)],
        ok: true,
    }
    .build()
}

fn run(txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
    build_solana_wallet_ledger(&pk(W), txs, &pump_bonding_curve_decoder().unwrap()).unwrap()
}

fn lam(n: i128) -> Money {
    lamports_to_money(n).unwrap()
}

#[test]
fn golden_buy_then_full_sell_exact_pnl_with_fee_capitalization() {
    // Buy 1000 tokens: cost 1_000_000 + 10_000 + 5_000 = 1_015_000, tx fee 5_000 -> basis 1_020_000.
    // Sell 1000: proceeds 1_200_000 - 12_000 - 6_000 = 1_182_000, sale fee 5_000 -> 1_177_000.
    // PnL = 1_177_000 - 1_020_000 = 157_000.
    let txs = vec![
        trade_tx(
            1,
            100,
            TradeSide::Buy,
            M1,
            1000,
            0,
            1_000_000,
            10_000,
            5_000,
            1000,
            5_000,
            W,
        ),
        trade_tx(
            2,
            101,
            TradeSide::Sell,
            M1,
            1000,
            1000,
            1_200_000,
            12_000,
            6_000,
            1600,
            5_000,
            W,
        ),
    ];
    let r = run(&txs);
    assert_eq!(r.quote_unit, QuoteUnit::Lamports);
    assert_eq!(r.trades.buys, 1);
    assert_eq!(r.trades.sells, 1);
    assert_eq!(r.trades.priced, 2);
    assert_eq!(r.closed_episodes_known, 1);
    assert_eq!(r.closed_episodes_unknown, 0);
    assert_eq!(r.open_episodes, 0);
    assert_eq!(r.realized_trade_pnl_lamports, 157_000);
    assert_eq!(r.realized_trade_pnl_exact, lam(157_000));
    // Consumed basis = capitalized buy basis 1_020_000 => ROI 157_000/1_020_000.
    assert_eq!(r.consumed_acquisition_basis_lamports, 1_020_000);
    assert_eq!(r.consumed_acquisition_basis_exact, lam(1_020_000));
    assert_eq!(r.wins, 1);
    assert_eq!(r.median_holding_seconds, Some(600));
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 0);
    assert_eq!(r.diagnostics.unexplained_native_flow_txs, 0);
    assert_eq!(r.realized_net_pnl_lamports, 157_000);
    assert_eq!(r.activity.first_trade_timestamp, Some(1000));
    assert_eq!(r.activity.last_trade_timestamp, Some(1600));
    assert_eq!(r.activity.active_span_seconds, Some(600));
    assert_eq!(r.activity.timestamped_trades, 2);
    assert_eq!(r.activity.active_utc_days, 1);
    assert_eq!(r.activity.mint_day_pairs, 1);
    assert!(!r.has_unknown_basis_inventory);
    assert_eq!(
        r.win_rate,
        scout_analytics::RatioStatus::Value { value: lam(1) }
    );
}

#[test]
fn golden_adr004_c02_partial_sell_is_proportional_to_capitalized_basis() {
    // basis 1_020_000 for 1000 tokens; sell 400 (40%): net 495_000 - 5_000 = 490_000;
    // consumed basis 408_000; pnl 82_000 (open episode, separate sum).
    let txs = vec![
        trade_tx(
            1,
            100,
            TradeSide::Buy,
            M1,
            1000,
            0,
            1_000_000,
            10_000,
            5_000,
            1000,
            5_000,
            W,
        ),
        trade_tx(
            2,
            101,
            TradeSide::Sell,
            M1,
            400,
            1000,
            500_000,
            5_000,
            0,
            1100,
            5_000,
            W,
        ),
    ];
    let r = run(&txs);
    assert_eq!(r.open_episodes, 1);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.realized_trade_pnl_lamports, 0);
    assert_eq!(r.open_episode_known_disposal_pnl_lamports, 82_000);
    assert_eq!(r.open_episode_known_disposals, 1);
    assert_eq!(r.open_positions.len(), 1);
    assert_eq!(r.open_positions[0].open_amount_raw, 600);
    assert_eq!(r.open_positions[0].unknown_basis_amount_raw, 0);
    // Remaining 600: net 693_000 - 5_000 = 688_000; basis 612_000; pnl 76_000 -> episode 158_000.
    let mut more = txs;
    more.push(trade_tx(
        3,
        102,
        TradeSide::Sell,
        M1,
        600,
        600,
        700_000,
        7_000,
        0,
        1200,
        5_000,
        W,
    ));
    let r = run(&more);
    assert_eq!(r.closed_episodes_known, 1);
    assert_eq!(r.realized_trade_pnl_lamports, 158_000);
    assert_eq!(r.open_episode_known_disposal_pnl_lamports, 0);
    // 408_000 + 612_000 = whole capitalized basis of the closed episode.
    assert_eq!(r.consumed_acquisition_basis_lamports, 1_020_000);
}

#[test]
fn golden_two_buys_one_sell_fifo_with_sponsored_fee() {
    // Fee payer is another account: no fee cost anywhere.
    // A: 1000 tok cost 100_000; B: 1000 tok cost 300_000. Sell 1500 for 450_000:
    // basis = 100_000 + 500/1000*300_000 = 250_000 -> pnl 200_000 (open: 500 left).
    // Sell 500 for 100_000: basis 150_000 -> -50_000. Episode total 150_000.
    let mut txs = vec![
        trade_tx(
            1,
            100,
            TradeSide::Buy,
            M1,
            1000,
            0,
            100_000,
            0,
            0,
            1000,
            5_000,
            OTHER,
        ),
        trade_tx(
            2,
            101,
            TradeSide::Buy,
            M1,
            1000,
            1000,
            300_000,
            0,
            0,
            1010,
            5_000,
            OTHER,
        ),
        trade_tx(
            3,
            102,
            TradeSide::Sell,
            M1,
            1500,
            2000,
            450_000,
            0,
            0,
            1020,
            5_000,
            OTHER,
        ),
    ];
    let r = run(&txs);
    assert_eq!(r.open_episode_known_disposal_pnl_lamports, 200_000);
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 0);
    txs.push(trade_tx(
        4,
        103,
        TradeSide::Sell,
        M1,
        500,
        500,
        100_000,
        0,
        0,
        1030,
        5_000,
        OTHER,
    ));
    let r = run(&txs);
    assert_eq!(r.realized_trade_pnl_lamports, 150_000);
    assert_eq!(r.closed_episodes_known, 1);
    assert_eq!(r.wins, 1);
}

#[test]
fn two_trades_in_one_tx_split_fee_exactly_proportionally() {
    // Consideration 1_000_000 and 3_000_000, tx fee 5_001:
    // floor shares 1250 / 3750, remainder 1 -> first (earliest) = 1251.
    // Later sponsored sells at exactly cost => pnl = -share.
    let evs = [
        Ev {
            mint: M1,
            user: W,
            is_buy: true,
            sol: 1_000_000,
            tokens: 100,
            fee: 0,
            creator_fee: 0,
            ts: 50,
        },
        Ev {
            mint: M2,
            user: W,
            is_buy: true,
            sol: 3_000_000,
            tokens: 200,
            fee: 0,
            creator_fee: 0,
            ts: 50,
        },
    ];
    let buy = Tx {
        sig: 1,
        slot: 10,
        index: 0,
        ixs: vec![
            trade_ix(TradeSide::Buy, None, W, M1, 0),
            event_ix(&evs[0], 1),
            trade_ix(TradeSide::Buy, None, W, M2, 2),
            event_ix(&evs[1], 3),
        ],
        bals: vec![bal(M1, W, 0, 100), bal(M2, W, 0, 200)],
        fee: 5_001,
        payer: W,
        native: vec![nat(W, -4_005_001)],
        ok: true,
    }
    .build();
    let sells = [
        trade_tx(
            2,
            11,
            TradeSide::Sell,
            M1,
            100,
            100,
            1_000_000,
            0,
            0,
            60,
            5_000,
            OTHER,
        ),
        trade_tx(
            3,
            12,
            TradeSide::Sell,
            M2,
            200,
            200,
            3_000_000,
            0,
            0,
            60,
            5_000,
            OTHER,
        ),
    ];
    let txs = vec![buy, sells[0].clone(), sells[1].clone()];
    let r = run(&txs);
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 0);
    assert_eq!(r.closed_episodes_known, 2);
    let pnl = |m: u8| {
        r.episodes
            .iter()
            .find(|e| e.mint == pk(m))
            .map(|e| e.outcome)
            .unwrap()
    };
    assert_eq!(pnl(M1), EpisodeOutcome::ClosedKnown { pnl: lam(-1251) });
    assert_eq!(pnl(M2), EpisodeOutcome::ClosedKnown { pnl: lam(-3750) });
    assert_eq!(r.realized_trade_pnl_lamports, -5_001);
    assert_eq!(r.losses, 2);
}

#[test]
fn failed_tx_fee_overhead_only_when_wallet_paid_for_its_own_trade() {
    let ok = trade_tx(
        1,
        100,
        TradeSide::Buy,
        M1,
        1000,
        0,
        1_000_000,
        0,
        0,
        1000,
        0,
        OTHER,
    );
    let sell = trade_tx(
        2,
        101,
        TradeSide::Sell,
        M1,
        1000,
        1000,
        1_100_000,
        0,
        0,
        1100,
        0,
        OTHER,
    );
    let mut failed_own = Tx {
        sig: 3,
        slot: 102,
        index: 0,
        ixs: vec![trade_ix(TradeSide::Buy, None, W, M1, 0)],
        bals: vec![],
        fee: 7_000,
        payer: W,
        native: vec![],
        ok: false,
    }
    .build();
    let mut failed_sponsored = failed_own.clone();
    failed_sponsored.signature = [4; 64];
    failed_sponsored.fee_payer = pk(OTHER);
    let mut failed_no_trade = failed_own.clone();
    failed_no_trade.signature = [5; 64];
    failed_no_trade.instructions = vec![];
    let mut failed_other_user = failed_own.clone();
    failed_other_user.signature = [6; 64];
    failed_other_user.instructions = vec![trade_ix(TradeSide::Buy, None, 55, M1, 0)];
    failed_own.slot = 102;
    let txs = vec![
        ok,
        sell,
        failed_own,
        failed_sponsored,
        failed_no_trade,
        failed_other_user,
    ];
    let r = run(&txs);
    assert_eq!(r.failed_trade_fees_lamports, 7_000);
    assert_eq!(r.failed_trade_fee_txs, 1);
    assert_eq!(r.diagnostics.failed_transactions, 4);
    assert_eq!(r.realized_trade_pnl_lamports, 100_000);
    assert_eq!(r.realized_net_pnl_lamports, 93_000);
    assert_eq!(r.closed_episodes_known, 1);
}

#[test]
fn router_forward_buy_with_unexplained_outflow_makes_episode_unknown() {
    // Decoded buy for the wallet but its net token delta is 0: lot then outflow.
    let ev = Ev {
        mint: M1,
        user: W,
        is_buy: true,
        sol: 1_000_000,
        tokens: 1000,
        fee: 0,
        creator_fee: 0,
        ts: 10,
    };
    let tx = Tx {
        sig: 1,
        slot: 1,
        index: 0,
        ixs: vec![trade_ix(TradeSide::Buy, None, W, M1, 0), event_ix(&ev, 1)],
        bals: vec![bal(M1, 77, 0, 1000)],
        fee: 5_000,
        payer: W,
        native: vec![nat(W, -1_005_000)],
        ok: true,
    }
    .build();
    let r = run(&[tx]);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.realized_trade_pnl_lamports, 0);
    assert_eq!(r.diagnostics.continuity_breaks, 1);
    assert_eq!(r.wins + r.losses + r.breakeven, 0);
    assert_eq!(r.win_rate, scout_analytics::RatioStatus::Undefined);
    assert!(r.has_unknown_basis_inventory);
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::UnexplainedOutboundTokenMovement)
    );
}

#[test]
fn unexplained_inbound_transfer_then_sell_is_unknown_not_zero() {
    let transfer_in = Tx {
        sig: 1,
        slot: 1,
        index: 0,
        ixs: vec![],
        bals: vec![bal(M1, W, 0, 1000), bal(12, W, 0, 5)],
        fee: 0,
        payer: OTHER,
        native: vec![],
        ok: true,
    }
    .build();
    let sell = trade_tx(
        2,
        2,
        TradeSide::Sell,
        M1,
        1000,
        1000,
        900_000,
        0,
        0,
        100,
        0,
        OTHER,
    );
    let r = run(&[transfer_in, sell]);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.realized_trade_pnl_lamports, 0);
    assert_eq!(r.win_rate, scout_analytics::RatioStatus::Undefined);
    assert_eq!(r.unknown_basis_lots_created, 1);
    assert_eq!(r.diagnostics.continuity_breaks, 1);
    // Mint 12 never traded: out of scope, counted, no lots.
    assert_eq!(r.diagnostics.out_of_scope_token_movements, 1);
    assert_eq!(r.distinct_mints_traded, 1);
    let reasons = &r.episodes[0].unknown_reasons;
    assert!(reasons.contains(&UnknownReason::UnexplainedInboundTokenMovement));
    assert!(reasons.contains(&UnknownReason::UnknownBasisLotConsumed));
}

#[test]
fn non_wsol_v2_quote_is_unknown_unsupported_quote_asset() {
    let ev = Ev {
        mint: M1,
        user: W,
        is_buy: true,
        sol: 1_000,
        tokens: 500,
        fee: 0,
        creator_fee: 0,
        ts: 10,
    };
    let tx = Tx {
        sig: 1,
        slot: 1,
        index: 0,
        ixs: vec![
            trade_ix(TradeSide::Buy, Some(77), W, M1, 0),
            event_ix(&ev, 1),
        ],
        bals: vec![bal(M1, W, 0, 500)],
        fee: 5_000,
        payer: W,
        native: vec![],
        ok: true,
    }
    .build();
    let r = run(&[tx]);
    assert_eq!(r.trades.unsupported_quote, 1);
    assert_eq!(r.trades.priced, 0);
    assert_eq!(r.open_episodes, 1);
    assert_eq!(r.open_positions[0].open_amount_raw, 500);
    assert_eq!(r.open_positions[0].unknown_basis_amount_raw, 500);
    assert!(r.has_unknown_basis_inventory);
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::UnsupportedQuoteAsset)
    );
}

#[test]
fn missing_event_and_sell_underflow_are_unknown_and_counted() {
    // Buy without event: unpaired, tokens from continuity delta.
    let unpaired = Tx {
        sig: 1,
        slot: 1,
        index: 0,
        ixs: vec![trade_ix(TradeSide::Buy, None, W, M1, 0)],
        bals: vec![bal(M1, W, 0, 300)],
        fee: 0,
        payer: OTHER,
        native: vec![],
        ok: true,
    }
    .build();
    // Sell whose fees exceed sol_amount: malformed.
    let sell = trade_tx(
        2,
        2,
        TradeSide::Sell,
        M1,
        300,
        300,
        100,
        80,
        80,
        20,
        0,
        OTHER,
    );
    let r = run(&[unpaired, sell]);
    assert_eq!(r.trades.unpaired, 1);
    assert_eq!(r.trades.malformed_consideration, 1);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.realized_trade_pnl_lamports, 0);
}

#[test]
fn input_order_and_duplicates_do_not_change_the_report() {
    let a = trade_tx(
        1,
        100,
        TradeSide::Buy,
        M1,
        1000,
        0,
        1_000_000,
        10_000,
        5_000,
        1000,
        5_000,
        W,
    );
    let b = trade_tx(
        2,
        101,
        TradeSide::Sell,
        M1,
        400,
        1000,
        500_000,
        5_000,
        0,
        1100,
        5_000,
        W,
    );
    let c = trade_tx(
        3,
        102,
        TradeSide::Sell,
        M1,
        600,
        600,
        700_000,
        7_000,
        0,
        1200,
        5_000,
        W,
    );
    let canonical = run(&[a.clone(), b.clone(), c.clone()]);
    assert_eq!(run(&[c.clone(), a.clone(), b.clone()]), canonical);
    assert_eq!(run(&[b.clone(), c.clone(), a.clone()]), canonical);

    let mut dup = run(&[a.clone(), a.clone(), b.clone(), c.clone(), b.clone()]);
    assert_eq!(dup.diagnostics.duplicate_transactions_ignored, 2);
    dup.diagnostics.duplicate_transactions_ignored = 0;
    assert_eq!(dup, canonical);

    // Same-slot ordering uses transaction_index, not input position.
    let mut x = a.clone();
    x.slot = 500;
    x.transaction_index = 7;
    let mut y = c.clone();
    y.slot = 500;
    y.transaction_index = 9;
    let mut m = b.clone();
    m.slot = 500;
    m.transaction_index = 8;
    assert_eq!(run(&[y.clone(), m.clone(), x.clone()]), run(&[x, m, y]));
}

proptest! {
    #[test]
    fn fee_allocation_always_sums_exactly(
        fee in 0u64..(1u64 << 62),
        weights in proptest::collection::vec(0u64..(1u64 << 61), 1..8),
    ) {
        let shares = allocate_fee_proportionally(fee, &weights).unwrap();
        prop_assert_eq!(shares.len(), weights.len());
        let sum: u128 = shares.iter().map(|s| u128::from(*s)).sum();
        prop_assert_eq!(sum, u128::from(fee));
    }
}

#[test]
fn fee_allocation_edge_cases() {
    assert!(allocate_fee_proportionally(5, &[]).unwrap().is_empty());
    assert_eq!(allocate_fee_proportionally(5, &[0, 0]).unwrap(), vec![5, 0]);
    assert_eq!(
        allocate_fee_proportionally(5_001, &[1, 3]).unwrap(),
        vec![1251, 3750]
    );
}

// ---------------------------------------------------------------------
// Real fixtures through the real HeliusProvider decode path.
// ---------------------------------------------------------------------

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
            .unwrap();
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
    assert_eq!(out.len(), data.len());
    out
}

/// `(user, mint, side, sol, tokens, fee, creator_fee)` of the single paired
/// trade of a fixture tx, if it has exactly one.
fn single_paired(
    tx: &RawSolanaTransaction,
) -> Option<(SolanaPubkey, SolanaPubkey, TradeSide, u64, u64, u64, u64)> {
    let decoder = pump_bonding_curve_decoder().unwrap();
    let rep = pair_trades_with_events(&decoder, &tx.instructions, tx.slot, tx.transaction_index);
    if rep.trades.len() != 1 {
        return None;
    }
    let p = &rep.trades[0];
    match &p.pairing {
        TradeEventPairing::Paired(e) => Some((
            p.trade.user,
            p.trade.mint,
            p.trade.side,
            e.sol_amount,
            e.token_amount,
            e.fee,
            e.creator_fee,
        )),
        _ => None,
    }
}

/// Fixture tx assertions: per-tx native residual and exact basis.
async fn check_fixture_tx(sig_prefix: &str, expect_zero_residual: bool) -> i128 {
    let txs = fixture_txs("pump_variants_live_2026-10-02.json").await;
    let tx = txs
        .iter()
        .find(|t| {
            bs58::encode(t.signature)
                .into_string()
                .starts_with(sig_prefix)
        })
        .unwrap();
    let (user, mint, side, sol, tokens, fee, cfee) = single_paired(tx).expect("one paired trade");
    assert_eq!(side, TradeSide::Buy);
    let decoder = pump_bonding_curve_decoder().unwrap();
    let r = build_solana_wallet_ledger(&user, std::slice::from_ref(tx), &decoder).unwrap();
    let residual = r.diagnostics.unexplained_native_flow_lamports;
    eprintln!(
        "{sig_prefix}: payer_is_user={} sol={sol} fee={fee} cfee={cfee} tx_fee={} residual={residual} \
         residual_txs={} tokens={tokens} open={:?}",
        tx.fee_payer == user,
        tx.fee_lamports,
        r.diagnostics.unexplained_native_flow_txs,
        r.open_positions
    );
    if expect_zero_residual {
        assert_eq!(residual, 0, "{sig_prefix}");
        assert_eq!(r.diagnostics.unexplained_native_flow_txs, 0);
    }
    assert_eq!(r.trades.priced, 1);
    assert_eq!(r.open_positions.len(), 1);
    assert_eq!(r.open_positions[0].open_amount_raw, u128::from(tokens));
    assert_eq!(r.open_positions[0].unknown_basis_amount_raw, 0);

    // Recorded basis == sol + fee + creator_fee + tx fee (if user pays): probe by
    // a synthetic zero-fee sell at basis + 7 -> PnL exactly +7.
    let expected_basis = i128::from(sol)
        + i128::from(fee)
        + i128::from(cfee)
        + if tx.fee_payer == user {
            i128::from(tx.fee_lamports)
        } else {
            0
        };
    let mut sell_events = Vec::new();
    let sell_sol = u64::try_from(expected_basis + 7).unwrap();
    let ev = Ev {
        mint: 0,
        user: 0,
        is_buy: false,
        sol: sell_sol,
        tokens,
        fee: 0,
        creator_fee: 0,
        ts: 4_000_000_000,
    };
    let mut ixs = vec![trade_ix(TradeSide::Sell, None, 0, 0, 0), event_ix(&ev, 1)];
    // Retarget user/mint to the fixture's.
    ixs[0].accounts[6] = user;
    ixs[0].accounts[2] = mint;
    ixs[1].data[8 + 8..8 + 8 + 32].copy_from_slice(&mint);
    // mint(32) sol(8) tokens(8) is_buy(1) user(32) at payload offset: header 16.
    let user_off = 16 + 32 + 8 + 8 + 1;
    ixs[1].data[user_off..user_off + 32].copy_from_slice(&user);
    let mint_off = 16;
    ixs[1].data[mint_off..mint_off + 32].copy_from_slice(&mint);
    sell_events.push(RawSolanaTransaction {
        signature: [0xab; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot: tx.slot + 1000,
        transaction_index: 0,
        instructions: ixs,
        token_balance_changes: vec![SolanaTokenBalanceChange {
            mint,
            owner: Some(user),
            decimals: 6,
            pre_amount: Some(tokens),
            post_amount: 0,
            closed: false,
        }],
        fee_lamports: 5_000,
        fee_payer: pk(OTHER),
        signers: vec![pk(OTHER)],
        native_balance_changes: vec![],
    });
    let mut both = vec![tx.clone()];
    both.extend(sell_events);
    let r2 = build_solana_wallet_ledger(&user, &both, &decoder).unwrap();
    assert_eq!(r2.closed_episodes_known, 1, "{sig_prefix}");
    assert_eq!(
        r2.realized_trade_pnl_lamports, 7,
        "{sig_prefix}: basis != expected"
    );
    residual
}

#[tokio::test]
async fn live_clean_tx_58dd_has_zero_residual_and_exact_basis() {
    assert_eq!(check_fixture_tx("58ddWHqN", true).await, 0);
}

#[tokio::test]
async fn live_tx_5bdb_basis_and_residual_report() {
    // ADR-010 evidence names this tx among the 20; the residual is printed
    // (see test output) and the basis identity is asserted exactly.
    let residual = check_fixture_tx("5bDBFbv2", false).await;
    eprintln!("5bDBFbv2 residual = {residual}");
}

#[tokio::test]
async fn live_fixture_wallets_never_panic_and_count_everything() {
    let decoder = pump_bonding_curve_decoder().unwrap();
    for name in [
        "pump_variants_live_2026-10-02.json",
        "pump_bonding_curve_buy_probe.json",
    ] {
        let txs = fixture_txs(name).await;
        let mut users = std::collections::BTreeSet::new();
        for tx in &txs {
            if let Some((user, ..)) = single_paired(tx) {
                users.insert(user);
            }
        }
        assert!(!users.is_empty(), "{name}");
        for user in users {
            let r = build_solana_wallet_ledger(&user, &txs, &decoder).unwrap();
            assert_eq!(r.quote_unit, QuoteUnit::Lamports);
            assert!(r.trades.buys + r.trades.sells >= 1);
            let t = &r.trades;
            assert_eq!(
                t.priced
                    + t.unpaired
                    + t.mismatched
                    + t.unsupported_quote
                    + t.malformed_consideration,
                t.buys + t.sells
            );
            eprintln!(
                "{name} user={} trades={}/{} residual={} residual_txs={} unknown_lots={} open={}",
                bs58::encode(user).into_string(),
                t.buys,
                t.sells,
                r.diagnostics.unexplained_native_flow_lamports,
                r.diagnostics.unexplained_native_flow_txs,
                r.unknown_basis_lots_created,
                r.open_episodes,
            );
        }
    }
}
