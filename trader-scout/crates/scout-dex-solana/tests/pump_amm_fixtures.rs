//! Real-fixture checks for the PumpSwap AMM decoder, event decoder, pairing
//! and reconciliation. Fixtures are raw Helius JSON parsed directly here
//! (the `scout-engine` test `pump_amm_fixtures` runs the same files through
//! the real `HeliusProvider` and asserts the same totals).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use scout_core::{
    RawSolanaInstruction, RawSolanaTransaction, SolanaExecutionStatus, SolanaNativeBalanceChange,
    SolanaPubkey, SolanaTokenBalanceChange,
};
use scout_dex_solana::{
    AmmAttribution, AmmTradeEventPairing, PumpAmmDecoder, PumpAmmEvent, PumpAmmInstructionOutcome,
    PumpAmmTradeVariant, TradeSide, VariantVerification, WRAPPED_SOL_MINT,
    pair_amm_trades_with_events, reconcile_pump_amm_transaction,
};
use std::collections::BTreeSet;

const VARIANTS: &str = "pumpswap_variants_live_2026-10-02.json";
const WALLET_PAGE: &str = "pumpswap_wallet_page_2026-10-02.json";

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

fn fixture(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn idx(n: &serde_json::Value) -> usize {
    usize::try_from(n.as_u64().unwrap()).unwrap()
}

fn raw_ix(v: &serde_json::Value, keys: &[SolanaPubkey], index: u32) -> RawSolanaInstruction {
    RawSolanaInstruction {
        program_id: keys[idx(&v["programIdIndex"])],
        accounts: v["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| keys[idx(a)])
            .collect(),
        data: bs58::decode(v["data"].as_str().unwrap())
            .into_vec()
            .unwrap(),
        instruction_index: index,
    }
}

/// Minimal raw-JSON -> `RawSolanaTransaction` (flattened instructions in
/// execution order, owner-keyed token changes, native changes).
fn parse_tx(rec: &serde_json::Value) -> RawSolanaTransaction {
    let msg = &rec["transaction"]["message"];
    let meta = &rec["meta"];
    let mut keys: Vec<SolanaPubkey> = msg["accountKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| pubkey(k.as_str().unwrap()))
        .collect();
    for part in ["writable", "readonly"] {
        if let Some(a) = meta["loadedAddresses"][part].as_array() {
            keys.extend(a.iter().map(|k| pubkey(k.as_str().unwrap())));
        }
    }
    let mut instructions = Vec::new();
    let mut next = 0u32;
    for (top, ix) in msg["instructions"].as_array().unwrap().iter().enumerate() {
        instructions.push(raw_ix(ix, &keys, next));
        next += 1;
        for group in meta["innerInstructions"].as_array().unwrap() {
            if group["index"].as_u64().unwrap() == u64::try_from(top).unwrap() {
                for inner in group["instructions"].as_array().unwrap() {
                    instructions.push(raw_ix(inner, &keys, next));
                    next += 1;
                }
            }
        }
    }
    // Token changes keyed by account index.
    let mut by_account: BTreeMap<usize, SolanaTokenBalanceChange> = BTreeMap::new();
    let amount = |b: &serde_json::Value| -> u64 {
        b["uiTokenAmount"]["amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    for b in meta["preTokenBalances"].as_array().unwrap() {
        by_account.insert(
            idx(&b["accountIndex"]),
            SolanaTokenBalanceChange {
                mint: pubkey(b["mint"].as_str().unwrap()),
                owner: b["owner"].as_str().map(pubkey),
                decimals: 0,
                pre_amount: Some(amount(b)),
                post_amount: 0,
                closed: true,
            },
        );
    }
    for b in meta["postTokenBalances"].as_array().unwrap() {
        let e = by_account
            .entry(idx(&b["accountIndex"]))
            .or_insert(SolanaTokenBalanceChange {
                mint: pubkey(b["mint"].as_str().unwrap()),
                owner: b["owner"].as_str().map(pubkey),
                decimals: 0,
                pre_amount: None,
                post_amount: 0,
                closed: false,
            });
        e.post_amount = amount(b);
        e.closed = false;
    }
    let pre: Vec<u64> = meta["preBalances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let post: Vec<u64> = meta["postBalances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let native_balance_changes = pre
        .iter()
        .zip(&post)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| SolanaNativeBalanceChange {
            account: keys[i],
            pre_lamports: *a,
            post_lamports: *b,
        })
        .collect();
    let n_sigs = idx(&msg["header"]["numRequiredSignatures"]);
    let sig: [u8; 64] = bs58::decode(rec["transaction"]["signatures"][0].as_str().unwrap())
        .into_vec()
        .unwrap()
        .try_into()
        .unwrap();
    RawSolanaTransaction {
        log_messages: None,
        block_time: rec["blockTime"].as_i64(),
        signature: sig,
        execution: if meta["err"].is_null() {
            SolanaExecutionStatus::Succeeded
        } else {
            SolanaExecutionStatus::Failed {
                error: "err".to_string(),
            }
        },
        slot: rec["slot"].as_u64().unwrap(),
        transaction_index: rec["transactionIndex"].as_u64().unwrap_or(0),
        instructions,
        token_balance_changes: by_account.into_values().collect(),
        fee_lamports: meta["fee"].as_u64().unwrap(),
        fee_payer: keys[0],
        signers: keys[..n_sigs].to_vec(),
        native_balance_changes,
    }
}

fn txs(name: &str) -> Vec<RawSolanaTransaction> {
    fixture(name)["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["data"].as_array().unwrap().iter().map(parse_tx))
        .collect()
}

fn sig_prefix(tx: &RawSolanaTransaction) -> String {
    bs58::encode(tx.signature).into_string()[..8].to_string()
}

fn find<'a>(all: &'a [RawSolanaTransaction], prefix: &str) -> &'a RawSolanaTransaction {
    all.iter().find(|t| sig_prefix(t) == prefix).unwrap()
}

/// Flattened instruction indices of the top-level instructions.
fn top_level_indices(rec: &serde_json::Value) -> BTreeSet<u32> {
    let meta = &rec["meta"];
    let mut out = BTreeSet::new();
    let mut next = 0u32;
    for (top, _) in rec["transaction"]["message"]["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        out.insert(next);
        next += 1;
        for group in meta["innerInstructions"].as_array().unwrap() {
            if group["index"].as_u64().unwrap() == u64::try_from(top).unwrap() {
                next += u32::try_from(group["instructions"].as_array().unwrap().len()).unwrap();
            }
        }
    }
    out
}

fn records(name: &str) -> Vec<serde_json::Value> {
    fixture(name)["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["data"].as_array().unwrap().clone())
        .collect()
}

fn both() -> Vec<RawSolanaTransaction> {
    let mut v = txs(VARIANTS);
    v.extend(txs(WALLET_PAGE));
    v
}

fn b58(k: &SolanaPubkey) -> String {
    bs58::encode(k).into_string()
}

#[test]
fn pairing_counts_over_both_fixtures() {
    let d = PumpAmmDecoder::mainnet();
    let mut totals = [0usize; 9];
    let mut per_file = Vec::new();
    for name in [VARIANTS, WALLET_PAGE] {
        let mut c = [0usize; 9];
        let all = txs(name);
        for tx in &all {
            let r =
                pair_amm_trades_with_events(&d, &tx.instructions, tx.slot, tx.transaction_index);
            c[0] += r.trades.len();
            c[1] += r.paired();
            c[2] += r.missing();
            c[3] += r.mismatched();
            c[4] += r.orphan_events.len();
            c[5] += r.other_events;
            c[6] += r.malformed_trades + r.malformed_events + r.unknown_events;
            c[7] += r.unknown_instructions;
            c[8] += r.non_trade_instructions;
        }
        per_file.push((all.len(), c));
        for (t, v) in totals.iter_mut().zip(c) {
            *t += v;
        }
    }
    // (txs, [trades, paired, missing, mismatched, orphans, other_events,
    //        malformed+unknown events, unknown instructions, non-trade ix])
    assert_eq!(per_file[0], (28, [30, 29, 1, 0, 0, 4, 0, 0, 4]));
    assert_eq!(per_file[1], (100, [98, 98, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(totals, [128, 127, 1, 0, 0, 4, 0, 0, 4]);
}

#[test]
fn variant_and_arg_length_distribution_is_decoded_without_malformed() {
    let d = PumpAmmDecoder::mainnet();
    // (variant, track_volume.is_some()) -> count over every AMM trade instruction.
    let mut dist: BTreeMap<(&str, bool), usize> = BTreeMap::new();
    let mut inner = 0usize;
    let mut top = 0usize;
    for name in [VARIANTS, WALLET_PAGE] {
        for rec in records(name) {
            let tx = parse_tx(&rec);
            let tops = top_level_indices(&rec);
            for ix in &tx.instructions {
                match d.classify(ix, tx.slot, tx.transaction_index) {
                    PumpAmmInstructionOutcome::Trade(t) => {
                        *dist
                            .entry((t.variant.name(), t.track_volume.is_some()))
                            .or_default() += 1;
                        if tops.contains(&ix.instruction_index) {
                            top += 1;
                        } else {
                            inner += 1;
                        }
                    }
                    PumpAmmInstructionOutcome::Malformed { .. }
                    | PumpAmmInstructionOutcome::UnknownDiscriminator { .. } => {
                        panic!("unexpected malformed/unknown AMM instruction")
                    }
                    PumpAmmInstructionOutcome::NonTrade(_) | PumpAmmInstructionOutcome::NotMine => {
                    }
                }
            }
        }
    }
    let get = |v: &'static str, tv: bool| dist.get(&(v, tv)).copied().unwrap_or(0);
    assert_eq!(get("buy", false), 4); // 24-byte buys
    assert_eq!(get("buy", true), 60); // 25-byte buys (7 + 53)
    assert_eq!(get("buy_exact_quote_in", false), 5); // 24-byte
    assert_eq!(get("buy_exact_quote_in", true), 4); // 25-byte (one failed tx)
    assert_eq!(get("sell", false), 55);
    assert_eq!(get("sell", true), 0);
    assert_eq!((top, inner), (120, 8)); // 8 router-invoked inner trades
}

#[test]
fn golden_instruction_and_event_values() {
    let d = PumpAmmDecoder::mainnet();
    let all = txs(VARIANTS);

    // Top-level 25-byte buy.
    let tx = find(&all, "3q5xk5di");
    let r = pair_amm_trades_with_events(&d, &tx.instructions, tx.slot, tx.transaction_index);
    let t = &r.trades[0].trade;
    assert_eq!(t.variant, PumpAmmTradeVariant::Buy);
    assert_eq!(t.side, TradeSide::Buy);
    assert_eq!(
        (t.slot, t.transaction_index, t.instruction_index),
        (452_680_184, 1034, 2)
    );
    assert_eq!(t.args[0].name, "base_amount_out");
    assert_eq!(t.args[0].value, 1_072_772_149_410);
    assert_eq!(t.args[1].name, "max_quote_amount_in");
    assert_eq!(t.args[1].value, 122_511_667);
    assert_eq!(t.track_volume, Some(0));
    assert_eq!(t.quote_mint, WRAPPED_SOL_MINT);
    assert_eq!(b58(&t.user), tx_signer_b58(tx));
    let AmmTradeEventPairing::Paired(PumpAmmEvent::Buy(e)) = &r.trades[0].pairing else {
        panic!()
    };
    assert_eq!(e.timestamp, 1_790_962_807);
    assert_eq!(e.base_amount_out, 1_072_772_149_410);
    assert_eq!(e.quote_amount_in, 104_797_912);
    assert_eq!(e.lp_fee_basis_points, 2);
    assert_eq!(e.lp_fee, 20_960);
    assert_eq!(e.protocol_fee_basis_points, 93);
    assert_eq!(e.protocol_fee, 974_621);
    assert_eq!(e.quote_amount_in_with_lp_fee, 104_818_872);
    assert_eq!(e.user_quote_amount_in, 106_107_887);
    assert_eq!(e.coin_creator_fee_basis_points, 30);
    assert_eq!(e.coin_creator_fee, 314_394);
    assert_eq!(e.ix_name.as_deref(), Some("buy"));
    assert_eq!(e.buyback_fee, Some(487_310));
    assert_eq!(e.holder_rewards, Some(314_394));
    assert_eq!(e.last_field_present, "holder_rewards");
    assert_eq!(e.trailing_bytes, 8);
    assert_eq!(e.data_len, 497);
    assert_eq!(e.quote_cost(), Some(106_107_887));

    // 24-byte sell (no track_volume).
    let tx = find(&all, "gD8yChUH");
    let r = pair_amm_trades_with_events(&d, &tx.instructions, tx.slot, tx.transaction_index);
    let t = &r.trades[0].trade;
    assert_eq!(t.variant, PumpAmmTradeVariant::Sell);
    assert_eq!(t.args[0].name, "base_amount_in");
    assert_eq!(t.args[0].value, 636_205_117_450);
    assert_eq!(t.args[1].value, 0);
    assert_eq!(t.track_volume, None);
    let AmmTradeEventPairing::Paired(PumpAmmEvent::Sell(e)) = &r.trades[0].pairing else {
        panic!()
    };
    assert_eq!(e.base_amount_in, 636_205_117_450);
    assert_eq!(e.quote_amount_out, 497_363_435);
    assert_eq!(e.lp_fee, 994_727);
    assert_eq!(e.protocol_fee, 248_682);
    assert_eq!(e.quote_amount_out_without_lp_fee, 496_368_708);
    assert_eq!(e.user_quote_amount_out, 491_395_073);
    assert_eq!(e.coin_creator_fee, 4_724_953);
    assert_eq!(e.data_len, 449);
    assert_eq!(e.quote_proceeds(), Some(491_395_073));

    // 25-byte buy_exact_quote_in: user_quote_amount_in is NOT the cost.
    let tx = find(&all, "2G6Qafv3");
    let r = pair_amm_trades_with_events(&d, &tx.instructions, tx.slot, tx.transaction_index);
    let t = &r.trades[0].trade;
    assert_eq!(t.variant, PumpAmmTradeVariant::BuyExactQuoteIn);
    assert_eq!(t.args[0].name, "spendable_quote_in");
    assert_eq!(t.args[0].value, 440_033_519_921);
    assert_eq!(t.args[1].value, 2_798_855_771);
    assert_eq!(t.track_volume, Some(1));
    // Reversed pool: base mint is wSOL, quote is the token.
    assert_eq!(t.base_mint, WRAPPED_SOL_MINT);
    let AmmTradeEventPairing::Paired(PumpAmmEvent::Buy(e)) = &r.trades[0].pairing else {
        panic!()
    };
    assert_eq!(e.ix_name.as_deref(), Some("buy_exact_quote_in"));
    assert_eq!(e.quote_amount_in, 440_033_519_921);
    assert_eq!(e.quote_amount_in_with_lp_fee, 439_814_161_237);
    assert_eq!(e.user_quote_amount_in, 438_717_367_817);
    assert_eq!(e.quote_cost(), Some(440_033_519_921));
    assert_eq!(e.quote_cost(), Some(t.args[0].value));
    assert_eq!(e.base_amount_out, 3_288_663_178);

    // 24-byte router-invoked inner buy_exact_quote_in.
    let tx = find(&all, "4fDxyqJw");
    let r = pair_amm_trades_with_events(&d, &tx.instructions, tx.slot, tx.transaction_index);
    let t = &r.trades[0].trade;
    assert_eq!(t.variant, PumpAmmTradeVariant::BuyExactQuoteIn);
    assert_eq!((t.args[0].value, t.args[1].value), (725_654, 1));
    assert_eq!(t.track_volume, None);
    assert!(!tx.signers.contains(&t.user));
    let AmmTradeEventPairing::Paired(PumpAmmEvent::Buy(e)) = &r.trades[0].pairing else {
        panic!()
    };
    assert_eq!(e.base_amount_out, 59_052_700_559);
    assert_eq!(e.coin_creator_fee, 14_194);
    assert_eq!(e.trailing_bytes, 8);
    assert_eq!(e.quote_cost(), Some(725_654));
}

fn tx_signer_b58(tx: &RawSolanaTransaction) -> String {
    b58(&tx.fee_payer)
}

#[test]
fn event_amounts_match_instruction_args_and_consideration_formulas_hold() {
    let d = PumpAmmDecoder::mainnet();
    let mut checked = 0usize;
    for tx in both() {
        if !matches!(tx.execution, SolanaExecutionStatus::Succeeded) {
            continue;
        }
        let r = pair_amm_trades_with_events(&d, &tx.instructions, tx.slot, tx.transaction_index);
        for p in &r.trades {
            let AmmTradeEventPairing::Paired(ev) = &p.pairing else {
                panic!("successful tx trade without paired event");
            };
            let (a0, a1) = (p.trade.args[0].value, p.trade.args[1].value);
            match (p.trade.variant, ev) {
                (PumpAmmTradeVariant::Buy, PumpAmmEvent::Buy(e)) => {
                    assert_eq!(e.base_amount_out, a0);
                    assert_eq!(e.max_quote_amount_in, a1);
                    assert_eq!(e.quote_cost(), Some(e.user_quote_amount_in));
                    assert_eq!(
                        e.quote_amount_in + e.lp_fee + e.protocol_fee + e.coin_creator_fee,
                        e.user_quote_amount_in
                    );
                    assert!(e.user_quote_amount_in <= a1);
                }
                (PumpAmmTradeVariant::BuyExactQuoteIn, PumpAmmEvent::Buy(e)) => {
                    assert_eq!(e.quote_amount_in, a0);
                    assert_eq!(e.min_base_amount_out, Some(a1));
                    assert!(e.base_amount_out >= a1);
                    // gross spend == spendable; the "user_quote_amount_in" field is the net pool credit.
                    assert_eq!(e.quote_cost(), Some(a0));
                    assert_eq!(
                        e.quote_amount_in_with_lp_fee - e.lp_fee,
                        e.user_quote_amount_in
                    );
                    assert_ne!(e.quote_cost(), Some(e.user_quote_amount_in));
                }
                (PumpAmmTradeVariant::Sell, PumpAmmEvent::Sell(e)) => {
                    assert_eq!(e.base_amount_in, a0);
                    assert_eq!(e.min_quote_amount_out, a1);
                    assert_eq!(e.quote_proceeds(), Some(e.user_quote_amount_out));
                    assert_eq!(
                        e.quote_amount_out - e.lp_fee - e.protocol_fee - e.coin_creator_fee,
                        e.user_quote_amount_out
                    );
                    assert!(e.user_quote_amount_out >= a1);
                }
                other => panic!("variant/event kind mismatch {other:?}"),
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 127);
}

#[test]
fn failed_transaction_decodes_but_is_never_reconciled() {
    let d = PumpAmmDecoder::mainnet();
    let all = txs(VARIANTS);
    let tx = find(&all, "4qAKS1JU");
    assert!(matches!(tx.execution, SolanaExecutionStatus::Failed { .. }));
    let r = reconcile_pump_amm_transaction(&d, tx);
    assert!(!r.succeeded);
    assert_eq!(r.pairing.trades.len(), 1);
    assert_eq!(
        r.pairing.trades[0].trade.variant,
        PumpAmmTradeVariant::BuyExactQuoteIn
    );
    assert_eq!(
        r.pairing.trades[0].pairing,
        AmmTradeEventPairing::MissingEvent
    );
    assert!(r.users.is_empty());
}

#[test]
fn close_user_volume_accumulator_is_explicit_non_trade() {
    let d = PumpAmmDecoder::mainnet();
    let mut n = 0;
    let mut events = 0;
    for tx in txs(VARIANTS) {
        for ix in &tx.instructions {
            if let PumpAmmInstructionOutcome::NonTrade(name) = d.classify(ix, 0, 0)
                && name == "close_user_volume_accumulator"
            {
                n += 1;
            }
            if let scout_dex_solana::PumpAmmEventOutcome::OtherEvent { name, .. } =
                d.classify_event(ix)
            {
                assert_eq!(name, "CloseUserVolumeAccumulatorEvent");
                events += 1;
            }
        }
    }
    assert_eq!((n, events), (4, 4));
}

/// Per-variant verification evidence. Keys are the sorted variants of the
/// user group; every successful fixture group must be attributed with an
/// exact base leg, or be a router-forward (`NoUserDelta`, not attributed).
#[test]
fn reconciliation_evidence_promotes_all_three_variants() {
    let d = PumpAmmDecoder::mainnet();
    // (variant, attribution) -> count of trades
    let mut table: BTreeMap<(&str, String), usize> = BTreeMap::new();
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut quote_mints = BTreeSet::new();
    let mut base_wsol_groups = 0usize;
    for tx in both() {
        let r = reconcile_pump_amm_transaction(&d, &tx);
        for u in &r.users {
            *groups.entry(format!("{:?}", u.attribution)).or_default() += 1;
            for &i in &u.trade_indices {
                let t = &r.pairing.trades[i].trade;
                *table
                    .entry((t.variant.name(), format!("{:?}", u.attribution)))
                    .or_default() += 1;
                quote_mints.insert(t.quote_mint);
                if t.base_mint == WRAPPED_SOL_MINT {
                    base_wsol_groups += 1;
                }
            }
            // Never an unexplained base leg / unpaired / invalid event.
            assert!(
                !matches!(
                    u.attribution,
                    AmmAttribution::BaseLegMismatch
                        | AmmAttribution::Unpaired
                        | AmmAttribution::ConsiderationInvalid
                ),
                "{} {:?}",
                sig_prefix(&tx),
                u.attribution
            );
            // Attributed users are transaction signers; router-forwards are not.
            assert_eq!(
                u.attribution == AmmAttribution::NoUserDelta,
                !u.user_is_signer,
                "{}",
                sig_prefix(&tx)
            );
            for l in u.legs.iter().filter(|l| l.is_base) {
                if u.attribution != AmmAttribution::NoUserDelta {
                    assert_eq!(l.residual(), 0, "{} base leg", sig_prefix(&tx));
                }
            }
        }
    }
    eprintln!("groups {groups:?}");
    eprintln!("table {table:?}");
    eprintln!(
        "quote mints {:?} base_wsol_trades {base_wsol_groups}",
        quote_mints.iter().map(b58).collect::<Vec<_>>()
    );
    assert_eq!(groups.values().sum::<usize>(), 126);
    assert_eq!(groups.get("Exact").copied(), Some(116));
    assert_eq!(groups.get("QuoteResidual").copied(), Some(4));
    assert_eq!(groups.get("QuoteFundedElsewhere").copied(), Some(3));
    assert_eq!(groups.get("NoUserDelta").copied(), Some(3));
    for v in PumpAmmTradeVariant::ALL {
        assert_eq!(v.verification(), VariantVerification::FixtureVerified);
    }
}

/// Residual table (ADR-012 evidence): signature prefix, expected/actual quote
/// leg of the user, and the explained cause.
#[test]
fn residual_table_for_non_exact_groups() {
    let d = PumpAmmDecoder::mainnet();
    let all = txs(VARIANTS);
    // (sig, attribution, quote expected, quote actual, quote residual)
    let expect: [(&str, AmmAttribution, i128, i128, i128); 10] = [
        // ~1% bot platform fee (970,453) + 10,000 tip
        (
            "4n5jV2nG",
            AmmAttribution::QuoteResidual,
            97_045_370,
            96_064_917,
            -980_453,
        ),
        // base Token-2022 ATA rent 1,513,840 + user_volume_accumulator rent 1,346,200
        (
            "3J8cn7q6",
            AmmAttribution::QuoteResidual,
            -14_474_566,
            -17_334_606,
            -2_860_040,
        ),
        // user_volume_accumulator rent
        (
            "3Kb6kd8r",
            AmmAttribution::QuoteResidual,
            -49_268_061,
            -50_614_261,
            -1_346_200,
        ),
        // 34,999 = 0.5% router platform fee transfer
        (
            "49Jx5G4m",
            AmmAttribution::QuoteResidual,
            -6_965_000,
            -6_999_999,
            -34_999,
        ),
        // quote funded by another account of the transaction
        (
            "3HA7wLti",
            AmmAttribution::QuoteFundedElsewhere,
            -123_071_722,
            0,
            123_071_722,
        ),
        (
            "3YiGgvyq",
            AmmAttribution::QuoteFundedElsewhere,
            -309_326_800,
            0,
            309_326_800,
        ),
        (
            "2LBaVMPm",
            AmmAttribution::QuoteFundedElsewhere,
            -84_020_022,
            0,
            84_020_022,
        ),
        // router-forward: the decoded user nets zero on every leg
        (
            "4fDxyqJw",
            AmmAttribution::NoUserDelta,
            -725_654,
            0,
            725_654,
        ),
        (
            "24w5aqcE",
            AmmAttribution::NoUserDelta,
            194_031_429,
            0,
            -194_031_429,
        ),
        (
            "53hB8y2M",
            AmmAttribution::NoUserDelta,
            -4_975_000_000,
            0,
            4_975_000_000,
        ),
    ];
    for (sig, attribution, exp, act, res) in expect {
        let r = reconcile_pump_amm_transaction(&d, find(&all, sig));
        assert_eq!(r.users.len(), 1, "{sig}");
        let u = &r.users[0];
        assert_eq!(u.attribution, attribution, "{sig}");
        let q = u
            .legs
            .iter()
            .find(|l| l.is_quote && !l.is_base)
            .unwrap_or_else(|| panic!("{sig}: quote leg"));
        assert_eq!(
            (q.expected, q.actual, q.residual()),
            (exp, act, res),
            "{sig}"
        );
    }
    // The shared-user sell + buy_exact_quote_in transaction reconciles in one group.
    let r = reconcile_pump_amm_transaction(&d, find(&all, "3HoDy4Ac"));
    assert_eq!(r.users.len(), 1);
    assert_eq!(r.users[0].trade_indices.len(), 2);
    assert_eq!(r.users[0].attribution, AmmAttribution::Exact);
}
