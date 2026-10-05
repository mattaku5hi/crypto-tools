//! Robinhood Chain launchpad bonding curves: Pons V2 and Bags (ADR-020
//! amendment 8). Both deploy one curve contract per token; the CURVE emits
//! the trade events, so a curve is admitted like a pool
//! ([`crate::SwapVenueGate::admit_curve`]) before its events count.
//!
//! Pons V2 (official `ponsdotdev/pons-labs` @ `44a3db9193c365f6c25cf0d4c2efc396e6de0df5`, source pinned as
//! `docs/p0/measurements/fixtures/pons_PonsV2BondingCurve_44a3db91.sol.txt`; the
//! topic test parses the event lines of that text):
//! `CurveBuy(address indexed buyer, address indexed recipient, uint256 quoteIn,
//! uint256 tokensOut, uint256 fee, uint256 tax)` and `CurveSell(address indexed
//! seller, address indexed recipient, uint256 tokensIn, uint256 quoteOut,
//! uint256 fee, uint256 tax)`; `CurveBuyRefunded(address indexed buyer, uint256
//! refund)` and `CurveCompleted(address recipient, uint256 quoteOut, uint256
//! tokenOut)` are known non-trade events (verification accounting only).
//!
//! Bags (official docs.bags.fm + `bagsfm/bags-idl` @ `e55767f7…`, ABI pinned as
//! `bags_BagsBondingCurve_e55767f7.json`, sha256-asserted): `TokensBought(buyer
//! indexed, recipient indexed, grossQuoteIn, netQuoteIn, tokensOut, feeQuote,
//! vaultFeeQuote, creatorFeeWETH, refundQuote, price, virtualTokenReserves,
//! virtualQuoteReserves)` and `TokensSold(seller indexed, recipient indexed,
//! tokensIn, grossQuoteOut, netQuoteToRecipient, feeQuote, vaultFeeQuote,
//! creatorFeeWETH, price, virtualTokenReserves, virtualQuoteReserves)`.
//!
//! Semantics used by ADR-020 (never the event amounts): the event is evidence
//! of a curve swap on the curve's token for `account` (buyer/seller). A wallet's
//! trade amounts still come from its own net deltas; the extraction books the
//! trade only if `account` is the wallet (the signer) AND `recipient` is the
//! wallet too (a swap with another receiver is counted, never attributed).

use alloy_primitives::{Address, B256, U256, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

use crate::fourmeme::LaunchpadSide;
use crate::uniswap::{bad, shape, word_address, word_u256};

/// `CurveBuy(address,address,uint256,uint256,uint256,uint256)`.
pub const PONS_V2_CURVE_BUY_TOPIC0: B256 =
    b256!("ec36bf571f136799e8dc0b0b8bea4b04d8bd3d43de838aab0d5fc21d4cbfc455");
/// `CurveSell(address,address,uint256,uint256,uint256,uint256)`.
pub const PONS_V2_CURVE_SELL_TOPIC0: B256 =
    b256!("8113d738abdcb6b38357e9d53a54a7157861a09031b453651f0fe7fe151f59df");
/// `CurveBuyRefunded(address,uint256)` (known non-trade).
pub const PONS_V2_CURVE_BUY_REFUNDED_TOPIC0: B256 =
    b256!("a69e8258ccc7b9bbb70ab953fc2d1062b4ee28b8ca827534097e1732e87b0262");
/// `CurveCompleted(address,uint256,uint256)` (known non-trade: graduation).
pub const PONS_V2_CURVE_COMPLETED_TOPIC0: B256 =
    b256!("f8d37a90738ae063b8b8058b66f5880cf3cf7ab0c5d4fa78219696591dfbfb67");
/// Bags `TokensBought` (12 fields, 2 indexed).
pub const BAGS_TOKENS_BOUGHT_TOPIC0: B256 =
    b256!("6d9c6fad0db13f6f7fca7124777996deaeb1949d0750a4874c18611ff5d436b9");
/// Bags `TokensSold` (11 fields, 2 indexed).
pub const BAGS_TOKENS_SOLD_TOPIC0: B256 =
    b256!("813ea2e4b7710a4562c34494d61d6e80fd2ba5105790f740fdcbf64f8b05b80d");

/// Which launchpad's curve family emitted the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurveFamily {
    PonsV2,
    Bags,
}

/// A decoded curve trade (Pons `CurveBuy`/`CurveSell`, Bags `TokensBought`/`TokensSold`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveTrade {
    /// Emitting curve contract.
    pub curve: Address,
    pub family: CurveFamily,
    pub side: LaunchpadSide,
    /// `buyer` / `seller` named by the event (not necessarily `tx.from`).
    pub account: Address,
    /// Who receives the output (tokens on a buy, quote on a sell).
    pub recipient: Address,
    /// `tokensOut` (buy) / `tokensIn` (sell).
    pub token_amount: U256,
    /// Pons `quoteIn`/`quoteOut`; Bags `grossQuoteIn`/`netQuoteToRecipient`.
    pub quote_amount: U256,
    /// Pons `fee`; Bags `feeQuote`.
    pub fee: U256,
    /// Pons `tax` (creator tax); `None` for Bags.
    pub tax: Option<U256>,
    /// Bags buy `refundQuote`; `None` otherwise.
    pub refund: Option<U256>,
    pub log_index: u64,
}

/// Family and side of a curve-trade topic0; `None` for other topics.
#[must_use]
pub fn curve_topic_kind(topic0: &B256) -> Option<(CurveFamily, LaunchpadSide)> {
    if *topic0 == PONS_V2_CURVE_BUY_TOPIC0 {
        Some((CurveFamily::PonsV2, LaunchpadSide::Buy))
    } else if *topic0 == PONS_V2_CURVE_SELL_TOPIC0 {
        Some((CurveFamily::PonsV2, LaunchpadSide::Sell))
    } else if *topic0 == BAGS_TOKENS_BOUGHT_TOPIC0 {
        Some((CurveFamily::Bags, LaunchpadSide::Buy))
    } else if *topic0 == BAGS_TOKENS_SOLD_TOPIC0 {
        Some((CurveFamily::Bags, LaunchpadSide::Sell))
    } else {
        None
    }
}

/// Decode a Pons V2 `CurveBuy`/`CurveSell` or a Bags `TokensBought`/`TokensSold`.
#[must_use]
pub fn decode_curve_trade(log: &RawEvmLog) -> DecodeOutcome<CurveTrade> {
    let Some(topic0) = log.topics.first() else {
        return DecodeOutcome::NotMine;
    };
    let Some((family, side)) = curve_topic_kind(topic0) else {
        return DecodeOutcome::NotMine;
    };
    let (name, words) = match (family, side) {
        (CurveFamily::PonsV2, LaunchpadSide::Buy) => ("Pons V2 CurveBuy", 4),
        (CurveFamily::PonsV2, LaunchpadSide::Sell) => ("Pons V2 CurveSell", 4),
        (CurveFamily::Bags, LaunchpadSide::Buy) => ("Bags TokensBought", 10),
        (CurveFamily::Bags, LaunchpadSide::Sell) => ("Bags TokensSold", 9),
    };
    if let Err(out) = shape(log, *topic0, 3, words * 32, name) {
        return out;
    }
    let (Some(account_t), Some(recipient_t)) = (log.topics.get(1), log.topics.get(2)) else {
        return bad(name);
    };
    let (Some(account), Some(recipient)) = (
        word_address(&account_t.0, 0),
        word_address(&recipient_t.0, 0),
    ) else {
        return bad(name);
    };
    let d = log.data.as_ref();
    let at = |i: usize| word_u256(d, i);
    let fields = match (family, side) {
        // quoteIn, tokensOut, fee, tax
        (CurveFamily::PonsV2, LaunchpadSide::Buy) => {
            (at(1), at(0), at(2), at(3).map(Some), Some(None))
        }
        // tokensIn, quoteOut, fee, tax
        (CurveFamily::PonsV2, LaunchpadSide::Sell) => {
            (at(0), at(1), at(2), at(3).map(Some), Some(None))
        }
        // grossQuoteIn, netQuoteIn, tokensOut, feeQuote, vaultFeeQuote,
        // creatorFeeWETH, refundQuote, price, vTok, vQuote
        (CurveFamily::Bags, LaunchpadSide::Buy) => {
            (at(2), at(0), at(3), Some(None), at(6).map(Some))
        }
        // tokensIn, grossQuoteOut, netQuoteToRecipient, feeQuote, ...
        (CurveFamily::Bags, LaunchpadSide::Sell) => (at(0), at(2), at(3), Some(None), Some(None)),
    };
    let (Some(token_amount), Some(quote_amount), Some(fee), Some(tax), Some(refund)) = fields
    else {
        return bad(name);
    };
    DecodeOutcome::Decoded(CurveTrade {
        curve: log.address,
        family,
        side,
        account,
        recipient,
        token_amount,
        quote_amount,
        fee,
        tax,
        refund,
        log_index: log.log_index,
    })
}

/// Pons V2 `CurveBuyRefunded` (the quote the curve sent back to the buyer
/// when a buy was clamped at the sellable supply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PonsBuyRefunded {
    pub curve: Address,
    pub buyer: Address,
    pub refund: U256,
}

#[must_use]
pub fn decode_pons_buy_refunded(log: &RawEvmLog) -> DecodeOutcome<PonsBuyRefunded> {
    let name = "Pons V2 CurveBuyRefunded";
    if let Err(out) = shape(log, PONS_V2_CURVE_BUY_REFUNDED_TOPIC0, 2, 32, name) {
        return out;
    }
    let (Some(buyer), Some(refund)) = (
        log.topics.get(1).and_then(|t| word_address(&t.0, 0)),
        word_u256(log.data.as_ref(), 0),
    ) else {
        return bad(name);
    };
    DecodeOutcome::Decoded(PonsBuyRefunded {
        curve: log.address,
        buyer,
        refund,
    })
}

/// Pons V2 `CurveCompleted(recipient, quoteOut, tokenOut)`: the graduation
/// hand-over of the tracked reserves to the factory (no indexed field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PonsCurveCompleted {
    pub curve: Address,
    pub recipient: Address,
    pub quote_out: U256,
    pub token_out: U256,
}

#[must_use]
pub fn decode_pons_curve_completed(log: &RawEvmLog) -> DecodeOutcome<PonsCurveCompleted> {
    let name = "Pons V2 CurveCompleted";
    if let Err(out) = shape(log, PONS_V2_CURVE_COMPLETED_TOPIC0, 1, 96, name) {
        return out;
    }
    let d = log.data.as_ref();
    let (Some(recipient), Some(quote_out), Some(token_out)) =
        (word_address(d, 0), word_u256(d, 1), word_u256(d, 2))
    else {
        return bad(name);
    };
    DecodeOutcome::Decoded(PonsCurveCompleted {
        curve: log.address,
        recipient,
        quote_out,
        token_out,
    })
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, keccak256};
    use sha2::{Digest, Sha256};

    use super::*;

    fn fixture_bytes(name: &str) -> Vec<u8> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures")
            .join(name);
        std::fs::read(p).unwrap()
    }

    /// `(canonical signature, [indexed flags])` of `event <name>(...)` in
    /// Solidity source text (types only; names and `indexed` stripped).
    fn sol_event(src: &str, name: &str) -> (String, Vec<bool>) {
        let start = src
            .find(&format!("event {name}("))
            .unwrap_or_else(|| panic!("event {name} not in source"));
        let rest = &src[start + "event ".len() + name.len() + 1..];
        let inner = &rest[..rest.find(");").unwrap()];
        let mut types = Vec::new();
        let mut indexed = Vec::new();
        for part in inner.split(',') {
            let words: Vec<&str> = part.split_whitespace().collect();
            types.push(words[0].to_string());
            indexed.push(words.contains(&"indexed"));
        }
        (format!("{name}({})", types.join(",")), indexed)
    }

    #[test]
    fn pons_topics_are_keccak_of_the_pinned_solidity_event_lines() {
        let src =
            String::from_utf8(fixture_bytes("pons_PonsV2BondingCurve_44a3db91.sol.txt")).unwrap();
        for (name, topic, indexed) in [
            (
                "CurveBuy",
                PONS_V2_CURVE_BUY_TOPIC0,
                vec![true, true, false, false, false, false],
            ),
            (
                "CurveSell",
                PONS_V2_CURVE_SELL_TOPIC0,
                vec![true, true, false, false, false, false],
            ),
            (
                "CurveBuyRefunded",
                PONS_V2_CURVE_BUY_REFUNDED_TOPIC0,
                vec![true, false],
            ),
            (
                "CurveCompleted",
                PONS_V2_CURVE_COMPLETED_TOPIC0,
                vec![false, false, false],
            ),
        ] {
            let (sig, flags) = sol_event(&src, name);
            assert_eq!(keccak256(sig.as_bytes()), topic, "{sig}");
            assert_eq!(flags, indexed, "{sig}");
        }
        let (sig, _) = sol_event(&src, "CurveBuy");
        assert_eq!(
            sig,
            "CurveBuy(address,address,uint256,uint256,uint256,uint256)"
        );
        let (sig, _) = sol_event(&src, "CurveSell");
        assert_eq!(
            sig,
            "CurveSell(address,address,uint256,uint256,uint256,uint256)"
        );
    }

    #[test]
    fn bags_abi_is_the_pinned_bytes_and_topics_equal_it() {
        let bytes = fixture_bytes("bags_BagsBondingCurve_e55767f7.json");
        let sha: String = Sha256::digest(&bytes)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        assert_eq!(
            sha,
            "ebd7a955d55b542e87902345f2c999df9b7a1dcf891cd8a8c4f9ca3c606653a6"
        );
        let abi: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (name, topic, names) in [
            (
                "TokensBought",
                BAGS_TOKENS_BOUGHT_TOPIC0,
                vec![
                    "buyer",
                    "recipient",
                    "grossQuoteIn",
                    "netQuoteIn",
                    "tokensOut",
                    "feeQuote",
                    "vaultFeeQuote",
                    "creatorFeeWETH",
                    "refundQuote",
                    "price",
                    "virtualTokenReserves",
                    "virtualQuoteReserves",
                ],
            ),
            (
                "TokensSold",
                BAGS_TOKENS_SOLD_TOPIC0,
                vec![
                    "seller",
                    "recipient",
                    "tokensIn",
                    "grossQuoteOut",
                    "netQuoteToRecipient",
                    "feeQuote",
                    "vaultFeeQuote",
                    "creatorFeeWETH",
                    "price",
                    "virtualTokenReserves",
                    "virtualQuoteReserves",
                ],
            ),
        ] {
            let e = abi
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["type"] == "event" && e["name"] == name)
                .unwrap();
            let inputs = e["inputs"].as_array().unwrap();
            let sig = format!(
                "{name}({})",
                inputs
                    .iter()
                    .map(|i| i["type"].as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            assert_eq!(keccak256(sig.as_bytes()), topic, "{sig}");
            let got: Vec<&str> = inputs.iter().map(|i| i["name"].as_str().unwrap()).collect();
            assert_eq!(got, names, "{name} field order read by the decoder");
            let idx: Vec<bool> = inputs
                .iter()
                .map(|i| i["indexed"].as_bool().unwrap())
                .collect();
            assert!(idx[0] && idx[1] && idx[2..].iter().all(|b| !b), "{name}");
        }
    }

    fn word(v: u64) -> [u8; 32] {
        U256::from(v).to_be_bytes::<32>()
    }

    fn mk(emitter: Address, topic: B256, who: Address, to: Address, words: &[u64]) -> RawEvmLog {
        RawEvmLog {
            address: emitter,
            topics: vec![topic, who.into_word(), to.into_word()],
            data: Bytes::from(words.iter().flat_map(|w| word(*w)).collect::<Vec<u8>>()),
            block_number: 5,
            transaction_index: 2,
            log_index: 9,
        }
    }

    #[test]
    fn decodes_pons_and_bags_layouts() {
        let (curve, a, r) = (
            Address::repeat_byte(1),
            Address::repeat_byte(2),
            Address::repeat_byte(3),
        );
        let DecodeOutcome::Decoded(t) =
            decode_curve_trade(&mk(curve, PONS_V2_CURVE_BUY_TOPIC0, a, r, &[100, 7, 2, 1]))
        else {
            panic!()
        };
        assert_eq!(
            (t.family, t.side),
            (CurveFamily::PonsV2, LaunchpadSide::Buy)
        );
        assert_eq!((t.account, t.recipient, t.curve), (a, r, curve));
        assert_eq!(
            (t.quote_amount, t.token_amount, t.fee, t.tax, t.refund),
            (
                U256::from(100u8),
                U256::from(7u8),
                U256::from(2u8),
                Some(U256::from(1u8)),
                None
            )
        );
        let DecodeOutcome::Decoded(t) =
            decode_curve_trade(&mk(curve, PONS_V2_CURVE_SELL_TOPIC0, a, r, &[7, 90, 2, 1]))
        else {
            panic!()
        };
        assert_eq!(t.side, LaunchpadSide::Sell);
        assert_eq!(
            (t.token_amount, t.quote_amount),
            (U256::from(7u8), U256::from(90u8))
        );
        // Bags buy: gross 100, net 98, tokens 7, fee 2, vault 3, creator 4, refund 5, ...
        let DecodeOutcome::Decoded(t) = decode_curve_trade(&mk(
            curve,
            BAGS_TOKENS_BOUGHT_TOPIC0,
            a,
            a,
            &[100, 98, 7, 2, 3, 4, 5, 6, 7, 8],
        )) else {
            panic!()
        };
        assert_eq!((t.family, t.side), (CurveFamily::Bags, LaunchpadSide::Buy));
        assert_eq!(
            (t.token_amount, t.quote_amount, t.fee, t.tax, t.refund),
            (
                U256::from(7u8),
                U256::from(100u8),
                U256::from(2u8),
                None,
                Some(U256::from(5u8))
            )
        );
        // Bags sell: tokensIn 7, gross 100, net 97, fee 3, ...
        let DecodeOutcome::Decoded(t) = decode_curve_trade(&mk(
            curve,
            BAGS_TOKENS_SOLD_TOPIC0,
            a,
            r,
            &[7, 100, 97, 3, 1, 1, 5, 6, 7],
        )) else {
            panic!()
        };
        assert_eq!(t.side, LaunchpadSide::Sell);
        assert_eq!(
            (t.token_amount, t.quote_amount, t.fee),
            (U256::from(7u8), U256::from(97u8), U256::from(3u8))
        );
    }

    #[test]
    fn wrong_shapes_are_malformed_and_other_topics_not_mine() {
        let (c, a) = (Address::repeat_byte(1), Address::repeat_byte(2));
        // Pons topic with Bags' word count and vice versa.
        for (topic, words) in [
            (PONS_V2_CURVE_BUY_TOPIC0, 10usize),
            (PONS_V2_CURVE_SELL_TOPIC0, 3),
            (BAGS_TOKENS_BOUGHT_TOPIC0, 4),
            (BAGS_TOKENS_SOLD_TOPIC0, 10),
        ] {
            let l = mk(c, topic, a, a, &vec![1u64; words]);
            assert!(matches!(
                decode_curve_trade(&l),
                DecodeOutcome::Malformed(_)
            ));
        }
        // Dirty address topic.
        let mut l = mk(c, PONS_V2_CURVE_BUY_TOPIC0, a, a, &[1, 1, 1, 1]);
        l.topics[1] = B256::repeat_byte(0xff);
        assert!(matches!(
            decode_curve_trade(&l),
            DecodeOutcome::Malformed(_)
        ));
        // Missing indexed topic (a non-indexed variant is another event).
        let mut l = mk(c, PONS_V2_CURVE_BUY_TOPIC0, a, a, &[1, 1, 1, 1]);
        l.topics.pop();
        assert!(matches!(
            decode_curve_trade(&l),
            DecodeOutcome::Malformed(_)
        ));
        assert!(matches!(
            decode_curve_trade(&mk(c, B256::repeat_byte(9), a, a, &[1])),
            DecodeOutcome::NotMine
        ));
        // Known non-trade events are not trades.
        assert!(matches!(
            decode_curve_trade(&mk(c, PONS_V2_CURVE_COMPLETED_TOPIC0, a, a, &[1])),
            DecodeOutcome::NotMine
        ));
    }

    #[test]
    fn known_non_trade_events_decode() {
        let (c, a) = (Address::repeat_byte(1), Address::repeat_byte(2));
        let refunded = RawEvmLog {
            address: c,
            topics: vec![PONS_V2_CURVE_BUY_REFUNDED_TOPIC0, a.into_word()],
            data: Bytes::from(word(11).to_vec()),
            block_number: 1,
            transaction_index: 0,
            log_index: 0,
        };
        let DecodeOutcome::Decoded(r) = decode_pons_buy_refunded(&refunded) else {
            panic!()
        };
        assert_eq!((r.buyer, r.refund), (a, U256::from(11u8)));
        let completed = RawEvmLog {
            address: c,
            topics: vec![PONS_V2_CURVE_COMPLETED_TOPIC0],
            data: Bytes::from([a.into_word().0, word(5), word(6)].concat()),
            block_number: 1,
            transaction_index: 0,
            log_index: 0,
        };
        let DecodeOutcome::Decoded(r) = decode_pons_curve_completed(&completed) else {
            panic!()
        };
        assert_eq!(
            (r.recipient, r.quote_out, r.token_out),
            (a, U256::from(5u8), U256::from(6u8))
        );
        assert!(matches!(
            decode_pons_curve_completed(&refunded),
            DecodeOutcome::NotMine
        ));
    }
}
