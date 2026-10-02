//! Real-fixture checks for the pump.fun `TradeEvent` decoder and the
//! trade/event pairing. Fixtures are raw Helius JSON, parsed directly here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use scout_api::DeploymentScope;
use scout_core::{
    AddressBytes, ChainFamily, ChainKey, GenesisIdentity, NetworkId, RawSolanaInstruction,
    SolanaCluster, SolanaPubkey,
};
use scout_dex_solana::{
    BondingCurveBuyDecoder, PumpEventOutcome, PumpTradeVariant, TradeEvent, TradeEventPairing,
    TradeSide, classify_pump_event, pair_trades_with_events,
};

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

fn decoder() -> BondingCurveBuyDecoder {
    BondingCurveBuyDecoder::new(DeploymentScope {
        chain: ChainKey {
            family: ChainFamily::Solana,
            network_id: NetworkId::SolanaCluster(SolanaCluster::Mainnet),
            genesis_identity: GenesisIdentity::Unverified,
        },
        contract_addresses: vec![AddressBytes::Solana(pubkey(PUMP))],
        active_from: 0,
        active_until: None,
    })
}

fn fixture(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

struct Tx {
    signature: String,
    success: bool,
    slot: u64,
    index: u64,
    instructions: Vec<RawSolanaInstruction>,
    /// (owner, mint) -> net raw delta over all token accounts.
    deltas: BTreeMap<(SolanaPubkey, SolanaPubkey), i128>,
}

fn raw_ix(v: &serde_json::Value, keys: &[SolanaPubkey], index: u32) -> RawSolanaInstruction {
    let idx = |n: &serde_json::Value| usize::try_from(n.as_u64().unwrap()).unwrap();
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

fn parse_tx(rec: &serde_json::Value) -> Tx {
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
    let mut deltas: BTreeMap<(SolanaPubkey, SolanaPubkey), i128> = BTreeMap::new();
    for (field, sign) in [("preTokenBalances", -1i128), ("postTokenBalances", 1i128)] {
        for b in meta[field].as_array().unwrap() {
            let Some(owner) = b["owner"].as_str() else {
                continue;
            };
            let amount: i128 = b["uiTokenAmount"]["amount"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            *deltas
                .entry((pubkey(owner), pubkey(b["mint"].as_str().unwrap())))
                .or_default() += sign * amount;
        }
    }
    Tx {
        signature: rec["transaction"]["signatures"][0]
            .as_str()
            .unwrap()
            .to_string(),
        success: meta["err"].is_null(),
        slot: rec["slot"].as_u64().unwrap(),
        index: rec["transactionIndex"].as_u64().unwrap(),
        instructions,
        deltas,
    }
}

/// Every transaction record (`meta` + `transaction`) anywhere in the JSON.
fn collect_records<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
    match v {
        serde_json::Value::Object(m) => {
            if m.contains_key("meta") && m.contains_key("transaction") {
                out.push(v);
            } else {
                m.values().for_each(|x| collect_records(x, out));
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_records(x, out)),
        _ => {}
    }
}

/// (fixture name, tx). Economics are asserted only for the two trade
/// fixtures; the others are decode-only.
fn all_txs() -> Vec<(&'static str, Tx)> {
    let mut out = Vec::new();
    for name in [
        "pump_bonding_curve_buy_probe",
        "pump_variants_live_2026-10-02",
        "pump_mint1_full",
        "pump_mint2_full",
        "wallet_full_probe",
    ] {
        let doc = fixture(&format!("{name}.json"));
        let mut recs = Vec::new();
        collect_records(&doc, &mut recs);
        assert!(!recs.is_empty(), "{name}");
        out.extend(recs.into_iter().map(|r| (name, parse_tx(r))));
    }
    out
}

fn arg(t: &scout_dex_solana::DecodedBondingCurveTrade, name: &str) -> u64 {
    t.args.iter().find(|a| a.name == name).unwrap().value
}

/// (last field, trailing bytes, ix_name, shareholder count).
type Shape<'a> = (&'a str, usize, String, usize);

const EXPECTED_SHAPES: [(usize, &str, usize, &str, usize, usize); 5] = [
    (382, "holder_rewards", 0, "buy", 0, 7),
    (383, "holder_rewards", 0, "sell", 0, 2),
    (395, "holder_rewards", 0, "buy_exact_sol_in", 0, 6),
    (397, "holder_rewards", 0, "buy_exact_quote_in", 0, 5),
    (431, "holder_rewards", 0, "buy_exact_quote_in", 1, 1),
];

#[test]
fn every_fixture_trade_event_decodes_and_pairs() {
    let d = decoder();
    let (mut txs_n, mut ok_txs, mut trades, mut paired, mut events, mut other) = (0, 0, 0, 0, 0, 0);
    let mut economics = 0;
    let mut saw_router_forward = false;
    let mut by_len: BTreeMap<usize, (Shape, usize)> = BTreeMap::new();
    for (src, tx) in all_txs() {
        txs_n += 1;
        // Zero malformed/unknown events anywhere in the fixtures.
        for ix in &tx.instructions {
            match d.classify_event(ix) {
                PumpEventOutcome::Malformed { reason } => panic!("{}: {reason}", tx.signature),
                PumpEventOutcome::UnknownEvent { discriminator } => {
                    panic!("{}: unknown event {discriminator:?}", tx.signature)
                }
                PumpEventOutcome::Trade(ev) => {
                    events += 1;
                    let shape = (
                        ev.last_field_present,
                        ev.trailing_bytes,
                        ev.ix_name.clone().unwrap(),
                        ev.shareholders.as_ref().unwrap().len(),
                    );
                    let e = by_len.entry(ev.data_len).or_insert((shape.clone(), 0));
                    assert_eq!(e.0, shape);
                    e.1 += 1;
                }
                PumpEventOutcome::OtherEvent { .. } => other += 1,
                _ => {}
            }
        }
        let rep = pair_trades_with_events(&d, &tx.instructions, tx.slot, tx.index);
        assert_eq!(rep.malformed_events, 0, "{}", tx.signature);
        if !tx.success {
            continue;
        }
        ok_txs += 1;
        assert_eq!(rep.malformed_trades, 0, "{}", tx.signature);
        assert!(rep.orphan_events.is_empty(), "{}", tx.signature);
        for p in &rep.trades {
            trades += 1;
            let TradeEventPairing::Paired(ev) = &p.pairing else {
                panic!("{}: unpaired {:?}", tx.signature, p.pairing);
            };
            paired += 1;
            if src.starts_with("pump_bonding") || src.starts_with("pump_variants") {
                check_economics(&tx, &p.trade, ev);
                economics += 1;
                saw_router_forward |= tx.signature.starts_with("bUh87USD");
            }
        }
    }
    println!(
        "txs={txs_n} ok={ok_txs} trades={trades} paired={paired} trade_events={events} other_events={other}"
    );
    for (len, (shape, n)) in &by_len {
        println!("len {len}: {shape:?} count={n}");
    }
    // Locked observation table: data length -> (last field, trailing bytes,
    // ix_name, shareholders) and event count. Update deliberately when new
    // fixtures are committed.
    let observed: Vec<_> = by_len
        .iter()
        .map(|(l, ((last, tr, ix, sh), n))| (*l, *last, *tr, ix.as_str(), *sh, *n))
        .collect();
    assert_eq!(observed, EXPECTED_SHAPES);
    assert_eq!(trades, paired);
    assert_eq!((txs_n, ok_txs, trades, events, other), (32, 31, 21, 21, 3));
    assert_eq!(economics, 20);
    assert!(saw_router_forward);
}

fn check_economics(tx: &Tx, t: &scout_dex_solana::DecodedBondingCurveTrade, ev: &TradeEvent) {
    let sig = &tx.signature;
    let delta = tx.deltas.get(&(t.user, t.mint)).copied().unwrap_or(0);
    let tokens = i128::from(ev.token_amount);
    match t.variant {
        PumpTradeVariant::Buy | PumpTradeVariant::BuyV2 => {
            assert_eq!(ev.token_amount, arg(t, "amount"), "{sig} {:?}", t.variant);
            assert_eq!(delta, tokens, "{sig} {:?}", t.variant);
        }
        PumpTradeVariant::BuyExactSolIn | PumpTradeVariant::BuyExactQuoteInV2 => {
            let spend_name = if t.variant == PumpTradeVariant::BuyExactSolIn {
                "spendable_sol_in"
            } else {
                "spendable_quote_in"
            };
            assert!(ev.token_amount >= arg(t, "min_tokens_out"), "{sig}");
            if t.variant == PumpTradeVariant::BuyExactSolIn {
                assert!(ev.sol_amount <= arg(t, spend_name), "{sig}");
            } else if let Some(q) = ev.quote_amount {
                assert!(q <= arg(t, spend_name), "{sig} quote_amount");
            }
            if sig.starts_with("bUh87USD") {
                // Router-forward: the decoded user nets zero; the tokens
                // reached a different owner's account.
                assert_eq!(delta, 0, "{sig}");
                let forwarded: i128 = tx
                    .deltas
                    .iter()
                    .filter(|((o, m), v)| *m == t.mint && *o != t.user && **v > 0)
                    .map(|(_, v)| *v)
                    .sum();
                assert_eq!(forwarded, tokens, "{sig} forwarded");
            } else {
                assert_eq!(delta, tokens, "{sig}");
            }
        }
        PumpTradeVariant::Sell | PumpTradeVariant::SellV2 => {
            assert_eq!(ev.token_amount, arg(t, "amount"), "{sig}");
            assert!(ev.sol_amount >= arg(t, "min_sol_output"), "{sig}");
            assert_eq!(delta, -tokens, "{sig}");
        }
    }
    assert_eq!(ev.is_buy, t.side == TradeSide::Buy);
}

#[test]
fn foreign_program_event_cpi_is_not_mine() {
    let d = decoder();
    let (_, tx) = all_txs().into_iter().next().unwrap();
    let mut ix = tx
        .instructions
        .iter()
        .find(|i| classify_pump_event(i) != PumpEventOutcome::NotEventCpi)
        .unwrap()
        .clone();
    ix.program_id = pubkey("11111111111111111111111111111111");
    assert_eq!(d.classify_event(&ix), PumpEventOutcome::NotMine);
}
