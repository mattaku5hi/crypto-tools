//! Exact monetary types. See ADR-001 for the rationale.
//!
//! - [`RawAmount`]: unsigned on-chain native-precision amount (U256-backed).
//! - [`SignedAmount`]: signed native-precision delta (I256-backed).
//! - [`Money`]: fixed-point ledger currency value (i128 at [`MONEY_SCALE`]).
//!
//! None of these types implement `From<f32>`/`From<f64>` or expose any
//! floating-point arithmetic. Per AGENTS.md invariant #7, floats are
//! permitted only in statistical output built from these values, never in
//! the ledger itself.

use std::fmt;
use std::str::FromStr;

use alloy_primitives::{I256, U256};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ScoutError;

/// Fixed decimal places used by [`Money`]. 8 matches common stablecoin
/// decimals and avoids double-rounding when translating token amounts
/// priced in USDC/USDT (see ADR-001).
pub const MONEY_SCALE: u32 = 8;

/// Unsigned on-chain native-precision amount (no implicit decimals).
///
/// Decimals live on the asset's identity, not on the amount; applying them
/// is an explicit, checked operation performed by `scout-pricing`, never
/// implicit here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawAmount(U256);

impl RawAmount {
    pub const ZERO: RawAmount = RawAmount(U256::ZERO);

    #[must_use]
    pub fn from_u256(value: U256) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_u256(&self) -> U256 {
        self.0
    }

    /// Checked addition; `None` on overflow (never wraps).
    #[must_use]
    pub fn checked_add(&self, other: &RawAmount) -> Option<RawAmount> {
        self.0.checked_add(other.0).map(RawAmount)
    }

    /// Checked subtraction; `None` if it would go negative or overflow.
    #[must_use]
    pub fn checked_sub(&self, other: &RawAmount) -> Option<RawAmount> {
        self.0.checked_sub(other.0).map(RawAmount)
    }
}

impl fmt::Display for RawAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for RawAmount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for RawAmount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        U256::from_str(&s)
            .map(RawAmount)
            .map_err(serde::de::Error::custom)
    }
}

/// Signed native-precision delta (net wallet balance change in a tx).
///
/// Used for reconciliation; never displayed without a sign-aware
/// formatter, and never silently coerced to [`RawAmount`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignedAmount(I256);

impl SignedAmount {
    pub const ZERO: SignedAmount = SignedAmount(I256::ZERO);

    #[must_use]
    pub fn from_i256(value: I256) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_i256(&self) -> I256 {
        self.0
    }

    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.0.is_negative()
    }

    #[must_use]
    pub fn checked_add(&self, other: &SignedAmount) -> Option<SignedAmount> {
        self.0.checked_add(other.0).map(SignedAmount)
    }
}

impl fmt::Display for SignedAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for SignedAmount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for SignedAmount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        I256::from_str(&s)
            .map(SignedAmount)
            .map_err(serde::de::Error::custom)
    }
}

/// Fixed-point ledger currency value (USD by default; see
/// `quote_asset_policy` in `config/scout.example.toml` for alternatives).
///
/// Backed by `i128` at a fixed scale of [`MONEY_SCALE`] decimal places.
/// All arithmetic is checked; overflow produces
/// `ScoutError::ArithmeticOverflow`, never a panic or silent wraparound
/// (ACCEPTANCE C13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(i128);

impl Money {
    pub const ZERO: Money = Money(0);

    /// Construct from an already-scaled integer (i.e. `units` represents
    /// the value multiplied by `10^MONEY_SCALE`).
    #[must_use]
    pub fn from_scaled_units(units: i128) -> Self {
        Self(units)
    }

    #[must_use]
    pub fn scaled_units(&self) -> i128 {
        self.0
    }

    #[must_use = "this returns the result of the checked addition without modifying self"]
    pub fn checked_add(&self, other: &Money) -> Result<Money, ScoutError> {
        self.0
            .checked_add(other.0)
            .map(Money)
            .ok_or(ScoutError::ArithmeticOverflow {
                context: "Money::checked_add",
            })
    }

    #[must_use = "this returns the result of the checked subtraction without modifying self"]
    pub fn checked_sub(&self, other: &Money) -> Result<Money, ScoutError> {
        self.0
            .checked_sub(other.0)
            .map(Money)
            .ok_or(ScoutError::ArithmeticOverflow {
                context: "Money::checked_sub",
            })
    }

    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.0 < 0
    }

    #[must_use]
    pub fn is_positive(&self) -> bool {
        self.0 > 0
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Integer-only formatting: split scaled units into whole/fractional
        // parts without any floating-point conversion. Sign is handled
        // separately from the magnitude — mixing div_euclid/rem_euclid on
        // a negative value directly would print "-1.50000000" for -0.5
        // instead of "-0.50000000".
        let scale: i128 = 10i128.pow(MONEY_SCALE);
        let negative = self.0 < 0;
        let magnitude = self.0.unsigned_abs();
        let whole = magnitude.div_euclid(scale.unsigned_abs());
        let frac = magnitude.rem_euclid(scale.unsigned_abs());
        let width = usize::try_from(MONEY_SCALE).unwrap_or(8);
        let sign = if negative { "-" } else { "" };
        write!(f, "{sign}{whole}.{frac:0width$}")
    }
}

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        i128::from_str(&s)
            .map(Money)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_amount_roundtrips_through_json_as_string() {
        let amount = RawAmount::from_u256(U256::from(123_456_789_u64));
        let json = serde_json::to_string(&amount).unwrap();
        assert_eq!(json, "\"123456789\"");
        let back: RawAmount = serde_json::from_str(&json).unwrap();
        assert_eq!(back, amount);
    }

    #[test]
    fn raw_amount_checked_sub_none_on_underflow() {
        let small = RawAmount::from_u256(U256::from(1_u64));
        let big = RawAmount::from_u256(U256::from(2_u64));
        assert_eq!(small.checked_sub(&big), None);
    }

    #[test]
    fn money_checked_add_overflow_is_typed_error() {
        let max = Money::from_scaled_units(i128::MAX);
        let one = Money::from_scaled_units(1);
        let result = max.checked_add(&one);
        assert!(matches!(result, Err(ScoutError::ArithmeticOverflow { .. })));
    }

    #[test]
    fn money_display_matches_worked_example_from_acceptance_c01() {
        // consideration=1000, fee=10 -> scaled units at 8dp.
        let consideration = Money::from_scaled_units(1000 * 10i128.pow(MONEY_SCALE));
        assert_eq!(consideration.to_string(), "1000.00000000");
    }

    #[test]
    fn money_display_is_correct_for_negative_values() {
        // Regression test: naive div_euclid/rem_euclid directly on a
        // negative i128 prints "-1.50000000" for -0.5 instead of
        // "-0.50000000". Sign must be split from magnitude first.
        let half_negative = Money::from_scaled_units(-50_000_000);
        assert_eq!(half_negative.to_string(), "-0.50000000");

        let large_negative = Money::from_scaled_units(-100_000_000_001);
        assert_eq!(large_negative.to_string(), "-1000.00000001");
    }

    #[test]
    fn money_serializes_as_json_string_never_a_number() {
        let value = Money::from_scaled_units(3_800_000_000); // 380.00000000... scaled
        let json = serde_json::to_string(&value).unwrap();
        assert!(json.starts_with('"') && json.ends_with('"'));
    }
}
