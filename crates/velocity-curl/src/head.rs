//! The response head of a file transfer that has none of its own, FTP (#51) or SFTP (#53): the
//! head an HTTP server would have sent for the same request, built from the file's size and time.
//! The engine's gate, probe, validators and checkpoints then work unchanged.
//!
//! - **The file:** `200`, with `Content-Length` when the size is known.
//! - **A range:** `206` with `Content-Range: bytes a-b/size`; `416` with `bytes */size` when it
//!   starts past the end, or for byte 0 of an empty file. Without a size, nothing proves a range:
//!   `200` without a length.
//! - **Validators:** `Last-Modified` from the file's time, with a `Date` of now, so the date is
//!   strong once it is a second old (`validator.rs` in the engine).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::fetch::ResponseHead;

/// The head for `range` of a file of `size` bytes last changed at `modified`, served from `url`.
pub fn file_head(
    range: Option<(u64, Option<u64>)>,
    size: Option<u64>,
    modified: Option<SystemTime>,
    url: &str,
) -> ResponseHead {
    let status = match (range, size) {
        (None, _) | (Some(_), None) => 200,
        (Some((start, _)), Some(size)) if start >= size => 416,
        (Some(_), Some(_)) => 206,
    };
    head_with(
        status,
        range,
        size,
        modified.and_then(http_date),
        url,
        SystemTime::now(),
    )
}

/// The head with `status`, for `range` of a file of `size` bytes whose time, as an HTTP date, is
/// `modified`.
pub(crate) fn head_with(
    status: u16,
    range: Option<(u64, Option<u64>)>,
    size: Option<u64>,
    modified: Option<String>,
    url: &str,
    now: SystemTime,
) -> ResponseHead {
    let mut headers = Vec::new();
    match (status, range, size) {
        (206, Some((start, end)), Some(size)) => {
            let last = end.unwrap_or(u64::MAX).min(size - 1);
            headers.push((
                "Content-Range".to_owned(),
                format!("bytes {start}-{last}/{size}"),
            ));
            headers.push(("Content-Length".to_owned(), (last - start + 1).to_string()));
        }
        (416, _, Some(size)) => {
            headers.push(("Content-Range".to_owned(), format!("bytes */{size}")));
        }
        (200, _, Some(size)) => headers.push(("Content-Length".to_owned(), size.to_string())),
        _ => {}
    }
    if let Some(modified) = modified {
        headers.push(("Last-Modified".to_owned(), modified));
    }
    if let Some(date) = http_date(now) {
        headers.push(("Date".to_owned(), date));
    }
    ResponseHead {
        status,
        headers,
        http2: false,
        url: url.to_owned(),
    }
}

/// `now` as an HTTP date; `None` before 1970.
pub(crate) fn http_date(now: SystemTime) -> Option<String> {
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format_http_date(i64::try_from(seconds).ok()?))
}

/// Seconds since the Unix epoch as an IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`.
pub(crate) fn format_http_date(seconds: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize],
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn summary(head: &ResponseHead) -> (u16, Option<&str>, Option<&str>) {
        (
            head.status,
            head.header("content-range"),
            head.header("content-length"),
        )
    }

    #[test]
    fn a_file_head_answers_as_an_http_server_would() {
        let url = "sftp://h/f";
        assert_eq!(
            summary(&file_head(None, Some(1000), None, url)),
            (200, None, Some("1000"))
        );
        assert_eq!(
            summary(&file_head(None, None, None, url)),
            (200, None, None)
        );
        assert_eq!(
            summary(&file_head(
                Some((0, Some(u64::MAX - 1))),
                Some(1000),
                None,
                url
            )),
            (206, Some("bytes 0-999/1000"), Some("1000"))
        );
        assert_eq!(
            summary(&file_head(Some((100, Some(199))), Some(1000), None, url)),
            (206, Some("bytes 100-199/1000"), Some("100"))
        );
        assert_eq!(
            summary(&file_head(Some((1000, None)), Some(1000), None, url)),
            (416, Some("bytes */1000"), None)
        );
        assert_eq!(
            summary(&file_head(Some((0, Some(99))), Some(0), None, url)),
            (416, Some("bytes */0"), None)
        );
        // Without a size, nothing proves a range.
        assert_eq!(
            summary(&file_head(Some((100, None)), None, None, url)),
            (200, None, None)
        );
    }

    #[test]
    fn a_file_head_carries_the_files_time_and_a_date() {
        let modified = UNIX_EPOCH + Duration::from_secs(784_111_777);
        let head = file_head(None, Some(1), Some(modified), "sftp://h/f");
        assert_eq!(
            head.header("last-modified"),
            Some("Sun, 06 Nov 1994 08:49:37 GMT")
        );
        assert!(head.header("date").is_some());
        assert_eq!(head.url, "sftp://h/f");
        assert_eq!(
            file_head(None, Some(1), None, "sftp://h/f").header("last-modified"),
            None
        );
    }

    #[test]
    fn days_and_dates_round_trip() {
        for days in [-1, 0, 59, 10_957, 11_016, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }
}
