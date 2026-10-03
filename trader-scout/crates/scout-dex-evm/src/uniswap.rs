//! Uniswap v3 / v4 event decoders (ADR-020 §2b; research doc §2.1).
//!
//! Decoders prove the event *shape* only. Support for a deployment is a
//! separate claim made by [`crate::gate`] (invariant #16).
//!
//! Trader identity is NOT in any of these events: v3 `sender`/`recipient`
//! are the router, v4 `Swap` has only `sender` = locker/router and a hashed
//! pool id. Amounts/identity come from transaction-level net flows.
//!
//! Sign conventions:
//! - v3 `Swap`: `amount0/amount1` from the **pool's** view (positive = pool
//!   received that token).
//! - v4 `Swap`: `amount0/amount1` is the `BalanceDelta` from the
//!   **swapper's** view (negative = swapper pays), `int128` on the wire.

use alloy_primitives::{Address, B256, I256, U256, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;
use scout_evm::address_from_topic;

/// `Swap(address,address,int256,int256,uint160,uint128,int24)` (Uniswap v3,
/// Slipstream).
pub const V3_SWAP_TOPIC0: B256 =
    b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67");
/// `Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)` (v4).
pub const V4_SWAP_TOPIC0: B256 =
    b256!("40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f");
/// `Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)`.
pub const V4_INITIALIZE_TOPIC0: B256 =
    b256!("dd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438");
/// `PoolCreated(address,address,uint24,int24,address)` (Uniswap v3 factory).
pub const V3_POOL_CREATED_TOPIC0: B256 =
    b256!("783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118");
/// `PairCreated(address,address,address,uint256)` (v2-style factory).
pub const V2_PAIR_CREATED_TOPIC0: B256 =
    b256!("0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3Swap {
    pub pool: Address,
    pub sender: Address,
    pub recipient: Address,
    /// Pool's view; positive = pool received.
    pub amount0: I256,
    pub amount1: I256,
    pub sqrt_price_x96: U256,
    pub liquidity: U256,
    pub log_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V4Swap {
    /// Emitting PoolManager.
    pub pool_manager: Address,
    pub pool_id: B256,
    pub sender: Address,
    /// Swapper's view; negative = swapper pays.
    pub amount0: i128,
    pub amount1: i128,
    pub sqrt_price_x96: U256,
    pub liquidity: u128,
    pub fee: u32,
    pub log_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V4Initialize {
    pub pool_manager: Address,
    pub pool_id: B256,
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub hooks: Address,
    pub log_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3PoolCreated {
    pub factory: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub pool: Address,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2PairCreated {
    pub factory: Address,
    pub token0: Address,
    pub token1: Address,
    pub pair: Address,
}

fn word(data: &[u8], index: usize) -> Option<[u8; 32]> {
    let start = index.checked_mul(32)?;
    let end = start.checked_add(32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(data.get(start..end)?);
    Some(out)
}

fn word_u256(data: &[u8], index: usize) -> Option<U256> {
    word(data, index).map(U256::from_be_bytes)
}

fn word_i256(data: &[u8], index: usize) -> Option<I256> {
    word_u256(data, index).map(I256::from_raw)
}

fn word_address(data: &[u8], index: usize) -> Option<Address> {
    let w = word(data, index)?;
    // ABI-encoded address: upper 12 bytes must be zero.
    if w.get(..12)?.iter().any(|b| *b != 0) {
        return None;
    }
    Some(Address::from_slice(w.get(12..)?))
}

fn word_i128(data: &[u8], index: usize) -> Option<i128> {
    i128::try_from(word_i256(data, index)?).ok()
}

fn word_u128(data: &[u8], index: usize) -> Option<u128> {
    u128::try_from(word_u256(data, index)?).ok()
}

fn word_u32(data: &[u8], index: usize) -> Option<u32> {
    u32::try_from(word_u256(data, index)?).ok()
}

fn shape<T>(
    log: &RawEvmLog,
    topic0: B256,
    topics: usize,
    data_len: usize,
    name: &str,
) -> Result<(), DecodeOutcome<T>> {
    if log.topics.first() != Some(&topic0) {
        return Err(DecodeOutcome::NotMine);
    }
    if log.topics.len() != topics || log.data.len() != data_len {
        return Err(DecodeOutcome::Malformed(format!(
            "{name}: {} topics / {} data bytes, expected {topics} / {data_len}",
            log.topics.len(),
            log.data.len()
        )));
    }
    Ok(())
}

fn bad<T>(name: &str) -> DecodeOutcome<T> {
    DecodeOutcome::Malformed(format!("{name}: a field is out of range for its ABI type"))
}

#[must_use]
pub fn decode_v3_swap(log: &RawEvmLog) -> DecodeOutcome<V3Swap> {
    if let Err(out) = shape(log, V3_SWAP_TOPIC0, 3, 160, "v3 Swap") {
        return out;
    }
    let (Some(sender), Some(recipient)) = (log.topics.get(1), log.topics.get(2)) else {
        return bad("v3 Swap");
    };
    let d = log.data.as_ref();
    let (Some(amount0), Some(amount1), Some(sqrt_price_x96), Some(liquidity)) = (
        word_i256(d, 0),
        word_i256(d, 1),
        word_u256(d, 2),
        word_u256(d, 3),
    ) else {
        return bad("v3 Swap");
    };
    DecodeOutcome::Decoded(V3Swap {
        pool: log.address,
        sender: address_from_topic(sender),
        recipient: address_from_topic(recipient),
        amount0,
        amount1,
        sqrt_price_x96,
        liquidity,
        log_index: log.log_index,
    })
}

#[must_use]
pub fn decode_v4_swap(log: &RawEvmLog) -> DecodeOutcome<V4Swap> {
    if let Err(out) = shape(log, V4_SWAP_TOPIC0, 3, 192, "v4 Swap") {
        return out;
    }
    let (Some(id), Some(sender)) = (log.topics.get(1), log.topics.get(2)) else {
        return bad("v4 Swap");
    };
    let d = log.data.as_ref();
    let (Some(amount0), Some(amount1), Some(sqrt_price_x96), Some(liquidity), Some(fee)) = (
        word_i128(d, 0),
        word_i128(d, 1),
        word_u256(d, 2),
        word_u128(d, 3),
        word_u32(d, 5),
    ) else {
        return bad("v4 Swap");
    };
    DecodeOutcome::Decoded(V4Swap {
        pool_manager: log.address,
        pool_id: *id,
        sender: address_from_topic(sender),
        amount0,
        amount1,
        sqrt_price_x96,
        liquidity,
        fee,
        log_index: log.log_index,
    })
}

#[must_use]
pub fn decode_v4_initialize(log: &RawEvmLog) -> DecodeOutcome<V4Initialize> {
    if let Err(out) = shape(log, V4_INITIALIZE_TOPIC0, 4, 160, "v4 Initialize") {
        return out;
    }
    let (Some(id), Some(c0), Some(c1)) = (log.topics.get(1), log.topics.get(2), log.topics.get(3))
    else {
        return bad("v4 Initialize");
    };
    let d = log.data.as_ref();
    let (Some(fee), Some(hooks)) = (word_u32(d, 0), word_address(d, 2)) else {
        return bad("v4 Initialize");
    };
    DecodeOutcome::Decoded(V4Initialize {
        pool_manager: log.address,
        pool_id: *id,
        currency0: address_from_topic(c0),
        currency1: address_from_topic(c1),
        fee,
        hooks,
        log_index: log.log_index,
    })
}

#[must_use]
pub fn decode_v3_pool_created(log: &RawEvmLog) -> DecodeOutcome<V3PoolCreated> {
    if let Err(out) = shape(log, V3_POOL_CREATED_TOPIC0, 4, 64, "v3 PoolCreated") {
        return out;
    }
    let (Some(t0), Some(t1), Some(fee_topic)) =
        (log.topics.get(1), log.topics.get(2), log.topics.get(3))
    else {
        return bad("v3 PoolCreated");
    };
    let (Some(fee), Some(pool)) = (
        u32::try_from(U256::from_be_bytes(fee_topic.0)).ok(),
        word_address(log.data.as_ref(), 1),
    ) else {
        return bad("v3 PoolCreated");
    };
    DecodeOutcome::Decoded(V3PoolCreated {
        factory: log.address,
        token0: address_from_topic(t0),
        token1: address_from_topic(t1),
        fee,
        pool,
    })
}

#[must_use]
pub fn decode_v2_pair_created(log: &RawEvmLog) -> DecodeOutcome<V2PairCreated> {
    if let Err(out) = shape(log, V2_PAIR_CREATED_TOPIC0, 3, 64, "v2 PairCreated") {
        return out;
    }
    let (Some(t0), Some(t1)) = (log.topics.get(1), log.topics.get(2)) else {
        return bad("v2 PairCreated");
    };
    let Some(pair) = word_address(log.data.as_ref(), 0) else {
        return bad("v2 PairCreated");
    };
    DecodeOutcome::Decoded(V2PairCreated {
        factory: log.address,
        token0: address_from_topic(t0),
        token1: address_from_topic(t1),
        pair,
    })
}

/// CREATE2 address of a Uniswap-v3-style pool:
/// `keccak256(0xff ++ factory ++ keccak256(abi.encode(token0, token1, fee)) ++ init_code_hash)[12..]`.
///
/// `init_code_hash` is **per deployment** (it differs between chains and
/// forks) and is not pinned for Robinhood/Base/BSC yet, so the gate verifies
/// pools through the factory's `PoolCreated` log instead; this function is
/// the offline cross-check once a hash is pinned from a fixture.
#[must_use]
pub fn v3_pool_address_create2(
    factory: Address,
    token0: Address,
    token1: Address,
    fee: u32,
    init_code_hash: B256,
) -> Address {
    let mut enc = [0u8; 96];
    if let Some(s) = enc.get_mut(12..32) {
        s.copy_from_slice(token0.as_slice());
    }
    if let Some(s) = enc.get_mut(44..64) {
        s.copy_from_slice(token1.as_slice());
    }
    if let Some(s) = enc.get_mut(64..96) {
        s.copy_from_slice(&U256::from(fee).to_be_bytes::<32>());
    }
    factory.create2(alloy_primitives::keccak256(enc), init_code_hash)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, address, keccak256};

    use super::*;

    fn w(v: U256) -> [u8; 32] {
        v.to_be_bytes::<32>()
    }

    fn wi(v: i128) -> [u8; 32] {
        I256::try_from(v).unwrap().into_raw().to_be_bytes::<32>()
    }

    fn mk(address: Address, topics: Vec<B256>, words: &[[u8; 32]]) -> RawEvmLog {
        RawEvmLog {
            address,
            topics,
            data: Bytes::from(words.concat()),
            block_number: 10,
            transaction_index: 1,
            log_index: 4,
        }
    }

    #[test]
    fn topic0_constants_are_keccak_of_signatures() {
        let sig = |s: &str| keccak256(s);
        assert_eq!(
            V3_SWAP_TOPIC0,
            sig("Swap(address,address,int256,int256,uint160,uint128,int24)")
        );
        assert_eq!(
            V4_SWAP_TOPIC0,
            sig("Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)")
        );
        assert_eq!(
            V4_INITIALIZE_TOPIC0,
            sig("Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)")
        );
        assert_eq!(
            V3_POOL_CREATED_TOPIC0,
            sig("PoolCreated(address,address,uint24,int24,address)")
        );
        assert_eq!(
            V2_PAIR_CREATED_TOPIC0,
            sig("PairCreated(address,address,address,uint256)")
        );
        assert_eq!(
            crate::V2_SWAP_EVENT_SIGNATURE,
            sig("Swap(address,uint256,uint256,uint256,uint256,address)")
        );
    }

    #[test]
    fn v3_swap_round_trips_signed_amounts() {
        let pool = Address::repeat_byte(0x11);
        let sender = Address::repeat_byte(0x22);
        let recipient = Address::repeat_byte(0x33);
        let neg = I256::try_from(-5i64).unwrap().into_raw();
        let log = mk(
            pool,
            vec![V3_SWAP_TOPIC0, sender.into_word(), recipient.into_word()],
            &[
                w(U256::from(100u64)),
                w(neg),
                w(U256::from(7u64)),
                w(U256::from(9u64)),
                w(U256::ZERO),
            ],
        );
        let s = decode_v3_swap(&log).decoded().unwrap();
        assert_eq!(s.pool, pool);
        assert_eq!(s.amount0, I256::try_from(100i64).unwrap());
        assert_eq!(s.amount1, I256::try_from(-5i64).unwrap());
        assert_eq!((s.sender, s.recipient), (sender, recipient));
        assert!(
            decode_v3_swap(&RawEvmLog {
                data: Bytes::new(),
                ..log
            })
            .is_malformed()
        );
    }

    #[test]
    fn v4_swap_and_initialize_decode() {
        let pm = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
        let id = B256::repeat_byte(0xab);
        let router = Address::repeat_byte(0x44);
        let log = mk(
            pm,
            vec![V4_SWAP_TOPIC0, id, router.into_word()],
            &[
                wi(-1000),
                wi(250),
                w(U256::from(1u64)),
                w(U256::from(2u64)),
                wi(-3),
                w(U256::from(3000u64)),
            ],
        );
        let s = decode_v4_swap(&log).decoded().unwrap();
        assert_eq!(
            (s.amount0, s.amount1, s.fee, s.liquidity),
            (-1000, 250, 3000, 2)
        );
        assert_eq!((s.pool_manager, s.pool_id, s.sender), (pm, id, router));

        let c0 = Address::ZERO;
        let c1 = Address::repeat_byte(0x55);
        let hooks = Address::repeat_byte(0x66);
        let mut hooks_word = [0u8; 32];
        hooks_word[12..].copy_from_slice(hooks.as_slice());
        let init = mk(
            pm,
            vec![V4_INITIALIZE_TOPIC0, id, c0.into_word(), c1.into_word()],
            &[
                w(U256::from(10_000u64)),
                wi(200),
                hooks_word,
                w(U256::ZERO),
                wi(0),
            ],
        );
        let i = decode_v4_initialize(&init).decoded().unwrap();
        assert_eq!(
            (i.pool_id, i.currency0, i.currency1, i.fee, i.hooks),
            (id, c0, c1, 10_000, hooks)
        );
    }

    #[test]
    fn v4_swap_with_out_of_range_int128_is_malformed() {
        let big = w(U256::from(1u8) << 200);
        let log = mk(
            Address::ZERO,
            vec![V4_SWAP_TOPIC0, B256::ZERO, B256::ZERO],
            &[
                big,
                wi(0),
                w(U256::ZERO),
                w(U256::ZERO),
                wi(0),
                w(U256::ZERO),
            ],
        );
        assert!(decode_v4_swap(&log).is_malformed());
    }

    #[test]
    fn factory_logs_decode() {
        let f = Address::repeat_byte(0xf0);
        let t0 = Address::repeat_byte(1);
        let t1 = Address::repeat_byte(2);
        let pool = Address::repeat_byte(3);
        let mut pool_word = [0u8; 32];
        pool_word[12..].copy_from_slice(pool.as_slice());
        let created = mk(
            f,
            vec![
                V3_POOL_CREATED_TOPIC0,
                t0.into_word(),
                t1.into_word(),
                B256::from(w(U256::from(500u64))),
            ],
            &[wi(10), pool_word],
        );
        let c = decode_v3_pool_created(&created).decoded().unwrap();
        assert_eq!(
            (c.factory, c.token0, c.token1, c.fee, c.pool),
            (f, t0, t1, 500, pool)
        );
        let pair = mk(
            f,
            vec![V2_PAIR_CREATED_TOPIC0, t0.into_word(), t1.into_word()],
            &[pool_word, w(U256::from(1u64))],
        );
        assert_eq!(decode_v2_pair_created(&pair).decoded().unwrap().pair, pool);
    }

    #[test]
    fn create2_matches_known_uniswap_v3_mainnet_pool() {
        // Uniswap v3 mainnet USDC/WETH 0.05% pool; canonical init code hash.
        let got = v3_pool_address_create2(
            address!("1F98431c8aD98523631AE4a59f267346ea31F984"),
            address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
            address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
            500,
            b256!("e34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54"),
        );
        assert_eq!(got, address!("88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640"));
    }
}
