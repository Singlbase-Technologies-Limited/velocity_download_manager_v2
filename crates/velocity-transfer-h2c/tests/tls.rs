//! The downloader over TLS (#69), against the local TLS fixture and its test CA, trusted through
//! the test-only `test-ca` feature: segmented downloads over HTTPS and over HTTP/2 negotiated
//! through ALPN complete, and a refused certificate fails at once, permanently, with nothing on
//! disk. Real libcurl (Schannel on Windows, OpenSSL on Linux), real sockets, real files.

use std::path::{Path, PathBuf};

use velocity_core::domain::FailureClass;
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::tls::{Alpn, Leaf, TlsFront};
use velocity_test_http::{seeded_bytes, Http2Options, Http2Server, ServerOptions, TestServer};
use velocity_transfer::download::{download, Cadence, DownloadError, DownloadOptions, Mode};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const MIB: usize = 1 << 20;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-tls-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn is_empty(&self) -> bool {
        std::fs::read_dir(&self.0).unwrap().next().is_none()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn trusting(ca_file: &Path) -> CurlLoop {
    CurlLoop::start(
        &Libcurl::start().expect("libcurl starts"),
        LoopOptions {
            ca_file: Some(ca_file.to_owned()),
            ..LoopOptions::default()
        },
    )
    .expect("the loop starts")
}

const OPTIONS: DownloadOptions = DownloadOptions {
    connections: 4,
    probe_bytes: 256 * 1024,
    leases: Sizes::DEFAULT,
    checkpoints: Cadence::DEFAULT,
    retry: RetryPolicy::DEFAULT,
    adaptive: None,
};

#[test]
fn a_segmented_download_over_https_completes() {
    let data = seeded_bytes(4 * MIB, 72);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    let front = TlsFront::start(server.addr(), Leaf::Valid, Alpn::Http1);
    let dir = Dir::new("https");
    let dest = dir.file("out.bin");
    let report = download(
        &trusting(front.ca_file()),
        &front.url("f.bin"),
        &dest,
        OPTIONS,
    )
    .expect("it downloads");
    assert_eq!(report.mode, Mode::Ranges);
    assert!(std::fs::read(&dest).unwrap() == data);
    // Each connection is a TLS session of its own.
    assert!(front.sessions().len() >= 2, "{:?}", front.sessions());
}

#[test]
fn a_segmented_download_over_http2_through_alpn_completes() {
    let data = seeded_bytes(4 * MIB, 73);
    let server = Http2Server::start(
        vec![("f.bin".into(), data.clone())],
        Http2Options::default(),
    );
    let front = TlsFront::start(server.addr(), Leaf::Valid, Alpn::Http2);
    let dir = Dir::new("h2");
    let dest = dir.file("out.bin");
    let report = download(
        &trusting(front.ca_file()),
        &front.url("f.bin"),
        &dest,
        OPTIONS,
    )
    .expect("it downloads");
    assert_eq!(report.mode, Mode::Ranges);
    assert!(std::fs::read(&dest).unwrap() == data);
    assert!(
        front
            .sessions()
            .iter()
            .all(|s| s.alpn.as_deref() == Some("h2")),
        "{:?}",
        front.sessions()
    );
}

#[test]
fn a_refused_certificate_fails_permanently_with_nothing_on_disk() {
    let data = seeded_bytes(MIB, 74);
    let server = TestServer::start(vec![("f.bin".into(), data)], ServerOptions::default());
    let mut refused = vec![
        Leaf::ClientAuthOnly,
        Leaf::CodeSigningOnly,
        Leaf::UntrustedIssuer,
        Leaf::WrongHost,
        Leaf::Expired,
    ];
    // OpenSSL checks no revocation without a CRL file (tests/tls.rs in velocity-curl).
    if cfg!(windows) {
        refused.push(Leaf::Revoked);
    }
    for leaf in refused {
        let front = TlsFront::start(server.addr(), leaf, Alpn::Http1);
        let dir = Dir::new(&format!("{leaf:?}"));
        let dest = dir.file("out.bin");
        match download(
            &trusting(front.ca_file()),
            &front.url("f.bin"),
            &dest,
            OPTIONS,
        ) {
            Err(DownloadError::Transfer(e)) => {
                assert_eq!(e.class, FailureClass::Permanent, "{leaf:?}: {e:?}");
                assert!(!e.retryable, "{leaf:?}: never retried");
                assert!(
                    e.message.contains("certificate isn't trusted"),
                    "{leaf:?}: {}",
                    e.message
                );
            }
            other => panic!("{leaf:?}: expected a refused certificate, got {other:?}"),
        }
        assert!(dir.is_empty(), "{leaf:?}: no file and no part file");
    }
    assert!(
        server.counters().requests.is_empty(),
        "no request reached the server"
    );
}
