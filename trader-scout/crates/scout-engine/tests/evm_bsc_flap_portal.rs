//! ADR-020 amendment 12 verification of the Flap (flap.sh) Portal on BSC
//! against the committed live fixture
//! `flap_portal_trades_bsc_2026-10-07.json` (receipts of every transaction
//! with a Portal `TokenBought`/`TokenSold` in BSC blocks 123,161,810..=123,161,903,
//! logs trimmed to the Portal's own logs and the Transfer logs of the tokens
//! those events name).
//!
//! Equation per event (TOKEN side, exact): a `TokenBought(token, …, amount)`
//! is matched by a `Transfer(Portal -> x, amount)` of `token` in the same
//! transaction; a `TokenSold(token, …, amount)` by a `Transfer(x -> Portal,
//! amount)`. The quote side (BNB or a BEP20 the event does not name) is not
//! read: the ledger books the wallet's own deltas (amendment 8). The gate must
//! classify every event as a `FixtureVerified` FlapPortal swap with launchpad
//! evidence naming the event's token.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use alloy_primitives::{Address, B256, Bytes, U256};
use scout_core::RawEvmLog;
use scout_dex_evm::{
    FLAP_PORTAL_BSC, GateOutcome, LaunchpadSide, SwapVenue, SwapVenueGate, VenueVerification,
    decode_flap_trade,
};
use serde_json::Value;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/p0/measurements/fixtures/flap_portal_trades_bsc_2026-10-07.json"
);

fn hex_u64(v: &Value) -> u64 {
    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn log_of(l: &Value, block: u64, index: u64) -> RawEvmLog {
    RawEvmLog {
        address: l["address"].as_str().unwrap().parse().unwrap(),
        topics: l["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().parse::<B256>().unwrap())
            .collect(),
        data: l["data"].as_str().unwrap().parse::<Bytes>().unwrap(),
        block_number: block,
        transaction_index: index,
        log_index: hex_u64(&l["logIndex"]),
    }
}

#[test]
fn every_portal_trade_matches_the_portals_own_token_transfer_and_is_verified() {
    let f: Value = serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let gate = SwapVenueGate::new(56);
    let transfer: B256 = scout_evm::TRANSFER_TOPIC0;
    let (mut buys, mut sells) = (0usize, 0usize);
    let mut tokens = std::collections::BTreeSet::new();
    for r in f["receipts"].as_array().unwrap() {
        let block = hex_u64(&r["blockNumber"]);
        let index = hex_u64(&r["transactionIndex"]);
        let logs: Vec<RawEvmLog> = r["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| log_of(l, block, index))
            .collect();
        for log in logs.iter().filter(|l| l.address == FLAP_PORTAL_BSC) {
            let Some(t) = decode_flap_trade(log).decoded() else {
                continue;
            };
            // gate: FixtureVerified FlapPortal with evidence naming the token
            match gate.classify(log) {
                GateOutcome::Verified(v) => {
                    assert_eq!(v.venue, SwapVenue::FlapPortal);
                    assert_eq!(v.verification, VenueVerification::FixtureVerified);
                    assert_eq!(v.launchpad.unwrap().token, t.token);
                }
                other => panic!("not verified: {other:?}"),
            }
            let portal_word = FLAP_PORTAL_BSC.into_word();
            let matched = logs.iter().any(|x| {
                x.address == t.token
                    && x.topics.len() == 3
                    && x.topics[0] == transfer
                    && U256::from_be_slice(x.data.as_ref()) == t.token_amount
                    && match t.side {
                        LaunchpadSide::Buy => x.topics[1] == portal_word,
                        LaunchpadSide::Sell => x.topics[2] == portal_word,
                    }
            });
            assert!(matched, "token side mismatch in {}", r["transactionHash"]);
            tokens.insert(t.token);
            match t.side {
                LaunchpadSide::Buy => buys += 1,
                LaunchpadSide::Sell => sells += 1,
            }
        }
    }
    println!(
        "flap portal samples: buys={buys} sells={sells} tokens={}",
        tokens.len()
    );
    assert_eq!((buys, sells, tokens.len()), (261, 209, 38));
}

#[test]
fn a_flap_shaped_event_elsewhere_is_an_ungated_emitter() {
    let f: Value = serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let r = &f["receipts"][0];
    let mut log = r["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| log_of(l, 1, 0))
        .find(|l| l.address == FLAP_PORTAL_BSC && decode_flap_trade(l).decoded().is_some())
        .unwrap();
    log.address = Address::repeat_byte(0x42);
    assert!(matches!(
        SwapVenueGate::new(56).classify(&log),
        GateOutcome::UngatedEmitter {
            venue: SwapVenue::FlapPortal,
            ..
        }
    ));
}
