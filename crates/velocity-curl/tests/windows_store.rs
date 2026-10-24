//! The path users take on Windows (#69): no CA file, only the Windows certificate store, as
//! Schannel verifies against it. The test CA goes into the machine's Root store for the test and is
//! removed afterwards, which needs administrator rights, so the test is ignored by default. CI's
//! Windows job runs it (its runner is an administrator); locally, as an administrator:
//! `cargo test -p velocity-curl --test windows_store -- --ignored`.
#![cfg(windows)]

use std::process::Command;
use std::time::{Duration, Instant};

use velocity_curl::{
    CurlFailure, CurlLoop, FetchError, FetchEvent, FetchRequest, Gate, Libcurl, LoopOptions,
    ResponseHead,
};
use velocity_test_http::tls::{Alpn, Leaf, TlsFront};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};

const WAIT: Duration = Duration::from_secs(30);

/// The front's CA in the machine's Root store while it lives; removed by name on drop.
struct TrustedInStore(String);

impl TrustedInStore {
    fn add(front: &TlsFront) -> Self {
        let added = Command::new("certutil")
            .args(["-f", "-addstore", "Root"])
            .arg(front.ca_file())
            .output()
            .expect("certutil runs");
        assert!(
            added.status.success(),
            "certutil couldn't add the test CA to the machine's Root store (this test needs an \
             administrator): {}",
            String::from_utf8_lossy(&added.stdout)
        );
        TrustedInStore(front.ca_name().to_owned())
    }
}

impl Drop for TrustedInStore {
    fn drop(&mut self) {
        let removed = Command::new("certutil")
            .args(["-delstore", "Root", &self.0])
            .output();
        // A CA left in the store would stay trusted: say so loudly.
        if !removed.is_ok_and(|o| o.status.success()) {
            eprintln!(
                "REMOVE BY HAND: the test CA \"{}\" is still in the Root store",
                self.0
            );
        }
    }
}

fn fetch(url: &str) -> (Vec<u8>, Result<(), FetchError>) {
    let curl = Libcurl::start().expect("libcurl starts");
    // No CA file: Schannel uses the Windows certificate store, as in production.
    let lp = CurlLoop::start(&curl, LoopOptions::default()).expect("the loop starts");
    let fetch = lp.fetch(FetchRequest::get(url.to_owned()), |_: &ResponseHead| {
        Gate::Accept
    });
    let mut data = Vec::new();
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match fetch.next_event(left).expect("the fetch ends in time") {
            FetchEvent::Head(_) => {}
            FetchEvent::Data(chunk) => data.extend_from_slice(&chunk),
            FetchEvent::Done(done) => return (data, done),
        }
    }
}

#[test]
#[ignore = "changes the machine's trust store: needs an administrator; CI's Windows job runs it"]
fn a_ca_in_the_windows_store_is_trusted_and_every_refusal_is_a_certificate_failure() {
    let data = seeded_bytes(100_000, 75);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );

    let valid = TlsFront::start(server.addr(), Leaf::Valid, Alpn::Http1);
    let _trusted = TrustedInStore::add(&valid);
    let (got, done) = fetch(&valid.url("f.bin"));
    assert_eq!(done, Ok(()), "a valid certificate from a CA in the store");
    assert!(got == data);

    // Through the store, Schannel reports a refused certificate as a connect error (code 35) with
    // the reason in its detail; it must still be a certificate failure, never a transient one that
    // is retried (the regression this test found on CI).
    for (leaf, reason) in [
        (Leaf::Revoked, "0x80092010"),
        (Leaf::Expired, "expired"),
        (Leaf::WrongHost, ""),
        (Leaf::UntrustedIssuer, ""),
        (Leaf::ClientAuthOnly, ""),
    ] {
        let front = TlsFront::start(server.addr(), leaf, Alpn::Http1);
        let _trusted = TrustedInStore::add(&front);
        let (got, done) = fetch(&front.url("f.bin"));
        // Schannel's words on this path, for the per-platform record (docs/SECURITY.md).
        eprintln!("{leaf:?} through the store: {done:?}");
        match done {
            Err(FetchError::Curl { kind, message, .. }) => {
                assert_eq!(kind, CurlFailure::Certificate, "{leaf:?}: {message}");
                assert!(
                    message.to_ascii_lowercase().contains(reason),
                    "{leaf:?}: {message}"
                );
            }
            other => panic!("{leaf:?}: expected a refused certificate, got {other:?}"),
        }
        assert!(got.is_empty(), "{leaf:?}: nothing may be delivered");
    }
}
