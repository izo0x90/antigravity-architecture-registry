//! When something happened, written one way everywhere a person reads it
//! (plans, logs, messages): RFC 3339 in UTC, to the second
//! (`2026-10-20T22:11:28Z`).

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since 1970 now.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Now, written.
pub fn now() -> String {
    written(now_ms())
}

/// `at_ms` (milliseconds since 1970), written.
pub fn written(at_ms: u64) -> String {
    let secs = (at_ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A time as written (fractions of a second left out), in milliseconds
/// since 1970: none when it isn't one.
pub fn read(at: &str) -> Option<u64> {
    let (date, time) = at.trim().trim_end_matches('Z').split_once('T')?;
    let mut d = date.splitn(3, '-').map(str::parse::<i64>);
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.splitn(3, ':');
    let (h, min) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let s = t.next()?.split(['.', '+', '-']).next()?.parse::<i64>().ok()?;
    // Days from 1970-01-01 to the civil date (the same algorithm, backwards).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(((days * 24 + h) * 60 + min) * 60 + s).ok().map(|secs| secs * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_written_one_way_and_read_back() {
        assert_eq!(written(0), "1970-01-01T00:00:00Z");
        assert_eq!(written(1_791_334_297_000), "2026-10-07T00:51:37Z");
        assert_eq!(written(1_709_208_000_250), "2024-02-29T12:00:00Z");
        for at in [0, 1_791_334_297_000, 1_709_208_000_000, 951_782_400_000] {
            assert_eq!(read(&written(at)), Some(at));
        }
        assert_eq!(read("2024-02-29T12:00:00.250Z"), Some(1_709_208_000_000), "fractions left out");
        assert_eq!(read("yesterday"), None);
    }
}
