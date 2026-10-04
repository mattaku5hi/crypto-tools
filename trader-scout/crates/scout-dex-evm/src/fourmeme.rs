//! four.meme TokenManager trade events (BSC launchpad, ADR-020 amendment 5).
//!
//! Official ABIs (repo `four-meme-community/fourmeme-docs` @
//! `5f7f589b042e4e3b41c2c214d0fe3a81b36725a9`, pinned as
//! `docs/p0/measurements/fixtures/fourmeme_{TokenManager,TokenManager2}_5f7f589b.lite.json`;
//! the IDL-equality tests of this module read those files):
//!
//! - **V1** `TokenManager` `0xEC45...bFbC`: `TokenPurchase(address token,
//!   address account, uint256 tokenAmount, uint256 etherAmount, uint256 fee)`
//!   and `TokenSale(...same...)`. No indexed field: one topic (topic0), 160
//!   data bytes. `etherAmount` is BNB.
//! - **V2** `TokenManager2` `0x5c95...762b`: `TokenPurchase(address token,
//!   address account, uint256 price, uint256 amount, uint256 cost, uint256
//!   fee, uint256 offers, uint256 funds)` and `TokenSale(...same...)`. No
//!   indexed field: one topic, 256 data bytes. `cost` is in the curve's QUOTE
//!   asset, which is BNB for some tokens and a BEP20 for others and is NOT in
//!   the event.
//!
//! Semantics used by ADR-020 (never the event amounts): the event is evidence
//! of a bonding-curve swap on `token` for `account`. A wallet's trade amounts
//! still come from its own net deltas; the extraction books the trade only if
//! `account` is the wallet (the signer) and `token` is the traded token. An
//! `account` that is not `tx.from` (a router/bot contract) is not attributed.
//!
//! The decoders take the manager version from the topic, the gate pins which
//! manager address may emit which version: a V2-shaped event at the V1
//! manager is an ungated emitter, never a trade.

use alloy_primitives::{Address, B256, U256, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

use crate::uniswap::{bad, shape_len, word_address, word_u256};

/// V1 `TokenPurchase(address,address,uint256,uint256,uint256)`.
pub const FOURMEME_V1_PURCHASE_TOPIC0: B256 =
    b256!("00fe0e12b43090c1fc19a34aefa5cc138a4eeafc60ab800f855c730b3fb9480e");
/// V1 `TokenSale(address,address,uint256,uint256,uint256)`.
pub const FOURMEME_V1_SALE_TOPIC0: B256 =
    b256!("80d4e495cda89b31af98c8e977ff11f417bafcee26902a17a15be51830c47533");
/// V2 `TokenPurchase(address,address,uint256,uint256,uint256,uint256,uint256,uint256)`.
pub const FOURMEME_V2_PURCHASE_TOPIC0: B256 =
    b256!("7db52723a3b2cdd6164364b3b766e65e540d7be48ffa89582956d8eaebe62942");
/// V2 `TokenSale(address,address,uint256,uint256,uint256,uint256,uint256,uint256)`.
pub const FOURMEME_V2_SALE_TOPIC0: B256 =
    b256!("0a5575b3648bae2210cee56bf33254cc1ddfbc7bf637c0af2ac18b14fb1bae19");

/// `TokenCreate(address creator, address token, uint256 requestId, string name,
/// string symbol, uint256 totalSupply, uint256 launchTime, uint256 launchFee)`,
/// identical in both ABIs. A launch transaction moves the whole supply to the
/// manager (`totalSupply`) before the first trade of the token.
pub const FOURMEME_TOKEN_CREATE_TOPIC0: B256 =
    b256!("396d5e902b675b032348d3d2e9517ee8f0c4a926603fbc075d3d282ff00cad20");

/// The fields of a `TokenCreate` the evidence needs (head words only; the
/// two strings are dynamic and not read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FourMemeTokenCreate {
    pub manager: Address,
    pub creator: Address,
    pub token: Address,
    pub total_supply: U256,
    pub launch_fee: U256,
}

/// Decode a `TokenCreate` (one topic; the head is 8 words, then the strings).
#[must_use]
pub fn decode_fourmeme_token_create(log: &RawEvmLog) -> DecodeOutcome<FourMemeTokenCreate> {
    if log.topics.first() != Some(&FOURMEME_TOKEN_CREATE_TOPIC0) {
        return DecodeOutcome::NotMine;
    }
    let name = "four.meme TokenCreate";
    if log.topics.len() != 1 || log.data.len() < 8 * 32 {
        return bad(name);
    }
    let d = log.data.as_ref();
    let (Some(creator), Some(token), Some(total_supply), Some(launch_fee)) = (
        word_address(d, 0),
        word_address(d, 1),
        word_u256(d, 5),
        word_u256(d, 7),
    ) else {
        return bad(name);
    };
    DecodeOutcome::Decoded(FourMemeTokenCreate {
        manager: log.address,
        creator,
        token,
        total_supply,
        launch_fee,
    })
}

/// Trade direction from the ACCOUNT's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaunchpadSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FourMemeVersion {
    V1,
    V2,
}

/// A decoded four.meme `TokenPurchase`/`TokenSale`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FourMemeTrade {
    /// Emitting TokenManager.
    pub manager: Address,
    pub version: FourMemeVersion,
    pub side: LaunchpadSide,
    pub token: Address,
    /// Trader named by the event (not necessarily `tx.from`).
    pub account: Address,
    /// V1 `tokenAmount` / V2 `amount`.
    pub token_amount: U256,
    /// V1 `etherAmount` (BNB) / V2 `cost` (quote asset units).
    pub quote_amount: U256,
    pub fee: U256,
    /// V2 only.
    pub price: Option<U256>,
    /// V2 only.
    pub offers: Option<U256>,
    /// V2 only.
    pub funds: Option<U256>,
    pub log_index: u64,
}

/// Version and side of a four.meme trade topic0; `None` for other topics.
#[must_use]
pub fn fourmeme_topic_kind(topic0: &B256) -> Option<(FourMemeVersion, LaunchpadSide)> {
    if *topic0 == FOURMEME_V1_PURCHASE_TOPIC0 {
        Some((FourMemeVersion::V1, LaunchpadSide::Buy))
    } else if *topic0 == FOURMEME_V1_SALE_TOPIC0 {
        Some((FourMemeVersion::V1, LaunchpadSide::Sell))
    } else if *topic0 == FOURMEME_V2_PURCHASE_TOPIC0 {
        Some((FourMemeVersion::V2, LaunchpadSide::Buy))
    } else if *topic0 == FOURMEME_V2_SALE_TOPIC0 {
        Some((FourMemeVersion::V2, LaunchpadSide::Sell))
    } else {
        None
    }
}

/// Decode a V1 or V2 `TokenPurchase`/`TokenSale` (by topic0).
#[must_use]
pub fn decode_fourmeme_trade(log: &RawEvmLog) -> DecodeOutcome<FourMemeTrade> {
    let Some((version, side)) = log.topics.first().and_then(fourmeme_topic_kind) else {
        return DecodeOutcome::NotMine;
    };
    let (name, words) = match version {
        FourMemeVersion::V1 => ("four.meme v1 trade", 5),
        FourMemeVersion::V2 => ("four.meme v2 trade", 8),
    };
    if let Err(out) = shape_len(log, 1, words * 32, name) {
        return out;
    }
    let d = log.data.as_ref();
    let (Some(token), Some(account)) = (word_address(d, 0), word_address(d, 1)) else {
        return bad(name);
    };
    let at = |i: usize| word_u256(d, i);
    let out = match version {
        FourMemeVersion::V1 => {
            let (Some(token_amount), Some(quote_amount), Some(fee)) = (at(2), at(3), at(4)) else {
                return bad(name);
            };
            FourMemeTrade {
                manager: log.address,
                version,
                side,
                token,
                account,
                token_amount,
                quote_amount,
                fee,
                price: None,
                offers: None,
                funds: None,
                log_index: log.log_index,
            }
        }
        FourMemeVersion::V2 => {
            let (Some(price), Some(token_amount), Some(quote_amount), Some(fee)) =
                (at(2), at(3), at(4), at(5))
            else {
                return bad(name);
            };
            let (Some(offers), Some(funds)) = (at(6), at(7)) else {
                return bad(name);
            };
            FourMemeTrade {
                manager: log.address,
                version,
                side,
                token,
                account,
                token_amount,
                quote_amount,
                fee,
                price: Some(price),
                offers: Some(offers),
                funds: Some(funds),
                log_index: log.log_index,
            }
        }
    };
    DecodeOutcome::Decoded(out)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, keccak256};
    use sha2::{Digest, Sha256};

    use super::*;

    fn fixture(name: &str) -> (Vec<u8>, serde_json::Value) {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures")
            .join(name);
        let bytes = std::fs::read(&p).unwrap();
        let v = serde_json::from_slice(&bytes).unwrap();
        (bytes, v)
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// `(name, canonical signature, [(type, indexed)])` of the event `name`.
    fn event_of(abi: &serde_json::Value, name: &str) -> (String, Vec<(String, bool)>) {
        let e = abi
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "event" && e["name"] == name)
            .unwrap_or_else(|| panic!("event {name} not in the ABI"));
        let inputs: Vec<(String, bool)> = e["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["type"].as_str().unwrap().to_string(),
                    i["indexed"].as_bool().unwrap(),
                )
            })
            .collect();
        let sig = format!(
            "{name}({})",
            inputs
                .iter()
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        (sig, inputs)
    }

    #[test]
    fn pinned_abi_files_are_the_committed_bytes() {
        // sha256 of the pinned official ABI extracts (repo @ 5f7f589b).
        for (file, sha) in [
            (
                "fourmeme_TokenManager_5f7f589b.lite.json",
                "85a7aadbeed419bcc0db57cdf2ba3271f966763f3b71b3f56f47eb5a997e1747",
            ),
            (
                "fourmeme_TokenManager2_5f7f589b.lite.json",
                "1a71bdabd53b301b776bc0ff652e2d45428c712c25fbd0b6e9c115d2348cb6ef",
            ),
            (
                "fourmeme_TokenManagerHelper3_5f7f589b.lite.json",
                "b6cca53e99f7ffcd0b003b35675bb756857e29c07617f460bf52af306ced89c7",
            ),
        ] {
            let (bytes, _) = fixture(file);
            assert_eq!(hex(&Sha256::digest(&bytes)), sha, "{file}");
        }
    }

    #[test]
    fn topics_and_layouts_equal_the_pinned_official_abis() {
        let (_, v1) = fixture("fourmeme_TokenManager_5f7f589b.lite.json");
        let (_, v2) = fixture("fourmeme_TokenManager2_5f7f589b.lite.json");
        for (abi, name, topic, fields, version, side) in [
            (
                &v1,
                "TokenPurchase",
                FOURMEME_V1_PURCHASE_TOPIC0,
                5usize,
                FourMemeVersion::V1,
                LaunchpadSide::Buy,
            ),
            (
                &v1,
                "TokenSale",
                FOURMEME_V1_SALE_TOPIC0,
                5,
                FourMemeVersion::V1,
                LaunchpadSide::Sell,
            ),
            (
                &v2,
                "TokenPurchase",
                FOURMEME_V2_PURCHASE_TOPIC0,
                8,
                FourMemeVersion::V2,
                LaunchpadSide::Buy,
            ),
            (
                &v2,
                "TokenSale",
                FOURMEME_V2_SALE_TOPIC0,
                8,
                FourMemeVersion::V2,
                LaunchpadSide::Sell,
            ),
        ] {
            let (sig, inputs) = event_of(abi, name);
            assert_eq!(keccak256(sig.as_bytes()), topic, "{sig}");
            // No indexed field: one topic, `fields` data words.
            assert!(inputs.iter().all(|(_, indexed)| !indexed), "{sig}");
            assert_eq!(inputs.len(), fields, "{sig}");
            assert_eq!(fourmeme_topic_kind(&topic), Some((version, side)));
        }
        // Field order the decoder reads (names from the ABI).
        let names = |abi: &serde_json::Value, ev: &str| -> Vec<String> {
            abi.as_array()
                .unwrap()
                .iter()
                .find(|e| e["type"] == "event" && e["name"] == ev)
                .unwrap()["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            names(&v1, "TokenPurchase"),
            ["token", "account", "tokenAmount", "etherAmount", "fee"]
        );
        assert_eq!(
            names(&v2, "TokenSale"),
            [
                "token", "account", "price", "amount", "cost", "fee", "offers", "funds"
            ]
        );
        // TokenCreate: same signature in both ABIs, head layout read by the decoder.
        for abi in [&v1, &v2] {
            let (sig, inputs) = event_of(abi, "TokenCreate");
            assert_eq!(keccak256(sig.as_bytes()), FOURMEME_TOKEN_CREATE_TOPIC0);
            let names: Vec<&str> = abi
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["type"] == "event" && e["name"] == "TokenCreate")
                .unwrap()["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["name"].as_str().unwrap())
                .collect();
            assert_eq!(inputs.len(), 8);
            assert_eq!(names[1], "token");
            assert_eq!(names[5], "totalSupply");
            assert_eq!(names[7], "launchFee");
        }
        // LiquidityAdded(base, offers, quote, funds) is a known non-trade event.
        let (sig, _) = event_of(&v2, "LiquidityAdded");
        assert_eq!(
            keccak256(sig.as_bytes()),
            b256!("c18aa71171b358b706fe3dd345299685ba21a5316c66ffa9e319268b033c44b0")
        );
    }

    fn word(v: U256) -> [u8; 32] {
        v.to_be_bytes::<32>()
    }
    fn addr_word(a: Address) -> [u8; 32] {
        a.into_word().0
    }
    fn mk(topic: B256, words: &[[u8; 32]]) -> RawEvmLog {
        RawEvmLog {
            address: Address::repeat_byte(0xee),
            topics: vec![topic],
            data: Bytes::from(words.concat()),
            block_number: 5,
            transaction_index: 2,
            log_index: 9,
        }
    }

    #[test]
    fn decodes_v1_and_v2_exact_layouts() {
        let token = Address::repeat_byte(1);
        let acct = Address::repeat_byte(2);
        let n = |x: u64| word(U256::from(x));
        let v1 = mk(
            FOURMEME_V1_SALE_TOPIC0,
            &[addr_word(token), addr_word(acct), n(10), n(20), n(3)],
        );
        let DecodeOutcome::Decoded(t) = decode_fourmeme_trade(&v1) else {
            panic!("v1")
        };
        assert_eq!(
            (t.version, t.side),
            (FourMemeVersion::V1, LaunchpadSide::Sell)
        );
        assert_eq!((t.token, t.account), (token, acct));
        assert_eq!(
            (t.token_amount, t.quote_amount, t.fee),
            (U256::from(10u8), U256::from(20u8), U256::from(3u8))
        );
        assert_eq!((t.price, t.offers, t.funds), (None, None, None));
        assert_eq!(t.log_index, 9);

        let v2 = mk(
            FOURMEME_V2_PURCHASE_TOPIC0,
            &[
                addr_word(token),
                addr_word(acct),
                n(7),
                n(10),
                n(20),
                n(3),
                n(5),
                n(6),
            ],
        );
        let DecodeOutcome::Decoded(t) = decode_fourmeme_trade(&v2) else {
            panic!("v2")
        };
        assert_eq!(
            (t.version, t.side),
            (FourMemeVersion::V2, LaunchpadSide::Buy)
        );
        assert_eq!(
            (t.token_amount, t.quote_amount, t.fee),
            (U256::from(10u8), U256::from(20u8), U256::from(3u8))
        );
        assert_eq!(
            (t.price, t.offers, t.funds),
            (
                Some(U256::from(7u8)),
                Some(U256::from(5u8)),
                Some(U256::from(6u8))
            )
        );
    }

    #[test]
    fn wrong_shapes_are_malformed_and_other_topics_not_mine() {
        let n = |x: u64| word(U256::from(x));
        let a = addr_word(Address::repeat_byte(1));
        // V2 topic with V1's 5 words, V1 topic with V2's 8 words.
        assert!(matches!(
            decode_fourmeme_trade(&mk(FOURMEME_V2_SALE_TOPIC0, &[a, a, n(1), n(1), n(1)])),
            DecodeOutcome::Malformed(_)
        ));
        assert!(matches!(
            decode_fourmeme_trade(&mk(
                FOURMEME_V1_PURCHASE_TOPIC0,
                &[a, a, n(1), n(1), n(1), n(1), n(1), n(1)]
            )),
            DecodeOutcome::Malformed(_)
        ));
        // Dirty upper bytes of an address word.
        let mut dirty = a;
        dirty[0] = 1;
        assert!(matches!(
            decode_fourmeme_trade(&mk(
                FOURMEME_V1_PURCHASE_TOPIC0,
                &[dirty, a, n(1), n(1), n(1)]
            )),
            DecodeOutcome::Malformed(_)
        ));
        // Extra topic (an indexed variant would be another event).
        let mut l = mk(FOURMEME_V1_PURCHASE_TOPIC0, &[a, a, n(1), n(1), n(1)]);
        l.topics.push(B256::ZERO);
        assert!(matches!(
            decode_fourmeme_trade(&l),
            DecodeOutcome::Malformed(_)
        ));
        // Other topics: not this decoder's.
        assert!(matches!(
            decode_fourmeme_trade(&mk(B256::repeat_byte(7), &[a])),
            DecodeOutcome::NotMine
        ));
    }
}
