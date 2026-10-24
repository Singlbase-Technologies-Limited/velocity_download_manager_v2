//! FTPS downloads through the engine (#51), against the local FTP server's TLS on the test CA,
//! trusted through the test-only `test-ca` feature: explicit FTPS when the download requires
//! encryption, implicit FTPS for `ftps://`, and no fallback to plain FTP. Real libcurl (Schannel on
//! Windows, OpenSSL on Linux), real sockets, real files.

use std::path::{Path, PathBuf};
use std::time::Duration;

use velocity_core::domain::FailureClass;
use velocity_curl::{CurlLoop, Libcurl, LoopOptions, RequestOptions};
use velocity_test_http::ftp::{FtpOptions, FtpServer, FtpTls};
use velocity_test_http::seeded_bytes;
use velocity_test_http::tls::Leaf;
use velocity_transfer::download::{
    download_with, Cadence, Control, DownloadError, DownloadOptions,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const LEN: usize = 2 * 1024 * 1024 + 3;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-ftps-engine-{name}-{}", std::process::id()));
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

fn trusting(ca_file: Option<&Path>) -> CurlLoop {
    CurlLoop::start(
        &Libcurl::start().expect("libcurl starts"),
        LoopOptions {
            ca_file: ca_file.map(ToOwned::to_owned),
            ..LoopOptions::default()
        },
    )
    .expect("the loop starts")
}

fn options() -> DownloadOptions {
    DownloadOptions {
        connections: 4,
        probe_bytes: 256 * 1024,
        leases: Sizes::DEFAULT,
        checkpoints: Cadence::DEFAULT,
        retry: RetryPolicy {
            first_wait: Duration::from_millis(20),
            max_wait: Duration::from_millis(200),
            ..RetryPolicy::DEFAULT
        },
        adaptive: None,
    }
    .one_connection()
}

const USER: &str = "alice";
const PASSWORD: &str = "TOPSECRETFTP";

fn serve(tls: FtpTls) -> (FtpServer, Vec<u8>) {
    let data = seeded_bytes(LEN, 251);
    let server = FtpServer::start(
        vec![("/f.bin".into(), data.clone())],
        FtpOptions {
            login: Some((USER.into(), PASSWORD.into())),
            tls,
            ..FtpOptions::default()
        },
    );
    (server, data)
}

/// The download's control: its login, and whether it requires encryption.
fn control(require_encryption: bool) -> Control {
    Control::default().with_request(RequestOptions {
        login: Some((USER.into(), PASSWORD.into())),
        require_encryption,
        ..RequestOptions::default()
    })
}

#[test]
fn a_download_that_requires_encryption_completes_over_explicit_ftps() {
    let (server, data) = serve(FtpTls::Explicit(Leaf::Valid));
    let dir = Dir::new("explicit");
    let dest = dir.file("out.bin");
    download_with(
        &trusting(server.ca_file()),
        &server.url("f.bin"),
        &dest,
        options(),
        &control(true),
    )
    .expect("it downloads over FTPS");
    assert!(std::fs::read(&dest).unwrap() == data);
    let record = server.record();
    assert!(record.tls_controls >= 1, "{record:?}");
    assert!(
        !record.data_connections.is_empty() && record.data_connections.iter().all(|tls| *tls),
        "a data connection went in clear: {record:?}"
    );
    assert_eq!(record.logins, vec![(USER.to_owned(), PASSWORD.to_owned())]);
}

#[test]
fn an_ftps_link_completes_over_implicit_ftps() {
    let (server, data) = serve(FtpTls::Implicit(Leaf::Valid));
    let dir = Dir::new("implicit");
    let dest = dir.file("out.bin");
    download_with(
        &trusting(server.ca_file()),
        &server.url("f.bin"),
        &dest,
        options(),
        &control(false),
    )
    .expect("it downloads over FTPS");
    assert!(std::fs::read(&dest).unwrap() == data);
    let record = server.record();
    assert!(record.data_connections.iter().all(|tls| *tls), "{record:?}");
}

#[test]
fn requiring_encryption_from_a_plain_ftp_server_fails_at_once_without_the_login() {
    let (server, _) = serve(FtpTls::None);
    let dir = Dir::new("plain");
    let dest = dir.file("out.bin");
    let result = download_with(
        &trusting(None),
        &server.url("f.bin"),
        &dest,
        options(),
        &control(true),
    );
    match result {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Permanent, "{e:?}");
            assert!(e.message.contains("can't encrypt"), "{}", e.message);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    let record = server.record();
    assert!(
        record.logins.is_empty(),
        "the login went out in clear: {record:?}"
    );
    assert_eq!(record.connections, 1, "not retried: {record:?}");
    assert!(dir.is_empty(), "nothing is left on disk");
}

#[test]
fn an_untrusted_ftps_certificate_fails_at_once_without_the_login() {
    let (server, _) = serve(FtpTls::Explicit(Leaf::Expired));
    let dir = Dir::new("expired");
    let dest = dir.file("out.bin");
    let result = download_with(
        &trusting(server.ca_file()),
        &server.url("f.bin"),
        &dest,
        options(),
        &control(true),
    );
    match result {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Permanent, "{e:?}");
            assert!(e.message.contains("certificate"), "{}", e.message);
        }
        other => panic!("expected a refused certificate, got {other:?}"),
    }
    assert!(server.record().logins.is_empty());
    assert!(dir.is_empty());
}
