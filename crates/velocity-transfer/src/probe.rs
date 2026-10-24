//! Reading a server's answers (research report section 5.1). The first segment is also the probe:
//! its response says whether ranges work, without a HEAD and without a second full transfer.
//! Later segments must answer exactly what was asked, or they are aborted before any byte is
//! written. Pure functions over a response head, so every rule is unit tested.

use velocity_core::domain::{FailureClass, TransferError};
use velocity_curl::ResponseHead;

use crate::validator::Representation;

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
    /// The answer contradicts the request (a wrong or malformed `Content-Range`, a length that
    /// disagrees with it, a multipart body, an unrequested encoding). Worth one more request; a
    /// second anomaly on the same download ends ranges for it (report section 5.1, step 7).
    Anomaly(TransferError),
    /// A different total, a `416` for a range inside the old total, or another ETag or strong date
    /// than the first answer gave, or none where it gave one (#45): the file on the server isn't
    /// the one this download started (report section 5.3). Its old bytes are never mixed with new
    /// ones.
    Changed(TransferError),
    /// Another `Content-Encoding` than the first answer's (#45): ranges count encoded bytes, so
    /// they can't go on; the download starts over as one stream.
    EncodingChanged(TransferError),
    /// An error status, or an answer that can't be used. The retry policy decides from its class
    /// (#48).
    Invalid(TransferError),
}

/// What a request for the rest of a file of unknown size, `Range: bytes=start-`, returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Continuation {
    /// A `206` from `start` to `end`; the total, if the server gave it.
    More { end: u64, total: Option<u64> },
    /// A `416`: there is nothing at `start`, so the file ends there.
    End,
}

/// Classifies the answer to `Range: bytes=start-` after a probe that gave no total.
pub fn check_continuation(
    head: &ResponseHead,
    start: u64,
    file: &Representation,
) -> Result<Continuation, SegmentRejection> {
    later_status_error(head).map_err(SegmentRejection::Invalid)?;
    if head.status == 416 {
        // Only a 416 that agrees the file ends at `start` (or says nothing) ends it.
        return match head.header("content-range").map(str::trim) {
            None => Ok(Continuation::End),
            Some(cr) if cr == format!("bytes */{start}") => Ok(Continuation::End),
            Some(cr) => Err(SegmentRejection::Anomaly(dishonest(
                head,
                &format!("a 416 with Content-Range {cr} for a request from byte {start}"),
            ))),
        };
    }
    one_range(head).map_err(SegmentRejection::Anomaly)?;
    if matches!(head.status, 200 | 206) {
        same_file(head, file)?;
    }
    match head.status {
        200 => Err(SegmentRejection::RangesIgnored),
        206 => {
            let (got_start, end, total) = content_range(head).map_err(SegmentRejection::Anomaly)?;
            if got_start != start || total.is_some_and(|t| end >= t) {
                return Err(SegmentRejection::Anomaly(dishonest(
                    head,
                    &format!(
                        "Content-Range {got_start}-{end} doesn't answer a request from {start}"
                    ),
                )));
            }
            check_length(head, got_start, end).map_err(SegmentRejection::Anomaly)?;
            Ok(Continuation::More { end, total })
        }
        status => Err(SegmentRejection::Invalid(TransferError::new(
            FailureClass::Permanent,
            Some(status),
            &format!("unexpected HTTP status {status} for a range"),
        ))),
    }
}

/// Whether the answer is `multipart/byteranges`, which a single range should never get.
pub fn is_multipart(head: &ResponseHead) -> bool {
    head.header("content-type").is_some_and(|t| {
        t.trim_start()
            .to_ascii_lowercase()
            .starts_with("multipart/byteranges")
    })
}

/// Classifies the probe's final response to `Range: bytes=0-requested_end`.
pub fn classify_probe(
    head: &ResponseHead,
    requested_end: u64,
) -> Result<ProbeOutcome, TransferError> {
    status_error(head)?;
    one_range(head)?;
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
    file: &Representation,
) -> Result<u64, SegmentRejection> {
    later_status_error(head).map_err(SegmentRejection::Invalid)?;
    one_range(head).map_err(SegmentRejection::Anomaly)?;
    if matches!(head.status, 200 | 206) {
        // A 200 to If-Range with other validators is the new file (RFC 9110 §13.1.5); with the
        // same ones, the server stopped honouring ranges.
        same_file(head, file)?;
    }
    if head.status == 200 {
        return Err(SegmentRejection::RangesIgnored);
    }
    if head.status == 416 {
        return Err(SegmentRejection::Changed(TransferError::new(
            FailureClass::Permanent,
            Some(416),
            &format!("bytes {start}-{end} are no longer in the file: it is shorter than {total}"),
        )));
    }
    if head.status != 206 {
        return Err(SegmentRejection::Invalid(TransferError::new(
            FailureClass::Permanent,
            Some(head.status),
            &format!("unexpected HTTP status {} for a range", head.status),
        )));
    }
    let (got_start, got_end, got_total) = content_range(head).map_err(SegmentRejection::Anomaly)?;
    if got_start != start || got_end > end || got_end < got_start {
        return Err(SegmentRejection::Anomaly(dishonest(
            head,
            &format!("Content-Range {got_start}-{got_end} doesn't answer the range {start}-{end}"),
        )));
    }
    if got_total != Some(total) {
        // A different total means a different object (report section 5.3, #45).
        return Err(SegmentRejection::Changed(TransferError::new(
            FailureClass::Permanent,
            Some(206),
            &format!("the file's total changed from {total} to {got_total:?}"),
        )));
    }
    check_length(head, got_start, got_end).map_err(SegmentRejection::Anomaly)?;
    Ok(got_end)
}

/// Refuses an answer about another file than the first answer was, or in another encoding (report
/// section 5.3, #45).
fn same_file(head: &ResponseHead, file: &Representation) -> Result<(), SegmentRejection> {
    let error = |why: String| TransferError::new(FailureClass::Permanent, Some(head.status), &why);
    file.check(head)
        .map_err(|why| SegmentRejection::Changed(error(why)))?;
    file.same_encoding(head)
        .map_err(|why| SegmentRejection::EncodingChanged(error(why)))
}

/// Error statuses, classified with their messages (`failure::status_failure`, #48).
fn status_error(head: &ResponseHead) -> Result<(), TransferError> {
    match crate::failure::status_failure(head.status) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// [`status_error`] for an answer after the first, where a `403` means the link expired (#45).
fn later_status_error(head: &ResponseHead) -> Result<(), TransferError> {
    match crate::failure::later_status_failure(head.status) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// A single range never gets a `multipart/byteranges` answer.
fn one_range(head: &ResponseHead) -> Result<(), TransferError> {
    if is_multipart(head) {
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
            http2: false,
            url: "http://example.com/f".to_owned(),
        }
    }

    /// A file the first answer gave no validator for.
    fn none() -> Representation {
        Representation::default()
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
    fn keeps_an_encoding_it_didnt_ask_for() {
        // PO decision on #45, replacing #41's refusal: no Accept-Encoding is sent, and an encoding
        // the server applies anyway is kept, its bytes stored as sent (a .tar.gz served as gzip).
        let h = head(
            200,
            &[("Content-Encoding", "gzip"), ("Content-Length", "300")],
        );
        assert_eq!(
            classify_probe(&h, 999),
            Ok(ProbeOutcome::SingleStream { length: Some(300) })
        );
        let ranged = head(
            206,
            &[
                ("Content-Encoding", "gzip"),
                ("Content-Range", "bytes 0-999/5000"),
                ("Content-Length", "1000"),
            ],
        );
        assert_eq!(
            classify_probe(&ranged, 999),
            Ok(ProbeOutcome::RangesOk {
                received_end: 999,
                total: 5000
            })
        );
    }

    #[test]
    fn a_later_answer_with_another_encoding_is_refused_as_such() {
        // Report 5.4: ranges count encoded bytes, so they stop when the encoding changes.
        let gzip = Representation::of(&head(206, &[("Content-Encoding", "gzip")]));
        let segment = |encoding: Option<&str>, file: &Representation| {
            let mut headers = vec![
                ("Content-Range", "bytes 1000-1999/5000"),
                ("Content-Length", "1000"),
            ];
            headers.extend(encoding.map(|e| ("Content-Encoding", e)));
            check_segment(&head(206, &headers), 1000, 1999, 5000, file)
        };
        assert_eq!(segment(Some("gzip"), &gzip), Ok(1999));
        for (encoding, file) in [(None, &gzip), (Some("br"), &gzip), (Some("gzip"), &none())] {
            let r = segment(encoding, file);
            assert!(
                matches!(r, Err(SegmentRejection::EncodingChanged(_))),
                "{encoding:?}: {r:?}"
            );
        }
        let more = head(206, &[("Content-Range", "bytes 1000-4999/*")]);
        let r = check_continuation(&more, 1000, &gzip);
        assert!(
            matches!(r, Err(SegmentRejection::EncodingChanged(_))),
            "{r:?}"
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
        assert_eq!(check_segment(&ok, 1000, 1999, 5000, &none()), Ok(1999));
        let short = head(
            206,
            &[
                ("Content-Range", "bytes 1000-1499/5000"),
                ("Content-Length", "500"),
            ],
        );
        assert_eq!(check_segment(&short, 1000, 1999, 5000, &none()), Ok(1499));
    }

    #[test]
    fn a_200_for_a_later_segment_means_ranges_were_lost() {
        assert_eq!(
            check_segment(
                &head(200, &[("Content-Length", "5000")]),
                1000,
                1999,
                5000,
                &none()
            ),
            Err(SegmentRejection::RangesIgnored)
        );
    }

    #[test]
    fn a_later_segment_that_contradicts_its_request_is_an_anomaly() {
        // Report 5.1, step 7: worth one more try; a second anomaly ends ranges for the download.
        for (cr, cl) in [
            ("bytes 0-999/5000", "1000"),     // wrong start
            ("bytes 1000-2999/5000", "2000"), // past the end asked for
            ("bytes 1000-x/5000", "1000"),
            ("bytes 1000-1999/5000", "999"),
        ] {
            let r = check_segment(
                &head(206, &[("Content-Range", cr), ("Content-Length", cl)]),
                1000,
                1999,
                5000,
                &none(),
            );
            assert!(
                matches!(r, Err(SegmentRejection::Anomaly(_))),
                "{cr}: {r:?}"
            );
        }
        let multipart = head(206, &[("Content-Type", "multipart/byteranges; boundary=x")]);
        assert!(matches!(
            check_segment(&multipart, 1000, 1999, 5000, &none()),
            Err(SegmentRejection::Anomaly(_))
        ));
    }

    #[test]
    fn a_403_after_the_first_answer_means_the_link_expired() {
        // Report 5.3: a signed link that worked for the first answer and is refused later has
        // expired; it needs a fresh link from the browser (PO decision on #45: a message only).
        let expired = |r: Result<_, SegmentRejection>| match r {
            Err(SegmentRejection::Invalid(e)) => {
                assert_eq!(
                    (e.class, e.http_status),
                    (FailureClass::Permanent, Some(403))
                );
                e.message
            }
            other => panic!("expected a refusal, got {other:?}"),
        };
        let message = "The download link expired (403). Get a fresh link from the browser.";
        let segment = check_segment(&head(403, &[]), 1000, 1999, 5000, &none());
        assert_eq!(expired(segment.map(|_| ())), message);
        let continuation = check_continuation(&head(403, &[]), 1000, &none());
        assert_eq!(expired(continuation.map(|_| ())), message);
        // On the first answer, a 403 may be a real refusal.
        assert!(error(classify_probe(&head(403, &[]), 999))
            .message
            .contains("Access was denied"));
    }

    #[test]
    fn a_later_segment_with_another_total_or_an_error_fails() {
        // A different total means a different object (report section 5.3): `Changed`, which a
        // resumed download answers by starting over (#43). An error status is `Invalid`.
        let r = check_segment(
            &head(
                206,
                &[
                    ("Content-Range", "bytes 1000-1999/6000"),
                    ("Content-Length", "1000"),
                ],
            ),
            1000,
            1999,
            5000,
            &none(),
        );
        assert!(matches!(r, Err(SegmentRejection::Changed(_))), "{r:?}");
        let shorter = check_segment(
            &head(416, &[("Content-Range", "bytes */1200")]),
            1000,
            1999,
            5000,
            &none(),
        );
        assert!(
            matches!(shorter, Err(SegmentRejection::Changed(_))),
            "{shorter:?}"
        );
        let r = check_segment(&head(404, &[]), 1000, 1999, 5000, &none());
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

    #[test]
    fn a_continuation_without_a_total_goes_on_until_a_416() {
        let more = head(206, &[("Content-Range", "bytes 1000-4999/*")]);
        assert_eq!(
            check_continuation(&more, 1000, &none()),
            Ok(Continuation::More {
                end: 4999,
                total: None
            })
        );
        let known = head(206, &[("Content-Range", "bytes 1000-4999/5000")]);
        assert_eq!(
            check_continuation(&known, 1000, &none()),
            Ok(Continuation::More {
                end: 4999,
                total: Some(5000)
            })
        );
        // Past the last byte: the file ends where the request started.
        for h in [
            head(416, &[("Content-Range", "bytes */1000")]),
            head(416, &[]),
        ] {
            assert_eq!(check_continuation(&h, 1000, &none()), Ok(Continuation::End));
        }
    }

    #[test]
    fn a_continuation_that_doesnt_answer_its_start_is_refused() {
        assert!(matches!(
            check_continuation(
                &head(206, &[("Content-Range", "bytes 0-999/*")]),
                1000,
                &none()
            ),
            Err(SegmentRejection::Anomaly(_))
        ));
        // A 416 that says the file is longer than where the request started contradicts itself.
        assert!(matches!(
            check_continuation(
                &head(416, &[("Content-Range", "bytes */9000")]),
                1000,
                &none()
            ),
            Err(SegmentRejection::Anomaly(_))
        ));
        assert_eq!(
            check_continuation(&head(200, &[]), 1000, &none()),
            Err(SegmentRejection::RangesIgnored)
        );
        assert!(matches!(
            check_continuation(&head(503, &[]), 1000, &none()),
            Err(SegmentRejection::Invalid(_))
        ));
    }

    /// A file whose first answer gave the strong ETag `"v1"`.
    fn v1() -> Representation {
        Representation::of(&head(206, &[("ETag", "\"v1\"")]))
    }

    #[test]
    fn a_later_segment_of_another_file_is_changed() {
        let range = [
            ("Content-Range", "bytes 1000-1999/5000"),
            ("Content-Length", "1000"),
        ];
        let with = |etag: Option<&str>| {
            let mut headers = range.to_vec();
            headers.extend(etag.map(|e| ("ETag", e)));
            check_segment(&head(206, &headers), 1000, 1999, 5000, &v1())
        };
        assert_eq!(with(Some("\"v1\"")), Ok(1999));
        for etag in [Some("\"v2\""), None] {
            let r = with(etag);
            assert!(
                matches!(r, Err(SegmentRejection::Changed(_))),
                "{etag:?}: {r:?}"
            );
        }
    }

    #[test]
    fn a_200_to_if_range_is_changed_unless_it_is_the_same_file() {
        // RFC 9110 §13.1.5: a server whose file no longer matches If-Range answers 200 with the new
        // one. A 200 with the same validator is a server that stopped honouring ranges.
        let full = |etag: &str| head(200, &[("Content-Length", "5000"), ("ETag", etag)]);
        let r = check_segment(&full("\"v2\""), 1000, 1999, 5000, &v1());
        assert!(matches!(r, Err(SegmentRejection::Changed(_))), "{r:?}");
        assert_eq!(
            check_segment(&full("\"v1\""), 1000, 1999, 5000, &v1()),
            Err(SegmentRejection::RangesIgnored)
        );
        let r = check_continuation(&full("\"v2\""), 1000, &v1());
        assert!(matches!(r, Err(SegmentRejection::Changed(_))), "{r:?}");
    }

    #[test]
    fn a_continuation_of_another_file_is_changed() {
        let more = |etag: &str| {
            head(
                206,
                &[("Content-Range", "bytes 1000-4999/*"), ("ETag", etag)],
            )
        };
        assert!(check_continuation(&more("\"v1\""), 1000, &v1()).is_ok());
        let r = check_continuation(&more("\"v2\""), 1000, &v1());
        assert!(matches!(r, Err(SegmentRejection::Changed(_))), "{r:?}");
    }

    #[test]
    fn a_multipart_answer_is_recognised_as_one() {
        let h = head(206, &[("Content-Type", "Multipart/ByteRanges; boundary=x")]);
        assert!(is_multipart(&h));
        assert!(!is_multipart(&head(
            206,
            &[("Content-Type", "application/zip")]
        )));
    }
}
