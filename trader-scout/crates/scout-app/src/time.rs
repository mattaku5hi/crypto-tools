//! Tiny UTC timestamp formatter (no time crate in the workspace).

use std::time::{SystemTime, UNIX_EPOCH};

/// Current UTC time as RFC 3339 with second precision, e.g.
/// `2026-10-02T12:34:56Z`.
#[must_use]
pub fn now_utc_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_unix_utc(secs)
}

/// RFC 3339 UTC rendering of Unix seconds.
#[must_use]
#[allow(clippy::integer_division)] // calendar arithmetic is intentionally truncating
pub fn format_unix_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_unix_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_utc(1_782_995_696), "2026-07-02T12:34:56Z");
        assert_eq!(format_unix_utc(951_782_400), "2000-02-29T00:00:00Z");
    }
}
