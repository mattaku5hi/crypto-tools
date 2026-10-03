//! Pure integer swap math of the pump.fun bonding curve and PumpSwap
//! (ADR-019). Every function is checked (`None` on overflow, division by
//! zero or an impossible state) and uses `u128` intermediates only; there is
//! no floating point.
//!
//! Rounding rules (derived from, and verified exactly against, the committed
//! live paired events; see `tests/swap_math_evidence.rs`):
//!
//! * fee = `ceil(amount * bps / 10_000)` for every fee (protocol, creator,
//!   LP), computed on the **gross** quote amount;
//! * constant-product output = `floor(in * reserve_out / (reserve_in + in))`;
//! * constant-product input for an exact output (PumpSwap buy) =
//!   `ceil(reserve_in * out / (reserve_out - out))`.
//!
//! Curve (`TradeEvent`): reserves in the event are the **post-trade**
//! reserves; `sol_amount` is the gross SOL (a sell pays `sol_amount - fee -
//! creator_fee`; a buy costs `sol_amount + fee + creator_fee`).
//! PumpSwap (`SellEvent`/`BuyEvent`): `pool_*_token_reserves` are the
//! **pre-trade** pool vault balances and the swap runs on
//! `quote_reserve = pool_quote + virtual_quote_reserves`.

const BPS_DENOM: u128 = 10_000;

/// `ceil(amount * bps / 10_000)`.
#[must_use]
pub fn fee_ceil(amount: u64, bps: u64) -> Option<u64> {
    let n = u128::from(amount).checked_mul(u128::from(bps))?;
    let q = n.checked_add(BPS_DENOM - 1)?.checked_div(BPS_DENOM)?;
    u64::try_from(q).ok()
}

/// `floor(a * b / c)`.
fn mul_div_floor(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    a.checked_mul(b)?.checked_div(c)
}

/// `ceil(a * b / c)`.
fn mul_div_ceil(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    let n = a.checked_mul(b)?;
    n.checked_add(c.checked_sub(1)?)?.checked_div(c)
}

/// Result of selling tokens into the bonding curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurveSellQuote {
    /// Gross lamports out of the curve (`TradeEvent.sol_amount`).
    pub gross: u64,
    pub protocol_fee: u64,
    pub creator_fee: u64,
    /// Lamports the seller receives: `gross - protocol_fee - creator_fee`.
    pub net: u64,
}

/// Curve sell: `tokens_in` against virtual reserves (`virtual_sol`,
/// `virtual_token`) with protocol/creator fee bps.
#[must_use]
pub fn curve_sell_quote(
    tokens_in: u64,
    virtual_sol: u64,
    virtual_token: u64,
    fee_bps: u64,
    creator_fee_bps: u64,
) -> Option<CurveSellQuote> {
    let denom = u128::from(virtual_token).checked_add(u128::from(tokens_in))?;
    let gross = u64::try_from(mul_div_floor(
        u128::from(tokens_in),
        u128::from(virtual_sol),
        denom,
    )?)
    .ok()?;
    let protocol_fee = fee_ceil(gross, fee_bps)?;
    let creator_fee = fee_ceil(gross, creator_fee_bps)?;
    let net = gross.checked_sub(protocol_fee)?.checked_sub(creator_fee)?;
    Some(CurveSellQuote {
        gross,
        protocol_fee,
        creator_fee,
        net,
    })
}

/// Curve buy: tokens received for `sol_in` (lamports that enter the curve,
/// i.e. `TradeEvent.sol_amount`, fees excluded) at the given PRE-trade
/// virtual reserves.
#[must_use]
pub fn curve_buy_tokens_out(sol_in: u64, virtual_sol: u64, virtual_token: u64) -> Option<u64> {
    let denom = u128::from(virtual_sol).checked_add(u128::from(sol_in))?;
    u64::try_from(mul_div_floor(
        u128::from(sol_in),
        u128::from(virtual_token),
        denom,
    )?)
    .ok()
}

/// Result of selling base tokens into a PumpSwap pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmSellQuote {
    /// `SellEvent.quote_amount_out`: raw constant-product output.
    pub raw_out: u64,
    pub lp_fee: u64,
    pub protocol_fee: u64,
    pub creator_fee: u64,
    /// `SellEvent.quote_amount_out_without_lp_fee`.
    pub out_without_lp_fee: u64,
    /// `SellEvent.user_quote_amount_out`.
    pub net: u64,
}

/// Effective quote reserve: pool quote vault plus the (possibly negative)
/// `virtual_quote_reserves`. `None` if the result is negative or overflows.
#[must_use]
pub fn effective_quote_reserve(pool_quote: u64, virtual_quote: i128) -> Option<u128> {
    let v = i128::from(pool_quote).checked_add(virtual_quote)?;
    u128::try_from(v).ok()
}

/// PumpSwap sell: `base_in` against `base_reserve` / `quote_reserve`
/// (effective, see [`effective_quote_reserve`]).
#[must_use]
pub fn amm_sell_quote(
    base_in: u64,
    base_reserve: u64,
    quote_reserve: u128,
    lp_fee_bps: u64,
    protocol_fee_bps: u64,
    creator_fee_bps: u64,
) -> Option<AmmSellQuote> {
    let denom = u128::from(base_reserve).checked_add(u128::from(base_in))?;
    let raw_out = u64::try_from(mul_div_floor(u128::from(base_in), quote_reserve, denom)?).ok()?;
    let lp_fee = fee_ceil(raw_out, lp_fee_bps)?;
    let protocol_fee = fee_ceil(raw_out, protocol_fee_bps)?;
    let creator_fee = fee_ceil(raw_out, creator_fee_bps)?;
    let out_without_lp_fee = raw_out.checked_sub(lp_fee)?;
    let net = out_without_lp_fee
        .checked_sub(protocol_fee)?
        .checked_sub(creator_fee)?;
    Some(AmmSellQuote {
        raw_out,
        lp_fee,
        protocol_fee,
        creator_fee,
        out_without_lp_fee,
        net,
    })
}

/// Result of buying an exact base amount from a PumpSwap pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmBuyQuote {
    /// `BuyEvent.quote_amount_in`: raw constant-product input.
    pub raw_in: u64,
    pub lp_fee: u64,
    pub protocol_fee: u64,
    pub creator_fee: u64,
    /// `BuyEvent.quote_amount_in_with_lp_fee`.
    pub in_with_lp_fee: u64,
    /// `BuyEvent.user_quote_amount_in`.
    pub total_cost: u64,
}

/// PumpSwap buy of exactly `base_out` base tokens.
#[must_use]
pub fn amm_buy_quote(
    base_out: u64,
    base_reserve: u64,
    quote_reserve: u128,
    lp_fee_bps: u64,
    protocol_fee_bps: u64,
    creator_fee_bps: u64,
) -> Option<AmmBuyQuote> {
    let denom = u128::from(base_reserve).checked_sub(u128::from(base_out))?;
    let raw_in = u64::try_from(mul_div_ceil(quote_reserve, u128::from(base_out), denom)?).ok()?;
    let lp_fee = fee_ceil(raw_in, lp_fee_bps)?;
    let protocol_fee = fee_ceil(raw_in, protocol_fee_bps)?;
    let creator_fee = fee_ceil(raw_in, creator_fee_bps)?;
    let in_with_lp_fee = raw_in.checked_add(lp_fee)?;
    let total_cost = in_with_lp_fee
        .checked_add(protocol_fee)?
        .checked_add(creator_fee)?;
    Some(AmmBuyQuote {
        raw_in,
        lp_fee,
        protocol_fee,
        creator_fee,
        in_with_lp_fee,
        total_cost,
    })
}

/// Marginal (zero-size) price impact of a quote in basis points:
/// `(marginal_out - realized_out) * 10_000 / marginal_out`, where
/// `marginal_out = amount_in * reserve_out / reserve_in` (floor). `None` if
/// the marginal value is zero. Never negative (clamped at 0).
#[must_use]
pub fn price_impact_bps(
    amount_in: u64,
    reserve_in: u128,
    reserve_out: u128,
    realized_out: u64,
) -> Option<u64> {
    let marginal = mul_div_floor(u128::from(amount_in), reserve_out, reserve_in)?;
    if marginal == 0 {
        return None;
    }
    let diff = marginal.saturating_sub(u128::from(realized_out));
    u64::try_from(mul_div_floor(diff, BPS_DENOM, marginal)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_rounds_up() {
        assert_eq!(fee_ceil(10_000, 95), Some(95));
        assert_eq!(fee_ceil(10_001, 95), Some(96));
        assert_eq!(fee_ceil(0, 95), Some(0));
        assert_eq!(fee_ceil(u64::MAX, 10_000), Some(u64::MAX));
    }

    #[test]
    fn curve_sell_golden() {
        // 1_000_000 tokens into 30 SOL / 1_000_000_000 tokens virtual reserves.
        let q = curve_sell_quote(1_000_000, 30_000_000_000, 1_000_000_000, 95, 30).unwrap();
        // floor(1e6 * 3e10 / (1e9 + 1e6)) = 29_970_029
        assert_eq!(q.gross, 29_970_029);
        assert_eq!(q.protocol_fee, 284_716); // ceil(29_970_029 * .0095)
        assert_eq!(q.creator_fee, 89_911); // ceil(29_970_029 * .003)
        assert_eq!(q.net, 29_970_029 - 284_716 - 89_911);
    }

    #[test]
    fn amm_sell_golden() {
        // 1_000 base into 1_000_000 base / 2_000_000 quote, no virtual.
        let q = amm_sell_quote(1_000, 1_000_000, 2_000_000, 20, 5, 95).unwrap();
        assert_eq!(q.raw_out, 1_998); // floor(1000*2e6/1_001_000)
        assert_eq!(q.lp_fee, 4); // ceil(3.996)
        assert_eq!(q.protocol_fee, 1); // ceil(0.999)
        assert_eq!(q.creator_fee, 19); // ceil(18.981)
        assert_eq!(q.out_without_lp_fee, 1_994);
        assert_eq!(q.net, 1_974);
    }

    #[test]
    fn overflow_and_zero_are_none() {
        assert_eq!(curve_sell_quote(1, 1, 0, 0, 0).map(|q| q.gross), Some(1));
        assert!(curve_sell_quote(0, 1, 5, 0, 0).is_some_and(|q| q.gross == 0));
        assert!(curve_sell_quote(u64::MAX, u64::MAX, u64::MAX, 0, 0).is_some());
        assert!(curve_sell_quote(0, 0, 0, 0, 0).is_none());
        assert!(amm_sell_quote(0, 0, 10, 0, 0, 0).is_none());
        assert!(amm_buy_quote(5, 5, 10, 0, 0, 0).is_none());
        assert!(effective_quote_reserve(1, -2).is_none());
        assert_eq!(effective_quote_reserve(1, 2), Some(3));
    }
}
