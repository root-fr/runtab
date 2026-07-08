use std::time::{SystemTime, UNIX_EPOCH};

/// Current UNIX time in seconds.
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Today's UTC date as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    epoch_to_date(now_epoch())
}

/// Current time as an RFC 3339 UTC string (`2026-07-06T10:00:00Z`).
pub fn now_rfc3339() -> String {
    epoch_to_rfc3339(now_epoch())
}

/// `YYYY-MM-DD` of a UNIX timestamp (UTC).
pub fn epoch_to_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Full RFC 3339 UTC string of a UNIX timestamp.
pub fn epoch_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, rem % 3600 / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// `date - days` as `YYYY-MM-DD`.
pub fn date_minus_days(date: &str, days: i64) -> String {
    match parse_date_to_epoch(date) {
        Some(e) => epoch_to_date(e - days * 86_400),
        None => date.to_string(),
    }
}

/// Parse a `YYYY-MM-DD` date (UTC midnight) to epoch seconds.
pub fn parse_date_to_epoch(date: &str) -> Option<i64> {
    let b = date.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y = num(&date[0..4])?;
    let m = num(&date[5..7])?;
    let d = num(&date[8..10])?;
    Some(days_from_civil(y, m, d) * 86_400)
}

/// Parse an RFC 3339 UTC timestamp to epoch seconds. Accepts an optional
/// fractional second and a trailing `Z` (the shape both agents emit). Non-UTC
/// offsets and malformed strings return `None` rather than a guess.
pub fn parse_rfc3339_to_epoch(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let y = num(&ts[0..4])?;
    let m = num(&ts[5..7])?;
    let d = num(&ts[8..10])?;
    let hh = num(&ts[11..13])?;
    let mm = num(&ts[14..16])?;
    let ss = num(&ts[17..19])?;
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`: civil (y, m, d) from a day count.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
