//! Reading a server's answers (research report section 5.1). The first segment is also the probe:
//! its response says whether ranges work, without a HEAD and without a second full transfer.
//! Later segments must answer exactly what was asked, or they are aborted before any byte is
//! written. Pure functions over a response head, so every rule is unit tested.

use velocity_core::domain::{FailureClass, TransferError};
use velocity_curl::ResponseHead;

/// What the probe's response says about the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// `206` for `0-b` of a known total: ranges work. The probe's bytes are `0..=received_end`.
    RangesOk { received_end: u64, total: u64 },
    /// `206` for `0-b` of an unknown total (`*`): continue sequentially, no parallel segments.
    SequentialRanges { received_end: u64 },
    /// `200`: the server ignored the range; this response is the whole file, as one stream.
    SingleStream { length: Option<u64> },
    /// `416` with `bytes */0`: the file is empty.
    Empty,
}

/// Why a later segment's response was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentRejection {
    /// A `200` for a range that doesn't start at 0: the server stopped honouring ranges.
    RangesIgnored,
    /// Anything else that isn't exactly the requested bytes.
    Invalid(TransferError),
}

/// Classifies the probe's final response to `Range: bytes=0-requested_end`.
pub fn classify_probe(
    head: &ResponseHead,
    requested_end: u64,
) -> Result<ProbeOutcome, TransferError> {
    status_error(head)?;
    identity_encoding(head)?;
    match head.status {
        200 => Ok(ProbeOutcome::SingleStream {
            length: content_length(head)?,
        }),
        206 => {
            let (start, end, total) = content_range(head)?;
            if start != 0 || end > requested_end {
                return Err(dishonest(
                    head,
                    &format!(
                        "Content-Range {start}-{end} doesn't answer the range 0-{requested_end}"
                    ),
                ));
            }
            check_length(head, start, end)?;
            match total {
                Some(total) if end < total => Ok(ProbeOutcome::RangesOk {
                    received_end: end,
                    total,
                }),
                Some(total) => Err(dishonest(
                    head,
                    &format!("Content-Range ends at {end}, past the total {total}"),
                )),
                None => Ok(ProbeOutcome::SequentialRanges { received_end: end }),
            }
        }
        416 => match head.header("content-range") {
            Some(cr) if cr.trim() == "bytes */0" => Ok(ProbeOutcome::Empty),
            other => Err(dishonest(
                head,
                &format!("a 416 for byte 0 of a non-empty file ({other:?})"),
            )),
        },
        status => Err(TransferError::new(
            FailureClass::Permanent,
            Some(status),
            &format!("unexpected HTTP status {status}"),
        )),
    }
}

/// Accepts a later segment's response to `Range: bytes=start-end` only if it is a `206` for
/// exactly `start` to at most `end`, of `total` bytes. Returns the last byte it will deliver.
pub fn check_segment(
    head: &ResponseHead,
    start: u64,
    end: u64,
    total: u64,
) -> Result<u64, SegmentRejection> {
    status_error(head).map_err(SegmentRejection::Invalid)?;
    identity_encoding(head).map_err(SegmentRejection::Invalid)?;
    if head.status == 200 {
        return Err(SegmentRejection::RangesIgnored);
    }
    if head.status != 206 {
        return Err(SegmentRejection::Invalid(TransferError::new(
            FailureClass::Permanent,
            Some(head.status),
            &format!("unexpected HTTP status {} for a range", head.status),
        )));
    }
    let (got_start, got_end, got_total) = content_range(head).map_err(SegmentRejection::Invalid)?;
    if got_start != start || got_end > end || got_end < got_start {
        return Err(SegmentRejection::Invalid(dishonest(
            head,
            &format!("Content-Range {got_start}-{got_end} doesn't answer the range {start}-{end}"),
        )));
    }
    if got_total != Some(total) {
        // A different total means a different object (report section 5.3, #45).
        return Err(SegmentRejection::Invalid(TransferError::new(
            FailureClass::Permanent,
            Some(206),
            &format!("the file's total changed from {total} to {got_total:?}"),
        )));
    }
    check_length(head, got_start, got_end).map_err(SegmentRejection::Invalid)?;
    Ok(got_end)
}

/// Error statuses, classified for retries (#48 refines them).
fn status_error(head: &ResponseHead) -> Result<(), TransferError> {
    let s = head.status;
    let class = match s {
        401 | 407 => {
            return Err(TransferError::new(
                FailureClass::Permanent,
                Some(s),
                "the server needs credentials",
            ))
        }
        429 | 503 => FailureClass::Throttled,
        416 => return Ok(()),
        400..=499 => FailureClass::Permanent,
        500..=599 => FailureClass::Transient,
        _ => return Ok(()),
    };
    Err(TransferError::new(class, Some(s), &format!("HTTP {s}")))
}

/// No `Accept-Encoding` is sent, so any encoding but identity isn't the file's own bytes.
fn identity_encoding(head: &ResponseHead) -> Result<(), TransferError> {
    match head.header("content-encoding").map(str::trim) {
        None | Some("identity") => {}
        Some(other) => {
            return Err(dishonest(
                head,
                &format!("an unrequested Content-Encoding: {other}"),
            ))
        }
    }
    if head.header("content-type").is_some_and(|t| {
        t.trim_start()
            .to_ascii_lowercase()
            .starts_with("multipart/byteranges")
    }) {
        return Err(dishonest(
            head,
            "a multipart/byteranges answer to a single range",
        ));
    }
    Ok(())
}

/// `Content-Range: bytes a-b/T` (T may be `*`).
fn content_range(head: &ResponseHead) -> Result<(u64, u64, Option<u64>), TransferError> {
    let value = head
        .header("content-range")
        .ok_or_else(|| dishonest(head, "a 206 without a Content-Range"))?;
    let parse = || -> Option<(u64, u64, Option<u64>)> {
        let spec = value.trim().strip_prefix("bytes ")?;
        let (range, total) = spec.split_once('/')?;
        let (a, b) = range.split_once('-')?;
        let total = if total.trim() == "*" {
            None
        } else {
            Some(total.trim().parse().ok()?)
        };
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total))
    };
    match parse() {
        Some((a, b, t)) if a <= b => Ok((a, b, t)),
        _ => Err(dishonest(
            head,
            &format!("a malformed Content-Range: {value}"),
        )),
    }
}

fn content_length(head: &ResponseHead) -> Result<Option<u64>, TransferError> {
    match head.header("content-length") {
        None => Ok(None),
        Some(v) => v
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| dishonest(head, &format!("a malformed Content-Length: {v}"))),
    }
}

/// A `206`'s Content-Length, when present, must match its Content-Range.
fn check_length(head: &ResponseHead, start: u64, end: u64) -> Result<(), TransferError> {
    match content_length(head)? {
        Some(len) if len != end - start + 1 => Err(dishonest(
            head,
            &format!("Content-Length {len} disagrees with Content-Range {start}-{end}"),
        )),
        _ => Ok(()),
    }
}

/// A response that contradicts itself or the request: not worth retrying as-is.
fn dishonest(head: &ResponseHead, what: &str) -> TransferError {
    TransferError::new(
        FailureClass::Permanent,
        Some(head.status),
        &format!("the server sent {what}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(status: u16, headers: &[(&str, &str)]) -> ResponseHead {
        ResponseHead {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    fn error(r: Result<ProbeOutcome, TransferError>) -> TransferError {
        r.expect_err("the probe should be refused")
    }

    #[test]
    fn a_206_for_the_first_bytes_of_a_known_total_means_ranges_work() {
        let h = head(
            206,
            &[
                ("Content-Range", "bytes 0-4194303/100000000"),
                ("Content-Length", "4194304"),
            ],
        );
        assert_eq!(
            classify_probe(&h, 4_194_303),
            Ok(ProbeOutcome::RangesOk {
                received_end: 4_194_303,
                total: 100_000_000
            })
        );
    }

    #[test]
    fn a_shorter_206_keeps_what_was_sent() {
        // RFC 9110 §14.2: a server may send fewer bytes than asked.
        let h = head(
            206,
            &[
                ("Content-Range", "bytes 0-999/5000"),
                ("Content-Length", "1000"),
            ],
        );
        assert_eq!(
            classify_probe(&h, 4_194_303),
            Ok(ProbeOutcome::RangesOk {
                received_end: 999,
                total: 5000
            })
        );
    }

    #[test]
    fn a_206_covering_a_whole_small_file_is_ranges_ok() {
        let h = head(
            206,
            &[
                ("Content-Range", "bytes 0-99/100"),
                ("Content-Length", "100"),
            ],
        );
        assert_eq!(
            classify_probe(&h, 4_194_303),
            Ok(ProbeOutcome::RangesOk {
                received_end: 99,
                total: 100
            })
        );
    }

    #[test]
    fn a_206_with_an_unknown_total_means_sequential() {
        let h = head(206, &[("Content-Range", "bytes 0-999/*")]);
        assert_eq!(
            classify_probe(&h, 999),
            Ok(ProbeOutcome::SequentialRanges { received_end: 999 })
        );
    }

    #[test]
    fn a_200_means_one_stream_and_keeps_its_bytes() {
        assert_eq!(
            classify_probe(&head(200, &[("Content-Length", "5000")]), 999),
            Ok(ProbeOutcome::SingleStream { length: Some(5000) })
        );
        assert_eq!(
            classify_probe(&head(200, &[]), 999),
            Ok(ProbeOutcome::SingleStream { length: None })
        );
    }

    #[test]
    fn a_416_for_an_empty_file_is_empty() {
        assert_eq!(
            classify_probe(&head(416, &[("Content-Range", "bytes */0")]), 999),
            Ok(ProbeOutcome::Empty)
        );
        assert_eq!(
            error(classify_probe(
                &head(416, &[("Content-Range", "bytes */50")]),
                999
            ))
            .class,
            FailureClass::Permanent
        );
    }

    #[test]
    fn refuses_a_malformed_or_inconsistent_content_range() {
        for (cr, cl) in [
            ("bytes 0-oops/5000", "10"),
            ("bytes 5-9/5000", "5"),       // doesn't start at 0
            ("bytes 0-2000/5000", "2001"), // more than asked
            ("bytes 0-99/50", "100"),      // past the total
            ("items 0-9/5000", "10"),
            ("bytes 0-9/5000", "11"), // Content-Length disagrees
        ] {
            let e = error(classify_probe(
                &head(206, &[("Content-Range", cr), ("Content-Length", cl)]),
                999,
            ));
            assert_eq!(e.class, FailureClass::Permanent, "{cr}");
            assert_eq!(e.http_status, Some(206), "{cr}");
        }
        let e = error(classify_probe(&head(206, &[]), 999));
        assert!(e.message.contains("Content-Range"), "{}", e.message);
    }

    #[test]
    fn refuses_a_multipart_answer_to_one_range() {
        let h = head(
            206,
            &[
                ("Content-Type", "multipart/byteranges; boundary=x"),
                ("Content-Length", "500"),
            ],
        );
        assert!(error(classify_probe(&h, 999)).message.contains("multipart"));
    }

    #[test]
    fn refuses_an_encoding_it_didnt_ask_for() {
        // No Accept-Encoding is sent, so a compressed body isn't the file's bytes.
        let h = head(
            200,
            &[("Content-Encoding", "gzip"), ("Content-Length", "300")],
        );
        assert!(error(classify_probe(&h, 999)).message.contains("gzip"));
        assert_eq!(
            classify_probe(&head(200, &[("Content-Encoding", "identity")]), 999),
            Ok(ProbeOutcome::SingleStream { length: None })
        );
    }

    #[test]
    fn classifies_error_statuses() {
        for (status, class) in [
            (401, FailureClass::Permanent),
            (407, FailureClass::Permanent),
            (404, FailureClass::Permanent),
            (429, FailureClass::Throttled),
            (503, FailureClass::Throttled),
            (500, FailureClass::Transient),
            (502, FailureClass::Transient),
        ] {
            let e = error(classify_probe(&head(status, &[]), 999));
            assert_eq!((e.class, e.http_status), (class, Some(status)), "{status}");
        }
    }

    #[test]
    fn a_later_segment_must_be_exactly_the_range_asked_for() {
        let ok = head(
            206,
            &[
                ("Content-Range", "bytes 1000-1999/5000"),
                ("Content-Length", "1000"),
            ],
        );
        assert_eq!(check_segment(&ok, 1000, 1999, 5000), Ok(1999));
        let short = head(
            206,
            &[
                ("Content-Range", "bytes 1000-1499/5000"),
                ("Content-Length", "500"),
            ],
        );
        assert_eq!(check_segment(&short, 1000, 1999, 5000), Ok(1499));
    }

    #[test]
    fn a_200_for_a_later_segment_means_ranges_were_lost() {
        assert_eq!(
            check_segment(&head(200, &[("Content-Length", "5000")]), 1000, 1999, 5000),
            Err(SegmentRejection::RangesIgnored)
        );
    }

    #[test]
    fn refuses_a_later_segment_that_doesnt_match() {
        for (cr, cl) in [
            ("bytes 0-999/5000", "1000"),     // wrong start
            ("bytes 1000-2999/5000", "2000"), // past the end asked for
            ("bytes 1000-1999/6000", "1000"), // a different total: the object changed
            ("bytes 1000-x/5000", "1000"),
            ("bytes 1000-1999/5000", "999"),
        ] {
            let r = check_segment(
                &head(206, &[("Content-Range", cr), ("Content-Length", cl)]),
                1000,
                1999,
                5000,
            );
            assert!(
                matches!(r, Err(SegmentRejection::Invalid(_))),
                "{cr}: {r:?}"
            );
        }
        let r = check_segment(&head(404, &[]), 1000, 1999, 5000);
        assert!(
            matches!(
                r,
                Err(SegmentRejection::Invalid(TransferError {
                    http_status: Some(404),
                    ..
                }))
            ),
            "{r:?}"
        );
    }
}
