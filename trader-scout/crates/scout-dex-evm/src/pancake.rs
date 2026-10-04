//! PancakeSwap v3 `Swap` decoder (BSC, ADR-020 amendment 5).
//!
//! `PancakeV3Pool` emits
//! `Swap(address indexed sender, address indexed recipient, int256 amount0,
//! int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick,
//! uint128 protocolFeesToken0, uint128 protocolFeesToken1)`: Uniswap v3's
//! layout plus the two trailing protocol-fee words, hence ANOTHER topic0
//! ([`PANCAKE_V3_SWAP_TOPIC0`], keccak of the signature, never typed from a
//! third party). Source of the signature: the orchestrator's reading of
//! developer.pancakeswap.finance / pancake-v3-core (2026-10-04); the 64 live
//! pools in `evm_bsc_swaps_all_2026-10-04.json` emit this topic with exactly
//! 3 topics and 7 data words, which is what this decoder demands.
//!
//! Sign convention is Uniswap v3's: `amount0/amount1` are the POOL's view
//! (positive = the pool received that token). Amounts of a trade still come
//! from the wallet's own deltas (ADR-020); the event only evidences a swap.
//! `protocolFeesToken*` are not used for any amount.

use alloy_primitives::{Address, B256, I256, U256, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;
use scout_evm::address_from_topic;

use crate::uniswap::{bad, shape, word_i256, word_u128, word_u256};

/// `Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)`.
pub const PANCAKE_V3_SWAP_TOPIC0: B256 =
    b256!("19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PancakeV3Swap {
    pub pool: Address,
    pub sender: Address,
    pub recipient: Address,
    /// Pool's view; positive = pool received.
    pub amount0: I256,
    pub amount1: I256,
    pub sqrt_price_x96: U256,
    pub liquidity: u128,
    pub tick: i32,
    pub protocol_fees_token0: u128,
    pub protocol_fees_token1: u128,
    pub log_index: u64,
}

#[must_use]
pub fn decode_pancake_v3_swap(log: &RawEvmLog) -> DecodeOutcome<PancakeV3Swap> {
    if let Err(out) = shape(log, PANCAKE_V3_SWAP_TOPIC0, 3, 224, "pancake v3 Swap") {
        return out;
    }
    let (Some(sender), Some(recipient)) = (log.topics.get(1), log.topics.get(2)) else {
        return bad("pancake v3 Swap");
    };
    let d = log.data.as_ref();
    let (Some(amount0), Some(amount1), Some(sqrt), Some(liq), Some(tick), Some(f0), Some(f1)) = (
        word_i256(d, 0),
        word_i256(d, 1),
        word_u256(d, 2),
        word_u128(d, 3),
        word_i256(d, 4),
        word_u128(d, 5),
        word_u128(d, 6),
    ) else {
        return bad("pancake v3 Swap");
    };
    // uint160 / int24 range checks (the words are sign/zero-extended).
    if sqrt >> 160 != U256::ZERO {
        return bad("pancake v3 Swap");
    }
    let Some(tick) = i32::try_from(tick)
        .ok()
        .filter(|t| (-(1 << 23)..(1 << 23)).contains(t))
    else {
        return bad("pancake v3 Swap");
    };
    DecodeOutcome::Decoded(PancakeV3Swap {
        pool: log.address,
        sender: address_from_topic(sender),
        recipient: address_from_topic(recipient),
        amount0,
        amount1,
        sqrt_price_x96: sqrt,
        liquidity: liq,
        tick,
        protocol_fees_token0: f0,
        protocol_fees_token1: f1,
        log_index: log.log_index,
    })
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, keccak256};

    use super::*;

    fn w(v: U256) -> [u8; 32] {
        v.to_be_bytes::<32>()
    }
    fn wi(v: i128) -> [u8; 32] {
        I256::try_from(v).unwrap().into_raw().to_be_bytes::<32>()
    }
    fn mk(topics: Vec<B256>, words: &[[u8; 32]]) -> RawEvmLog {
        RawEvmLog {
            address: Address::repeat_byte(0xaa),
            topics,
            data: Bytes::from(words.concat()),
            block_number: 10,
            transaction_index: 1,
            log_index: 4,
        }
    }

    #[test]
    fn topic0_is_the_keccak_of_the_pancake_signature_and_differs_from_uniswap_v3() {
        assert_eq!(
            PANCAKE_V3_SWAP_TOPIC0,
            keccak256("Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)")
        );
        assert_ne!(PANCAKE_V3_SWAP_TOPIC0, crate::V3_SWAP_TOPIC0);
    }

    #[test]
    fn decodes_amounts_tick_and_protocol_fees() {
        let l = mk(
            vec![
                PANCAKE_V3_SWAP_TOPIC0,
                Address::repeat_byte(1).into_word(),
                Address::repeat_byte(2).into_word(),
            ],
            &[
                wi(-5),
                wi(7),
                w(U256::from(1u8) << 96),
                w(U256::from(9u8)),
                wi(-120),
                w(U256::from(3u8)),
                w(U256::from(4u8)),
            ],
        );
        let DecodeOutcome::Decoded(s) = decode_pancake_v3_swap(&l) else {
            panic!("decode");
        };
        assert_eq!(
            (s.amount0, s.amount1),
            (I256::try_from(-5).unwrap(), I256::try_from(7).unwrap())
        );
        assert_eq!((s.tick, s.liquidity), (-120, 9));
        assert_eq!((s.protocol_fees_token0, s.protocol_fees_token1), (3, 4));
        assert_eq!(s.sender, Address::repeat_byte(1));
        assert_eq!(s.recipient, Address::repeat_byte(2));
        assert_eq!(s.log_index, 4);
    }

    #[test]
    fn shape_and_range_violations_are_malformed_and_other_topics_not_mine() {
        let ok_words = [
            wi(1),
            wi(-1),
            w(U256::from(1u8)),
            w(U256::from(1u8)),
            wi(0),
            w(U256::ZERO),
            w(U256::ZERO),
        ];
        let topics = vec![
            PANCAKE_V3_SWAP_TOPIC0,
            Address::ZERO.into_word(),
            Address::ZERO.into_word(),
        ];
        // The Uniswap v3 layout (5 words) under the Pancake topic is malformed.
        let short = mk(topics.clone(), &ok_words[..5]);
        assert!(matches!(
            decode_pancake_v3_swap(&short),
            DecodeOutcome::Malformed(_)
        ));
        // Tick outside int24.
        let mut w_bad = ok_words;
        w_bad[4] = wi(1 << 23);
        assert!(matches!(
            decode_pancake_v3_swap(&mk(topics.clone(), &w_bad)),
            DecodeOutcome::Malformed(_)
        ));
        // sqrtPriceX96 above uint160.
        let mut w_bad = ok_words;
        w_bad[2] = w(U256::from(1u8) << 160);
        assert!(matches!(
            decode_pancake_v3_swap(&mk(topics.clone(), &w_bad)),
            DecodeOutcome::Malformed(_)
        ));
        // Fee above uint128.
        let mut w_bad = ok_words;
        w_bad[5] = w(U256::from(1u8) << 128);
        assert!(matches!(
            decode_pancake_v3_swap(&mk(topics.clone(), &w_bad)),
            DecodeOutcome::Malformed(_)
        ));
        // Another topic0 (Uniswap v3) is not this decoder's event.
        let mut other = topics;
        other[0] = crate::V3_SWAP_TOPIC0;
        assert!(matches!(
            decode_pancake_v3_swap(&mk(other, &ok_words)),
            DecodeOutcome::NotMine
        ));
    }
}
