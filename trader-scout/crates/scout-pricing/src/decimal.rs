//! Exact non-negative decimal numbers from JSON number text.

use std::fmt;

/// Largest power of ten that fits `u128` (`10^38`).
const MAX_POW10: u32 = 38;

/// Why a number token was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecimalParseError {
    #[error("empty number token")]
    Empty,
    #[error("number token is not a plain non-negative decimal")]
    Syntax,
    #[error("number does not fit u128 mantissa")]
    TooLarge,
}

/// `mantissa / 10^scale`, normalized (no trailing fractional zeros), so
/// equal values compare equal. Never negative, never a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecimalPrice {
    mantissa: u128,
    scale: u32,
}

impl DecimalPrice {
    /// The value 1 (USDC par).
    pub const ONE: DecimalPrice = DecimalPrice {
        mantissa: 1,
        scale: 0,
    };

    /// Build from parts, normalizing trailing zeros.
    #[must_use]
    pub fn new(mantissa: u128, scale: u32) -> Self {
        let (mut m, mut s) = (mantissa, scale);
        while s > 0 && m != 0 && m.rem_euclid(10) == 0 {
            m = m.div_euclid(10);
            s -= 1;
        }
        if m == 0 {
            s = 0;
        }
        Self {
            mantissa: m,
            scale: s,
        }
    }

    #[must_use]
    pub fn mantissa(&self) -> u128 {
        self.mantissa
    }

    /// Number of fractional decimal digits (`value = mantissa / 10^scale`).
    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.mantissa == 0
    }

    /// Parse JSON number text: digits, optional `.digits`, optional
    /// `e[+-]digits`. Signs on the mantissa, `NaN`, hex etc. are rejected.
    pub fn parse(text: &str) -> Result<Self, DecimalParseError> {
        if text.is_empty() {
            return Err(DecimalParseError::Empty);
        }
        let (mant_text, exp_text) = match text.find(['e', 'E']) {
            Some(i) => (text.get(..i).unwrap_or(""), text.get(i + 1..)),
            None => (text, None),
        };
        let (int_part, frac_part) = match mant_text.split_once('.') {
            Some((i, f)) => (i, f),
            None => (mant_text, ""),
        };
        let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if int_part.is_empty()
            || !all_digits(int_part)
            || !all_digits(frac_part)
            || (mant_text.contains('.') && frac_part.is_empty())
        {
            return Err(DecimalParseError::Syntax);
        }
        let mut mantissa: u128 = 0;
        for b in int_part.bytes().chain(frac_part.bytes()) {
            mantissa = mantissa
                .checked_mul(10)
                .and_then(|m| m.checked_add(u128::from(b - b'0')))
                .ok_or(DecimalParseError::TooLarge)?;
        }
        let mut scale = u32::try_from(frac_part.len()).map_err(|_| DecimalParseError::TooLarge)?;
        if let Some(exp) = exp_text {
            let (neg, digits) = match exp.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, exp.strip_prefix('+').unwrap_or(exp)),
            };
            if digits.is_empty() || !all_digits(digits) || digits.len() > 3 {
                return Err(DecimalParseError::Syntax);
            }
            let e: u32 = digits.parse().map_err(|_| DecimalParseError::Syntax)?;
            if neg {
                scale = scale.checked_add(e).ok_or(DecimalParseError::TooLarge)?;
            } else if e >= scale {
                let up = e - scale;
                scale = 0;
                if up > MAX_POW10 && mantissa != 0 {
                    return Err(DecimalParseError::TooLarge);
                }
                for _ in 0..up.min(MAX_POW10 + 1) {
                    mantissa = mantissa
                        .checked_mul(10)
                        .ok_or(DecimalParseError::TooLarge)?;
                }
            } else {
                scale -= e;
            }
        }
        if scale > MAX_POW10 + 20 {
            return Err(DecimalParseError::TooLarge);
        }
        Ok(Self::new(mantissa, scale))
    }

    /// Exact decimal string (no exponent).
    #[must_use]
    pub fn to_decimal_string(&self) -> String {
        let digits = self.mantissa.to_string();
        let scale = usize::try_from(self.scale).unwrap_or(0);
        if scale == 0 {
            return digits;
        }
        if digits.len() <= scale {
            let zeros = "0".repeat(scale - digits.len());
            format!("0.{zeros}{digits}")
        } else {
            let split = digits.len() - scale;
            format!(
                "{}.{}",
                digits.get(..split).unwrap_or(""),
                digits.get(split..).unwrap_or("")
            )
        }
    }
}

impl fmt::Display for DecimalPrice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_decimal_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exactly_with_many_digits() {
        let text = "121.123456789012345678901234567891";
        let p = DecimalPrice::parse(text).unwrap();
        assert_eq!(p.mantissa(), 121_123_456_789_012_345_678_901_234_567_891);
        assert_eq!(p.scale(), 30);
        assert_eq!(p.to_decimal_string(), text);
    }

    #[test]
    fn normalizes_trailing_zeros_and_compares_by_value() {
        assert_eq!(
            DecimalPrice::parse("121.50").unwrap(),
            DecimalPrice::parse("121.5").unwrap()
        );
        assert_eq!(DecimalPrice::parse("0.0").unwrap(), DecimalPrice::new(0, 5));
        assert_eq!(DecimalPrice::parse("121").unwrap().scale(), 0);
        assert_eq!(DecimalPrice::parse("121.75").unwrap().mantissa(), 12175);
    }

    #[test]
    fn exponent_forms() {
        assert_eq!(
            DecimalPrice::parse("1.5e-7").unwrap().to_decimal_string(),
            "0.00000015"
        );
        assert_eq!(
            DecimalPrice::parse("1.5e2").unwrap().to_decimal_string(),
            "150"
        );
        assert_eq!(
            DecimalPrice::parse("12E+1").unwrap().to_decimal_string(),
            "120"
        );
    }

    #[test]
    fn rejects_garbage_and_overflow() {
        for bad in [
            "", "-1", "+1", "1.", ".5", "1.2.3", "abc", "0x10", "1e", "NaN", "1e+",
        ] {
            assert!(DecimalPrice::parse(bad).is_err(), "{bad}");
        }
        let huge = "9".repeat(40);
        assert_eq!(DecimalPrice::parse(&huge), Err(DecimalParseError::TooLarge));
    }

    #[test]
    fn display_small_values_keep_leading_zeros() {
        assert_eq!(DecimalPrice::new(5, 3).to_decimal_string(), "0.005");
        assert_eq!(DecimalPrice::ONE.to_decimal_string(), "1");
    }
}
