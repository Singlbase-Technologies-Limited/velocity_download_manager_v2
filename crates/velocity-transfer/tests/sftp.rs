//! SFTP downloads through the engine (#53), against the machine's OpenSSH server: real russh, real
//! sshd, real files. The engine's probe, checkpoints and retries are the same as for HTTP and
//! FTP, so an SFTP download resumes at an offset after a cut and ends with the same checks. Where
//! sshd isn't installed, each test says NOT RUN and returns.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use velocity_core::domain::{FailureClass, TransferError};
use velocity_curl::{RateLimit, RequestOptions, SshKeyFile};
use velocity_sftp::known_hosts::KnownHosts;
use velocity_sftp::{SftpOptions, SftpTransport};
use velocity_test_http::seeded_bytes;
use velocity_test_http::ssh::{HostKey, SshServer};
use velocity_transfer::download::{
    download_with, Cadence, Control, DownloadError, DownloadOptions,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const LEN: usize = 1_500_000;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-sftp-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options() -> DownloadOptions {
    DownloadOptions {
        connections: 4,
        probe_bytes: 64 * 1024,
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

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn serve() -> Result<(SshServer, Vec<u8>), String> {
    let data = seeded_bytes(LEN, 353);
    SshServer::start(vec![("f.bin".into(), data.clone())], HostKey::Ed25519).map(|s| (s, data))
}

fn login(server: &SshServer) -> RequestOptions {
    RequestOptions {
        login: Some((server.user().to_owned(), String::new())),
        ssh_key: Some(SshKeyFile {
            path: server.client_key(),
            passphrase: None,
        }),
        ..RequestOptions::default()
    }
}

fn transport(dir: &Dir) -> SftpTransport {
    SftpTransport::start(
        KnownHosts::new(dir.file("known_hosts")),
        SftpOptions::default(),
    )
    .unwrap()
}

/// The download's failure, which must be a transfer error.
fn transfer_error(result: Result<impl std::fmt::Debug, DownloadError>) -> TransferError {
    match result {
        Err(DownloadError::Transfer(e)) => e,
        other => panic!("expected a transfer failure, got {other:?}"),
    }
}

/// Downloads once to meet the server's unknown key, then trusts exactly that key, as the app does
/// once the user confirms the fingerprint.
fn trust_server(sftp: &SftpTransport, server: &SshServer, dir: &Dir) {
    let control = Control::default().with_request(login(server));
    let first = download_with(
        sftp,
        &server.url("f.bin"),
        &dir.file("probe.bin"),
        options(),
        &control,
    );
    let check = transfer_error(first).host_key.expect("an unknown host key");
    sftp.trust_host_key(&check.host, check.port, &check.fingerprint)
        .expect("trusted");
}

#[test]
fn an_unknown_host_key_fails_at_once_then_the_trusted_server_downloads() {
    let Ok((server, data)) = serve() else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let dir = Dir::new("unknown");
    let sftp = transport(&dir);
    let dest = dir.file("out.bin");
    let started = Instant::now();
    let control = Control::default().with_request(login(&server));
    let e = transfer_error(download_with(
        &sftp,
        &server.url("f.bin"),
        &dest,
        options(),
        &control,
    ));
    assert_eq!(e.class, FailureClass::Permanent, "{e:?}");
    assert!(!e.retryable);
    let check = e.host_key.clone().expect("the key for the user to confirm");
    assert_eq!(check.port, server.addr().port());
    assert!(check.fingerprint.starts_with("SHA256:"), "{check:?}");
    assert_eq!(check.pinned, None);
    // Refused at once: no retry waits.
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(
        !dest.exists() && !part_of(&dest).exists(),
        "nothing on disk"
    );

    sftp.trust_host_key(&check.host, check.port, &check.fingerprint)
        .expect("trusted");
    let report = download_with(&sftp, &server.url("f.bin"), &dest, options(), &control)
        .expect("it downloads once trusted");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    assert_eq!(report.total, LEN as u64);
    assert_eq!(report.fetches, 1, "{report:?}");
}

#[test]
fn a_changed_host_key_fails_at_once_and_says_what_was_pinned() {
    let Ok((mut server, _)) = serve() else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let dir = Dir::new("changed");
    let sftp = transport(&dir);
    trust_server(&sftp, &server, &dir);
    server.restart(true).expect("restarts with a new key");
    let control = Control::default().with_request(login(&server));
    let e = transfer_error(download_with(
        &sftp,
        &server.url("f.bin"),
        &dir.file("out.bin"),
        options(),
        &control,
    ));
    assert_eq!(e.class, FailureClass::Permanent);
    let check = e.host_key.expect("the changed key");
    assert!(check.pinned.is_some(), "{check:?}");
    assert_ne!(check.pinned.as_deref(), Some(check.fingerprint.as_str()));
    assert!(e.message.contains("changed"), "{}", e.message);
}

#[test]
fn a_cut_transfer_resumes_at_its_offset_and_still_matches() {
    let Ok((mut server, data)) = serve() else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let dir = Dir::new("cut");
    let sftp = transport(&dir);
    trust_server(&sftp, &server, &dir);
    let dest = dir.file("out.bin");
    let written = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&written);
    // Slowed by a speed limit, so the cut lands part way.
    let control = Control::with_progress(move |w, _| seen.store(w, Ordering::Release))
        .with_request(login(&server))
        .with_limits(vec![RateLimit::new(Some(400_000))]);
    let url = server.url("f.bin");
    let report = std::thread::scope(|scope| {
        let run = scope.spawn(|| download_with(&sftp, &url, &dest, options(), &control));
        let deadline = Instant::now() + Duration::from_secs(30);
        while written.load(Ordering::Acquire) < 300_000 {
            assert!(Instant::now() < deadline, "no progress in time");
            std::thread::sleep(Duration::from_millis(10));
        }
        server.stop();
        std::thread::sleep(Duration::from_millis(100));
        server.restart(false).expect("the same server comes back");
        run.join().unwrap()
    })
    .expect("it resumes after the cut");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    assert!(
        report.fetches >= 2,
        "the rest came in a second fetch: {report:?}"
    );
    assert_eq!(report.wasted_bytes, 0, "{report:?}");
}

#[test]
fn a_refused_login_fails_at_once_without_retries() {
    let Ok((server, _)) = serve() else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let dir = Dir::new("refused");
    let sftp = transport(&dir);
    trust_server(&sftp, &server, &dir);
    let mut wrong = login(&server);
    wrong.login = Some(("velocity-no-such-user".to_owned(), String::new()));
    let started = Instant::now();
    let e = transfer_error(download_with(
        &sftp,
        &server.url("f.bin"),
        &dir.file("out.bin"),
        options(),
        &Control::default().with_request(wrong),
    ));
    assert_eq!(e.class, FailureClass::Permanent, "{e:?}");
    assert!(!e.retryable);
    assert!(e.message.contains("refused the login"), "{}", e.message);
    assert!(started.elapsed() < Duration::from_secs(10));
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}
