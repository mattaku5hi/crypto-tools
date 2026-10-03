//! ADR-020 step 2 / ADR-009 style verification of Uniswap v4 on Robinhood
//! Chain against the live fixture `evm_robinhood_token_aiden_v4_2026-10-03.json`.
//!
//! For every `Swap` of the gated PoolManager `0x8366a39c...` the net ERC-20
//! flow of the PoolManager in the same transaction (Transfers to minus from
//! the PoolManager, per token) is compared with the event's `amount0`/
//! `amount1`. v4 convention (verified here): the amounts are the SWAPPER's
//! `BalanceDelta` — negative = the swapper pays the pool (PoolManager
//! receives), positive = the swapper receives (PoolManager sends). So for an
//! ERC-20 currency `c` on side `i`: `net_into_PoolManager(c) == -amount_i`.
//! The native-ETH currency (`address(0)`) has no ERC-20 log: that side is
//! NOT visible in logs and is recorded as such; where the swapper sent
//! ETH, `tx.value == -amount0` is reported as extra (informational)
//! evidence.
//!
//! poolId -> currencies comes from `Initialize` logs when they are inside
//! the capture window; otherwise the currencies are derived from the
//! PoolManager Transfers of the same transaction and that is recorded.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, I256, U256};
use scout_api::DecodeOutcome;
use scout_dex_evm::{
    V4_INITIALIZE_TOPIC0, VENUE_DEPLOYMENTS, VenueVerification, decode_v4_initialize,
    decode_v4_swap,
};
use scout_engine::{
    Consideration, EvmExtractionConfig, EvmTxOutcome, NoTradeReason, extract_evm_trades,
};
use scout_evm::{ROBINHOOD, decode_erc20_transfer};

#[path = "support/evm_aiden.rs"]
mod evm_aiden;
use evm_aiden::{AIDEN, POOL_MANAGER};

struct Row {
    tx: B256,
    pool_id: B256,
    currency_source: &'static str,
    erc20_side: usize,
    amount_erc20: i128,
    pm_net_into: I256,
    native_visible: bool,
    value_matches_amount0: Option<bool>,
}

#[tokio::test]
async fn uniswap_v4_on_robinhood_matches_poolmanager_token_deltas() {
    let server = evm_aiden::serve(evm_aiden::replay()).await;
    let scanned = evm_aiden::scan(&server).await;
    let txs = evm_aiden::transactions(&scanned);
    assert_eq!(txs.len(), 48, "fixture transaction count");

    // Initialize logs inside the capture (pool id -> currencies).
    let mut init: BTreeMap<B256, (Address, Address)> = BTreeMap::new();
    for tx in txs {
        for log in &tx.logs {
            if log.address == POOL_MANAGER && log.topics.first() == Some(&V4_INITIALIZE_TOPIC0) {
                match decode_v4_initialize(log) {
                    DecodeOutcome::Decoded(i) => {
                        init.insert(i.pool_id, (i.currency0, i.currency1));
                    }
                    other => panic!("Initialize must decode: {other:?}"),
                }
            }
        }
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut swaps = 0usize;
    for tx in txs {
        let swap_logs: Vec<_> = tx
            .logs
            .iter()
            .filter(|l| {
                l.address == POOL_MANAGER
                    && l.topics.first() == Some(&scout_dex_evm::V4_SWAP_TOPIC0)
            })
            .collect();
        // Per-token net flow into the PoolManager in this transaction.
        let mut net: BTreeMap<Address, I256> = BTreeMap::new();
        for l in &tx.logs {
            if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l) {
                if t.to == POOL_MANAGER {
                    let e = net.entry(t.token).or_insert(I256::ZERO);
                    *e = e.checked_add(I256::try_from(t.amount).unwrap()).unwrap();
                }
                if t.from == POOL_MANAGER {
                    let e = net.entry(t.token).or_insert(I256::ZERO);
                    *e = e.checked_sub(I256::try_from(t.amount).unwrap()).unwrap();
                }
            }
        }
        // One swap per transaction in this fixture keeps per-tx netting
        // equal to per-swap netting; assert it rather than assume it.
        assert!(swap_logs.len() <= 1, "fixture has no multi-swap tx");
        for log in swap_logs {
            swaps += 1;
            let DecodeOutcome::Decoded(sw) = decode_v4_swap(log) else {
                panic!("gated swap must decode");
            };
            let amounts = [sw.amount0, sw.amount1];
            let (currencies, source): (Option<(Address, Address)>, &'static str) =
                match init.get(&sw.pool_id) {
                    Some(c) => (Some(*c), "Initialize"),
                    None => (None, "derived_from_poolmanager_transfers"),
                };
            // Find the side whose ERC-20 net matches the event, exactly.
            let mut matched: Option<(usize, I256, i128)> = None;
            for (token, pm_net) in &net {
                for (i, amount) in amounts.iter().enumerate() {
                    let side_ok = currencies.is_none_or(|(c0, c1)| {
                        let want = if i == 0 { c0 } else { c1 };
                        want == *token
                    });
                    if side_ok && *pm_net == -I256::try_from(*amount).unwrap() && *amount != 0 {
                        matched = Some((i, *pm_net, *amount));
                    }
                }
            }
            let (side, pm_net_into, amount) = matched.unwrap_or_else(|| {
                panic!(
                    "tx {:#x}: no ERC-20 side of the Swap matches the PoolManager deltas",
                    tx.hash
                )
            });
            let native_side_is_visible = currencies
                .map(|(c0, c1)| if side == 0 { c1 } else { c0 })
                .is_some_and(|other| other != Address::ZERO);
            let value_matches =
                if currencies.is_some_and(|(c0, _)| c0 == Address::ZERO) && sw.amount0 < 0 {
                    Some(tx.value == U256::from(sw.amount0.unsigned_abs()))
                } else {
                    None
                };
            rows.push(Row {
                tx: tx.hash,
                pool_id: sw.pool_id,
                currency_source: source,
                erc20_side: side,
                amount_erc20: amount,
                pm_net_into,
                native_visible: native_side_is_visible,
                value_matches_amount0: value_matches,
            });
        }
    }

    // ---- the evidence table (run with --nocapture to regenerate the doc).
    println!(
        "| # | tx | poolId | currencies from | ERC-20 side | event amount | PoolManager net in | other side visible in logs | tx.value == -amount0 |"
    );
    println!(
        "|---|----|--------|-----------------|-------------|--------------|--------------------|----------------------------|----------------------|"
    );
    for (n, r) in rows.iter().enumerate() {
        println!(
            "| {} | `{:#x}` | `{:#x}` | {} | amount{} | {} | {} | {} | {} |",
            n + 1,
            r.tx,
            r.pool_id,
            r.currency_source,
            r.erc20_side,
            r.amount_erc20,
            r.pm_net_into,
            if r.native_visible {
                "yes (ERC-20)"
            } else {
                "no (native ETH)"
            },
            r.value_matches_amount0
                .map_or("n/a".to_string(), |b| b.to_string()),
        );
    }

    assert_eq!(swaps, 45, "gated v4 Swap events in the fixture");
    assert_eq!(rows.len(), 45, "every sample passes");
    // One pool, announced by an in-window Initialize: ETH (address(0)) / Aiden.
    assert_eq!(init.len(), 1);
    let (_, (c0, c1)) = init.iter().next().unwrap();
    assert_eq!((*c0, *c1), (Address::ZERO, AIDEN));
    assert!(rows.iter().all(|r| r.currency_source == "Initialize"));
    // Aiden is currency1 in every sample; the native side is never in logs.
    assert!(rows.iter().all(|r| r.erc20_side == 1 && !r.native_visible));
    // Sign convention: negative amount1 (swapper pays Aiden = sell) means the
    // PoolManager received Aiden; positive means it sent Aiden.
    assert!(
        rows.iter()
            .all(|r| (r.amount_erc20 < 0) == (r.pm_net_into > I256::ZERO))
    );
    // Extra evidence on the invisible native side: for every swap where the
    // swapper paid ETH, tx.value equals -amount0 exactly.
    let paid: Vec<_> = rows
        .iter()
        .filter_map(|r| r.value_matches_amount0)
        .collect();
    assert!(!paid.is_empty() && paid.iter().all(|b| *b), "{paid:?}");

    // Both sides of the trade direction were exercised.
    assert!(rows.iter().any(|r| r.amount_erc20 > 0) && rows.iter().any(|r| r.amount_erc20 < 0));

    // The deployment is FixtureVerified only because every sample passed.
    let dep = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == 4663 && d.venue == scout_dex_evm::SwapVenue::UniswapV4)
        .unwrap();
    assert_eq!(dep.verification, VenueVerification::FixtureVerified);
    assert_eq!(dep.anchor, POOL_MANAGER);
    assert_eq!(
        dep.active_from_block, 0,
        "activation block is not derivable offline (no historical state on the public RPC)"
    );
}

#[tokio::test]
async fn malformed_logs_of_the_fixture_are_erc721_position_nfts_not_corruption() {
    let server = evm_aiden::serve(evm_aiden::replay()).await;
    let scanned = evm_aiden::scan(&server).await;
    let txs = evm_aiden::transactions(&scanned);
    let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
    let (out, sum) = extract_evm_trades(txs, &cfg, None, Some(AIDEN));

    // Step 1 counted 2 `malformed_log`: both are ERC-721 `Transfer`s of the
    // Uniswap v4 PositionManager 0x58daec31... (liquidity add = NFT mint in
    // 0x54c62a7c..., liquidity remove = NFT burn in 0x8b216c7d...), same
    // topic0 as ERC-20 but 4 topics and empty data. They are not trades.
    assert_eq!(sum.nft_transfer_logs, 2);
    assert_eq!(sum.no_trade.get("malformed_log"), None);
    assert_eq!(sum.no_trade.get("no_verified_swap_event"), Some(&3));
    let lp: Vec<_> = out
        .iter()
        .filter(|e| e.nft_transfer_logs > 0)
        .map(|e| format!("{:#x}", e.tx_hash))
        .collect();
    assert_eq!(lp.len(), 2);
    assert!(
        lp[0].starts_with("0x54c62a7c") && lp[1].starts_with("0x8b216c7d"),
        "{lp:?}"
    );
    for e in out.iter().filter(|e| e.nft_transfer_logs > 0) {
        assert!(matches!(
            e.outcome,
            EvmTxOutcome::NoTrade(NoTradeReason::NoVerifiedSwapEvent)
        ));
    }

    // Headline numbers of the fixture, now with FixtureVerified evidence.
    assert_eq!(sum.transactions, 48);
    // 45 swaps, 44 trades: tx 0x588186e4... paid ETH but the router delivered
    // the tokens to another address (0x2be87ad7...), so the signer's own
    // net delta has no traded token (`no_traded_token`, rule a/c).
    assert_eq!(sum.trades, 44);
    assert_eq!(sum.no_trade.get("no_traded_token"), Some(&1));
    assert_eq!(sum.unknown_consideration, 19);
    let unknown_sells = out
        .iter()
        .filter_map(|e| match &e.outcome {
            EvmTxOutcome::Trade(t) => Some(t),
            _ => None,
        })
        .filter(|t| matches!(t.consideration, Consideration::Unknown(_)))
        .count();
    assert_eq!(unknown_sells, 19);
}
