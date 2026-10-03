//! IDL-equality and synthetic-case tests for the PumpSwap AMM decoder:
//! every discriminator/account position/layout constant is re-derived from
//! the committed official IDL file; malformed, gate and gap handling use
//! hand-built instructions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::{
    AddressBytes, ChainFamily, ChainKey, GenesisIdentity, NetworkId, RawSolanaInstruction,
    SolanaCluster, SolanaPubkey,
};
use scout_dex_solana::{
    AMM_BUY_DISCRIMINATOR, AMM_BUY_EVENT_DISCRIMINATOR, AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
    AMM_EVENT_CPI_NAME, AMM_EVENT_REQUIRED_LEN, AMM_NON_TRADE_INSTRUCTIONS, AMM_OTHER_EVENTS,
    AMM_SELL_DISCRIMINATOR, AMM_SELL_EVENT_DISCRIMINATOR, AmmTradeEventPairing, BASE_MINT_IDX,
    EVENT_CPI_DISCRIMINATOR, MAX_AMM_TRAILING_EVENT_BYTES, POOL_IDX, PUMP_AMM_IDL_SHA256,
    PUMP_AMM_PROGRAM_ID, PUMP_AMM_PROGRAM_ID_BYTES, PumpAmmDecoder, PumpAmmEvent,
    PumpAmmEventOutcome, PumpAmmInstruction, PumpAmmInstructionOutcome, PumpAmmTradeVariant,
    QUOTE_MINT_IDX, TradeSide, USER_BASE_TOKEN_ACCOUNT_IDX, USER_IDX, USER_QUOTE_TOKEN_ACCOUNT_IDX,
    VariantVerification, WRAPPED_SOL_MINT, classify_pump_amm_event, pair_amm_trades_with_events,
};
use sha2::{Digest as _, Sha256};

fn idl() -> serde_json::Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures/pump_amm_idl_e0687ae9.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn disc_of(v: &serde_json::Value) -> [u8; 8] {
    let b: Vec<u8> = v["discriminator"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| u8::try_from(n.as_u64().unwrap()).unwrap())
        .collect();
    b.try_into().unwrap()
}

fn ix<'a>(idl: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    idl["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap()
}

fn account_names(i: &serde_json::Value) -> Vec<String> {
    i["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap().to_string())
        .collect()
}

fn type_fields(idl: &serde_json::Value, name: &str) -> Vec<(String, String)> {
    let t = idl["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap();
    t["type"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap().to_string(),
                f["type"].as_str().unwrap_or("defined").to_string(),
            )
        })
        .collect()
}

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn amm() -> SolanaPubkey {
    PUMP_AMM_PROGRAM_ID_BYTES
}

fn decoder() -> PumpAmmDecoder {
    PumpAmmDecoder::mainnet()
}

#[test]
fn idl_file_is_the_pinned_one() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures/pump_amm_idl_e0687ae9.json");
    let digest = Sha256::digest(std::fs::read(p).unwrap());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, PUMP_AMM_IDL_SHA256);
    assert_eq!(idl()["address"], PUMP_AMM_PROGRAM_ID);
}

#[test]
fn program_id_and_wsol_constants_match_base58() {
    assert_eq!(
        bs58::decode(PUMP_AMM_PROGRAM_ID).into_vec().unwrap(),
        PUMP_AMM_PROGRAM_ID_BYTES.to_vec()
    );
    assert_eq!(
        bs58::decode("So11111111111111111111111111111111111111112")
            .into_vec()
            .unwrap(),
        WRAPPED_SOL_MINT.to_vec()
    );
}

#[test]
fn trade_variants_match_idl_discriminators_accounts_and_args() {
    let idl = idl();
    for v in PumpAmmTradeVariant::ALL {
        let spec = v.spec();
        let i = ix(&idl, spec.name);
        assert_eq!(i["discriminator"].as_array().unwrap().len(), 8);
        assert_eq!(disc_of(i), spec.discriminator, "{}", spec.name);
        let names = account_names(i);
        assert_eq!(names.len(), spec.min_accounts, "{}", spec.name);
        assert_eq!(names[POOL_IDX], "pool");
        assert_eq!(names[USER_IDX], "user");
        assert_eq!(names[BASE_MINT_IDX], "base_mint");
        assert_eq!(names[QUOTE_MINT_IDX], "quote_mint");
        assert_eq!(
            names[USER_BASE_TOKEN_ACCOUNT_IDX],
            "user_base_token_account"
        );
        assert_eq!(
            names[USER_QUOTE_TOKEN_ACCOUNT_IDX],
            "user_quote_token_account"
        );
        let args = i["args"].as_array().unwrap();
        assert_eq!(args[0]["name"], spec.arg_names[0]);
        assert_eq!(args[1]["name"], spec.arg_names[1]);
        assert_eq!(args[0]["type"], "u64");
        assert_eq!(args[1]["type"], "u64");
        assert_eq!(args.len() == 3, spec.has_track_volume, "{}", spec.name);
        if spec.has_track_volume {
            assert_eq!(args[2]["name"], "track_volume");
        }
        assert_eq!(spec.data_len, 8 + 16 + usize::from(spec.has_track_volume));
        assert!(v.index() < PumpAmmTradeVariant::COUNT);
    }
    assert_eq!(AMM_BUY_DISCRIMINATOR, disc_of(ix(&idl, "buy")));
    assert_eq!(
        AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
        disc_of(ix(&idl, "buy_exact_quote_in"))
    );
    assert_eq!(AMM_SELL_DISCRIMINATOR, disc_of(ix(&idl, "sell")));
    assert_eq!(PumpAmmTradeVariant::Buy.side(), TradeSide::Buy);
    assert_eq!(PumpAmmTradeVariant::BuyExactQuoteIn.side(), TradeSide::Buy);
    assert_eq!(PumpAmmTradeVariant::Sell.side(), TradeSide::Sell);
}

#[test]
fn non_trade_and_event_tables_equal_the_idl() {
    let idl = idl();
    let trades: BTreeSet<&str> = ["buy", "buy_exact_quote_in", "sell"].into();
    let mut expected: BTreeSet<(String, [u8; 8])> = idl["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| !trades.contains(i["name"].as_str().unwrap()))
        .map(|i| (i["name"].as_str().unwrap().to_string(), disc_of(i)))
        .collect();
    expected.insert((AMM_EVENT_CPI_NAME.to_string(), EVENT_CPI_DISCRIMINATOR));
    let actual: BTreeSet<(String, [u8; 8])> = AMM_NON_TRADE_INSTRUCTIONS
        .iter()
        .map(|(n, d)| ((*n).to_string(), *d))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(idl["instructions"].as_array().unwrap().len(), 32);
    // No overlap between trade and non-trade discriminators.
    for (_, d) in AMM_NON_TRADE_INSTRUCTIONS {
        assert!(
            PumpAmmTradeVariant::ALL
                .iter()
                .all(|v| v.spec().discriminator != d)
        );
    }

    let events: BTreeSet<(String, [u8; 8])> = idl["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap().to_string(), disc_of(e)))
        .collect();
    let mut ours: BTreeSet<(String, [u8; 8])> = AMM_OTHER_EVENTS
        .iter()
        .map(|(n, d)| ((*n).to_string(), *d))
        .collect();
    ours.insert(("BuyEvent".into(), AMM_BUY_EVENT_DISCRIMINATOR));
    ours.insert(("SellEvent".into(), AMM_SELL_EVENT_DISCRIMINATOR));
    assert_eq!(ours, events);
}

const BUY_EVENT_FIELDS: [&str; 40] = [
    "timestamp",
    "base_amount_out",
    "max_quote_amount_in",
    "user_base_token_reserves",
    "user_quote_token_reserves",
    "pool_base_token_reserves",
    "pool_quote_token_reserves",
    "quote_amount_in",
    "lp_fee_basis_points",
    "lp_fee",
    "protocol_fee_basis_points",
    "protocol_fee",
    "quote_amount_in_with_lp_fee",
    "user_quote_amount_in",
    "pool",
    "user",
    "user_base_token_account",
    "user_quote_token_account",
    "protocol_fee_recipient",
    "protocol_fee_recipient_token_account",
    "coin_creator",
    "coin_creator_fee_basis_points",
    "coin_creator_fee",
    "track_volume",
    "total_unclaimed_tokens",
    "total_claimed_tokens",
    "current_sol_volume",
    "last_update_timestamp",
    "min_base_amount_out",
    "ix_name",
    "cashback_fee_basis_points",
    "cashback",
    "buyback_fee_basis_points",
    "buyback_fee",
    "virtual_quote_reserves",
    "can_boost",
    "base_supply",
    "holder_rewards_bps",
    "holder_rewards",
    "<end>",
];

const SELL_EVENT_FIELDS: [&str; 33] = [
    "timestamp",
    "base_amount_in",
    "min_quote_amount_out",
    "user_base_token_reserves",
    "user_quote_token_reserves",
    "pool_base_token_reserves",
    "pool_quote_token_reserves",
    "quote_amount_out",
    "lp_fee_basis_points",
    "lp_fee",
    "protocol_fee_basis_points",
    "protocol_fee",
    "quote_amount_out_without_lp_fee",
    "user_quote_amount_out",
    "pool",
    "user",
    "user_base_token_account",
    "user_quote_token_account",
    "protocol_fee_recipient",
    "protocol_fee_recipient_token_account",
    "coin_creator",
    "coin_creator_fee_basis_points",
    "coin_creator_fee",
    "cashback_fee_basis_points",
    "cashback",
    "buyback_fee_basis_points",
    "buyback_fee",
    "virtual_quote_reserves",
    "can_boost",
    "base_supply",
    "holder_rewards_bps",
    "holder_rewards",
    "<end>",
];

#[test]
fn event_field_order_and_required_prefix_equal_the_idl() {
    let idl = idl();
    for (name, expected) in [
        ("BuyEvent", &BUY_EVENT_FIELDS[..39]),
        ("SellEvent", &SELL_EVENT_FIELDS[..32]),
    ] {
        let fields = type_fields(&idl, name);
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, expected, "{name}");
        let mut required = 0usize;
        for (n, t) in &fields {
            required += match t.as_str() {
                "i64" | "u64" => 8,
                "pubkey" => 32,
                other => panic!("unexpected type {other} in prefix at {n}"),
            };
            if n == "coin_creator_fee" {
                break;
            }
        }
        assert_eq!(required, AMM_EVENT_REQUIRED_LEN, "{name}");
    }
}

// ---------------------------------------------------------------------------
// Synthetic instruction cases
// ---------------------------------------------------------------------------

fn trade_ix(
    program: SolanaPubkey,
    disc: [u8; 8],
    data_len: usize,
    accounts: usize,
) -> RawSolanaInstruction {
    let mut data = disc.to_vec();
    data.extend_from_slice(&7u64.to_le_bytes());
    data.extend_from_slice(&9u64.to_le_bytes());
    data.resize(data_len, 1);
    data.truncate(data_len);
    RawSolanaInstruction {
        program_id: program,
        accounts: (0..accounts)
            .map(|i| pk(u8::try_from(i).unwrap() + 100))
            .collect(),
        data,
        instruction_index: 3,
    }
}

#[test]
fn decodes_each_variant_with_idl_positions() {
    let d = decoder();
    for (disc, len, accounts, variant) in [
        (AMM_BUY_DISCRIMINATOR, 24, 23, PumpAmmTradeVariant::Buy),
        (AMM_BUY_DISCRIMINATOR, 25, 26, PumpAmmTradeVariant::Buy),
        (AMM_BUY_DISCRIMINATOR, 26, 26, PumpAmmTradeVariant::Buy),
        (
            AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
            24,
            23,
            PumpAmmTradeVariant::BuyExactQuoteIn,
        ),
        (
            AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
            25,
            25,
            PumpAmmTradeVariant::BuyExactQuoteIn,
        ),
        (
            AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
            26,
            24,
            PumpAmmTradeVariant::BuyExactQuoteIn,
        ),
        (AMM_SELL_DISCRIMINATOR, 24, 21, PumpAmmTradeVariant::Sell),
    ] {
        let PumpAmmInstructionOutcome::Trade(t) =
            d.classify(&trade_ix(amm(), disc, len, accounts), 11, 22)
        else {
            panic!("{variant:?} len {len} should decode");
        };
        assert_eq!(t.variant, variant);
        assert_eq!(t.pool, pk(100));
        assert_eq!(t.user, pk(101));
        assert_eq!(t.base_mint, pk(103));
        assert_eq!(t.quote_mint, pk(104));
        assert_eq!(t.user_base_token_account, pk(105));
        assert_eq!(t.user_quote_token_account, pk(106));
        assert_eq!(t.args[0].value, 7);
        assert_eq!(t.args[1].value, 9);
        assert_eq!(t.args[0].name, variant.spec().arg_names[0]);
        assert_eq!(t.track_volume, if len >= 25 { Some(1) } else { None });
        assert_eq!(
            (t.slot, t.transaction_index, t.instruction_index),
            (11, 22, 3)
        );
        assert_eq!(t.verification(), VariantVerification::FixtureVerified);
    }
}

#[test]
fn malformed_lengths_and_account_counts_are_counted_never_guessed() {
    let d = decoder();
    for (disc, len, accounts) in [
        (AMM_BUY_DISCRIMINATOR, 23, 23),
        (AMM_BUY_DISCRIMINATOR, 27, 23),
        (AMM_BUY_DISCRIMINATOR, 24, 22),
        (AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR, 27, 23),
        (AMM_SELL_DISCRIMINATOR, 26, 21),
        (AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR, 16, 23),
        (AMM_SELL_DISCRIMINATOR, 25, 21),
        (AMM_SELL_DISCRIMINATOR, 24, 20),
        (AMM_SELL_DISCRIMINATOR, 8, 21),
    ] {
        let out = d.classify(&trade_ix(amm(), disc, len, accounts), 0, 0);
        assert!(
            matches!(
                out,
                PumpAmmInstructionOutcome::Malformed {
                    variant: Some(_),
                    ..
                }
            ),
            "len {len} accounts {accounts}: {out:?}"
        );
    }
    // 26 bytes with an invalid Option<bool> tail stays malformed (ADR-009
    // amendment 2026-10-03); `trade_ix` pads with 0x01, so patch bytes 24/25.
    for disc in [AMM_BUY_DISCRIMINATOR, AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR] {
        for tail in [[0u8, 1], [0, 0], [2, 1], [1, 2]] {
            let mut ix = trade_ix(amm(), disc, 26, 26);
            ix.data[24..26].copy_from_slice(&tail);
            assert!(
                matches!(
                    d.classify(&ix, 0, 0),
                    PumpAmmInstructionOutcome::Malformed {
                        variant: Some(_),
                        ..
                    }
                ),
                "{tail:?}"
            );
        }
    }
    // fewer than 8 data bytes
    let mut short = trade_ix(amm(), AMM_SELL_DISCRIMINATOR, 24, 21);
    short.data.truncate(5);
    assert!(matches!(
        d.classify(&short, 0, 0),
        PumpAmmInstructionOutcome::Malformed { variant: None, .. }
    ));
    // TxDecoder surface reports Malformed too.
    let bad = trade_ix(amm(), AMM_SELL_DISCRIMINATOR, 30, 21);
    assert!(matches!(d.decode(&bad), DecodeOutcome::Malformed(_)));
}

#[test]
fn program_id_gate_ignores_same_discriminator_under_another_program() {
    let d = decoder();
    for disc in [
        AMM_BUY_DISCRIMINATOR,
        AMM_BUY_EXACT_QUOTE_IN_DISCRIMINATOR,
        AMM_SELL_DISCRIMINATOR,
    ] {
        let other = trade_ix(pk(7), disc, 24, 25);
        assert_eq!(d.classify(&other, 0, 0), PumpAmmInstructionOutcome::NotMine);
        assert!(matches!(d.decode(&other), DecodeOutcome::NotMine));
    }
    // Event-CPI under another program is not an event either.
    let mut ev = trade_ix(pk(7), EVENT_CPI_DISCRIMINATOR, 16, 1);
    ev.data[8..16].copy_from_slice(&AMM_BUY_EVENT_DISCRIMINATOR);
    assert_eq!(d.classify_event(&ev), PumpAmmEventOutcome::NotMine);
    // An empty scope matches nothing, not everything.
    let empty = PumpAmmDecoder::new(DeploymentScope {
        chain: ChainKey {
            family: ChainFamily::Solana,
            network_id: NetworkId::SolanaCluster(SolanaCluster::Mainnet),
            genesis_identity: GenesisIdentity::Unverified,
        },
        contract_addresses: vec![],
        active_from: 0,
        active_until: None,
    });
    assert_eq!(
        empty.classify(&trade_ix(amm(), AMM_BUY_DISCRIMINATOR, 25, 25), 0, 0),
        PumpAmmInstructionOutcome::NotMine
    );
    assert!(matches!(
        decoder().scope().contract_addresses[0],
        AddressBytes::Solana(p) if p == PUMP_AMM_PROGRAM_ID_BYTES
    ));
}

#[test]
fn unknown_discriminator_is_a_gap_and_known_non_trade_is_explicit() {
    let d = decoder();
    let unknown = trade_ix(amm(), [9, 9, 9, 9, 9, 9, 9, 9], 24, 5);
    assert_eq!(
        d.classify(&unknown, 0, 0),
        PumpAmmInstructionOutcome::UnknownDiscriminator {
            discriminator: [9; 8]
        }
    );
    assert!(matches!(d.decode(&unknown), DecodeOutcome::Malformed(_)));

    let close = trade_ix(
        amm(),
        [0xf9, 0x45, 0xa4, 0xda, 0x96, 0x67, 0x54, 0x8a],
        8,
        4,
    );
    assert_eq!(
        d.classify(&close, 0, 0),
        PumpAmmInstructionOutcome::NonTrade("close_user_volume_accumulator")
    );
    assert!(matches!(
        d.decode(&close),
        DecodeOutcome::Decoded(PumpAmmInstruction::NonTrade(
            "close_user_volume_accumulator"
        ))
    ));
    let tag = trade_ix(amm(), EVENT_CPI_DISCRIMINATOR, 16, 1);
    assert_eq!(
        d.classify(&tag, 0, 0),
        PumpAmmInstructionOutcome::NonTrade(AMM_EVENT_CPI_NAME)
    );
}

// ---------------------------------------------------------------------------
// Synthetic events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct EvSpec {
    buy: bool,
    user: u8,
    pool: u8,
    ub: u8,
    uq: u8,
}

fn le(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

/// Required prefix of an event (352 bytes).
fn event_prefix(s: EvSpec) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(1_700_000_000i64.to_le_bytes()); // timestamp
    // 13 u64s: amounts in, limits, reserves, quote amounts and fees.
    // buy:  base_out, max_in, ubr, uqr, pbr, pqr, quote_in, lp_bps, lp_fee, pf_bps, pf,
    //       quote_in_with_lp, user_quote_in
    // sell: base_in, min_out, ubr, uqr, pbr, pqr, quote_out, lp_bps, lp_fee, pf_bps, pf,
    //       quote_out_without_lp, user_quote_out
    let vals: [u64; 13] = if s.buy {
        [1000, 5000, 11, 12, 13, 14, 1000, 20, 2, 5, 3, 1002, 1100]
    } else {
        [1000, 10, 11, 12, 13, 14, 2000, 20, 4, 5, 6, 1996, 1980]
    };
    for v in vals {
        p.extend(le(v));
    }
    for b in [s.pool, s.user, s.ub, s.uq, 0xf1, 0xf2, 0xcc] {
        p.extend(pk(b));
    }
    p.extend(le(30)); // coin_creator_fee_basis_points
    p.extend(le(100)); // coin_creator_fee
    assert_eq!(p.len(), AMM_EVENT_REQUIRED_LEN);
    p
}

fn event_ix(s: EvSpec, payload: Vec<u8>, program: SolanaPubkey, idx: u32) -> RawSolanaInstruction {
    let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
    data.extend(if s.buy {
        AMM_BUY_EVENT_DISCRIMINATOR
    } else {
        AMM_SELL_EVENT_DISCRIMINATOR
    });
    data.extend(payload);
    RawSolanaInstruction {
        program_id: program,
        accounts: vec![pk(0xea)],
        data,
        instruction_index: idx,
    }
}

fn buy_spec() -> EvSpec {
    EvSpec {
        buy: true,
        user: 101,
        pool: 100,
        ub: 105,
        uq: 106,
    }
}

/// Full BuyEvent tail from `track_volume` to `holder_rewards`.
fn buy_tail() -> Vec<u8> {
    let mut t = vec![1u8]; // track_volume
    for v in [5u64, 6, 7] {
        t.extend(le(v)); // unclaimed, claimed, current_sol_volume
    }
    t.extend(8i64.to_le_bytes()); // last_update_timestamp
    t.extend(le(9)); // min_base_amount_out
    t.extend(3u32.to_le_bytes());
    t.extend(b"buy"); // ix_name
    for v in [0u64, 0, 5000, 50] {
        t.extend(le(v)); // cashback bps, cashback, buyback bps, buyback fee
    }
    t.extend((-5i128).to_le_bytes()); // virtual_quote_reserves
    t.push(1); // can_boost
    t.extend(le(777)); // base_supply
    t.extend(le(30)); // holder_rewards_bps
    t.extend(le(31)); // holder_rewards
    t
}

#[test]
fn buy_event_decodes_all_fields_and_formula() {
    let s = buy_spec();
    let mut payload = event_prefix(s);
    payload.extend(buy_tail());
    payload.extend([0xaa; 8]); // unknown appended bytes
    let PumpAmmEventOutcome::Trade(PumpAmmEvent::Buy(e)) =
        decoder().classify_event(&event_ix(s, payload, amm(), 4))
    else {
        panic!("buy event expected")
    };
    assert_eq!(e.timestamp, 1_700_000_000);
    assert_eq!(e.base_amount_out, 1000);
    assert_eq!(e.max_quote_amount_in, 5000);
    assert_eq!(e.quote_amount_in, 1000);
    assert_eq!(e.lp_fee, 2);
    assert_eq!(e.protocol_fee, 3);
    assert_eq!(e.quote_amount_in_with_lp_fee, 1002);
    assert_eq!(e.user_quote_amount_in, 1100);
    assert_eq!(e.coin_creator_fee, 100);
    assert_eq!(e.user, pk(101));
    assert_eq!(e.pool, pk(100));
    assert_eq!(e.track_volume, Some(true));
    assert_eq!(e.min_base_amount_out, Some(9));
    assert_eq!(e.ix_name.as_deref(), Some("buy"));
    assert_eq!(e.buyback_fee, Some(50));
    assert_eq!(e.virtual_quote_reserves, Some(-5));
    assert_eq!(e.can_boost, Some(true));
    assert_eq!(e.base_supply, Some(777));
    assert_eq!(e.holder_rewards, Some(31));
    assert_eq!(e.last_field_present, "holder_rewards");
    assert_eq!(e.trailing_bytes, 8);
    // quote cost = 1002 + 3 + 100
    assert_eq!(e.quote_cost(), Some(1105));
    assert_eq!(e.instruction_index, 4);
}

#[test]
fn sell_event_decodes_and_proceeds_formula() {
    let s = EvSpec {
        buy: false,
        ..buy_spec()
    };
    let PumpAmmEventOutcome::Trade(PumpAmmEvent::Sell(e)) =
        decoder().classify_event(&event_ix(s, event_prefix(s), amm(), 0))
    else {
        panic!("sell event expected")
    };
    assert_eq!(e.base_amount_in, 1000);
    assert_eq!(e.quote_amount_out, 2000);
    assert_eq!(e.quote_amount_out_without_lp_fee, 1996);
    assert_eq!(e.protocol_fee, 6);
    assert_eq!(e.coin_creator_fee, 100);
    assert_eq!(e.cashback, None);
    assert_eq!(e.last_field_present, "coin_creator_fee");
    // 1996 - 6 - 100 = 1890 (the synthetic user_quote_amount_out field is
    // arbitrary; the formula is what is under test).
    assert_eq!(e.quote_proceeds(), Some(1890));
    let mut under = (*e).clone();
    under.protocol_fee = u64::MAX;
    assert_eq!(under.quote_proceeds(), None);
    let mut over = PumpAmmEvent::Sell(e.clone());
    assert_eq!(over.quote_consideration(), Some(1890));
    if let PumpAmmEvent::Sell(x) = &mut over {
        x.coin_creator_fee = u64::MAX;
    }
    assert_eq!(over.quote_consideration(), None);
}

#[test]
fn event_length_policy_required_optional_and_bounds() {
    let s = buy_spec();
    let d = decoder();
    let run = |payload: Vec<u8>| d.classify_event(&event_ix(s, payload, amm(), 0));

    // Too short for the required prefix.
    let mut short = event_prefix(s);
    short.pop();
    assert!(matches!(run(short), PumpAmmEventOutcome::Malformed { .. }));

    // Exactly the required prefix: optional tail all None.
    let PumpAmmEventOutcome::Trade(PumpAmmEvent::Buy(e)) = run(event_prefix(s)) else {
        panic!()
    };
    assert_eq!(e.track_volume, None);
    assert_eq!(e.ix_name, None);
    assert_eq!(e.holder_rewards, None);

    // Ends exactly at a field boundary (after total_unclaimed_tokens).
    let mut p = event_prefix(s);
    p.push(0);
    p.extend(le(5));
    let PumpAmmEventOutcome::Trade(PumpAmmEvent::Buy(e)) = run(p) else {
        panic!()
    };
    assert_eq!(e.total_unclaimed_tokens, Some(5));
    assert_eq!(e.total_claimed_tokens, None);
    assert_eq!(e.last_field_present, "total_unclaimed_tokens");

    // Ends inside a field => Malformed.
    let mut p = event_prefix(s);
    p.push(0);
    p.extend([1, 2, 3]);
    assert!(matches!(run(p), PumpAmmEventOutcome::Malformed { .. }));

    // Invalid Borsh bool.
    let mut p = event_prefix(s);
    p.push(2);
    assert!(matches!(run(p), PumpAmmEventOutcome::Malformed { .. }));

    // Control character in ix_name.
    let mut p = event_prefix(s);
    let mut tail = buy_tail();
    let pos = tail.windows(3).position(|w| w == b"buy").unwrap();
    tail[pos] = 0x07;
    p.extend(tail);
    assert!(matches!(run(p), PumpAmmEventOutcome::Malformed { .. }));

    // Over-long ix_name length.
    let mut p = event_prefix(s);
    let mut tail = buy_tail();
    let pos = tail.windows(3).position(|w| w == b"buy").unwrap() - 4;
    tail[pos..pos + 4].copy_from_slice(&1000u32.to_le_bytes());
    p.extend(tail);
    assert!(matches!(run(p), PumpAmmEventOutcome::Malformed { .. }));

    // Trailing bytes bound.
    let mut p = event_prefix(s);
    p.extend(buy_tail());
    p.extend(vec![0u8; MAX_AMM_TRAILING_EVENT_BYTES]);
    assert!(matches!(run(p.clone()), PumpAmmEventOutcome::Trade(_)));
    p.push(0);
    assert!(matches!(run(p), PumpAmmEventOutcome::Malformed { .. }));
}

#[test]
fn event_cpi_classification_other_unknown_and_header() {
    let d = decoder();
    let mk = |disc: [u8; 8]| {
        let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
        data.extend(disc);
        RawSolanaInstruction {
            program_id: amm(),
            accounts: vec![],
            data,
            instruction_index: 0,
        }
    };
    let (name, disc) = AMM_OTHER_EVENTS[0];
    assert_eq!(
        d.classify_event(&mk(disc)),
        PumpAmmEventOutcome::OtherEvent {
            discriminator: disc,
            name
        }
    );
    assert_eq!(
        d.classify_event(&mk([1; 8])),
        PumpAmmEventOutcome::UnknownEvent {
            discriminator: [1; 8]
        }
    );
    let mut cut = mk([1; 8]);
    cut.data.truncate(12);
    assert!(matches!(
        classify_pump_amm_event(&cut),
        PumpAmmEventOutcome::Malformed { .. }
    ));
    let plain = trade_ix(amm(), AMM_SELL_DISCRIMINATOR, 24, 21);
    assert_eq!(d.classify_event(&plain), PumpAmmEventOutcome::NotEventCpi);
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

fn buy_trade(idx: u32) -> RawSolanaInstruction {
    let mut t = trade_ix(amm(), AMM_BUY_DISCRIMINATOR, 25, 23);
    t.instruction_index = idx;
    t
}

fn full_event(s: EvSpec, idx: u32) -> RawSolanaInstruction {
    let mut p = event_prefix(s);
    if s.buy {
        p.extend(buy_tail());
    }
    event_ix(s, p, amm(), idx)
}

#[test]
fn pairing_is_one_to_one_and_reports_mismatch_missing_and_orphans() {
    let d = decoder();
    let junk = RawSolanaInstruction {
        program_id: pk(55),
        accounts: vec![],
        data: vec![1, 2, 3],
        instruction_index: 0,
    };
    // trade, event, trade (no event), orphan event after another program's ix.
    let list = vec![
        buy_trade(0),
        junk.clone(),
        full_event(buy_spec(), 2),
        buy_trade(3),
        trade_ix(amm(), [9; 8], 24, 3),
    ];
    let r = pair_amm_trades_with_events(&d, &list, 1, 2);
    assert_eq!(r.trades.len(), 2);
    assert_eq!(r.paired(), 1);
    assert_eq!(r.missing(), 1);
    assert_eq!(r.mismatched(), 0);
    assert_eq!(r.unknown_instructions, 1);
    assert!(r.orphan_events.is_empty());

    // Event of another user -> Mismatch, never re-paired.
    let mut wrong = buy_spec();
    wrong.user = 120;
    let r = pair_amm_trades_with_events(&d, &[buy_trade(0), full_event(wrong, 1)], 0, 0);
    assert_eq!(r.mismatched(), 1);
    let AmmTradeEventPairing::Mismatch { mismatch, .. } = &r.trades[0].pairing else {
        panic!()
    };
    assert!(mismatch.user && !mismatch.pool && !mismatch.side);

    // Wrong side, pool and token accounts are each detected.
    let sell = EvSpec {
        buy: false,
        pool: 130,
        ub: 131,
        uq: 132,
        ..buy_spec()
    };
    let r = pair_amm_trades_with_events(&d, &[buy_trade(0), full_event(sell, 1)], 0, 0);
    let AmmTradeEventPairing::Mismatch { mismatch, .. } = &r.trades[0].pairing else {
        panic!()
    };
    assert!(mismatch.side && mismatch.pool);
    assert!(mismatch.user_base_token_account && mismatch.user_quote_token_account);

    // Event before any trade, and a second event for the same trade, are orphans.
    let r = pair_amm_trades_with_events(
        &d,
        &[
            full_event(buy_spec(), 0),
            buy_trade(1),
            full_event(buy_spec(), 2),
            full_event(buy_spec(), 3),
        ],
        0,
        0,
    );
    assert_eq!(r.paired(), 1);
    assert_eq!(r.orphan_events.len(), 2);

    // Events of another program are ignored entirely.
    let foreign = event_ix(buy_spec(), event_prefix(buy_spec()), pk(55), 1);
    let r = pair_amm_trades_with_events(&d, &[buy_trade(0), foreign], 0, 0);
    assert_eq!(r.missing(), 1);
    assert!(r.orphan_events.is_empty());

    // Malformed trade is counted and closes the open slot.
    let bad = trade_ix(amm(), AMM_SELL_DISCRIMINATOR, 30, 21);
    let r = pair_amm_trades_with_events(&d, &[buy_trade(0), bad, full_event(buy_spec(), 2)], 0, 0);
    assert_eq!(r.malformed_trades, 1);
    assert_eq!(r.orphan_events.len(), 1);
}
