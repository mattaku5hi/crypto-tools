//! On-chain exit quotes (ADR-019 EVM amendment): calldata builders, response
//! decoders, the pinned official quoter table and the exact Uniswap-v2-style
//! constant-product formula. Pure; the `eth_call`s live in the engine.
//!
//! Official quoters are pinned ONLY where a source names them and the
//! address was read back live through its factory / pool-manager getter
//! (ADR-020 amendment 7): research doc 2.2-2.4 (Uniswap v3 Base/Robinhood,
//! v4 Robinhood) and the 2026-10-04 BSC/Base verification (PancakeSwap v3,
//! Uniswap v3 BSC, Uniswap v4 Base/BSC, the three Slipstream generations).
//! Every other (chain, family) is unpinned and the valuation says
//! `venue_quoter_unpinned` (invariant 16: no address from memory).

use alloy_primitives::{Address, B256, I256, U256, address, keccak256};

use crate::gate::{SwapVenue, VENUE_DEPLOYMENTS};

/// Quoter ABI family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuoterFamily {
    /// Uniswap v3 `QuoterV2` (`fee` pool selector).
    UniswapV3,
    /// PancakeSwap v3 `QuoterV2` (same struct as Uniswap's).
    PancakeV3,
    /// Aerodrome Slipstream `QuoterV2` (`tickSpacing` selector).
    Slipstream,
    /// Uniswap v4 `V4Quoter`.
    UniswapV4,
}

impl QuoterFamily {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UniswapV3 => "uniswap_v3_quoter_v2",
            Self::PancakeV3 => "pancake_v3_quoter_v2",
            Self::Slipstream => "slipstream_quoter_v2",
            Self::UniswapV4 => "uniswap_v4_quoter",
        }
    }
}

/// The pinned official quoter of `(chain_id, family)`; `None` = unpinned.
#[must_use]
pub fn pinned_quoter(chain_id: u64, family: QuoterFamily) -> Option<Address> {
    match (chain_id, family) {
        // developers.uniswap.org v3 Robinhood deployments (research doc 2.4).
        (4663, QuoterFamily::UniswapV3) => {
            Some(address!("33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7"))
        }
        // developers.uniswap.org v3-base-deployments (research doc 2.2).
        (8453, QuoterFamily::UniswapV3) => {
            Some(address!("3d4e44Eb1374240CE5F1B871ab261CD16335B76a"))
        }
        // developers.uniswap.org v4 deployments, Robinhood (research doc 2.4).
        (4663, QuoterFamily::UniswapV4) => {
            Some(address!("8dc178efb8111bb0973dd9d722ebeff267c98f94"))
        }
        // developer.pancakeswap.finance v3 addresses; live getters on
        // 2026-10-04: factory() = 0x0bfbcf9f..1865, deployer() = 0x41ff9aa7..71c9
        // on both chains.
        (8453, QuoterFamily::PancakeV3) => {
            Some(address!("4c650FB471fe4e0f476fD3437C3411B1122c4e3B"))
        }
        (56, QuoterFamily::PancakeV3) => Some(address!("B048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997")),
        // developers.uniswap.org v3 BNB deployments; live factory() = 0xdb1d1001..61f7.
        (56, QuoterFamily::UniswapV3) => Some(address!("78D78E420Da98ad378D7799bE8f4AF69033EB077")),
        // developers.uniswap.org v4 deployments; live poolManager() =
        // 0x498581ff..2b2b (Base) / 0x28e2ea09..e9df (BSC).
        (8453, QuoterFamily::UniswapV4) => {
            Some(address!("0d5e0f971ed27fbff6c2837bf31316121532048d"))
        }
        (56, QuoterFamily::UniswapV4) => Some(address!("9f75dd27d6664c475b90e105573e550ff69437b0")),
        // Slipstream quoters are per factory generation: see
        // `pinned_slipstream_quoter`.
        _ => None,
    }
}

/// The pinned Slipstream quoter of a pool's `factory()` on `chain_id`
/// (github.com/aerodrome-finance/slipstream README; each quoter's live
/// `factory()` getter equals the key on 2026-10-04). The calldata shape
/// (`tickSpacing` struct) is the orchestrator-supplied one: the orchestrator
/// confirms it with a live quote.
#[must_use]
pub fn pinned_slipstream_quoter(chain_id: u64, factory: Address) -> Option<Address> {
    if chain_id != 8453 {
        return None;
    }
    // gen1 QuoterV2, gen2 Quoter, gen3 Quoter.
    [
        (
            address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A"),
            address!("254cF9E1E6e233aa1AC962CB9B05b2cfeAaE15b0"),
        ),
        (
            address!("aDe65c38CD4849aDBA595a4323a8C7DdfE89716a"),
            address!("3d4C22254F86f64B7eC90ab8F7aeC1FBFD271c6C"),
        ),
        (
            address!("f8f2eB4940CFE7d13603DDDD87f123820Fc061Ef"),
            address!("514c8B5f54112481E28028F1166Bd78501089259"),
        ),
    ]
    .iter()
    .find(|(f, _)| *f == factory)
    .map(|(_, q)| *q)
}

/// First 4 bytes of `keccak256(signature)`.
#[must_use]
pub fn selector(signature: &str) -> [u8; 4] {
    let h = keccak256(signature.as_bytes());
    let mut s = [0u8; 4];
    for (d, b) in s.iter_mut().zip(h.as_slice()) {
        *d = *b;
    }
    s
}

fn word_addr(a: Address) -> [u8; 32] {
    let mut w = [0u8; 32];
    for (d, b) in w.iter_mut().skip(12).zip(a.as_slice()) {
        *d = *b;
    }
    w
}

fn word_int(v: i32) -> [u8; 32] {
    I256::try_from(i64::from(v))
        .unwrap_or_default()
        .into_raw()
        .to_be_bytes::<32>()
}

fn hex_call(sel: [u8; 4], words: &[[u8; 32]]) -> String {
    let mut b = Vec::with_capacity(4 + 32 * words.len());
    b.extend_from_slice(&sel);
    for w in words {
        b.extend_from_slice(w);
    }
    format!("0x{}", alloy_primitives::hex::encode(b))
}

/// `quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96))`
/// of the Uniswap / Pancake v3 `QuoterV2` (`pool_selector` = fee) or, for
/// Slipstream, `(..., int24 tickSpacing, uint160)`.
#[must_use]
pub fn v3_quote_calldata(
    family: QuoterFamily,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    pool_selector: i32,
) -> String {
    let sig = if family == QuoterFamily::Slipstream {
        "quoteExactInputSingle((address,address,uint256,int24,uint160))"
    } else {
        "quoteExactInputSingle((address,address,uint256,uint24,uint160))"
    };
    hex_call(
        selector(sig),
        &[
            word_addr(token_in),
            word_addr(token_out),
            amount_in.to_be_bytes::<32>(),
            word_int(pool_selector),
            [0u8; 32],
        ],
    )
}

/// A Uniswap v4 `PoolKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V4PoolKey {
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}

impl V4PoolKey {
    fn words(&self) -> [[u8; 32]; 5] {
        [
            word_addr(self.currency0),
            word_addr(self.currency1),
            U256::from(self.fee).to_be_bytes::<32>(),
            word_int(self.tick_spacing),
            word_addr(self.hooks),
        ]
    }

    /// `keccak256(abi.encode(poolKey))`: the v4 `PoolId`.
    #[must_use]
    pub fn pool_id(&self) -> B256 {
        let mut b = Vec::with_capacity(160);
        for w in self.words() {
            b.extend_from_slice(&w);
        }
        keccak256(b)
    }
}

/// `quoteExactInputSingle(((address,address,uint24,int24,address),bool,uint128,bytes))`
/// of the `V4Quoter` with empty `hookData`.
#[must_use]
pub fn v4_quote_calldata(key: &V4PoolKey, zero_for_one: bool, exact_amount: u128) -> String {
    let mut words: Vec<[u8; 32]> = vec![U256::from(0x20u8).to_be_bytes::<32>()];
    words.extend(key.words());
    words.push(U256::from(u8::from(zero_for_one)).to_be_bytes::<32>());
    words.push(U256::from(exact_amount).to_be_bytes::<32>());
    // hookData offset from the struct start: 8 head words.
    words.push(U256::from(0x100u16).to_be_bytes::<32>());
    words.push([0u8; 32]); // hookData length 0
    hex_call(
        selector(
            "quoteExactInputSingle(((address,address,uint24,int24,address),bool,uint128,bytes))",
        ),
        &words,
    )
}

/// Aerodrome v2 pool `getAmountOut(uint256,address)`.
#[must_use]
pub fn aerodrome_amount_out_calldata(amount_in: U256, token_in: Address) -> String {
    hex_call(
        selector("getAmountOut(uint256,address)"),
        &[amount_in.to_be_bytes::<32>(), word_addr(token_in)],
    )
}

/// v2 pair `getReserves()`.
#[must_use]
pub fn v2_reserves_calldata() -> String {
    hex_call(selector("getReserves()"), &[])
}

fn word_at(b: &[u8], i: usize) -> Option<U256> {
    let s = i.checked_mul(32)?;
    Some(U256::from_be_slice(b.get(s..s.checked_add(32)?)?))
}

/// First word (`amountOut`) of a quoter / `getAmountOut` answer carrying at
/// least `min_words` words (QuoterV2: 4, V4Quoter: 2, getAmountOut: 1).
#[must_use]
pub fn decode_amount_out(b: &[u8], min_words: usize) -> Option<U256> {
    if b.len() < min_words.checked_mul(32)? {
        return None;
    }
    word_at(b, 0)
}

/// `(reserve0, reserve1)` of `getReserves()` (3 words; reserves are uint112).
#[must_use]
pub fn decode_reserves(b: &[u8]) -> Option<(U256, U256)> {
    if b.len() < 96 {
        return None;
    }
    let max = (U256::from(1u8) << 112) - U256::from(1u8);
    let (r0, r1) = (word_at(b, 0)?, word_at(b, 1)?);
    (r0 <= max && r1 <= max).then_some((r0, r1))
}

/// Swap fee of a v2-style pair as `amount_in * num / den` kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V2Fee {
    /// Numerator of the fraction of the input that stays after the fee.
    pub num: u32,
    pub den: u32,
}

/// Uniswap v2: 0.30% (`UniswapV2Library.getAmountOut`: `amountIn * 997 / 1000`).
pub const UNISWAP_V2_FEE: V2Fee = V2Fee {
    num: 997,
    den: 1000,
};
/// PancakeSwap v2: 0.25% (`PancakeLibrary.getAmountOut`: `amountIn * 9975 / 10000`;
/// the pair's K check uses `balance * 10000 - amountIn * 25`).
pub const PANCAKE_V2_FEE: V2Fee = V2Fee {
    num: 9975,
    den: 10_000,
};
/// PancakeSwap v2 factory on BSC (research doc 2.3).
pub const PANCAKE_V2_BSC_FACTORY: Address = address!("cA143Ce32Fe78f1f7019d7d551a6402fC5350c73");

/// The fee of a pinned v2-style factory of `chain_id`; `None` for a factory
/// that is not a pinned v2 deployment (no fee is guessed).
#[must_use]
pub fn v2_fee_for_factory(chain_id: u64, factory: Address) -> Option<V2Fee> {
    VENUE_DEPLOYMENTS
        .iter()
        .find(|d| {
            d.chain_id == chain_id
                && d.venue == SwapVenue::UniswapV2
                && d.anchor == factory
                && d.role == crate::gate::AnchorRole::PoolFactory
        })
        .map(|d| {
            if d.anchor == PANCAKE_V2_BSC_FACTORY {
                PANCAKE_V2_FEE
            } else {
                UNISWAP_V2_FEE
            }
        })
}

/// Exact v2 output: `floor(in*num*rout / (rin*den + in*num))`. `None` on an
/// empty reserve, zero input or overflow.
#[must_use]
pub fn v2_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    fee: V2Fee,
) -> Option<U256> {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() {
        return None;
    }
    let in_fee = amount_in.checked_mul(U256::from(fee.num))?;
    let num = in_fee.checked_mul(reserve_out)?;
    let den = reserve_in
        .checked_mul(U256::from(fee.den))?
        .checked_add(in_fee)?;
    num.checked_div(den)
}

/// Price impact in bps of selling `full` (got `out_full`) versus a probe of
/// `probe` units (got `out_probe`), pool fee cancelled:
/// `floor((1 - (out_full/full) / (out_probe/probe)) * 10_000)`, 0 when the
/// full sale got an equal or better rate. `None` on zero inputs/overflow.
#[must_use]
pub fn price_impact_bps(full: U256, out_full: U256, probe: U256, out_probe: U256) -> Option<u64> {
    if full.is_zero() || probe.is_zero() || out_probe.is_zero() {
        return None;
    }
    let spot = out_probe.checked_mul(full)?;
    let real = out_full.checked_mul(probe)?;
    if real >= spot {
        return Some(0);
    }
    let bps = (spot - real)
        .checked_mul(U256::from(10_000u16))?
        .checked_div(spot)?;
    u64::try_from(bps).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_match_the_known_values() {
        // Uniswap QuoterV2.quoteExactInputSingle((address,address,uint256,uint24,uint160)).
        assert_eq!(
            selector("quoteExactInputSingle((address,address,uint256,uint24,uint160))"),
            [0xc6, 0xa5, 0x02, 0x6a]
        );
        assert_eq!(selector("getReserves()"), [0x09, 0x02, 0xf1, 0xac]);
    }

    #[test]
    fn v4_calldata_layout_and_pool_id() {
        let key = V4PoolKey {
            currency0: Address::ZERO,
            currency1: Address::repeat_byte(0x11),
            fee: 3000,
            tick_spacing: 60,
            hooks: Address::ZERO,
        };
        let d = v4_quote_calldata(&key, true, 5);
        // selector + 1 offset + 8 head + 1 length words.
        assert_eq!(d.len(), 2 + 8 + 64 * 10);
        assert_eq!(key.pool_id(), keccak256([key.words().concat()].concat()));
    }

    #[test]
    fn negative_tick_spacing_is_sign_extended() {
        assert_eq!(word_int(-60)[0], 0xff);
        assert_eq!(word_int(60)[31], 60);
    }

    #[test]
    fn v2_formula_golden() {
        // 1e18 in, reserves 100e18 / 200e18, 0.3%: classic router value.
        let out = v2_amount_out(
            U256::from(10u64).pow(U256::from(18u8)),
            U256::from(100u64) * U256::from(10u64).pow(U256::from(18u8)),
            U256::from(200u64) * U256::from(10u64).pow(U256::from(18u8)),
            UNISWAP_V2_FEE,
        )
        .unwrap();
        assert_eq!(out, U256::from(1_974_316_068_794_122_597u64));
        assert!(
            v2_amount_out(U256::ZERO, U256::from(1u8), U256::from(1u8), UNISWAP_V2_FEE).is_none()
        );
    }

    #[test]
    fn fee_by_factory() {
        assert_eq!(
            v2_fee_for_factory(56, PANCAKE_V2_BSC_FACTORY),
            Some(PANCAKE_V2_FEE)
        );
        assert_eq!(
            v2_fee_for_factory(8453, address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6")),
            Some(UNISWAP_V2_FEE)
        );
        assert_eq!(v2_fee_for_factory(8453, Address::repeat_byte(1)), None);
    }

    #[test]
    fn pinned_quoters_only_where_documented() {
        assert!(pinned_quoter(8453, QuoterFamily::UniswapV3).is_some());
        assert!(pinned_quoter(4663, QuoterFamily::UniswapV4).is_some());
        assert!(pinned_quoter(8453, QuoterFamily::UniswapV4).is_some());
        assert!(pinned_quoter(56, QuoterFamily::PancakeV3).is_some());
        assert!(pinned_quoter(8453, QuoterFamily::PancakeV3).is_some());
        // Not documented: Uniswap v3 on a synthetic chain, Robinhood Pancake.
        assert!(pinned_quoter(999, QuoterFamily::UniswapV3).is_none());
        assert!(pinned_quoter(4663, QuoterFamily::PancakeV3).is_none());
        assert!(pinned_quoter(8453, QuoterFamily::Slipstream).is_none());
        let gen2 = address!("aDe65c38CD4849aDBA595a4323a8C7DdfE89716a");
        assert!(pinned_slipstream_quoter(8453, gen2).is_some());
        assert!(pinned_slipstream_quoter(56, gen2).is_none());
        assert!(pinned_slipstream_quoter(8453, Address::repeat_byte(1)).is_none());
    }

    #[test]
    fn impact_bps() {
        let u = U256::from;
        // probe 10 -> 100 out (rate 10); full 1000 -> 9000 out (rate 9) = 10%.
        assert_eq!(
            price_impact_bps(u(1000u32), u(9000u32), u(10u32), u(100u32)),
            Some(1000)
        );
        assert_eq!(
            price_impact_bps(u(1000u32), u(10_000u32), u(10u32), u(100u32)),
            Some(0)
        );
    }
}
