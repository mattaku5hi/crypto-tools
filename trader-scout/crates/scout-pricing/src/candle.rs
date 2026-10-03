//! Coinbase candle bodies, parsed from the raw JSON text without floats.
//!
//! The body is `[[time, low, high, open, close, volume], ...]`, newest
//! first, every element a JSON number. Numbers are read as TEXT tokens and
//! converted by [`DecimalPrice::parse`], so a price keeps every digit the
//! provider sent (a float parse would not). This module is a deliberately
//! small scanner for exactly that shape; anything else is rejected.

use crate::decimal::{DecimalParseError, DecimalPrice};

/// Hard cap on candles accepted from one body (the provider limit is 300).
pub const MAX_CANDLES_PER_BODY: usize = 1_000;

/// One one-minute candle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candle {
    /// Start of the minute, unix seconds.
    pub time: i64,
    pub low: DecimalPrice,
    pub high: DecimalPrice,
    pub open: DecimalPrice,
    pub close: DecimalPrice,
    pub volume: DecimalPrice,
}

/// Why a body was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseCandlesError {
    #[error("body is not a JSON array of 6-number arrays")]
    Shape,
    #[error("more than {MAX_CANDLES_PER_BODY} candles")]
    TooMany,
    #[error("candle time is not an integer")]
    Time,
    #[error("candle number rejected: {0}")]
    Number(DecimalParseError),
    #[error("candle time is not a whole minute")]
    NotMinuteAligned,
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn skip_ws(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn expect(&mut self, want: u8) -> Result<(), ParseCandlesError> {
        if self.peek() == Some(want) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ParseCandlesError::Shape)
        }
    }

    /// The raw text of one number token.
    fn number_token(&mut self) -> Result<&'a str, ParseCandlesError> {
        self.skip_ws();
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.pos += 1;
        }
        let slice = self
            .bytes
            .get(start..self.pos)
            .ok_or(ParseCandlesError::Shape)?;
        if slice.is_empty() {
            return Err(ParseCandlesError::Shape);
        }
        std::str::from_utf8(slice).map_err(|_| ParseCandlesError::Shape)
    }

    fn decimal(&mut self) -> Result<DecimalPrice, ParseCandlesError> {
        let t = self.number_token()?;
        DecimalPrice::parse(t).map_err(ParseCandlesError::Number)
    }
}

/// Parse a Coinbase candles body. Order is preserved (newest first as sent).
pub fn parse_candles(body: &[u8]) -> Result<Vec<Candle>, ParseCandlesError> {
    let mut s = Scanner {
        bytes: body,
        pos: 0,
    };
    s.expect(b'[')?;
    let mut out = Vec::new();
    if s.peek() == Some(b']') {
        s.pos += 1;
    } else {
        loop {
            if out.len() >= MAX_CANDLES_PER_BODY {
                return Err(ParseCandlesError::TooMany);
            }
            s.expect(b'[')?;
            let time_text = s.number_token()?;
            if time_text.is_empty() || !time_text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseCandlesError::Time);
            }
            let time: i64 = time_text.parse().map_err(|_| ParseCandlesError::Time)?;
            if time.rem_euclid(60) != 0 {
                return Err(ParseCandlesError::NotMinuteAligned);
            }
            let mut vals = [DecimalPrice::ONE; 5];
            for v in &mut vals {
                s.expect(b',')?;
                *v = s.decimal()?;
            }
            s.expect(b']')?;
            out.push(Candle {
                time,
                low: vals[0],
                high: vals[1],
                open: vals[2],
                close: vals[3],
                volume: vals[4],
            });
            match s.peek() {
                Some(b',') => s.pos += 1,
                Some(b']') => {
                    s.pos += 1;
                    break;
                }
                _ => return Err(ParseCandlesError::Shape),
            }
        }
    }
    if s.peek().is_some() {
        return Err(ParseCandlesError::Shape);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "[[1790942700,121.73,121.84,121.75,121.75,102.7895735],\
        [1790942640,121.67,121.77,121.67,121.74,358.14388924],\
        [1790942400,121.72,121.92,121.79,121.89,226.4044408]]";

    #[test]
    fn parses_the_recorded_shape_exactly() {
        let c = parse_candles(BODY.as_bytes()).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].time, 1_790_942_700);
        assert_eq!(c[2].close.to_decimal_string(), "121.89");
        assert_eq!(c[1].volume.to_decimal_string(), "358.14388924");
        assert_eq!(c[0].low.to_decimal_string(), "121.73");
    }

    #[test]
    fn keeps_digits_a_float_would_lose() {
        let body = "[[60,1.00000000000000001,2,1.5,1.12345678901234567890123,7]]";
        let c = parse_candles(body.as_bytes()).unwrap();
        assert_eq!(c[0].low.to_decimal_string(), "1.00000000000000001");
        assert_eq!(c[0].close.to_decimal_string(), "1.12345678901234567890123");
    }

    #[test]
    fn empty_array_and_whitespace() {
        assert!(parse_candles(b" [ ] \n").unwrap().is_empty());
        let c = parse_candles(b"[ [ 60 , 1 , 2 , 3 , 4 , 5 ] ]").unwrap();
        assert_eq!(c[0].open.to_decimal_string(), "3");
    }

    #[test]
    fn rejects_other_shapes() {
        for bad in [
            "{\"message\":\"NotFound\"}",
            "[[60,1,2,3,4]]",
            "[[60,1,2,3,4,5,6]]",
            "[[60,\"1\",2,3,4,5]]",
            "[[60.5,1,2,3,4,5]]",
            "[[61,1,2,3,4,5]]",
            "[[60,1,2,3,4,5]] x",
            "[[60,1,2,3,4,5],]",
            "[[60,-1,2,3,4,5]]",
            "",
        ] {
            assert!(parse_candles(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn caps_candle_count() {
        let mut body = String::from("[");
        for i in 0..=MAX_CANDLES_PER_BODY {
            if i > 0 {
                body.push(',');
            }
            body.push_str(&format!("[{},1,2,3,4,5]", (i + 1) * 60));
        }
        body.push(']');
        assert_eq!(
            parse_candles(body.as_bytes()),
            Err(ParseCandlesError::TooMany)
        );
    }
}
