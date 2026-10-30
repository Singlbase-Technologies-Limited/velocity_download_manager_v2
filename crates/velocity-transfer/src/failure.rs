//! How a failure is classified and described (#48): HTTP statuses, libcurl failures and local disk
//! errors become a [`FailureClass`] with a message a user can act on, in V1's words where V1 had
//! them (`crates/velocity-core/src/aria2/codes.rs` at `e9d717a8`). The retry policy decides from
//! the class alone.

use std::io;
use std::time::{Duration, SystemTime};

use velocity_core::domain::{Confirm, FailureClass, HostKeyCheck, TransferError};
use velocity_core::transfer::HashAlgorithm;
use velocity_curl::{AddressProblem, CurlFailure, FetchError, HostKeyProblem, ResponseHead};

/// The failure an error status stands for, or `None` for a status that isn't one here (`416` is
/// read by the probe and the segment checks).
pub(crate) fn status_failure(status: u16) -> Option<TransferError> {
    use FailureClass::{Permanent, Throttled, Transient};
    let (class, message) = match status {
        401 => (
            Permanent,
            "The server requires authentication (401).".to_owned(),
        ),
        407 => (
            Permanent,
            "The proxy requires authentication (407).".to_owned(),
        ),
        403 => (
            Permanent,
            "Access was denied by the server (403). The link may have expired.".to_owned(),
        ),
        404 => (
            Permanent,
            "The file was not found on the server (404). The link may have expired.".to_owned(),
        ),
        410 => (
            Permanent,
            "The file is no longer available (410).".to_owned(),
        ),
        // RFC 9110 §15.5.9: the server closed a connection it thought idle; the request may be
        // repeated.
        408 => (
            Transient,
            "The server timed out waiting for the request (408).".to_owned(),
        ),
        429 => (
            Throttled,
            "The server is rate limiting requests (429).".to_owned(),
        ),
        503 => (
            Throttled,
            "The server is temporarily unavailable or overloaded (503).".to_owned(),
        ),
        416 => return None,
        400..=499 => (
            Permanent,
            format!("The server refused the request (HTTP {status})."),
        ),
        500..=599 => (
            Transient,
            format!("The server reported an error (HTTP {status})."),
        ),
        _ => return None,
    };
    Some(TransferError::new(class, Some(status), &message))
}

/// [`status_failure`] for an answer after a download's first: a `403` there means a signed link
/// that worked has expired, which needs a fresh link from the browser (report section 5.3, #45).
pub(crate) fn later_status_failure(status: u16) -> Option<TransferError> {
    if status == 403 {
        return Some(TransferError::new(
            FailureClass::Permanent,
            Some(403),
            "The download link expired (403). Get a fresh link from the browser.",
        ));
    }
    status_failure(status)
}

/// A whole file that doesn't match its checksum (#46, report section 5.3). Worth a retry by hand,
/// which downloads it again, so not `Permanent`; the engine doesn't restart it on its own, so the
/// part file stays for inspection.
pub(crate) fn checksum_mismatch(algorithm: HashAlgorithm, got: &str) -> TransferError {
    let name = match algorithm {
        HashAlgorithm::Md5 => "MD5",
        HashAlgorithm::Sha1 => "SHA-1",
        HashAlgorithm::Sha224 => "SHA-224",
        HashAlgorithm::Sha256 => "SHA-256",
        HashAlgorithm::Sha384 => "SHA-384",
        HashAlgorithm::Sha512 => "SHA-512",
    };
    TransferError::new(
        FailureClass::Transient,
        None,
        &format!(
            "The downloaded file doesn't match its {name} checksum (the file's is {got}). It was \
             kept as a .vdmpart file, not renamed; retrying downloads it again."
        ),
    )
}

/// A piece that was still bad after it was fetched again `refetches` times (#46, PO decision): the
/// server or a mirror sends bad data. Retrying by hand downloads the whole file again.
pub(crate) fn piece_still_bad(index: usize, count: usize, refetches: u32) -> TransferError {
    TransferError::new(
        FailureClass::Transient,
        None,
        &format!(
            "The downloaded file's piece {} of {count} doesn't match its trusted hash, even after \
             fetching it again {refetches} times: the server sends bad data. It was kept as a \
             .vdmpart file, not renamed; retrying downloads it again.",
            index + 1
        ),
    )
}

/// A piece that was bad again after the whole file was downloaded again, from a server that
/// can't send part of it (#46, PO decision).
pub(crate) fn piece_bad_again(index: usize, count: usize) -> TransferError {
    TransferError::new(
        FailureClass::Transient,
        None,
        &format!(
            "The downloaded file's piece {} of {count} doesn't match its trusted hash, in two \
             downloads of the whole file: the server sends bad data. It was kept as a .vdmpart \
             file, not renamed; retrying downloads it again.",
            index + 1
        ),
    )
}

/// A piece list that can't describe the file: its pieces don't add up to the file's size. The
/// list or the server is wrong, so retrying won't help.
pub(crate) fn piece_list_misfit(count: usize, length: u64, total: u64) -> TransferError {
    TransferError::new(
        FailureClass::Permanent,
        None,
        &format!(
            "The file's trusted piece list doesn't fit it: {count} pieces of {length} bytes \
             can't make up its {total} bytes."
        ),
    )
}

/// A libcurl-side failure.
pub(crate) fn fetch_failure(e: FetchError) -> TransferError {
    match e {
        FetchError::Aborted(reason) => TransferError::new(FailureClass::Permanent, None, &reason),
        FetchError::Cancelled => {
            TransferError::new(FailureClass::Permanent, None, "the transfer was cancelled")
        }
        FetchError::Curl {
            kind,
            code,
            message,
        } => {
            let (class, words) = curl_failure(kind);
            TransferError::new(
                class,
                None,
                &format!("{words} (libcurl error {code}: {message})"),
            )
        }
        FetchError::RedirectRefused { kind, message } => {
            let (class, words) = curl_failure(kind);
            TransferError::new(class, None, &format!("{words} ({message})"))
        }
        FetchError::Sftp { kind, message } => {
            let (class, words) = curl_failure(kind);
            TransferError::new(class, None, &format!("{words} (SFTP: {message})"))
        }
        FetchError::HostKey(problem) => host_key_failure(problem),
        FetchError::InsecureRedirect { host } => TransferError::new(
            FailureClass::Permanent,
            None,
            &format!(
                "The server redirected to an insecure http:// address on {host}. Continue over \
                 http only if you trust the network you're on."
            ),
        )
        .with_confirm(Confirm::InsecureRedirect { host }),
        FetchError::LocalAddress(AddressProblem { host, address }) => TransferError::new(
            FailureClass::Permanent,
            None,
            &format!(
                "This link from the browser leads to {address}, on this computer or your local \
                 network. Download it only if you expected that."
            ),
        )
        .with_confirm(Confirm::LocalAddress { host, address }),
        FetchError::LoopStopped => {
            TransferError::new(FailureClass::Transient, None, "the libcurl loop stopped")
        }
    }
}

/// An SFTP server's untrusted host key (#53): permanent, so the engine never retries it on its
/// own, with the key for the app to show. An unknown key waits for the user to confirm its
/// fingerprint; a changed one is refused for good.
fn host_key_failure(problem: HostKeyProblem) -> TransferError {
    let HostKeyProblem {
        host,
        port,
        fingerprint,
        pinned,
    } = problem;
    let message = match &pinned {
        None => format!(
            "The server's identity isn't confirmed yet. Check that its host key fingerprint is \
             {fingerprint}, then trust it to continue."
        ),
        Some(before) => format!(
            "The server's host key changed (it was {before}, it is now {fingerprint}). This could \
             be an attack, so the connection was refused."
        ),
    };
    TransferError::new(FailureClass::Permanent, None, &message).with_host_key(HostKeyCheck {
        host,
        port,
        fingerprint,
        pinned,
    })
}

/// A libcurl failure's class and message. Network trouble is worth retrying; a certificate that
/// isn't trusted, a refused protocol or a broken answer is not (TLS verification is never turned
/// off, #69).
fn curl_failure(kind: CurlFailure) -> (FailureClass, &'static str) {
    use FailureClass::{Permanent, Transient};
    match kind {
        CurlFailure::Resolve => (Transient, "The server name could not be resolved."),
        CurlFailure::Connect => (Transient, "Couldn't connect to the server."),
        CurlFailure::Timeout => (Transient, "The connection timed out."),
        CurlFailure::Interrupted => (Transient, "The connection was interrupted."),
        CurlFailure::TlsHandshake => (Transient, "The secure connection couldn't be set up."),
        CurlFailure::Certificate => (
            Permanent,
            "The server's certificate isn't trusted, so the connection was refused.",
        ),
        CurlFailure::TooManyRedirects => (Permanent, "The server redirected too many times."),
        CurlFailure::Refused => (
            Permanent,
            "The address, or a redirect, uses a protocol that is not allowed.",
        ),
        CurlFailure::InsecureRedirect => (
            Permanent,
            "The server redirected to an insecure http:// address, which isn't allowed.",
        ),
        CurlFailure::Protocol => (Permanent, "The server sent an unexpected response."),
        CurlFailure::NotFound => (Permanent, "The file wasn't found on the server."),
        CurlFailure::AccessDenied => (
            Permanent,
            "The server refused the login, or access to the file.",
        ),
        CurlFailure::EncryptionRefused => (
            Permanent,
            "The server can't encrypt the connection, and this download requires encryption.",
        ),
        CurlFailure::Local => (Permanent, "A local error stopped the transfer."),
        CurlFailure::Other => (Transient, "A network problem occurred."),
    }
}

/// How long a `429` or `503` asks the client to wait (RFC 9110 §10.2.3): delay-seconds or an
/// HTTP date, measured from `now`. A date in the past is no wait. Anything else is ignored.
pub(crate) fn retry_after(head: &ResponseHead, now: SystemTime) -> Option<Duration> {
    let value = head.header("retry-after")?.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        // More digits than a u64 holds is a wait longer than any cap.
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// The message for a local disk error a user can fix, such as a full disk.
pub(crate) fn disk_message(e: &io::Error) -> Option<&'static str> {
    match e.kind() {
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => {
            Some("There is not enough disk space.")
        }
        _ => None,
    }
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

    fn curl(kind: CurlFailure) -> TransferError {
        fetch_failure(FetchError::Curl {
            kind,
            code: 99,
            message: "libcurl's own words".to_owned(),
        })
    }

    #[test]
    fn error_statuses_have_a_class_and_a_message() {
        for (status, class, words) in [
            (401, FailureClass::Permanent, "requires authentication"),
            (
                407,
                FailureClass::Permanent,
                "proxy requires authentication",
            ),
            (403, FailureClass::Permanent, "denied"),
            (404, FailureClass::Permanent, "not found"),
            (410, FailureClass::Permanent, "no longer available"),
            (400, FailureClass::Permanent, "HTTP 400"),
            // RFC 9110 §15.5.9: the server closed an idle connection; the request may be repeated.
            (408, FailureClass::Transient, "timed out"),
            (429, FailureClass::Throttled, "rate limiting"),
            (503, FailureClass::Throttled, "temporarily unavailable"),
            (500, FailureClass::Transient, "HTTP 500"),
            (502, FailureClass::Transient, "HTTP 502"),
        ] {
            let e = status_failure(status).unwrap_or_else(|| panic!("{status} is an error"));
            assert_eq!((e.class, e.http_status), (class, Some(status)), "{status}");
            assert!(e.message.contains(words), "{status}: {}", e.message);
        }
        for status in [200, 206, 301, 416] {
            assert_eq!(status_failure(status), None, "{status}");
        }
    }

    #[test]
    fn network_failures_are_transient_and_trust_failures_are_not() {
        for (kind, class, words) in [
            (
                CurlFailure::Resolve,
                FailureClass::Transient,
                "name could not be resolved",
            ),
            (CurlFailure::Connect, FailureClass::Transient, "connect"),
            (CurlFailure::Timeout, FailureClass::Transient, "timed out"),
            (
                CurlFailure::Interrupted,
                FailureClass::Transient,
                "interrupted",
            ),
            (
                CurlFailure::TlsHandshake,
                FailureClass::Transient,
                "secure connection",
            ),
            (
                CurlFailure::Other,
                FailureClass::Transient,
                "network problem",
            ),
            (
                CurlFailure::Certificate,
                FailureClass::Permanent,
                "certificate",
            ),
            (
                CurlFailure::TooManyRedirects,
                FailureClass::Permanent,
                "redirected too many times",
            ),
            (CurlFailure::Refused, FailureClass::Permanent, "not allowed"),
            (
                CurlFailure::Protocol,
                FailureClass::Permanent,
                "unexpected response",
            ),
            (CurlFailure::Local, FailureClass::Permanent, "local"),
            (
                CurlFailure::InsecureRedirect,
                FailureClass::Permanent,
                "insecure http://",
            ),
            (
                CurlFailure::NotFound,
                FailureClass::Permanent,
                "wasn't found",
            ),
            (
                CurlFailure::AccessDenied,
                FailureClass::Permanent,
                "refused the login",
            ),
            (
                CurlFailure::EncryptionRefused,
                FailureClass::Permanent,
                "can't encrypt",
            ),
        ] {
            let e = curl(kind);
            assert_eq!(e.class, class, "{kind:?}");
            assert!(e.message.contains(words), "{kind:?}: {}", e.message);
            // libcurl's code and words stay in the message, for diagnostics.
            assert!(
                e.message.contains("libcurl error 99"),
                "{kind:?}: {}",
                e.message
            );
        }
    }

    #[test]
    fn an_untrusted_host_key_is_permanent_and_carries_the_key() {
        let unknown = fetch_failure(FetchError::HostKey(HostKeyProblem {
            host: "files.example.org".to_owned(),
            port: 22,
            fingerprint: "SHA256:new".to_owned(),
            pinned: None,
        }));
        assert_eq!(unknown.class, FailureClass::Permanent);
        assert!(!unknown.retryable);
        assert!(
            unknown.message.contains("SHA256:new"),
            "{}",
            unknown.message
        );
        assert!(unknown.message.contains("trust it"), "{}", unknown.message);
        let check = unknown.host_key.expect("the key for the app to show");
        assert_eq!(
            (check.host.as_str(), check.port, check.fingerprint.as_str()),
            ("files.example.org", 22, "SHA256:new")
        );
        assert_eq!(check.pinned, None);

        let changed = fetch_failure(FetchError::HostKey(HostKeyProblem {
            host: "files.example.org".to_owned(),
            port: 22,
            fingerprint: "SHA256:new".to_owned(),
            pinned: Some("SHA256:old".to_owned()),
        }));
        assert_eq!(changed.class, FailureClass::Permanent);
        assert!(changed.message.contains("changed"), "{}", changed.message);
        assert!(
            changed.message.contains("SHA256:old"),
            "{}",
            changed.message
        );
        assert_eq!(
            changed.host_key.and_then(|c| c.pinned).as_deref(),
            Some("SHA256:old")
        );
    }

    #[test]
    fn an_sftp_failure_keeps_its_kind_and_its_own_words() {
        let e = fetch_failure(FetchError::Sftp {
            kind: CurlFailure::NotFound,
            message: "no such file".to_owned(),
        });
        assert_eq!(e.class, FailureClass::Permanent);
        assert!(e.message.contains("wasn't found"), "{}", e.message);
        assert!(e.message.contains("SFTP: no such file"), "{}", e.message);
        assert!(e.host_key.is_none());
    }

    #[test]
    fn a_refused_redirect_is_permanent_and_says_why() {
        let e = fetch_failure(FetchError::RedirectRefused {
            kind: CurlFailure::InsecureRedirect,
            message: "a redirect from https:// to http://, which isn't allowed".to_owned(),
        });
        assert_eq!((e.class, e.http_status), (FailureClass::Permanent, None));
        assert!(e.message.contains("insecure http://"), "{}", e.message);
        assert!(
            e.message.contains("from https:// to http://"),
            "{}",
            e.message
        );
        assert!(!e.message.contains("libcurl error"), "{}", e.message);
    }

    #[test]
    fn an_unconfirmed_redirect_to_http_waits_for_the_user() {
        let e = fetch_failure(FetchError::InsecureRedirect {
            host: "mirror.example.com".to_owned(),
        });
        assert_eq!(e.class, FailureClass::Permanent, "never retried on its own");
        assert_eq!(
            e.confirm.as_deref(),
            Some(&Confirm::InsecureRedirect {
                host: "mirror.example.com".to_owned()
            })
        );
        assert!(e.message.contains("mirror.example.com"), "{}", e.message);
    }

    #[test]
    fn a_link_from_the_browser_to_a_local_address_waits_for_the_user() {
        let e = fetch_failure(FetchError::LocalAddress(AddressProblem {
            host: "router.example".to_owned(),
            address: "192.168.1.1".to_owned(),
        }));
        assert_eq!(e.class, FailureClass::Permanent, "never retried on its own");
        assert_eq!(
            e.confirm.as_deref(),
            Some(&Confirm::LocalAddress {
                host: "router.example".to_owned(),
                address: "192.168.1.1".to_owned(),
            })
        );
        // The address the link really leads to is what the user must see (ADR-0006).
        assert!(e.message.contains("192.168.1.1"), "{}", e.message);
    }

    #[test]
    fn retry_after_reads_seconds_and_http_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_000_000);
        let date = |t: SystemTime| httpdate::fmt_http_date(t);
        let cases = [
            ("5", Some(Duration::from_secs(5))),
            (" 120 ", Some(Duration::from_secs(120))),
            ("0", Some(Duration::ZERO)),
            ("-1", None),
            ("5.5", None),
            ("soon", None),
            ("", None),
        ];
        for (value, expected) in cases {
            let h = head(429, &[("Retry-After", value)]);
            assert_eq!(retry_after(&h, now), expected, "{value:?}");
        }
        let later = date(now + Duration::from_secs(30));
        assert_eq!(
            retry_after(&head(503, &[("retry-after", &later)]), now),
            Some(Duration::from_secs(30))
        );
        let earlier = date(now - Duration::from_secs(30));
        assert_eq!(
            retry_after(&head(503, &[("Retry-After", &earlier)]), now),
            Some(Duration::ZERO)
        );
        // RFC 9110 §5.6.7: the two obsolete date formats are accepted too.
        for obsolete in ["Sunday, 06-Nov-94 08:49:37 GMT", "Sun Nov  6 08:49:37 1994"] {
            let then = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
            let h = head(503, &[("Retry-After", obsolete)]);
            assert_eq!(
                retry_after(&h, then - Duration::from_secs(7)),
                Some(Duration::from_secs(7)),
                "{obsolete}"
            );
        }
        assert_eq!(retry_after(&head(429, &[]), now), None);
    }

    #[test]
    fn a_full_disk_says_so() {
        for kind in [io::ErrorKind::StorageFull, io::ErrorKind::QuotaExceeded] {
            let message = disk_message(&io::Error::from(kind)).expect("a disk message");
            assert!(message.contains("not enough disk space"), "{message}");
        }
        assert_eq!(
            disk_message(&io::Error::from(io::ErrorKind::PermissionDenied)),
            None
        );
    }
}
