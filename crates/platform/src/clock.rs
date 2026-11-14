//! UTC civil date and RFC 3339 second-precision timestamps.

use std::time::{SystemTime, UNIX_EPOCH};

/// `YYYY-MM-DD` in UTC.
pub fn utc_date(t: SystemTime) -> String {
    let (y, m, d, _, _, _) = civil(t);
    format!("{y:04}-{m:02}-{d:02}")
}

/// RFC 3339 in UTC with second precision.
pub fn utc_timestamp(t: SystemTime) -> String {
    let (y, m, d, hh, mm, ss) = civil(t);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Inverse of Howard Hinnant's days_from_civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (rem / 3_600) as u32,
        ((rem % 3_600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::utc_timestamp;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn utc_timestamp_matches_the_pre_move_string_for_epoch_plus_one_day() {
        // Pinned from state::utc_timestamp(UNIX_EPOCH + 86_400s) before the move.
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_secs(86_400)),
            "1970-01-02T00:00:00Z"
        );
    }
}
