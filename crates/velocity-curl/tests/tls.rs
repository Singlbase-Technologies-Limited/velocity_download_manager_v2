//! TLS verification through the linked libcurl (#69): Schannel on Windows, OpenSSL on Linux,
//! against the local TLS fixture and its test CA. Verification is never turned off; the test CA is
//! the only extra trust, passed with the test-only `test-ca` feature (`CURLOPT_CAINFO`).

use std::time::{Duration, Instant};

use velocity_curl::{
    CurlFailure, CurlLoop, FetchError, FetchEvent, FetchRequest, Gate, Libcurl, LoopOptions,
    ResponseHead,
};
use velocity_test_http::tls::{Alpn, Leaf, TlsFront};
use velocity_test_http::{seeded_bytes, Http2Options, Http2Server, ServerOptions, TestServer};

const WAIT: Duration = Duration::from_secs(30);

struct Outcome {
    head: Option<ResponseHead>,
    data: Vec<u8>,
    done: Result<(), FetchError>,
}

fn fetch(front: &TlsFront, trust_ca: bool, name: &str) -> Outcome {
    let curl = Libcurl::start().expect("libcurl starts");
    let options = LoopOptions {
        ca_file: trust_ca.then(|| front.ca_file().to_owned()),
        ..LoopOptions::default()
    };
    let lp = CurlLoop::start(&curl, options).expect("the loop starts");
    let fetch = lp.fetch(FetchRequest::get(front.url(name)), |_: &ResponseHead| {
        Gate::Accept
    });
    let (mut head, mut data) = (None, Vec::new());
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match fetch.next_event(left).expect("the fetch ends in time") {
            FetchEvent::Head(h) => head = Some(h),
            FetchEvent::Data(chunk) => data.extend_from_slice(&chunk),
            FetchEvent::Done(done) => return Outcome { head, data, done },
        }
    }
}

fn backend() -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(200_000, 69);
    (
        TestServer::start(
            vec![("f.bin".into(), data.clone())],
            ServerOptions::default(),
        ),
        data,
    )
}

fn assert_accepted(leaf: Leaf) {
    let (server, data) = backend();
    let front = TlsFront::start(server.addr(), leaf, Alpn::Http1);
    let o = fetch(&front, true, "f.bin");
    assert_eq!(o.done, Ok(()), "{leaf:?}");
    assert!(o.data == data, "{leaf:?}: the body differs");
}

/// What each TLS backend says when it refuses, as measured on #69: Schannel on Windows, OpenSSL on
/// Linux. Checking it proves the certificate was refused for its own reason.
struct Reason {
    schannel: &'static str,
    openssl: &'static str,
}

fn assert_refused(leaf: Leaf, trust_ca: bool, reason: Reason) {
    let (server, _) = backend();
    let front = TlsFront::start(server.addr(), leaf, Alpn::Http1);
    let o = fetch(&front, trust_ca, "f.bin");
    // What the TLS backend said, for the per-platform record (docs/SECURITY.md).
    eprintln!("{leaf:?} (CA trusted: {trust_ca}): {:?}", o.done);
    match &o.done {
        Err(FetchError::Curl { kind, message, .. }) => {
            assert_eq!(*kind, CurlFailure::Certificate, "{leaf:?}: {:?}", o.done);
            let expected = if cfg!(windows) {
                reason.schannel
            } else {
                reason.openssl
            };
            assert!(message.contains(expected), "{leaf:?}: {message}");
        }
        other => panic!("{leaf:?}: expected a refused certificate, got {other:?}"),
    }
    assert!(
        o.head.is_none() && o.data.is_empty(),
        "{leaf:?}: nothing may be delivered"
    );
    assert!(
        server.counters().requests.is_empty(),
        "{leaf:?}: no request may reach the server"
    );
}

#[test]
fn a_valid_server_certificate_is_accepted() {
    assert_accepted(Leaf::Valid);
}

/// RFC 5280 §4.2.1.12: without the extension, a certificate may be used for any purpose.
#[test]
fn a_certificate_without_an_extended_key_usage_is_accepted() {
    assert_accepted(Leaf::NoExtendedKeyUsage);
}

/// The CVE-2026-8367 class: a certificate from a trusted CA that isn't for TLS servers.
#[test]
fn a_certificate_not_for_server_authentication_is_refused() {
    // Schannel's 0x10 is CERT_TRUST_IS_NOT_VALID_FOR_USAGE.
    assert_refused(
        Leaf::ClientAuthOnly,
        true,
        Reason {
            schannel: "schannel: error 0x00000010",
            openssl: "unsuitable certificate purpose (26)",
        },
    );
    assert_refused(
        Leaf::CodeSigningOnly,
        true,
        Reason {
            schannel: "schannel: error 0x00000010",
            openssl: "unsuitable certificate purpose (26)",
        },
    );
}

#[test]
fn a_certificate_from_an_untrusted_issuer_is_refused() {
    assert_refused(
        Leaf::UntrustedIssuer,
        true,
        Reason {
            schannel: "the certificate chain is incomplete",
            openssl: "unable to get local issuer certificate (20)",
        },
    );
}

#[test]
fn a_certificate_for_another_host_is_refused() {
    assert_refused(
        Leaf::WrongHost,
        true,
        Reason {
            schannel: "failed to match connection hostname (localhost)",
            openssl: "no alternative certificate subject name matches target hostname 'localhost'",
        },
    );
}

#[test]
fn an_expired_certificate_is_refused() {
    assert_refused(
        Leaf::Expired,
        true,
        Reason {
            schannel: "is not time valid",
            openssl: "certificate has expired (10)",
        },
    );
}

#[test]
fn the_test_ca_is_not_trusted_unless_it_is_given() {
    assert_refused(
        Leaf::Valid,
        false,
        Reason {
            schannel: "SEC_E_UNTRUSTED_ROOT",
            openssl: "unable to get local issuer certificate (20)",
        },
    );
}

/// Schannel checks revocation, through the CRL distribution point the fixture serves. libcurl with
/// OpenSSL checks no revocation unless it is given a CRL file (`CURLOPT_CRLFILE`), which V2 doesn't
/// set: there a revoked certificate is accepted. Recorded as measured, not as policy (#69).
#[test]
fn a_revoked_certificate_is_refused_on_windows_and_accepted_on_linux() {
    if cfg!(windows) {
        assert_refused(
            Leaf::Revoked,
            true,
            Reason {
                schannel: "has been revoked",
                openssl: "unused: OpenSSL accepts it",
            },
        );
    } else {
        assert_accepted(Leaf::Revoked);
    }
}

#[test]
fn http2_is_negotiated_through_alpn() {
    let data = seeded_bytes(300_000, 70);
    let server = Http2Server::start(
        vec![("f.bin".into(), data.clone())],
        Http2Options::default(),
    );
    let front = TlsFront::start(server.addr(), Leaf::Valid, Alpn::Http2);
    let o = fetch(&front, true, "f.bin");
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    assert!(o.head.expect("a head").http2, "the answer came over HTTP/2");
    assert_eq!(front.sessions()[0].alpn.as_deref(), Some("h2"));
}

#[test]
fn the_tls_version_negotiated_is_recorded() {
    let (server, _) = backend();
    let front = TlsFront::start(server.addr(), Leaf::Valid, Alpn::Http1);
    assert_eq!(fetch(&front, true, "f.bin").done, Ok(()));
    let sessions = front.sessions();
    // Both backends negotiate TLS 1.3 with rustls: Schannel on Windows 11 and OpenSSL 3.5.
    assert_eq!(sessions[0].version, "TLSv1_3", "{sessions:?}");
}
