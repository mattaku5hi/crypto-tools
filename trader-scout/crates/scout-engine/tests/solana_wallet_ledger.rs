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
    AMM_BUY_DISCRIMINATOR, AMM_BUY_EVENT_DISCRIMINATOR, AMM_SELL_DISCRIMINATOR,
    AMM_SELL_EVENT_DISCRIMINATOR, AmmAttribution, AmmTradeEventPairing,
    BUY_INSTRUCTION_DISCRIMINATOR, BUY_V2_INSTRUCTION_DISCRIMINATOR, EVENT_CPI_DISCRIMINATOR,
    JUPITER_EVENT_AUTHORITY_BYTES, JUPITER_SWAPS_EVENT_DISCRIMINATOR, JUPITER_V6_PROGRAM_ID_BYTES,
    PUMP_AMM_PROGRAM_ID_BYTES, SELL_INSTRUCTION_DISCRIMINATOR, TRADE_EVENT_DISCRIMINATOR,
    TradeEventPairing, TradeSide, WRAPPED_SOL_MINT, pair_trades_with_events,
    reconcile_pump_amm_transaction,
};
use scout_engine::{
    EpisodeOutcome, LedgerDecoders, LedgerOptions, PUMP_BONDING_CURVE_PROGRAM_ID, QuoteUnit,
    RouteRejections, RouteSwapRecord, SolanaWalletLedgerReport, USDC_MINT, USDT_MINT,
    UnknownReason, Venue, allocate_fee_proportionally, build_solana_wallet_ledger,
    build_solana_wallet_ledger_venues, build_solana_wallet_ledger_with_options, format_quote_money,
    lamports_to_money, pump_amm_decoder, pump_bonding_curve_decoder, solana_mainnet_chain,
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
            block_time: None,
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
        block_time: None,
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
                    + t.malformed_consideration
                    + t.route_leg_not_wallet_price,
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

fn run_windowed(txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
    build_solana_wallet_ledger_with_options(
        &pk(W),
        txs,
        &pump_bonding_curve_decoder().unwrap(),
        LedgerOptions {
            left_censoring: true,
        },
    )
    .unwrap()
}

fn golden_buy_sell() -> Vec<RawSolanaTransaction> {
    vec![
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
    ]
}

#[test]
fn windowed_sell_only_is_left_censored_not_closed_unknown() {
    // Pre-window buy is not in the input: the sell (net 1_177_000) exceeds
    // the observed inventory.
    let sell = golden_buy_sell().remove(1);
    let r = run_windowed(std::slice::from_ref(&sell));
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.closed_episodes_unknown, 0);
    assert_eq!(r.left_censored_episodes, 1);
    assert_eq!(r.left_censored_amount_raw, 1000);
    assert_eq!(r.episodes.len(), 1);
    assert_eq!(r.episodes[0].outcome, EpisodeOutcome::LeftCensored);
    assert_eq!(r.episodes[0].left_censored_amount_raw, 1000);
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::LeftCensored)
    );
    assert!(
        !r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::UnknownBasisLotConsumed)
    );
    assert!(!r.has_unknown_basis_inventory, "not an in-window cause");
    assert!(r.has_left_censored_inventory);
    assert_eq!(r.unknown_basis_lots_created, 0);
    assert_eq!(r.diagnostics.continuity_breaks, 0);
    assert_eq!(r.diagnostics.left_censored_disposals, 1);
    // Never valued: no PnL, no basis, no win rate.
    assert_eq!(r.realized_trade_pnl_lamports, 0);
    assert_eq!(r.consumed_acquisition_basis_lamports, 0);
    assert_eq!(r.win_rate, scout_analytics::RatioStatus::Undefined);
    assert!(r.open_positions.is_empty());
    // Same input without the window keeps the ADR-010 classification.
    let plain = run(std::slice::from_ref(&sell));
    assert_eq!(plain.closed_episodes_unknown, 1);
    assert_eq!(plain.left_censored_episodes, 0);
    assert!(plain.has_unknown_basis_inventory);
    assert!(!plain.has_left_censored_inventory);
    assert!(
        plain.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::InventoryNotObserved)
    );
}

#[test]
fn windowed_buy_and_sell_inside_window_matches_the_unwindowed_numbers() {
    let txs = golden_buy_sell();
    let w = run_windowed(&txs);
    let p = run(&txs);
    assert_eq!(w.closed_episodes_known, 1);
    assert_eq!(w.left_censored_episodes, 0);
    assert!(!w.has_left_censored_inventory);
    assert_eq!(w.realized_trade_pnl_lamports, 157_000);
    assert_eq!(w.realized_trade_pnl_exact, p.realized_trade_pnl_exact);
    assert_eq!(w.consumed_acquisition_basis_lamports, 1_020_000);
    assert_eq!(w.win_rate, p.win_rate);
    assert_eq!(w.median_holding_seconds, Some(600));
    assert_eq!(w.episodes, p.episodes);
}

#[test]
fn windowed_partial_shortfall_mixes_known_and_censored_lots_into_left_censored() {
    // In-window buy of 1000; the sell disposes 1500: 500 predates the window.
    let buy = golden_buy_sell().remove(0);
    let sell = trade_tx(
        2,
        101,
        TradeSide::Sell,
        M1,
        1500,
        1500,
        1_200_000,
        0,
        0,
        1600,
        0,
        OTHER,
    );
    let r = run_windowed(&[buy, sell]);
    assert_eq!(r.left_censored_episodes, 1);
    assert_eq!(r.left_censored_amount_raw, 500);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.realized_trade_pnl_lamports, 0);
}

#[test]
fn windowed_transfer_in_inside_window_then_sell_stays_closed_unknown() {
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
    let r = run_windowed(&[transfer_in, sell]);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.left_censored_episodes, 0);
    assert_eq!(r.closed_episodes_known, 0);
    assert!(r.has_unknown_basis_inventory);
    assert_eq!(r.unknown_basis_lots_created, 1);
    assert_eq!(r.episodes[0].outcome, EpisodeOutcome::ClosedUnknown);
}

#[test]
fn windowed_censored_shortfall_with_in_window_unknown_proceeds_is_closed_unknown() {
    // Sell without a paired event: in-window cause, plus a pre-window lot.
    let sell = Tx {
        sig: 5,
        slot: 5,
        index: 0,
        ixs: vec![trade_ix(TradeSide::Sell, None, W, M1, 0)],
        bals: vec![bal(M1, W, 300, 0)],
        fee: 0,
        payer: OTHER,
        native: vec![],
        ok: true,
    }
    .build();
    let r = run_windowed(&[sell]);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.left_censored_episodes, 0);
    assert!(r.has_unknown_basis_inventory);
    assert!(
        r.has_left_censored_inventory,
        "the shortfall is still reported"
    );
    assert_eq!(r.left_censored_amount_raw, 300);
}

#[test]
fn left_censored_daily_activity_is_reported_per_utc_day() {
    // ts 1000 and 1600 both fall on UTC day 0.
    let r = run_windowed(&golden_buy_sell());
    assert_eq!(r.daily_activity.len(), 1);
    assert_eq!(r.daily_activity[0].day, 0);
    assert_eq!(r.daily_activity[0].trades, 2);
    assert_eq!(r.daily_activity[0].distinct_mints, 1);
}

// ---------------------------------------------------------------------
// ADR-012: PumpSwap AMM venue (synthetic golden cases, hand-computed).
// ---------------------------------------------------------------------

const M3: u8 = 12;
const M4: u8 = 13;
const M5: u8 = 14;
const M6: u8 = 15;
const ROUTER: u8 = 77;

#[derive(Clone, Copy)]
struct Amm {
    buy: bool,
    user: u8,
    base: SolanaPubkey,
    quote: SolanaPubkey,
    base_amount: u64,
    /// `quote_amount_in_with_lp_fee` (buy) / `quote_amount_out_without_lp_fee` (sell).
    quote_core: u64,
    protocol_fee: u64,
    creator_fee: u64,
    ts: i64,
}

fn amm_pair(a: Amm, idx: u32) -> Vec<RawSolanaInstruction> {
    let (pool, ub, uq) = (pk(0xa0), pk(0xb0), pk(0xb1));
    let mut accounts: Vec<SolanaPubkey> = (0..23u8).map(|i| pk(100 + i)).collect();
    accounts[0] = pool;
    accounts[1] = pk(a.user);
    accounts[3] = a.base;
    accounts[4] = a.quote;
    accounts[5] = ub;
    accounts[6] = uq;
    let mut data = if a.buy {
        AMM_BUY_DISCRIMINATOR.to_vec()
    } else {
        AMM_SELL_DISCRIMINATOR.to_vec()
    };
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&2u64.to_le_bytes());
    if a.buy {
        data.push(1);
    }
    let trade = RawSolanaInstruction {
        program_id: PUMP_AMM_PROGRAM_ID_BYTES,
        accounts,
        data,
        instruction_index: idx,
    };
    let le = |v: u64| v.to_le_bytes();
    let mut p = Vec::new();
    p.extend(a.ts.to_le_bytes());
    // base, limit, 4 reserves, quote (with/without lp, copy), lp bps, lp fee,
    // protocol bps, protocol fee, quote with/without lp, user quote.
    for v in [
        a.base_amount,
        0,
        11,
        12,
        13,
        14,
        a.quote_core,
        20,
        0,
        5,
        a.protocol_fee,
        a.quote_core,
        0,
    ] {
        p.extend(le(v));
    }
    for k in [pool, pk(a.user), ub, uq, pk(0xf1), pk(0xf2), pk(0xcc)] {
        p.extend(k);
    }
    p.extend(le(30));
    p.extend(le(a.creator_fee));
    let mut ev = EVENT_CPI_DISCRIMINATOR.to_vec();
    ev.extend(if a.buy {
        AMM_BUY_EVENT_DISCRIMINATOR
    } else {
        AMM_SELL_EVENT_DISCRIMINATOR
    });
    ev.extend(p);
    let event = RawSolanaInstruction {
        program_id: PUMP_AMM_PROGRAM_ID_BYTES,
        accounts: vec![pk(0xea)],
        data: ev,
        instruction_index: idx + 1,
    };
    vec![trade, event]
}

fn bal_pk(mint: SolanaPubkey, owner: u8, pre: u64, post: u64) -> SolanaTokenBalanceChange {
    SolanaTokenBalanceChange {
        mint,
        owner: Some(pk(owner)),
        decimals: 6,
        pre_amount: Some(pre),
        post_amount: post,
        closed: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn amm_tx(
    sig: u8,
    slot: u64,
    ixs: Vec<RawSolanaInstruction>,
    bals: Vec<SolanaTokenBalanceChange>,
    native_w: i64,
    fee: u64,
    payer: u8,
) -> RawSolanaTransaction {
    Tx {
        sig,
        slot,
        index: 0,
        ixs,
        bals,
        fee,
        payer,
        native: vec![nat(W, native_w)],
        ok: true,
    }
    .build()
}

fn run_both(txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    build_solana_wallet_ledger_venues(
        &pk(W),
        txs,
        &LedgerDecoders {
            curve: &curve,
            amm: Some(&amm),
        },
        LedgerOptions::default(),
    )
    .unwrap()
}

#[test]
fn amm_golden_curve_buy_then_amm_sell_is_one_closed_known_episode() {
    // Curve buy 1000: cost 1_000_000 + 10_000 + 5_000 = 1_015_000 + fee 5_000 => basis 1_020_000.
    // AMM sell 1000 (normal pool): proceeds = 1_300_000 - 13_000 - 7_000 = 1_280_000,
    // network fee 5_000 => 1_275_000. PnL = 1_275_000 - 1_020_000 = 255_000.
    let sell = amm_tx(
        2,
        101,
        amm_pair(
            Amm {
                buy: false,
                user: W,
                base: pk(M1),
                quote: WRAPPED_SOL_MINT,
                base_amount: 1000,
                quote_core: 1_300_000,
                protocol_fee: 13_000,
                creator_fee: 7_000,
                ts: 1600,
            },
            0,
        ),
        vec![
            bal(M1, W, 1000, 0),
            bal_pk(WRAPPED_SOL_MINT, W, 3_000_000, 4_280_000),
        ],
        -5_000,
        5_000,
        W,
    );
    let buy = trade_tx(
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
    let r = run_both(&[sell, buy]);
    assert_eq!(r.closed_episodes_known, 1);
    assert_eq!(r.closed_episodes_unknown, 0);
    assert_eq!(r.left_censored_episodes, 0);
    assert_eq!(r.open_episodes, 0);
    assert_eq!(r.realized_trade_pnl_lamports, 255_000);
    assert_eq!(r.realized_trade_pnl_exact, lam(255_000));
    assert_eq!(r.consumed_acquisition_basis_lamports, 1_020_000);
    assert_eq!(r.median_holding_seconds, Some(600));
    assert_eq!(r.trades.bonding_curve.buys, 1);
    assert_eq!(r.trades.bonding_curve.sells, 0);
    assert_eq!(r.trades.pump_amm.sells, 1);
    assert_eq!(r.trades.pump_amm.buys, 0);
    assert_eq!((r.trades.buys, r.trades.sells, r.trades.priced), (1, 1, 2));
    let keys: Vec<(Venue, &str, u64)> = r
        .variant_trades
        .iter()
        .map(|v| (v.venue, v.variant, v.trades))
        .collect();
    assert_eq!(
        keys,
        vec![(Venue::BondingCurve, "buy", 1), (Venue::PumpAmm, "sell", 1)]
    );
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 0);
    assert_eq!(r.diagnostics.unexplained_native_flow_txs, 0);
    assert_eq!(r.diagnostics.continuity_breaks, 0);
    assert_eq!(r.diagnostics.out_of_scope_token_movements, 0);
    // The curve-only entry point does not see the AMM leg: the sale is a
    // continuity break, never a known PnL.
    let curve_only = run(&[
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
        amm_tx(2, 101, vec![], vec![bal(M1, W, 1000, 0)], -5_000, 5_000, W),
    ]);
    assert_eq!(curve_only.closed_episodes_known, 0);
    assert_eq!(curve_only.closed_episodes_unknown, 1);
}

#[test]
fn amm_golden_reversed_pool_acquire_then_dispose_exact_sol_numbers() {
    // Pool base = wSOL, quote = token M2.
    // Acquire = instruction `sell`: SOL cost = base_amount_in = 2_000_000,
    //   tokens = 5_100 - 60 - 40 = 5_000; + network fee 5_000 => basis 2_005_000.
    // Dispose = instruction `buy`: SOL proceeds = base_amount_out = 2_500_000,
    //   tokens given = 4_900 + 60 + 40 = 5_000; - fee 5_000 => 2_495_000.
    // PnL = 2_495_000 - 2_005_000 = 490_000.
    let acquire = amm_tx(
        1,
        100,
        amm_pair(
            Amm {
                buy: false,
                user: W,
                base: WRAPPED_SOL_MINT,
                quote: pk(M2),
                base_amount: 2_000_000,
                quote_core: 5_100,
                protocol_fee: 60,
                creator_fee: 40,
                ts: 2000,
            },
            0,
        ),
        vec![
            bal_pk(WRAPPED_SOL_MINT, W, 4_000_000, 2_000_000),
            bal(M2, W, 0, 5_000),
        ],
        -5_000,
        5_000,
        W,
    );
    let dispose = amm_tx(
        2,
        101,
        amm_pair(
            Amm {
                buy: true,
                user: W,
                base: WRAPPED_SOL_MINT,
                quote: pk(M2),
                base_amount: 2_500_000,
                quote_core: 4_900,
                protocol_fee: 60,
                creator_fee: 40,
                ts: 2300,
            },
            0,
        ),
        vec![
            bal_pk(WRAPPED_SOL_MINT, W, 2_000_000, 4_500_000),
            bal(M2, W, 5_000, 0),
        ],
        -5_000,
        5_000,
        W,
    );
    let r = run_both(&[acquire, dispose]);
    assert_eq!(r.closed_episodes_known, 1);
    assert_eq!(r.realized_trade_pnl_lamports, 490_000);
    assert_eq!(r.consumed_acquisition_basis_lamports, 2_005_000);
    assert_eq!(r.trades.pump_amm.buys, 1);
    assert_eq!(r.trades.pump_amm.sells, 1);
    assert_eq!(r.diagnostics.reversed_pool_trades, 2);
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 0);
    assert_eq!(r.diagnostics.continuity_breaks, 0);
    assert_eq!(r.median_holding_seconds, Some(300));
    let m2 = &r.episodes[0];
    assert_eq!(m2.mint, pk(M2));
}

#[test]
fn amm_non_sol_pair_is_recorded_with_unknown_pnl() {
    let buy = amm_tx(
        1,
        100,
        amm_pair(
            Amm {
                buy: true,
                user: W,
                base: pk(M3),
                quote: pk(M4),
                base_amount: 700,
                quote_core: 1_000,
                protocol_fee: 10,
                creator_fee: 5,
                ts: 10,
            },
            0,
        ),
        vec![bal(M3, W, 0, 700), bal(M4, W, 5_000, 3_985)],
        -5_000,
        5_000,
        W,
    );
    let sell = amm_tx(
        2,
        101,
        amm_pair(
            Amm {
                buy: false,
                user: W,
                base: pk(M3),
                quote: pk(M4),
                base_amount: 700,
                quote_core: 1_200,
                protocol_fee: 12,
                creator_fee: 6,
                ts: 20,
            },
            0,
        ),
        vec![bal(M3, W, 700, 0), bal(M4, W, 3_985, 5_167)],
        -5_000,
        5_000,
        W,
    );
    let r = run_both(&[buy, sell]);
    assert_eq!(r.trades.unsupported_quote, 2);
    assert_eq!(r.trades.priced, 0);
    assert_eq!(r.trades.pump_amm.buys + r.trades.pump_amm.sells, 2);
    assert_eq!(r.closed_episodes_known, 0);
    assert_eq!(r.closed_episodes_unknown, 1);
    assert_eq!(r.realized_trade_pnl_lamports, 0);
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::UnsupportedQuoteAsset)
    );
}

#[test]
fn amm_quote_funded_elsewhere_is_unknown_basis_never_zero() {
    // Another account (the fee payer) paid the quote; the wallet only
    // received 1000 tokens. Basis is unknown, not 0 and not the event cost.
    let funded = |sig: u8| {
        amm_tx(
            sig,
            100,
            amm_pair(
                Amm {
                    buy: true,
                    user: W,
                    base: pk(M5),
                    quote: WRAPPED_SOL_MINT,
                    base_amount: 1000,
                    quote_core: 900_000,
                    protocol_fee: 5_000,
                    creator_fee: 5_000,
                    ts: 100,
                },
                0,
            ),
            vec![bal(M5, W, 0, 1000)],
            0,
            5_000,
            OTHER,
        )
    };
    let open = run_both(&[funded(1)]);
    assert_eq!(open.trades.quote_funded_elsewhere, 1);
    assert_eq!(open.trades.priced, 0);
    assert_eq!(open.diagnostics.quote_funded_elsewhere_trades, 1);
    assert_eq!(open.unknown_basis_lots_created, 1);
    assert_eq!(open.open_positions.len(), 1);
    assert_eq!(open.open_positions[0].open_amount_raw, 1000);
    assert_eq!(open.open_positions[0].unknown_basis_amount_raw, 1000);
    assert!(open.has_unknown_basis_inventory);

    let closed = run_both(&[
        funded(1),
        trade_tx(
            2,
            101,
            TradeSide::Sell,
            M5,
            1000,
            1000,
            1_000_000,
            0,
            0,
            200,
            5_000,
            OTHER,
        ),
    ]);
    assert_eq!(closed.closed_episodes_known, 0);
    assert_eq!(closed.closed_episodes_unknown, 1);
    assert_eq!(closed.realized_trade_pnl_lamports, 0);
    assert!(
        closed.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::QuoteFundedByAnotherAccount)
    );
}

#[test]
fn amm_router_forward_is_not_attributed_and_transfer_is_a_continuity_lot() {
    // The decoded user is the router (not a signer, nothing moves for it);
    // the wallet signed and received 500 tokens.
    let mut tx = amm_tx(
        1,
        100,
        amm_pair(
            Amm {
                buy: true,
                user: ROUTER,
                base: pk(M6),
                quote: WRAPPED_SOL_MINT,
                base_amount: 500,
                quote_core: 1_000,
                protocol_fee: 10,
                creator_fee: 5,
                ts: 7,
            },
            0,
        ),
        vec![bal(M6, W, 0, 500)],
        -5_000,
        5_000,
        W,
    );
    tx.signers = vec![pk(W)];
    let r = run_both(&[tx]);
    assert_eq!(r.diagnostics.router_forward_trades_not_attributed, 1);
    assert_eq!(r.trades.pump_amm.buys + r.trades.pump_amm.sells, 0);
    assert_eq!(r.trades.priced, 0);
    assert_eq!(r.diagnostics.continuity_breaks, 1);
    assert_eq!(r.diagnostics.out_of_scope_token_movements, 0);
    assert_eq!(r.open_positions.len(), 1);
    assert_eq!(r.open_positions[0].unknown_basis_amount_raw, 500);
}

#[test]
fn amm_and_curve_trades_in_one_tx_split_the_network_fee_exactly() {
    // Curve buy M1 100 tokens for 1_000_000 and AMM buy M2 200 tokens for
    // 2_980_000 + 12_000 + 8_000 = 3_000_000; fee 5_001 split 1250 / 3750,
    // remainder 1 to the earliest instruction (the curve trade) = 1251.
    // Later sponsored sells at exactly cost => pnl = -share per mint.
    let mut ixs = vec![
        trade_ix(TradeSide::Buy, None, W, M1, 0),
        event_ix(
            &Ev {
                mint: M1,
                user: W,
                is_buy: true,
                sol: 1_000_000,
                tokens: 100,
                fee: 0,
                creator_fee: 0,
                ts: 50,
            },
            1,
        ),
    ];
    ixs.extend(amm_pair(
        Amm {
            buy: true,
            user: W,
            base: pk(M2),
            quote: WRAPPED_SOL_MINT,
            base_amount: 200,
            quote_core: 2_980_000,
            protocol_fee: 12_000,
            creator_fee: 8_000,
            ts: 50,
        },
        2,
    ));
    let buy = amm_tx(
        1,
        10,
        ixs,
        vec![
            bal(M1, W, 0, 100),
            bal(M2, W, 0, 200),
            bal_pk(WRAPPED_SOL_MINT, W, 5_000_000, 2_000_000),
        ],
        -1_005_001,
        5_001,
        W,
    );
    let curve_sell = trade_tx(
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
    );
    let amm_sell = amm_tx(
        3,
        12,
        amm_pair(
            Amm {
                buy: false,
                user: W,
                base: pk(M2),
                quote: WRAPPED_SOL_MINT,
                base_amount: 200,
                quote_core: 3_012_000,
                protocol_fee: 8_000,
                creator_fee: 4_000,
                ts: 60,
            },
            0,
        ),
        vec![
            bal(M2, W, 200, 0),
            bal_pk(WRAPPED_SOL_MINT, W, 2_000_000, 5_000_000),
        ],
        0,
        5_000,
        OTHER,
    );
    let r = run_both(&[buy, curve_sell, amm_sell]);
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 0);
    assert_eq!(r.diagnostics.unexplained_native_flow_txs, 0);
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
    assert_eq!(r.trades.bonding_curve.buys, 1);
    assert_eq!(r.trades.pump_amm.buys, 1);
    assert_eq!(
        (r.trades.bonding_curve.sells, r.trades.pump_amm.sells),
        (1, 1)
    );
}

// ---------------------------------------------------------------------
// ADR-012 live fixture: wallet page through the real HeliusProvider path.
// ---------------------------------------------------------------------

const PUMPSWAP_WALLET: &str = "2tgUbS9UMoQD6GkDZBiqKYCURnGrSb6ocYwRABrSJUvY";

#[tokio::test]
async fn live_pumpswap_wallet_page_ledger_numbers_and_invariants() {
    let wallet = pubkey(PUMPSWAP_WALLET);
    let all = fixture_txs("pumpswap_wallet_page_2026-10-02.json").await;
    assert_eq!(all.len(), 100);
    // Windowed mode: since = oldest blockTime of the page, so inventory
    // acquired before the page is left-censored, not "not observed".
    let since = all.iter().filter_map(|t| t.block_time).min().unwrap();
    let txs: Vec<RawSolanaTransaction> = all
        .into_iter()
        .filter(|t| t.block_time.is_some_and(|b| b >= since))
        .collect();
    assert_eq!(txs.len(), 100);
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let decoders = LedgerDecoders {
        curve: &curve,
        amm: Some(&amm),
    };
    let opts = LedgerOptions {
        left_censoring: true,
    };
    let r = build_solana_wallet_ledger_venues(&wallet, &txs, &decoders, opts).unwrap();

    // Independent view: every wallet-user paired trade the reconciler does
    // not call a router forward must appear exactly once in the report.
    let mut seen = std::collections::BTreeSet::new();
    let mut expected_attributed = 0u64;
    let mut expected_priced = 0u64;
    let mut exact_txs: Vec<&RawSolanaTransaction> = Vec::new();
    let mut inexact_txs = 0u64;
    for tx in &txs {
        let rec = reconcile_pump_amm_transaction(&amm, tx);
        let mut all_exact = true;
        let mut touched = false;
        for u in rec.users.iter().filter(|u| u.user == wallet) {
            touched = true;
            all_exact &= u.attribution == AmmAttribution::Exact;
            if u.attribution == AmmAttribution::NoUserDelta {
                continue;
            }
            for &i in &u.trade_indices {
                let p = &rec.pairing.trades[i];
                assert!(
                    seen.insert((tx.signature, p.trade.instruction_index)),
                    "trade counted twice"
                );
                expected_attributed += 1;
                if matches!(p.pairing, AmmTradeEventPairing::Paired(_))
                    && matches!(
                        u.attribution,
                        AmmAttribution::Exact | AmmAttribution::QuoteResidual
                    )
                {
                    expected_priced += 1;
                }
            }
        }
        if touched {
            if all_exact {
                exact_txs.push(tx);
            } else {
                inexact_txs += 1;
            }
        }
    }
    let amm_total = r.trades.pump_amm.buys + r.trades.pump_amm.sells;
    assert_eq!(amm_total, expected_attributed);
    assert_eq!(r.trades.priced, expected_priced);
    assert_eq!(
        r.variant_trades.iter().map(|v| v.trades).sum::<u64>(),
        amm_total
    );
    assert_eq!(
        r.trades.priced
            + r.trades.unpaired
            + r.trades.mismatched
            + r.trades.unsupported_quote
            + r.trades.malformed_consideration
            + r.trades.quote_funded_elsewhere
            + r.trades.unreconciled,
        r.trades.buys + r.trades.sells
    );
    assert_eq!(
        r.trades.bonding_curve.buys + r.trades.bonding_curve.sells,
        0
    );

    // Exact reconciliation => the wallet's native residual is 0 per tx.
    for tx in &exact_txs {
        let one =
            build_solana_wallet_ledger_venues(&wallet, std::slice::from_ref(*tx), &decoders, opts)
                .unwrap();
        assert_eq!(
            one.diagnostics.unexplained_native_flow_lamports,
            0,
            "tx {}",
            bs58::encode(tx.signature).into_string()
        );
    }
    // All 98 trade txs reconcile Exactly; the aggregate native residual comes
    // only from the page's non-trade transactions (plain SOL movements).
    assert_eq!((exact_txs.len(), inexact_txs), (98, 0));
    let mut non_trade_residual = 0i128;
    let mut non_trade_txs = 0u64;
    for tx in &txs {
        if exact_txs.iter().any(|e| e.signature == tx.signature) {
            continue;
        }
        let one =
            build_solana_wallet_ledger_venues(&wallet, std::slice::from_ref(tx), &decoders, opts)
                .unwrap();
        non_trade_residual += one.diagnostics.unexplained_native_flow_lamports;
        non_trade_txs += one.diagnostics.unexplained_native_flow_txs;
    }
    assert_eq!(
        non_trade_residual,
        r.diagnostics.unexplained_native_flow_lamports
    );
    assert_eq!(non_trade_txs, r.diagnostics.unexplained_native_flow_txs);

    // No continuity-caused Unknown on PumpSwap-only mints.
    assert_eq!(r.closed_episodes_unknown, 0);
    assert_eq!(r.diagnostics.continuity_breaks, 0);
    for e in &r.episodes {
        assert!(
            e.unknown_reasons
                .iter()
                .all(|x| *x == UnknownReason::LeftCensored),
            "{:?}",
            e.unknown_reasons
        );
    }

    // Pinned live numbers (measured 2026-10-02 on this fixture).
    assert_eq!((r.trades.pump_amm.buys, r.trades.pump_amm.sells), (53, 45));
    assert_eq!(r.closed_episodes_known, 9);
    assert_eq!(r.left_censored_episodes, 26);
    assert_eq!(r.open_episodes, 13);
    assert_eq!(r.realized_trade_pnl_lamports, 182_081_259);
    assert_eq!(r.diagnostics.unexplained_native_flow_lamports, 7_216_315);
    assert_eq!(r.diagnostics.unexplained_native_flow_txs, 2);
    assert_eq!(r.diagnostics.router_forward_trades_not_attributed, 0);
    assert_eq!(r.diagnostics.quote_funded_elsewhere_trades, 0);
    assert_eq!(r.diagnostics.reversed_pool_trades, 0);
    assert_eq!(r.diagnostics.out_of_scope_token_movements, 0);
}

// ---------------------------------------------------------------------
// Atomic round-trip cohort signal (diagnostic, not PnL).
// ---------------------------------------------------------------------

fn venue_ix(id: &str, idx: u32) -> RawSolanaInstruction {
    RawSolanaInstruction {
        program_id: pubkey(id),
        accounts: vec![pk(W)],
        data: vec![0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8],
        instruction_index: idx,
    }
}

const DLMM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
const WHIRLPOOL_ID: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

fn round_trip_tx(
    sig: u8,
    ixs: Vec<RawSolanaInstruction>,
    bals: Vec<SolanaTokenBalanceChange>,
    native_delta: i64,
    fee: u64,
    ok: bool,
) -> RawSolanaTransaction {
    Tx {
        sig,
        slot: u64::from(sig),
        index: 0,
        ixs,
        bals,
        fee,
        payer: W,
        native: vec![nat(W, native_delta)],
        ok,
    }
    .build()
}

fn round_trip_report(txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
    run_both(txs)
}

#[test]
fn atomic_round_trip_counts_only_zero_token_delta_with_sol_delta() {
    let two_venues = || vec![venue_ix(DLMM_ID, 0), venue_ix(WHIRLPOOL_ID, 1)];
    // Counted: two venues, no token movement, +7_000 net of a 5_000 fee.
    let arb_gain = round_trip_tx(1, two_venues(), vec![], 7_000, 5_000, true);
    // Counted: wSOL-only movement is SOL, not a token (-3_000 native incl.
    // fee, +1_000 wSOL).
    let arb_wsol = round_trip_tx(
        2,
        two_venues(),
        vec![bal_pk(WRAPPED_SOL_MINT, W, 0, 1_000)],
        -3_000,
        5_000,
        true,
    );
    // Not counted: a token moved (directional swap through two venues).
    let swap = round_trip_tx(
        3,
        two_venues(),
        vec![bal(M1, W, 0, 50)],
        -9_000,
        5_000,
        true,
    );
    // Not counted: a single venue.
    let single = round_trip_tx(4, vec![venue_ix(DLMM_ID, 0)], vec![], 7_000, 5_000, true);
    // Not counted: SOL delta exactly zero.
    let flat = round_trip_tx(5, two_venues(), vec![], 0, 0, true);
    // Not counted: failed transaction.
    let failed = round_trip_tx(6, two_venues(), vec![], 7_000, 5_000, false);
    let r = round_trip_report(&[arb_gain, arb_wsol, swap, single, flat, failed]);
    assert_eq!(r.diagnostics.atomic_round_trip_txs, 2);
    assert_eq!(
        r.diagnostics.atomic_round_trip_sol_lamports,
        7_000 - 3_000 + 1_000
    );
    // Diagnostic only: nothing booked.
    assert_eq!(r.closed_episodes_known + r.closed_episodes_unknown, 0);
}

#[test]
fn atomic_round_trip_requires_wallet_signature() {
    let mut tx = round_trip_tx(
        1,
        vec![venue_ix(DLMM_ID, 0), venue_ix(WHIRLPOOL_ID, 1)],
        vec![],
        7_000,
        0,
        true,
    );
    tx.signers = vec![pk(OTHER)];
    tx.fee_payer = pk(OTHER);
    let r = round_trip_report(&[tx]);
    assert_eq!(r.diagnostics.atomic_round_trip_txs, 0);
    assert_eq!(r.diagnostics.atomic_round_trip_sol_lamports, 0);
}

const ROUTER_WALLET_9OC3: &str = "9oC3XYAs2oeU39NeNFke8m3JGixMq7g8PfANsmsbKR8W";
const ROUTER_WALLET_TAWV: &str = "tAwv75TULEMoR5bU8sNgPrDM2d8gdagDUy4b3qo8VXY";

async fn router_wallet_report(name: &str, wallet: &str) -> SolanaWalletLedgerReport {
    let txs = fixture_txs(name).await;
    assert_eq!(txs.len(), 100);
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let decoders = LedgerDecoders {
        curve: &curve,
        amm: Some(&amm),
    };
    let opts = LedgerOptions {
        left_censoring: true,
    };
    build_solana_wallet_ledger_venues(&pubkey(wallet), &txs, &decoders, opts).unwrap()
}

/// GMGN "top trader" wallets that trade USDC <-> tokens through routers
/// (2026-10-02 pages, newest 100 txs each). Evidence: 34 / 65 successful
/// signed transactions touch two or more venue programs, but every one of
/// them moves a non-wSOL token (USDC or the traded token) for the wallet and
/// none leaves the wallet's token deltas at zero with a SOL gain: these
/// pages contain no atomic arbitrage. The wallets are not the fee payer
/// (relayer pays), so SOL moves only by ATA rent / tips.
#[tokio::test]
async fn router_wallet_pages_contain_no_atomic_round_trips() {
    let a = router_wallet_report(
        "router_wallet_9oC3_page_2026-10-02.json",
        ROUTER_WALLET_9OC3,
    )
    .await;
    let b = router_wallet_report(
        "router_wallet_tAwv_page_2026-10-02.json",
        ROUTER_WALLET_TAWV,
    )
    .await;
    for r in [&a, &b] {
        assert_eq!(r.diagnostics.atomic_round_trip_txs, 0);
        assert_eq!(r.diagnostics.atomic_round_trip_sol_lamports, 0);
    }
    // ADR-015 (ledger/6): +2 / +6 route swaps whose only swap-leg evidence is
    // a Jupiter v6 event (36 / 62 under ledger/5, see tests/jupiter_swap_legs.rs).
    // ADR-013 (ledger/4): every successful signed tx with a FixtureVerified
    // pump leg on these pages is a route swap booked from the wallet's own
    // USDC/token deltas. ledger/3 had priced 3+7 of them from a hop's event
    // (SOL, wrong), left 12+42 unreconciled, 6+5 funded elsewhere, 6+0
    // unsupported quote and 8+8 router-forward.
    assert_eq!(
        (
            a.trades.priced,
            a.trades.route_swaps,
            a.trades.route_swaps_by_quote.usdc,
            a.trades.unreconciled,
            a.trades.quote_funded_elsewhere,
            a.trades.unsupported_quote,
            a.diagnostics.router_forward_trades_not_attributed,
        ),
        (38, 38, 38, 0, 0, 0, 0)
    );
    assert_eq!(
        (
            b.trades.priced,
            b.trades.route_swaps,
            b.trades.route_swaps_by_quote.usdc,
            b.trades.unreconciled,
            b.trades.quote_funded_elsewhere,
            b.trades.unsupported_quote,
            b.diagnostics.router_forward_trades_not_attributed,
        ),
        (68, 68, 68, 0, 0, 0, 0)
    );
}

fn sig_str(sig: &[u8; 64]) -> String {
    bs58::encode(sig).into_string()
}

fn route_rec<'a>(r: &'a SolanaWalletLedgerReport, prefix: &str) -> &'a RouteSwapRecord {
    r.route_swap_log
        .iter()
        .find(|x| sig_str(&x.signature).starts_with(prefix))
        .unwrap_or_else(|| panic!("{prefix} is not a booked route swap"))
}

/// ADR-013 fixture evidence on both router pages (ground truth from the
/// wallets' own balance deltas).
#[tokio::test]
async fn router_pages_are_booked_wallet_side_in_usdc() {
    let a = router_wallet_report(
        "router_wallet_9oC3_page_2026-10-02.json",
        ROUTER_WALLET_9OC3,
    )
    .await;
    let b = router_wallet_report(
        "router_wallet_tAwv_page_2026-10-02.json",
        ROUTER_WALLET_TAWV,
    )
    .await;
    let exact = |r: &SolanaWalletLedgerReport, p: &str, side, tokens, usdc| {
        let x = route_rec(r, p);
        assert_eq!(
            (x.side, x.unit, x.token_amount, x.quote_amount),
            (side, QuoteUnit::UsdcUnits, tokens, usdc),
            "{p}"
        );
    };
    exact(
        &a,
        "4mWy9Cyk5j",
        TradeSide::Buy,
        10_711_610_293_044,
        5_000_000_000,
    );
    exact(
        &b,
        "5ZLd1GL9zm",
        TradeSide::Buy,
        6_657_065_375_403,
        1_500_000_000,
    );
    // Pass-through: the leg user is `ARu4n5mF...` (non-signer, nets 0).
    exact(
        &a,
        "2mD5CtKfFd",
        TradeSide::Sell,
        1_403_477_039_775,
        1_261_908_554,
    );
    // The 7 ledger/3 bug cases: USDC from the wallet's deltas, not SOL from
    // the 24.47 SOL hop event (wallet SOL moved only -550_840 of rent).
    for (p, tokens, usdc) in [
        ("4SSojKf9ao", 648_062_069_092, 3_000_000_000),
        ("2mTX4qouxE", 2_829_384_670_616, 3_000_000_000),
        ("4TrtWRJ6oP", 4_029_280_822_121, 3_000_000_000),
        ("5sM5m4jw2H", 1_942_621_723_810, 1_000_000_000),
        ("5ysTvFYiea", 4_005_614_455_616, 2_000_000_000),
        ("YtfpxhcUfz", 4_798_658_875_465, 2_000_000_000),
        ("3tBhzKhDAY", 10_997_446_852_615, 5_000_000_000),
    ] {
        exact(&b, p, TradeSide::Buy, tokens, usdc);
    }
    // Split routes: event base differs from the wallet delta; the delta wins.
    exact(
        &b,
        "4643cWP7",
        TradeSide::Buy,
        1_163_420_181_740,
        3_000_000_000,
    );
    exact(
        &b,
        "124TBXVa",
        TradeSide::Sell,
        9_345_876_563_219,
        28_264_643_855,
    );
    for r in [&a, &b] {
        assert_eq!(r.trades.route_swaps_by_quote.sol, 0);
        assert_eq!(r.trades.route_swaps_by_quote.usdt, 0);
        assert_eq!(
            u64::try_from(r.route_swap_log.len()).unwrap(),
            r.trades.route_swaps
        );
        assert_eq!(r.trades.route_leg_not_wallet_price, 0);
        assert_eq!(r.diagnostics.route_rejected, RouteRejections::default());
        assert!(
            r.route_swap_log
                .iter()
                .all(|x| x.unit == QuoteUnit::UsdcUnits)
        );
        assert_eq!(
            r.unit_block(QuoteUnit::Lamports)
                .unwrap()
                .closed_episodes_known,
            0
        );
        assert_eq!(r.realized_trade_pnl_lamports, 0);
    }
    let ua = a.unit_block(QuoteUnit::UsdcUnits).unwrap();
    assert_eq!(
        (
            ua.closed_episodes_known,
            ua.wins,
            ua.losses,
            ua.realized_trade_pnl_raw,
            ua.consumed_acquisition_basis_raw
        ),
        (5, 0, 5, -24_745_390_534, 62_532_744_053)
    );
    let ub = b.unit_block(QuoteUnit::UsdcUnits).unwrap();
    assert_eq!(
        (
            ub.closed_episodes_known,
            ub.wins,
            ub.losses,
            ub.realized_trade_pnl_raw,
            ub.consumed_acquisition_basis_raw
        ),
        // ledger/5 had (5, 2, 3, -7_030_355_795, 41_500_000_000); the 6 Jupiter-
        // evidenced route swaps (ADR-015) close one more episode, in profit.
        (6, 3, 3, 2_701_449_089, 83_500_000_000)
    );
    assert_eq!(
        format_quote_money(QuoteUnit::UsdcUnits, ua.realized_trade_pnl_exact).unwrap(),
        "-24745.390534"
    );
}

// ---------------------------------------------------------------------
// ADR-013 synthetic goldens: event guard, route swaps, quote units.
// ---------------------------------------------------------------------

const RELAYER: u8 = 88;
const PASS: u8 = 55;
const HOP_EVENT_SOL: u64 = 24_474_347_355;

fn usdc() -> SolanaPubkey {
    pubkey(USDC_MINT)
}

/// A route tx of wallet `W`: a curve leg of `leg_user` whose event says
/// 24.47 SOL, while the wallet really moves `tokens` of `mint` against
/// `quote_amt` raw units of `quote` (USDC/USDT), SOL only -550_840 of rent.
/// The relayer pays the fee; `signers` = relayer + wallet.
#[allow(clippy::too_many_arguments)]
fn route_tx(
    sig: u8,
    slot: u64,
    side: TradeSide,
    mint: u8,
    tokens: u64,
    pre_tokens: u64,
    quote: SolanaPubkey,
    quote_amt: u64,
    leg_user: u8,
) -> RawSolanaTransaction {
    let buy = side == TradeSide::Buy;
    let ev = Ev {
        mint,
        user: leg_user,
        is_buy: buy,
        sol: HOP_EVENT_SOL,
        tokens,
        fee: 0,
        creator_fee: 0,
        ts: 1_000 + i64::from(sig),
    };
    let post = if buy {
        pre_tokens + tokens
    } else {
        pre_tokens - tokens
    };
    let (q_pre, q_post) = if buy {
        (quote_amt * 2, quote_amt)
    } else {
        (0, quote_amt)
    };
    let mut bals = vec![
        bal(mint, W, pre_tokens, post),
        bal_pk(quote, W, q_pre, q_post),
    ];
    if leg_user == PASS {
        bals.push(bal(mint, PASS, 100, 100));
    }
    let mut tx = Tx {
        sig,
        slot,
        index: 0,
        ixs: vec![trade_ix(side, None, leg_user, mint, 0), event_ix(&ev, 1)],
        bals,
        fee: 5_000,
        payer: RELAYER,
        native: vec![nat(W, -550_840)],
        ok: true,
    }
    .build();
    tx.signers = vec![pk(RELAYER), pk(W)];
    tx
}

fn rejected(r: &SolanaWalletLedgerReport) -> RouteRejections {
    r.diagnostics.route_rejected
}

#[test]
fn guard_event_of_a_route_leg_is_not_the_wallet_price() {
    // Wallet is the leg user (event 24.47 SOL) but paid only 5_000 of fee in
    // SOL: no quote leg => never priced from the hop event.
    let ev = Ev {
        mint: M1,
        user: W,
        is_buy: true,
        sol: HOP_EVENT_SOL,
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
        bals: vec![bal(M1, W, 0, 1000)],
        fee: 5_000,
        payer: W,
        native: vec![nat(W, -5_000)],
        ok: true,
    }
    .build();
    let r = run(&[tx]);
    assert_eq!(r.trades.priced, 0);
    assert_eq!(r.trades.route_leg_not_wallet_price, 1);
    assert_eq!(r.trades.route_swaps, 0);
    assert_eq!(rejected(&r).no_quote_leg, 1);
    assert_eq!(r.open_positions.len(), 1);
    assert_eq!(r.open_positions[0].unknown_basis_amount_raw, 1000);
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::RouteLegNotWalletPrice)
    );
}

#[test]
fn guard_direct_sol_trade_keeps_event_pricing() {
    // Unchanged ledger/3 golden: payer W, buy 1_000_000 + fees.
    let tx = trade_tx(
        1,
        1,
        TradeSide::Buy,
        M1,
        1000,
        0,
        1_000_000,
        10_000,
        5_000,
        10,
        5_000,
        W,
    );
    let r = run(&[tx]);
    assert_eq!((r.trades.priced, r.trades.route_swaps), (1, 0));
    assert_eq!(r.trades.route_leg_not_wallet_price, 0);
}

#[test]
fn route_usdc_buy_is_booked_from_wallet_deltas_not_the_event() {
    let tx = route_tx(1, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    let r = run(&[tx]);
    assert_eq!(r.trades.route_swaps, 1);
    assert_eq!(r.trades.route_swaps_by_quote.usdc, 1);
    assert_eq!(r.trades.route_swaps_by_quote.sol, 0);
    assert_eq!(r.trades.priced, 1);
    let x = &r.route_swap_log[0];
    assert_eq!(
        (x.side, x.unit, x.token_amount, x.quote_amount),
        (TradeSide::Buy, QuoteUnit::UsdcUnits, 1000, 5_000_000)
    );
    assert_eq!(rejected(&r), RouteRejections::default());
    assert_eq!(r.open_positions[0].open_amount_raw, 1000);
    assert_eq!(r.open_positions[0].unknown_basis_amount_raw, 0);
}

#[test]
fn route_usdc_round_trip_is_closed_known_in_usdc_with_exact_pnl() {
    let buy = route_tx(1, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    let sell = route_tx(2, 2, TradeSide::Sell, M1, 1000, 1000, usdc(), 7_250_000, W);
    let r = run(&[buy, sell]);
    assert_eq!((r.closed_episodes_known, r.closed_episodes_unknown), (1, 0));
    let EpisodeOutcome::ClosedKnown { pnl } = r.episodes[0].outcome else {
        panic!("{:?}", r.episodes[0].outcome)
    };
    assert_eq!(r.episodes[0].quote_unit, Some(QuoteUnit::UsdcUnits));
    assert_eq!(
        format_quote_money(QuoteUnit::UsdcUnits, pnl).unwrap(),
        "2.250000"
    );
    let u = r.unit_block(QuoteUnit::UsdcUnits).unwrap();
    assert_eq!(
        (
            u.closed_episodes_known,
            u.wins,
            u.realized_trade_pnl_raw,
            u.consumed_acquisition_basis_raw
        ),
        (1, 1, 2_250_000, 5_000_000)
    );
    // The SOL block and legacy SOL figures stay empty: units are not mixed.
    assert_eq!(
        r.unit_block(QuoteUnit::Lamports)
            .unwrap()
            .closed_episodes_known,
        0
    );
    assert_eq!(r.realized_trade_pnl_lamports, 0);
    assert_eq!(r.closed_episodes_known, 1);
}

#[test]
fn route_passthrough_leg_user_with_zero_net_is_accepted() {
    let tx = route_tx(1, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, PASS);
    let r = run(&[tx]);
    assert_eq!(r.trades.route_swaps, 1);
    assert_eq!(rejected(&r), RouteRejections::default());
}

#[test]
fn route_usdt_is_its_own_unit() {
    let tx = route_tx(
        1,
        1,
        TradeSide::Buy,
        M1,
        1000,
        0,
        pubkey(USDT_MINT),
        2_000_000,
        W,
    );
    let r = run(&[tx]);
    assert_eq!(r.trades.route_swaps_by_quote.usdt, 1);
    assert_eq!(r.route_swap_log[0].unit, QuoteUnit::UsdtUnits);
}

#[test]
fn route_rejections_are_counted_and_not_booked() {
    // Pass-through that receives value.
    let mut tx = route_tx(1, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, PASS);
    tx.token_balance_changes.pop();
    tx.token_balance_changes.push(bal(M1, PASS, 0, 7));
    let r = run(&[tx]);
    assert_eq!(
        (r.trades.route_swaps, rejected(&r).passthrough_nonzero),
        (0, 1)
    );

    // Pass-through that signs.
    let mut tx = route_tx(2, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, PASS);
    tx.signers.push(pk(PASS));
    let r = run(&[tx]);
    assert_eq!(
        (r.trades.route_swaps, rejected(&r).passthrough_nonzero),
        (0, 1)
    );

    // Wallet is not a signer.
    let mut tx = route_tx(3, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    tx.signers = vec![pk(RELAYER)];
    let r = run(&[tx]);
    assert_eq!(
        (r.trades.route_swaps, rejected(&r).wallet_not_signer),
        (0, 1)
    );

    // Multi-asset: a second token moves for the wallet.
    let mut tx = route_tx(4, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    tx.token_balance_changes.push(bal(M2, W, 0, 9));
    let r = run(&[tx]);
    assert_eq!((r.trades.route_swaps, rejected(&r).multi_asset), (0, 1));

    // No verified leg for the wallet's token: the only leg trades M2.
    let mut tx = route_tx(5, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    for ix in &mut tx.instructions {
        // retarget the leg's mint (accounts[2]) so no leg trades M1
        if ix.program_id == pump() && ix.accounts.len() > 2 && ix.data.len() < 40 {
            ix.accounts[2] = pk(M2);
        }
    }
    let r = run(&[tx]);
    assert_eq!(r.trades.route_swaps, 0);
    assert_eq!(rejected(&r).no_verified_leg + rejected(&r).multi_asset, 1);

    // Same-sign deltas.
    let mut tx = route_tx(6, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    tx.token_balance_changes[1] = bal_pk(usdc(), W, 0, 5_000_000);
    let r = run(&[tx]);
    assert_eq!(
        (r.trades.route_swaps, rejected(&r).not_opposite_signs),
        (0, 1)
    );
}

#[test]
fn cross_quote_unit_disposal_is_unknown_never_mixed() {
    // USDC buy then SOL sell of the same mint (one FIFO, units differ).
    let buy = route_tx(1, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    let sell = trade_tx(
        2,
        2,
        TradeSide::Sell,
        M1,
        1000,
        1000,
        1_300_000,
        0,
        0,
        20,
        5_000,
        W,
    );
    let r = run(&[buy, sell]);
    assert_eq!((r.closed_episodes_known, r.closed_episodes_unknown), (0, 1));
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::CrossQuoteUnit)
    );
    for b in &r.unit_blocks {
        assert_eq!(b.closed_episodes_known, 0);
        assert_eq!(b.realized_trade_pnl_raw, 0);
    }
    assert_eq!(r.diagnostics.unknown_disposals, 1);
}

#[test]
fn one_episode_realized_in_two_units_is_unknown() {
    let b1 = route_tx(1, 1, TradeSide::Buy, M1, 500, 0, usdc(), 1_000_000, W);
    let b2 = trade_tx(
        2,
        2,
        TradeSide::Buy,
        M1,
        500,
        500,
        600_000,
        0,
        0,
        20,
        5_000,
        W,
    );
    let s1 = route_tx(3, 3, TradeSide::Sell, M1, 500, 1000, usdc(), 1_500_000, W);
    let s2 = trade_tx(
        4,
        4,
        TradeSide::Sell,
        M1,
        500,
        500,
        900_000,
        0,
        0,
        40,
        5_000,
        W,
    );
    let r = run(&[b1, b2, s1, s2]);
    assert_eq!((r.closed_episodes_known, r.closed_episodes_unknown), (0, 1));
    assert!(
        r.episodes[0]
            .unknown_reasons
            .contains(&UnknownReason::CrossQuoteUnit)
    );
}

#[test]
fn per_unit_sums_are_never_mixed_and_win_rate_is_overall() {
    // M1: USDC round trip +2_000_000 USDC units. M2: SOL curve round trip.
    let b1 = route_tx(1, 1, TradeSide::Buy, M1, 1000, 0, usdc(), 5_000_000, W);
    let s1 = route_tx(2, 2, TradeSide::Sell, M1, 1000, 1000, usdc(), 7_000_000, W);
    let b2 = trade_tx(
        3,
        3,
        TradeSide::Buy,
        M2,
        1000,
        0,
        1_000_000,
        0,
        0,
        30,
        5_000,
        W,
    );
    let s2 = trade_tx(
        4,
        4,
        TradeSide::Sell,
        M2,
        1000,
        1000,
        800_000,
        0,
        0,
        40,
        5_000,
        W,
    );
    let r = run(&[b1, s1, b2, s2]);
    assert_eq!(r.closed_episodes_known, 2);
    let usdc_b = r.unit_block(QuoteUnit::UsdcUnits).unwrap();
    let sol_b = r.unit_block(QuoteUnit::Lamports).unwrap();
    assert_eq!(usdc_b.realized_trade_pnl_raw, 2_000_000);
    // SOL: basis 1_000_000 + 5_000 fee, proceeds 800_000 - 5_000 fee.
    assert_eq!(sol_b.realized_trade_pnl_raw, -210_000);
    assert_eq!((usdc_b.wins, usdc_b.losses), (1, 0));
    assert_eq!((sol_b.wins, sol_b.losses), (0, 1));
    // Legacy SOL figures equal the SOL block; overall counts span units.
    assert_eq!(r.realized_trade_pnl_lamports, -210_000);
    assert_eq!((r.wins, r.losses), (1, 1));
    assert_eq!(
        r.win_rate,
        scout_analytics::RatioStatus::Value {
            value: Money::from_scaled_units(50_000_000)
        }
    );
    assert_eq!(
        usdc_b.win_rate,
        scout_analytics::RatioStatus::Value {
            value: Money::from_scaled_units(100_000_000)
        }
    );
}

// ---------------------------------------------------------------------
// ADR-015: Jupiter v6 swap legs as route evidence (synthetic goldens).
// ---------------------------------------------------------------------

/// `(venue program, input mint, input amount, output mint, output amount)`.
type JupHop = (SolanaPubkey, SolanaPubkey, u64, SolanaPubkey, u64);

fn jup_event_ix(hops: &[JupHop], idx: u32, trailing: &[u8]) -> RawSolanaInstruction {
    let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
    data.extend(JUPITER_SWAPS_EVENT_DISCRIMINATOR);
    data.extend(u32::try_from(hops.len()).unwrap().to_le_bytes());
    for (amm, im, ia, om, oa) in hops {
        data.extend(im);
        data.extend(ia.to_le_bytes());
        data.extend(om);
        data.extend(oa.to_le_bytes());
        data.extend(amm);
    }
    data.extend_from_slice(trailing);
    RawSolanaInstruction {
        program_id: JUPITER_V6_PROGRAM_ID_BYTES,
        accounts: vec![JUPITER_EVENT_AUTHORITY_BYTES],
        data,
        instruction_index: idx,
    }
}

/// Signer wallet W trades `tokens` of `mint` against `quote_amt` USDC; the
/// only decoded evidence is the given Jupiter instructions (no pump leg).
fn jup_route_tx(
    sig: u8,
    slot: u64,
    side: TradeSide,
    mint: u8,
    tokens: u64,
    quote_amt: u64,
    ixs: Vec<RawSolanaInstruction>,
) -> RawSolanaTransaction {
    let buy = side == TradeSide::Buy;
    let (t_pre, t_post) = if buy { (0, tokens) } else { (tokens, 0) };
    let (q_pre, q_post) = if buy { (quote_amt, 0) } else { (0, quote_amt) };
    let mut tx = Tx {
        sig,
        slot,
        index: 0,
        ixs,
        bals: vec![
            bal(mint, W, t_pre, t_post),
            bal_pk(usdc(), W, q_pre, q_post),
        ],
        fee: 5_000,
        payer: RELAYER,
        native: vec![nat(W, -550_840)],
        ok: true,
    }
    .build();
    tx.signers = vec![pk(RELAYER), pk(W)];
    tx
}

fn dlmm() -> SolanaPubkey {
    pubkey("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo")
}

#[test]
fn jupiter_only_route_is_booked_from_wallet_deltas() {
    // Two hops USDC -> X -> T (T is only the second hop's output); no pump leg.
    let x = pk(77);
    let hops = [
        (dlmm(), usdc(), 5_000_000, x, 900),
        (dlmm(), x, 900, pk(M1), 1000),
    ];
    // The wallet pays a little more USDC than the first hop (platform fee).
    let buy = jup_route_tx(
        1,
        1,
        TradeSide::Buy,
        M1,
        1000,
        5_002_500,
        vec![jup_event_ix(&hops, 3, &[])],
    );
    let sell_hops = [(dlmm(), pk(M1), 1000, usdc(), 5_100_000)];
    let sell = jup_route_tx(
        2,
        2,
        TradeSide::Sell,
        M1,
        1000,
        5_000_000,
        vec![jup_event_ix(&sell_hops, 3, &[])],
    );
    let r = run_both(&[buy.clone(), sell]);
    assert_eq!(r.trades.route_swaps, 2);
    assert_eq!(r.trades.route_swaps_by_quote.usdc, 2);
    let e = r.trades.route_swaps_by_evidence;
    assert_eq!(
        (e.curve, e.pump_amm, e.jupiter, e.jupiter_only),
        (0, 0, 2, 2)
    );
    // Booked at the wallet's own deltas, never at the event amounts.
    let (b, s) = (&r.route_swap_log[0], &r.route_swap_log[1]);
    assert_eq!(
        (b.side, b.token_amount, b.quote_amount),
        (TradeSide::Buy, 1000, 5_002_500)
    );
    assert_eq!(
        (s.side, s.token_amount, s.quote_amount),
        (TradeSide::Sell, 1000, 5_000_000)
    );
    assert_eq!(rejected(&r), RouteRejections::default());
    assert_eq!(r.diagnostics.jupiter_malformed_events, 0);
    // Without Jupiter evidence the same transactions are not trades.
    let mut bare = buy.clone();
    bare.instructions.clear();
    assert_eq!(run_both(&[bare]).trades.route_swaps, 0);
    // Bonding-curve-only runs do not use Jupiter evidence (pre-ADR-012 shape).
    assert_eq!(run(&[buy]).trades.route_swaps, 0);
}

#[test]
fn jupiter_leg_not_trading_the_token_is_not_evidence() {
    // Hops trade USDC <-> M2, the wallet's token is M1.
    let hops = [(dlmm(), usdc(), 5_000_000, pk(M2), 900)];
    let tx = jup_route_tx(
        1,
        1,
        TradeSide::Buy,
        M1,
        1000,
        5_000_000,
        vec![jup_event_ix(&hops, 3, &[])],
    );
    let r = run_both(&[tx]);
    assert_eq!(r.trades.route_swaps, 0);
    assert_eq!(rejected(&r).no_verified_leg, 1);
    assert_eq!(r.trades.route_swaps_by_evidence.jupiter, 0);
}

#[test]
fn malformed_jupiter_event_is_not_trusted_and_is_counted_with_evidence() {
    let hops = [(dlmm(), usdc(), 5_000_000, pk(M1), 1000)];
    let tx = jup_route_tx(
        9,
        1,
        TradeSide::Buy,
        M1,
        1000,
        5_000_000,
        vec![jup_event_ix(&hops, 3, &[0])],
    );
    let r = run_both(&[tx]);
    assert_eq!(r.trades.route_swaps, 0);
    assert_eq!(r.diagnostics.jupiter_malformed_events, 1);
    assert_eq!(r.evidence_samples.len(), 1);
    let e = &r.evidence_samples[0];
    assert_eq!(e.signature, [9; 64]);
    assert_eq!(e.program, JUPITER_V6_PROGRAM_ID_BYTES);
    assert_eq!(e.variant_or_discriminator, "982f4eebc0606e6a");
    assert_eq!((e.data_len, e.accounts_len), (16 + 4 + 112 + 1, 1));
    assert!(e.reason.contains("SwapsEvent"), "{}", e.reason);
}

#[test]
fn passthrough_rule_still_applies_with_jupiter_evidence() {
    // A Jupiter leg proves the swap, but a pump leg of another user that is
    // not a zero-net pass-through (it received M1) still rejects the booking.
    let hops = [(dlmm(), usdc(), 5_000_000, pk(M1), 1000)];
    let mut tx = jup_route_tx(
        1,
        1,
        TradeSide::Buy,
        M1,
        1000,
        5_000_000,
        vec![
            trade_ix(TradeSide::Buy, None, PASS, M1, 0),
            event_ix(
                &Ev {
                    mint: M1,
                    user: PASS,
                    is_buy: true,
                    sol: HOP_EVENT_SOL,
                    tokens: 1000,
                    fee: 0,
                    creator_fee: 0,
                    ts: 1_001,
                },
                1,
            ),
            jup_event_ix(&hops, 2, &[]),
        ],
    );
    tx.token_balance_changes.push(bal(M1, PASS, 0, 7));
    let r = run_both(&[tx]);
    assert_eq!(r.trades.route_swaps, 0);
    assert_eq!(rejected(&r).passthrough_nonzero, 1);
    // Same shape with a zero-net pass-through user is booked, with both
    // evidence sources counted.
    let mut ok = jup_route_tx(
        2,
        2,
        TradeSide::Buy,
        M1,
        1000,
        5_000_000,
        vec![
            trade_ix(TradeSide::Buy, None, PASS, M1, 0),
            event_ix(
                &Ev {
                    mint: M1,
                    user: PASS,
                    is_buy: true,
                    sol: HOP_EVENT_SOL,
                    tokens: 1000,
                    fee: 0,
                    creator_fee: 0,
                    ts: 1_002,
                },
                1,
            ),
            jup_event_ix(&hops, 2, &[]),
        ],
    );
    ok.token_balance_changes.push(bal(M1, PASS, 100, 100));
    let r = run_both(&[ok]);
    let e = r.trades.route_swaps_by_evidence;
    assert_eq!(
        (r.trades.route_swaps, e.curve, e.jupiter, e.jupiter_only),
        (1, 1, 1, 0)
    );
}

#[test]
fn evidence_samples_are_bounded_and_locate_malformed_and_orphan_items() {
    let mut txs = Vec::new();
    // 6 malformed curve buys (truncated args) + 1 orphan TradeEvent.
    for i in 0..6u8 {
        let mut ix = trade_ix(TradeSide::Buy, None, W, M1, 0);
        ix.data.truncate(12);
        txs.push(
            Tx {
                sig: 10 + i,
                slot: u64::from(i) + 1,
                index: 0,
                ixs: vec![ix],
                bals: vec![],
                fee: 5_000,
                payer: W,
                native: vec![],
                ok: true,
            }
            .build(),
        );
    }
    txs.push(
        Tx {
            sig: 50,
            slot: 0,
            index: 0,
            ixs: vec![event_ix(
                &Ev {
                    mint: M1,
                    user: W,
                    is_buy: true,
                    sol: 1,
                    tokens: 1,
                    fee: 0,
                    creator_fee: 0,
                    ts: 5,
                },
                0,
            )],
            bals: vec![],
            fee: 5_000,
            payer: W,
            native: vec![],
            ok: true,
        }
        .build(),
    );
    let r = run_both(&txs);
    assert_eq!(r.diagnostics.malformed_trade_instructions, 6);
    assert_eq!(r.diagnostics.orphan_trade_events, 1);
    assert_eq!(r.evidence_samples.len(), 5);
    // Canonical order: the orphan (slot 0) first, then the lowest slots.
    let first = &r.evidence_samples[0];
    assert_eq!(first.signature, [50; 64]);
    assert_eq!(first.kind.label(), "orphan_event");
    assert_eq!(first.variant_or_discriminator, "TradeEvent");
    let second = &r.evidence_samples[1];
    assert_eq!(second.signature, [10; 64]);
    assert_eq!(second.kind.label(), "malformed_trade_instruction");
    assert_eq!(second.variant_or_discriminator, "buy");
    assert_eq!((second.data_len, second.accounts_len), (12, 16));
    assert_eq!(second.program, pump());
    assert!(!second.reason.is_empty());
    // Deterministic under input order.
    txs.reverse();
    assert_eq!(run_both(&txs).evidence_samples, r.evidence_samples);
}
