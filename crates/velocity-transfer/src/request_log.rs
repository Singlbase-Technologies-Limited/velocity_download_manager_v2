//! The engine's log of its requests (#263): one line for each request when it ends, written to
//! the engine's stderr, which the app keeps as the engine's log (`velocity_child::log::ChildLog`,
//! redacted, the last 500 lines). The line says where the request went (scheme and host only),
//! which bytes it asked for, on which connection, how it ended, how many bytes came, and how long
//! it took. Requests that end the same way as the one before are counted, not written, so a
//! download in a retry loop can't push the rest of the log out.

use std::sync::Mutex;

use velocity_curl::{FetchError, RequestEnded, RequestHook};

use crate::failure::fetch_failure;

/// The loop's hook that writes each request's lines to stderr, the engine's log, as
/// `INFO …` or `WARN …`. It runs on the loop's thread and only formats and writes.
pub fn stderr_hook() -> RequestHook {
    let log = Mutex::new(RequestLog::default());
    RequestHook::new(move |ended| {
        let lines = log.lock().unwrap_or_else(|p| p.into_inner()).record(ended);
        for l in lines {
            eprintln!("{} {}", if l.warn { "WARN" } else { "INFO" }, l.text);
        }
    })
}

/// How many requests that end the same way are counted before a note says so, so a long run of
/// them still shows in the log as it happens.
const REPEATS_PER_NOTE: u32 = 50;

/// One line, at warn for a request that failed and at info for the rest.
#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    pub warn: bool,
    pub text: String,
}

/// What was last written, so the requests after it that end the same way are only counted.
#[derive(Default)]
pub struct RequestLog {
    /// The host and the outcome of the last line written, with the repeats counted since.
    last: Option<(String, String)>,
    repeats: u32,
}

impl RequestLog {
    /// The lines `ended` makes: none while it ends as the ones before it did.
    pub fn record(&mut self, ended: &RequestEnded) -> Vec<Line> {
        let key = (ended.origin.clone(), outcome_key(ended));
        let mut lines = Vec::new();
        if self.last.as_ref() == Some(&key) {
            self.repeats += 1;
            if self.repeats == REPEATS_PER_NOTE {
                lines.push(self.note());
            }
            return lines;
        }
        if self.repeats > 0 {
            lines.push(self.note());
        }
        self.last = Some(key);
        lines.push(line(ended));
        lines
    }

    /// Says how many requests to the last host ended as its last line did, and counts again.
    fn note(&mut self) -> Line {
        let origin = self.last.as_ref().map_or("", |(origin, _)| origin);
        let line = Line {
            warn: false,
            text: format!(
                "{origin}: {} more requests ended the same way",
                self.repeats
            ),
        };
        self.repeats = 0;
        line
    }
}

/// What makes two requests end "the same way": the status, or the kind of failure without its
/// changing detail (a time, a byte count).
fn outcome_key(ended: &RequestEnded) -> String {
    let failure = match &ended.error {
        None => String::new(),
        Some(FetchError::Curl { code, .. }) => format!("libcurl {code}"),
        Some(other) => format!("{:?}", std::mem::discriminant(other)),
    };
    format!("{:?} {failure}", ended.status)
}

fn line(ended: &RequestEnded) -> Line {
    let range = match ended.range {
        None => "the whole file".to_owned(),
        Some((start, None)) => format!("bytes {start}-"),
        Some((start, Some(end))) => format!("bytes {start}-{end}"),
    };
    let connection = if ended.new_connection {
        "new"
    } else {
        "reused"
    };
    let status = ended
        .status
        .map(|s| format!("HTTP {s}, "))
        .unwrap_or_default();
    let (bytes, ms) = (ended.bytes, ended.elapsed.as_millis());
    let (warn, how) = match &ended.error {
        None => (false, format!("{status}{bytes} bytes in {ms} ms")),
        Some(FetchError::Cancelled) => (
            false,
            format!(
                "{status}stopped by the engine (a pause, a split or a reset), after {bytes} \
                 bytes in {ms} ms"
            ),
        ),
        Some(e) => {
            let reason = fetch_failure(e.clone()).message;
            // With a status, the answer came and failed later; without one, it never came.
            let failed = if status.is_empty() {
                "failed"
            } else {
                "then failed"
            };
            (
                true,
                format!("{status}{failed}: {reason}, after {bytes} bytes in {ms} ms"),
            )
        }
    };
    Line {
        warn,
        text: format!("{} {range}, {connection} connection: {how}", ended.origin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use velocity_curl::{CurlFailure, FetchError};

    fn ended(
        range: Option<(u64, Option<u64>)>,
        status: Option<u32>,
        error: Option<FetchError>,
    ) -> RequestEnded {
        RequestEnded {
            origin: "https://cdn.example.com".to_owned(),
            range,
            new_connection: true,
            status,
            bytes: 4_194_304,
            elapsed: Duration::from_millis(1250),
            error,
        }
    }

    fn cut() -> FetchError {
        FetchError::Curl {
            kind: CurlFailure::Interrupted,
            code: 56,
            message: "Failure when receiving data from the peer: schannel: server closed abruptly"
                .to_owned(),
        }
    }

    fn texts(lines: Vec<Line>) -> Vec<String> {
        lines.into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn a_request_that_completed_is_one_info_line_with_what_it_asked_for_and_got() {
        let mut log = RequestLog::default();
        assert_eq!(
            log.record(&ended(Some((0, Some(4_194_303))), Some(206), None)),
            [Line {
                warn: false,
                text: "https://cdn.example.com bytes 0-4194303, new connection: HTTP 206, \
                       4194304 bytes in 1250 ms"
                    .to_owned()
            }]
        );
        let mut whole = ended(None, Some(200), None);
        whole.new_connection = false;
        assert_eq!(
            texts(RequestLog::default().record(&whole)),
            [
                "https://cdn.example.com the whole file, reused connection: HTTP 200, 4194304 \
              bytes in 1250 ms"
            ]
        );
        assert_eq!(
            texts(RequestLog::default().record(&ended(Some((100, None)), Some(206), None))),
            [
                "https://cdn.example.com bytes 100-, new connection: HTTP 206, 4194304 bytes in \
              1250 ms"
            ]
        );
    }

    #[test]
    fn a_request_that_failed_is_a_warning_with_libcurl_s_detail() {
        let mut log = RequestLog::default();
        let lines = log.record(&ended(Some((0, None)), Some(206), Some(cut())));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].warn);
        assert_eq!(
            lines[0].text,
            "https://cdn.example.com bytes 0-, new connection: HTTP 206, then failed: The \
             connection was interrupted. (libcurl error 56: Failure when receiving data from \
             the peer: schannel: server closed abruptly), after 4194304 bytes in 1250 ms"
        );
    }

    #[test]
    fn a_request_that_got_no_answer_says_it_failed() {
        let mut log = RequestLog::default();
        let text = &log.record(&ended(Some((0, Some(4_194_303))), None, Some(cut())))[0].text;
        assert!(
            text.starts_with(
                "https://cdn.example.com bytes 0-4194303, new connection: failed: The connection"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_request_the_engine_stopped_is_info_and_says_so() {
        let mut log = RequestLog::default();
        let lines = log.record(&ended(
            Some((0, None)),
            Some(206),
            Some(FetchError::Cancelled),
        ));
        assert_eq!(
            lines,
            [Line {
                warn: false,
                text: "https://cdn.example.com bytes 0-, new connection: HTTP 206, stopped by \
                       the engine (a pause, a split or a reset), after 4194304 bytes in 1250 ms"
                    .to_owned()
            }]
        );
    }

    #[test]
    fn requests_that_end_the_same_way_are_counted_not_written() {
        let mut log = RequestLog::default();
        let mut lines = Vec::new();
        for _ in 0..100 {
            lines.extend(log.record(&ended(Some((0, None)), None, Some(cut()))));
        }
        lines.extend(log.record(&ended(Some((0, Some(9))), Some(206), None)));
        // The first failure, a note every 50 repeats, the rest when it changes, then the change.
        let lines = texts(lines);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[0].contains("libcurl error 56"), "{lines:?}");
        assert_eq!(
            lines[1],
            "https://cdn.example.com: 50 more requests ended the same way"
        );
        assert_eq!(
            lines[2],
            "https://cdn.example.com: 49 more requests ended the same way"
        );
        assert!(lines[3].contains("HTTP 206"), "{lines:?}");
    }

    #[test]
    fn a_repeat_note_says_how_many_more_ended_the_same_way() {
        let mut log = RequestLog::default();
        let mut lines = Vec::new();
        for _ in 0..4 {
            lines.extend(log.record(&ended(Some((0, None)), None, Some(cut()))));
        }
        lines.extend(log.record(&ended(Some((0, Some(9))), Some(206), None)));
        let lines = texts(lines);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            lines[1],
            "https://cdn.example.com: 3 more requests ended the same way"
        );
        assert!(lines[2].contains("HTTP 206"));
    }

    #[test]
    fn a_different_host_or_outcome_is_written_again() {
        let mut log = RequestLog::default();
        let mut other = ended(Some((0, None)), Some(206), None);
        other.origin = "https://mirror.example.org".to_owned();
        let mut lines = log.record(&ended(Some((0, None)), Some(206), None));
        lines.extend(log.record(&other));
        lines.extend(log.record(&ended(Some((0, None)), Some(404), None)));
        assert_eq!(lines.len(), 3, "{lines:?}");
    }
}
