//! FTP's answers as a response head (#51). FTP has no response head: libcurl passes the server's
//! control replies to the header callback instead (`213 1234` for a `SIZE`, `350` for a `REST`),
//! and only its debug callback shows which command each one answers. [`FtpReplies`] pairs the two
//! and builds, before the first body byte, the head an HTTP server would have sent for the same
//! request, so the engine's gate, probe, validators and checkpoints apply unchanged:
//!
//! - **The file:** `200`, with `Content-Length` when `SIZE` gave the size.
//! - **A range** (sent as `REST`, its end enforced by the loop): `206` with
//!   `Content-Range: bytes a-b/size`; `416` with `bytes */size` when it starts past the end, or
//!   for byte 0 of an empty file.
//! - **No range proven:** a `200` when `REST` was refused, or when there is no size to prove a
//!   range against. The loop aborts a `200` for a range after byte 0 before any byte, whatever the
//!   gate says.
//! - **Validators:** `Last-Modified` from `MDTM`, with a `Date` of now, so the date is strong once
//!   it is a second old (`validator.rs` in the engine). No `ETag`: FTP has none.
//!
//! Commands are read from libcurl's debug output only for their verb. The line that carries the
//! password is never kept.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::fetch::ResponseHead;

/// The command whose reply comes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    Size,
    Mdtm,
    Rest,
    Retr,
    Other,
}

/// What an FTP fetch's control replies said so far, and the head they make.
#[derive(Debug)]
pub(crate) struct FtpReplies {
    /// The request's range: `(start, end)`, inclusive.
    range: Option<(u64, Option<u64>)>,
    url: String,
    sent: Sent,
    size: Option<u64>,
    modified: Option<String>,
    /// A head was built: there is at most one.
    done: bool,
}

/// Whether `url` is FTP or FTPS: the scheme, compared without case.
pub(crate) fn is_ftp(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("ftp") || scheme.eq_ignore_ascii_case("ftps")
    })
}

impl FtpReplies {
    pub(crate) fn new(range: Option<(u64, Option<u64>)>, url: String) -> Self {
        FtpReplies {
            range,
            url,
            sent: Sent::Other,
            size: None,
            modified: None,
            done: false,
        }
    }

    /// A command libcurl sent (its debug output). Only the verb is read.
    pub(crate) fn sent(&mut self, line: &[u8]) {
        let verb = line
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .map(u8::to_ascii_uppercase)
            .collect::<Vec<u8>>();
        self.sent = match verb.as_slice() {
            b"SIZE" => Sent::Size,
            b"MDTM" => Sent::Mdtm,
            b"REST" => Sent::Rest,
            b"RETR" => Sent::Retr,
            _ => Sent::Other,
        };
    }

    /// A reply line from the server. Returns the head once the replies decide it: at the `RETR`'s
    /// preliminary reply, or earlier when they rule the range out.
    pub(crate) fn received(&mut self, line: &str, now: SystemTime) -> Option<ResponseHead> {
        let (code, text) = final_reply(line)?;
        let sent = std::mem::replace(&mut self.sent, Sent::Other);
        if self.done {
            return None;
        }
        let start = self.range.map(|(start, _)| start);
        let status = match sent {
            Sent::Size => {
                if code == 213 {
                    self.size = text.trim().parse().ok();
                }
                // libcurl doesn't retrieve a file whose size ends at or before the range's start.
                match (start, self.size) {
                    (Some(start), Some(size)) if start > 0 && start >= size => 416,
                    _ => return None,
                }
            }
            Sent::Mdtm => {
                if code == 213 {
                    self.modified = http_date_of_mdtm(text.trim());
                }
                return None;
            }
            // A refused REST: the server can't start part way.
            Sent::Rest if code != 350 => 200,
            Sent::Retr if code == 125 || code == 150 => match (start, self.size) {
                (None, _) | (Some(_), None) => 200,
                (Some(_), Some(0)) => 416,
                (Some(_), Some(_)) => 206,
            },
            _ => return None,
        };
        self.done = true;
        Some(self.head(status, now))
    }

    /// The head with `status`, from what the replies said.
    fn head(&self, status: u16, now: SystemTime) -> ResponseHead {
        let mut headers = Vec::new();
        match (status, self.range, self.size) {
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
        if let Some(modified) = &self.modified {
            headers.push(("Last-Modified".to_owned(), modified.clone()));
        }
        if let Some(date) = http_date(now) {
            headers.push(("Date".to_owned(), date));
        }
        ResponseHead {
            status,
            headers,
            http2: false,
            url: self.url.clone(),
        }
    }

    /// The head said `416`: the transfer ends without a body, as an HTTP `416` does, even when
    /// libcurl then refuses to start past the end (`CURLE_BAD_DOWNLOAD_RESUME`).
    pub(crate) fn unsatisfiable(&self) -> bool {
        self.done
            && matches!((self.range, self.size), (Some((start, _)), Some(size)) if start >= size)
    }
}

/// A reply's last line: its code and text. `None` for a line that continues a multi-line reply
/// (`211-…`, or text without a code).
fn final_reply(line: &str) -> Option<(u16, &str)> {
    let line = line.trim_end_matches(['\r', '\n']);
    let code = line.get(..3)?;
    if !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rest = &line[3..];
    if rest.starts_with('-') {
        return None;
    }
    Some((code.parse().ok()?, rest.trim_start()))
}

/// `MDTM`'s `YYYYMMDDHHMMSS[.sss]` (RFC 3659 §2.3, always UTC) as an HTTP date (RFC 9110 §5.6.7),
/// to the second. `None` for anything else.
fn http_date_of_mdtm(value: &str) -> Option<String> {
    let digits = value.split('.').next()?;
    if digits.len() != 14 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let field = |a: usize, b: usize| digits[a..b].parse::<u32>().ok();
    let (year, month, day) = (field(0, 4)?, field(4, 6)?, field(6, 8)?);
    let (hour, minute, second) = (field(8, 10)?, field(10, 12)?, field(12, 14)?);
    let leap = |y: u32| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let month_days = match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    // RFC 3659 allows a leap second (60); an HTTP date doesn't.
    if year < 1970 || day == 0 || day > month_days || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    let seconds = days * 86_400 + i64::from(hour * 3600 + minute * 60 + second);
    Some(format_http_date(seconds))
}

/// `now` as an HTTP date; `None` before 1970.
fn http_date(now: SystemTime) -> Option<String> {
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format_http_date(i64::try_from(seconds).ok()?))
}

/// Seconds since the Unix epoch as an IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`.
fn format_http_date(seconds: i64) -> String {
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
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
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

    const NOW: u64 = 1_792_600_000;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW)
    }

    /// The head `replies` (command, reply) make for `range`.
    fn head_of(
        range: Option<(u64, Option<u64>)>,
        replies: &[(&str, &str)],
    ) -> Option<ResponseHead> {
        let mut r = FtpReplies::new(range, "ftp://h/f".to_owned());
        let mut head = None;
        for (command, reply) in replies {
            r.sent(command.as_bytes());
            if let Some(h) = r.received(reply, now()) {
                assert!(head.is_none(), "a second head");
                head = Some(h);
            }
        }
        head
    }

    fn summary(head: &ResponseHead) -> (u16, Option<&str>, Option<&str>) {
        (
            head.status,
            head.header("content-range"),
            head.header("content-length"),
        )
    }

    const LOGIN: [(&str, &str); 4] = [
        ("USER alice\r\n", "331 Password required"),
        ("PASS secret\r\n", "230 Logged in"),
        ("PWD\r\n", "257 \"/\" is the current directory"),
        ("EPSV\r\n", "229 Entering Extended Passive Mode (|||5000|)"),
    ];

    fn with_login(rest: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        LOGIN.iter().chain(rest).copied().collect()
    }

    #[test]
    fn the_whole_file_is_a_200_with_its_size_and_date() {
        let head = head_of(
            None,
            &with_login(&[
                ("MDTM f\r\n", "213 20200102030405"),
                ("SIZE f\r\n", "213 1234"),
                ("RETR f\r\n", "150 Opening BINARY mode data connection"),
            ]),
        )
        .unwrap();
        assert_eq!(summary(&head), (200, None, Some("1234")));
        assert_eq!(
            head.header("last-modified"),
            Some("Thu, 02 Jan 2020 03:04:05 GMT")
        );
        assert_eq!(head.header("date"), Some("Wed, 21 Oct 2026 16:26:40 GMT"));
        assert_eq!(head.url, "ftp://h/f");
    }

    #[test]
    fn a_range_is_a_206_up_to_its_end_or_the_files() {
        let replies = |rest: &'static str| {
            with_login(&[
                ("SIZE f\r\n", "213 1000"),
                ("REST 100\r\n", rest),
                ("RETR f\r\n", "150 Opening"),
            ])
        };
        let open = head_of(Some((100, None)), &replies("350 Restarting at 100")).unwrap();
        assert_eq!(
            summary(&open),
            (206, Some("bytes 100-999/1000"), Some("900"))
        );
        let ended = head_of(Some((100, Some(199))), &replies("350 Restarting")).unwrap();
        assert_eq!(
            summary(&ended),
            (206, Some("bytes 100-199/1000"), Some("100"))
        );
        let past = head_of(Some((100, Some(5000))), &replies("350 Restarting")).unwrap();
        assert_eq!(
            summary(&past),
            (206, Some("bytes 100-999/1000"), Some("900"))
        );
        // From byte 0 libcurl sends no REST.
        let probe = head_of(
            Some((0, Some(u64::MAX - 1))),
            &with_login(&[("SIZE f\r\n", "213 1000"), ("RETR f\r\n", "125 Data open")]),
        )
        .unwrap();
        assert_eq!(
            summary(&probe),
            (206, Some("bytes 0-999/1000"), Some("1000"))
        );
    }

    #[test]
    fn a_refused_rest_is_a_200_at_once() {
        let head = head_of(
            Some((100, None)),
            &with_login(&[
                ("SIZE f\r\n", "213 1000"),
                ("REST 100\r\n", "502 REST not implemented"),
            ]),
        )
        .unwrap();
        assert_eq!(summary(&head), (200, None, Some("1000")));
    }

    #[test]
    fn without_a_size_nothing_proves_a_range() {
        let head = head_of(
            Some((100, None)),
            &with_login(&[
                ("SIZE f\r\n", "502 SIZE not implemented"),
                ("REST 100\r\n", "350 Restarting"),
                ("RETR f\r\n", "150 Opening"),
            ]),
        )
        .unwrap();
        assert_eq!(summary(&head), (200, None, None));
    }

    #[test]
    fn a_range_at_or_past_the_end_is_a_416_at_the_size() {
        for start in [1000, 1001] {
            let head = head_of(
                Some((start, None)),
                &with_login(&[("SIZE f\r\n", "213 1000")]),
            );
            assert_eq!(summary(&head.unwrap()), (416, Some("bytes */1000"), None));
        }
        // An empty file has no byte 0.
        let empty = head_of(
            Some((0, Some(99))),
            &with_login(&[("SIZE e\r\n", "213 0"), ("RETR e\r\n", "150 Opening")]),
        )
        .unwrap();
        assert_eq!(summary(&empty), (416, Some("bytes */0"), None));
        let whole = head_of(
            None,
            &with_login(&[("SIZE e\r\n", "213 0"), ("RETR e\r\n", "150 Opening")]),
        )
        .unwrap();
        assert_eq!(summary(&whole), (200, None, Some("0")));
    }

    #[test]
    fn a_failed_retr_makes_no_head() {
        let head = head_of(
            None,
            &with_login(&[
                ("SIZE m\r\n", "550 No such file"),
                ("RETR m\r\n", "550 No such file"),
            ]),
        );
        assert_eq!(head, None);
    }

    #[test]
    fn a_reply_is_read_from_its_last_line_only() {
        let mut r = FtpReplies::new(None, "ftp://h/f".to_owned());
        r.sent(b"SIZE f\r\n");
        assert_eq!(r.received("213-Size follows", now()), None);
        assert_eq!(r.received(" not a code", now()), None);
        assert_eq!(r.received("213 77\r\n", now()), None);
        r.sent(b"RETR f\r\n");
        let head = r.received("150 Opening", now()).unwrap();
        assert_eq!(head.header("content-length"), Some("77"));
        assert_eq!(r.received("226 Transfer complete", now()), None);
    }

    #[test]
    fn mdtm_times_become_http_dates_or_nothing() {
        for (mdtm, date) in [
            ("19700101000000", Some("Thu, 01 Jan 1970 00:00:00 GMT")),
            ("19941106084937", Some("Sun, 06 Nov 1994 08:49:37 GMT")),
            ("20000229235959.123", Some("Tue, 29 Feb 2000 23:59:59 GMT")),
            ("20261021202640", Some("Wed, 21 Oct 2026 20:26:40 GMT")),
            ("20210229000000", None),
            ("19000229000000", None),
            ("20201301000000", None),
            ("20200101240000", None),
            ("20200101000060", None),
            ("2020010100000", None),
            ("2020010100000x", None),
            ("", None),
        ] {
            assert_eq!(http_date_of_mdtm(mdtm).as_deref(), date, "{mdtm}");
        }
    }

    #[test]
    fn days_and_dates_round_trip() {
        for days in [-1, 0, 59, 10_957, 11_016, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn only_ftp_and_ftps_urls_are_ftp() {
        assert!(is_ftp("ftp://h/f"));
        assert!(is_ftp("FTPS://h/f"));
        assert!(!is_ftp("https://h/f"));
        assert!(!is_ftp("sftp://h/f"));
        assert!(!is_ftp("ftp:/h"));
    }
}
