//! Flap (flap.sh) Portal events (BSC launchpad, ADR-020 amendment 12).
//!
//! Source: Flap's published contract list (docs.flap.sh, "Deployed Contract
//! Addresses": Portal `0xe2cE6ab80874Fa9Fa2aAE65D277Dd6B8e65C9De0` on BNB
//! Chain) and the event names it documents (`TokenCreated`, `TokenBought`,
//! `TokenSold`, `LaunchedToDEX`). The field layouts were confirmed live
//! (2026-10-07) and every topic0 below is the keccak of the signature:
//!
//! - `TokenCreated(uint256 ts, address creator, uint256 nonce, address token,
//!   string name, string symbol, string meta)` — no indexed field.
//! - `TokenBought(uint256 ts, address token, address buyer, uint256 amount,
//!   uint256 quoteAmount, uint256 fee, uint256 postPrice)` and `TokenSold`
//!   (same layout, `seller`) — no indexed field, 7 data words. `buyer` /
//!   `seller` is the address the Portal transfers to / from: often a router
//!   (`0x1de460f3…`), not the wallet. `quoteAmount` is in the token's quote
//!   asset (BNB or a BEP20 such as `NVDAB`), not named in the event.
//! - `LaunchedToDEX(address token, address pool, uint256 amount, uint256 eth)`
//!   — graduation of the curve to a PancakeSwap pool.
//!
//! Semantics (ADR-020 amendment 8): a trade event is evidence of a curve swap
//! on `token`; trader identity and amounts come from the transaction (signer
//! and its own net deltas), the event account is informational.

use alloy_primitives::{Address, B256, U256, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

use crate::fourmeme::LaunchpadSide;
use crate::uniswap::{bad, shape_len, word_address, word_u256};

/// Flap Portal on BNB Chain (docs.flap.sh deployed contracts).
pub const FLAP_PORTAL_BSC: Address =
    alloy_primitives::address!("e2cE6ab80874Fa9Fa2aAE65D277Dd6B8e65C9De0");
/// `TokenBought(uint256,address,address,uint256,uint256,uint256,uint256)`.
pub const FLAP_TOKEN_BOUGHT_TOPIC0: B256 =
    b256!("a800a2038683844fac66747f771bfdfae862eb28b16bcfa387afa9fbacce8ff7");
/// `TokenSold(uint256,address,address,uint256,uint256,uint256,uint256)`.
pub const FLAP_TOKEN_SOLD_TOPIC0: B256 =
    b256!("03a4693e592f5e75dc7c136acb39b146d2b4966c0e509c34f362dee02b3b861a");
/// `TokenCreated(uint256,address,uint256,address,string,string,string)`.
pub const FLAP_TOKEN_CREATED_TOPIC0: B256 =
    b256!("504e7f360b2e5fe33cbaaae4c593bc55305328341bf79009e43e0e3b7f699603");
/// `LaunchedToDEX(address,address,uint256,uint256)`.
pub const FLAP_LAUNCHED_TO_DEX_TOPIC0: B256 =
    b256!("6e4f47630b8745b8cacbd44f42a8a33e7eea7cc08ef22fc7630f4f385784ff7d");

/// A decoded Flap `TokenBought` / `TokenSold`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlapTrade {
    pub portal: Address,
    pub side: LaunchpadSide,
    pub token: Address,
    /// `buyer` / `seller` of the event (the Portal's counterparty; often a
    /// router, not the wallet).
    pub account: Address,
    pub token_amount: U256,
    /// In the token's quote asset (not named by the event).
    pub quote_amount: U256,
    pub fee: U256,
    pub post_price: U256,
    pub timestamp: U256,
    pub log_index: u64,
}

/// Side of a Flap trade topic0; `None` for other topics.
#[must_use]
pub fn flap_topic_side(topic0: &B256) -> Option<LaunchpadSide> {
    if *topic0 == FLAP_TOKEN_BOUGHT_TOPIC0 {
        Some(LaunchpadSide::Buy)
    } else if *topic0 == FLAP_TOKEN_SOLD_TOPIC0 {
        Some(LaunchpadSide::Sell)
    } else {
        None
    }
}

/// Decode a Flap `TokenBought` / `TokenSold` (by topic0).
#[must_use]
pub fn decode_flap_trade(log: &RawEvmLog) -> DecodeOutcome<FlapTrade> {
    const NAME: &str = "flap portal trade";
    let Some(side) = log.topics.first().and_then(flap_topic_side) else {
        return DecodeOutcome::NotMine;
    };
    if let Err(out) = shape_len(log, 1, 7 * 32, NAME) {
        return out;
    }
    let d = log.data.as_ref();
    let at = |i: usize| word_u256(d, i);
    let (Some(timestamp), Some(token), Some(account)) =
        (at(0), word_address(d, 1), word_address(d, 2))
    else {
        return bad(NAME);
    };
    let (Some(token_amount), Some(quote_amount), Some(fee), Some(post_price)) =
        (at(3), at(4), at(5), at(6))
    else {
        return bad(NAME);
    };
    DecodeOutcome::Decoded(FlapTrade {
        portal: log.address,
        side,
        token,
        account,
        token_amount,
        quote_amount,
        fee,
        post_price,
        timestamp,
        log_index: log.log_index,
    })
}

/// A decoded Flap `TokenCreated` (head words only; the strings are not read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlapTokenCreated {
    pub creator: Address,
    pub token: Address,
    pub timestamp: U256,
    pub nonce: U256,
}

/// Decode a Flap `TokenCreated` (3 dynamic strings follow the 4 head words).
#[must_use]
pub fn decode_flap_token_created(log: &RawEvmLog) -> DecodeOutcome<FlapTokenCreated> {
    const NAME: &str = "flap token created";
    if log.topics.first() != Some(&FLAP_TOKEN_CREATED_TOPIC0) {
        return DecodeOutcome::NotMine;
    }
    let d = log.data.as_ref();
    if log.topics.len() != 1 || d.len() < 7 * 32 {
        return bad(NAME);
    }
    let (Some(timestamp), Some(creator), Some(nonce), Some(token)) = (
        word_u256(d, 0),
        word_address(d, 1),
        word_u256(d, 2),
        word_address(d, 3),
    ) else {
        return bad(NAME);
    };
    DecodeOutcome::Decoded(FlapTokenCreated {
        creator,
        token,
        timestamp,
        nonce,
    })
}

/// A decoded Flap `LaunchedToDEX` (curve graduation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlapLaunchedToDex {
    pub token: Address,
    pub pool: Address,
    pub token_amount: U256,
    pub quote_amount: U256,
}

/// Decode a Flap `LaunchedToDEX`.
#[must_use]
pub fn decode_flap_launched_to_dex(log: &RawEvmLog) -> DecodeOutcome<FlapLaunchedToDex> {
    const NAME: &str = "flap launched to dex";
    if log.topics.first() != Some(&FLAP_LAUNCHED_TO_DEX_TOPIC0) {
        return DecodeOutcome::NotMine;
    }
    if let Err(out) = shape_len(log, 1, 4 * 32, NAME) {
        return out;
    }
    let d = log.data.as_ref();
    let (Some(token), Some(pool), Some(token_amount), Some(quote_amount)) = (
        word_address(d, 0),
        word_address(d, 1),
        word_u256(d, 2),
        word_u256(d, 3),
    ) else {
        return bad(NAME);
    };
    DecodeOutcome::Decoded(FlapLaunchedToDex {
        token,
        pool,
        token_amount,
        quote_amount,
    })
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, address};

    use super::*;

    fn log(topic0: B256, words: &[[u8; 32]]) -> RawEvmLog {
        RawEvmLog {
            address: FLAP_PORTAL_BSC,
            topics: vec![topic0],
            data: Bytes::from(words.concat()),
            block_number: 1,
            transaction_index: 0,
            log_index: 3,
        }
    }

    fn w_addr(a: Address) -> [u8; 32] {
        a.into_word().0
    }

    fn w_u(v: u64) -> [u8; 32] {
        U256::from(v).to_be_bytes()
    }

    #[test]
    fn launched_to_dex_and_token_created_decode_their_head_words() {
        let token = address!("aac3a43e4514e94ccccf409dddf5fa7117a97777");
        let pool = address!("7ad9fb8343902452e79bc74ae9544a7d9797edd7");
        let l = log(
            FLAP_LAUNCHED_TO_DEX_TOPIC0,
            &[w_addr(token), w_addr(pool), w_u(7), w_u(9)],
        );
        let d = decode_flap_launched_to_dex(&l).decoded().unwrap();
        assert_eq!(
            (d.token, d.pool, d.token_amount, d.quote_amount),
            (token, pool, U256::from(7), U256::from(9))
        );
        let creator = Address::repeat_byte(0xcc);
        // ts, creator, nonce, token, then 3 string offsets
        let l = log(
            FLAP_TOKEN_CREATED_TOPIC0,
            &[
                w_u(1_789_984_968),
                w_addr(creator),
                w_u(5),
                w_addr(token),
                w_u(224),
                w_u(256),
                w_u(288),
            ],
        );
        let c = decode_flap_token_created(&l).decoded().unwrap();
        assert_eq!(
            (c.creator, c.token, c.nonce),
            (creator, token, U256::from(5))
        );
        // a trade topic is not a TokenCreated
        assert!(matches!(
            decode_flap_token_created(&log(FLAP_TOKEN_BOUGHT_TOPIC0, &[w_u(0); 7])),
            DecodeOutcome::NotMine
        ));
    }

    #[test]
    fn trade_with_a_dirty_address_word_is_malformed() {
        let mut bad_token = [0xffu8; 32];
        bad_token[31] = 1;
        let l = log(
            FLAP_TOKEN_SOLD_TOPIC0,
            &[
                w_u(1),
                bad_token,
                w_addr(Address::ZERO),
                w_u(1),
                w_u(1),
                w_u(0),
                w_u(1),
            ],
        );
        assert!(matches!(decode_flap_trade(&l), DecodeOutcome::Malformed(_)));
    }
}
