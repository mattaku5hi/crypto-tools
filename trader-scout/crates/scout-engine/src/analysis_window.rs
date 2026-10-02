//! Analysis window for wallet runs (ADR-011 §1).
//!
//! A run has a half-open UTC window `[since, until)` in unix seconds, from
//! `--since/--until` or `--period <N>d` (= `[until - N*86400, until)`,
//! `until` defaulting to `as_of`, the run start pinned once). No window
//! flags = [`AnalysisWindow::none`]: the whole history is required.
//!
//! The parsers here are deliberately strict: only the exact shapes
//! `YYYY-MM-DDTHH:MM:SSZ` and `<N>d` are accepted (no offsets, no
//! fractions, no lowercase), so a run is reproducible from its `run_meta`.

/// Upper bound for `--period` (days); the `max_backfill_days` default.
pub const MAX_PERIOD_DAYS: u32 = 365;

const SECONDS_PER_DAY: i64 = 86_400;

/// Where the window came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSource {
    /// `--period`.
    Period,
    /// `--since` (and optionally `--until`).
    Explicit,
    /// No window: full history.
    None,
}

impl WindowSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Period => "period",
            Self::Explicit => "explicit",
            Self::None => "none",
        }
    }
}

/// Half-open window `[since, until)` plus the pinned run start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisWindow {
    pub since: i64,
    pub until: i64,
    pub as_of: i64,
    pub source: WindowSource,
}

/// Invalid window arguments (CLI exit 2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WindowError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, WindowError> {
    Err(WindowError(msg.into()))
}

impl AnalysisWindow {
    /// No window (full history). `since`/`until` are meaningless and unused.
    #[must_use]
    pub const fn none(as_of: i64) -> Self {
        Self {
            since: i64::MIN,
            until: i64::MAX,
            as_of,
            source: WindowSource::None,
        }
    }

    /// True when a real window applies.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.source != WindowSource::None
    }

    /// `since <= t < until`; always true without a window.
    #[must_use]
    pub fn contains(&self, t: i64) -> bool {
        !self.is_bounded() || (self.since <= t && t < self.until)
    }

    /// `since`/`until` for output; `None` without a window.
    #[must_use]
    pub fn bounds(&self) -> Option<(i64, i64)> {
        self.is_bounded().then_some((self.since, self.until))
    }

    /// Resolve CLI arguments. `as_of` is the run start in unix seconds.
    ///
    /// * no flags: [`AnalysisWindow::none`];
    /// * `period` conflicts with `since`;
    /// * `period` alone: `[as_of - N*86400, as_of)`; with `until`:
    ///   `[until - N*86400, until)`;
    /// * `since` alone: `[since, as_of)`;
    /// * `until` without `since`/`period` is an error;
    /// * `until > as_of` and `since >= until` are errors.
    pub fn resolve(
        period: Option<&str>,
        since: Option<&str>,
        until: Option<&str>,
        as_of: i64,
    ) -> Result<Self, WindowError> {
        if period.is_some() && since.is_some() {
            return err("--period conflicts with --since");
        }
        if period.is_none() && since.is_none() {
            return if until.is_some() {
                err("--until requires --since or --period")
            } else {
                Ok(Self::none(as_of))
            };
        }
        let until_ts = match until {
            Some(u) => parse_rfc3339_utc(u).map_err(|e| WindowError(format!("--until: {e}")))?,
            None => as_of,
        };
        if until_ts > as_of {
            return err("--until is after the run start (as_of); a future window is not scannable");
        }
        let (since_ts, source) = match (period, since) {
            (Some(p), _) => {
                let days = parse_period_days(p)?;
                let span = i64::from(days)
                    .checked_mul(SECONDS_PER_DAY)
                    .ok_or_else(|| WindowError("--period overflow".to_string()))?;
                let s = until_ts
                    .checked_sub(span)
                    .ok_or_else(|| WindowError("--period underflow".to_string()))?;
                (s, WindowSource::Period)
            }
            (None, Some(s)) => (
                parse_rfc3339_utc(s).map_err(|e| WindowError(format!("--since: {e}")))?,
                WindowSource::Explicit,
            ),
            (None, None) => return Ok(Self::none(as_of)),
        };
        if since_ts >= until_ts {
            return err("window is empty: --since must be earlier than --until");
        }
        Ok(Self {
            since: since_ts,
            until: until_ts,
            as_of,
            source,
        })
    }
}

/// `<N>d` with `1 <= N <= MAX_PERIOD_DAYS`.
pub fn parse_period_days(text: &str) -> Result<u32, WindowError> {
    let digits = text.strip_suffix('d').unwrap_or("");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return err(format!("--period must look like 30d, got {text:?}"));
    }
    let n: u32 = digits
        .parse()
        .map_err(|_| WindowError(format!("--period out of range: {text:?}")))?;
    if n == 0 || n > MAX_PERIOD_DAYS {
        return err(format!("--period must be 1..={MAX_PERIOD_DAYS} days"));
    }
    Ok(n)
}

/// Strict `YYYY-MM-DDTHH:MM:SSZ` (UTC only) -> unix seconds.
pub fn parse_rfc3339_utc(text: &str) -> Result<i64, String> {
    let bad = || format!("expected UTC RFC 3339 like 2026-08-01T00:00:00Z, got {text:?}");
    let b = text.as_bytes();
    if b.len() != 20 {
        return Err(bad());
    }
    let sep = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(sep(4, b'-')
        && sep(7, b'-')
        && sep(10, b'T')
        && sep(13, b':')
        && sep(16, b':')
        && sep(19, b'Z'))
    {
        return Err(bad());
    }
    let num = |from: usize, to: usize| -> Result<i64, String> {
        let part = b.get(from..to).ok_or_else(bad)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return Err(bad());
        }
        std::str::from_utf8(part)
            .map_err(|_| bad())?
            .parse::<i64>()
            .map_err(|_| bad())
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return Err(bad());
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dim = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    if day < 1 || day > dim {
        return Err(bad());
    }
    Ok(days_from_civil(year, month, day) * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second)
}

/// Howard Hinnant's `days_from_civil` (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9).rem_euclid(12);
    let doy = (153 * mp + 2).div_euclid(5) + day - 1;
    let doe = yoe * 365 + yoe.div_euclid(4) - yoe.div_euclid(100) + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const AS_OF: i64 = 1_790_000_000;

    #[test]
    fn rfc3339_known_instants() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(
            parse_rfc3339_utc("2000-02-29T00:00:00Z").unwrap(),
            951_782_400
        );
        assert_eq!(
            parse_rfc3339_utc("2026-07-02T12:34:56Z").unwrap(),
            1_782_995_696
        );
        assert_eq!(parse_rfc3339_utc("1969-12-31T23:59:59Z").unwrap(), -1);
    }

    #[test]
    fn rfc3339_rejects_everything_but_the_strict_utc_form() {
        for bad in [
            "2026-08-01T00:00:00+03:00",
            "2026-08-01T00:00:00+00:00",
            "2026-08-01T00:00:00.000Z",
            "2026-08-01t00:00:00z",
            "2026-08-01 00:00:00Z",
            "2026-08-01",
            "2026-13-01T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-08-01T24:00:00Z",
            "2026-08-01T00:60:00Z",
            "2026-08-01T00:00:60Z",
            "+026-08-01T00:00:00Z",
            "",
        ] {
            assert!(parse_rfc3339_utc(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn period_parser_bounds() {
        assert_eq!(parse_period_days("1d").unwrap(), 1);
        assert_eq!(parse_period_days("365d").unwrap(), 365);
        for bad in [
            "0d", "366d", "30x", "30", "d", "-3d", "+3d", "3.5d", "30D", " 30d",
        ] {
            assert!(parse_period_days(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolve_period_explicit_and_none() {
        let w = AnalysisWindow::resolve(Some("30d"), None, None, AS_OF).unwrap();
        assert_eq!(
            (w.since, w.until, w.as_of, w.source),
            (AS_OF - 30 * 86_400, AS_OF, AS_OF, WindowSource::Period)
        );
        let e = AnalysisWindow::resolve(
            None,
            Some("2026-08-01T00:00:00Z"),
            Some("2026-09-01T00:00:00Z"),
            AS_OF,
        )
        .unwrap();
        assert_eq!(e.source, WindowSource::Explicit);
        assert_eq!(e.until - e.since, 31 * 86_400);
        let since_only =
            AnalysisWindow::resolve(None, Some("2026-09-20T00:00:00Z"), None, AS_OF).unwrap();
        assert_eq!(since_only.until, AS_OF);
        let p_until =
            AnalysisWindow::resolve(Some("7d"), None, Some("2026-09-20T00:00:00Z"), AS_OF).unwrap();
        assert_eq!(p_until.until - p_until.since, 7 * 86_400);
        let n = AnalysisWindow::resolve(None, None, None, AS_OF).unwrap();
        assert!(!n.is_bounded() && n.contains(i64::MIN) && n.contains(i64::MAX - 1));
        assert!(n.bounds().is_none());
    }

    #[test]
    fn resolve_rejects_conflicts_and_bad_ranges() {
        let r = |p, s, u| AnalysisWindow::resolve(p, s, u, AS_OF);
        assert!(r(Some("30d"), Some("2026-08-01T00:00:00Z"), None).is_err());
        assert!(r(None, None, Some("2026-08-01T00:00:00Z")).is_err());
        assert!(r(Some("0d"), None, None).is_err());
        assert!(r(None, Some("2026-08-01T00:00:00+03:00"), None).is_err());
        assert!(
            r(
                None,
                Some("2026-09-02T00:00:00Z"),
                Some("2026-09-01T00:00:00Z")
            )
            .is_err()
        );
        assert!(
            r(
                None,
                Some("2026-09-01T00:00:00Z"),
                Some("2026-09-01T00:00:00Z")
            )
            .is_err()
        );
        assert!(
            r(
                None,
                Some("2026-08-01T00:00:00Z"),
                Some("2099-01-01T00:00:00Z")
            )
            .is_err()
        );
    }

    #[test]
    fn contains_is_half_open() {
        let w = AnalysisWindow {
            since: 10,
            until: 20,
            as_of: 20,
            source: WindowSource::Explicit,
        };
        assert!(!w.contains(9) && w.contains(10) && w.contains(19) && !w.contains(20));
    }
}
