//! SFTP fetches against the machine's OpenSSH server (#53): real russh, real sshd, real sockets.
//! Where sshd isn't installed, each test says NOT RUN and returns. Password logins run only
//! against a provisioned server (`VELOCITY_SFTP_PASSWORD_SERVER`): CI's, or the local one from
//! `scripts/local-sftp-server.sh` (#176).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use velocity_curl::{
    CurlFailure, Fetch, FetchError, FetchEvent, FetchRequest, Gate, RequestOptions, ResponseHead,
    SshKeyFile, Transport,
};
use velocity_sftp::known_hosts::{KnownHosts, Verdict};
use velocity_sftp::{SftpOptions, SftpTransport, TrustError};
use velocity_test_http::seeded_bytes;
use velocity_test_http::ssh::{HostKey, PasswordServer, SshServer, KEY_PASSPHRASE};

const WAIT: Duration = Duration::from_secs(30);
const LEN: usize = 700_000;

struct Outcome {
    head: Option<ResponseHead>,
    data: Vec<u8>,
    done: Result<(), FetchError>,
}

fn collect(fetch: &Fetch) -> Outcome {
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

/// A known_hosts file of its own, removed when dropped.
struct Hosts(PathBuf);

impl Hosts {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-sftp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Hosts(p)
    }

    fn file(&self) -> KnownHosts {
        KnownHosts::new(self.0.join("known_hosts"))
    }
}

impl Drop for Hosts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

macro_rules! server_or_skip {
    ($files:expr, $key:expr) => {
        match SshServer::start($files, $key) {
            Ok(s) => s,
            Err(why) => {
                eprintln!("NOT RUN: {why}");
                return;
            }
        }
    };
}

fn serve(host_key: HostKey) -> Result<(SshServer, Vec<u8>), String> {
    let data = seeded_bytes(LEN, 253);
    SshServer::start(vec![("f.bin".into(), data.clone())], host_key).map(|s| (s, data))
}

/// The server's host key, already trusted.
fn trusting(server: &SshServer, hosts: &Hosts) -> SftpTransport {
    let key = russh::keys::PublicKey::from_openssh(&server.host_key()).unwrap();
    hosts
        .file()
        .trust("127.0.0.1", server.addr().port(), &key)
        .unwrap();
    SftpTransport::start(hosts.file(), SftpOptions::default()).unwrap()
}

/// Logging in with the server's key without a passphrase.
fn with_key(server: &SshServer) -> RequestOptions {
    RequestOptions {
        login: Some((server.user().to_owned(), String::new())),
        ssh_key: Some(SshKeyFile {
            path: server.client_key(),
            passphrase: None,
        }),
        ..RequestOptions::default()
    }
}

fn fetch(sftp: &SftpTransport, request: FetchRequest, options: RequestOptions) -> Outcome {
    let request = request.with_options(Arc::new(options));
    collect(&sftp.fetch(request, Box::new(|_: &ResponseHead| Gate::Accept)))
}

fn kind(done: &Result<(), FetchError>) -> CurlFailure {
    match done {
        Err(FetchError::Sftp { kind, .. }) => *kind,
        other => panic!("expected an SFTP failure, got {other:?}"),
    }
}

fn host_key_fingerprint(server: &SshServer) -> String {
    let key = russh::keys::PublicKey::from_openssh(&server.host_key()).unwrap();
    velocity_sftp::known_hosts::fingerprint(&key)
}

#[test]
fn an_unknown_host_key_is_refused_until_the_user_trusts_it() {
    let Ok((server, data)) = serve(HostKey::Ed25519) else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let hosts = Hosts::new("unknown");
    let sftp = SftpTransport::start(hosts.file(), SftpOptions::default()).unwrap();
    let url = server.url("f.bin");
    let refused = fetch(&sftp, FetchRequest::get(url.clone()), with_key(&server));
    let port = server.addr().port();
    match &refused.done {
        Err(FetchError::HostKey(problem)) => {
            assert_eq!((problem.host.as_str(), problem.port), ("127.0.0.1", port));
            assert_eq!(problem.fingerprint, host_key_fingerprint(&server));
            assert_eq!(problem.pinned, None);
        }
        other => panic!("expected an unknown host key, got {other:?}"),
    }
    assert!(refused.head.is_none() && refused.data.is_empty());
    assert!(
        !hosts.0.join("known_hosts").exists(),
        "nothing trusted on its own"
    );

    // Only the key that was seen can be trusted, by its own fingerprint.
    assert_eq!(
        sftp.trust_host_key("127.0.0.1", port, "SHA256:not-the-key"),
        Err(TrustError::NotSeen)
    );
    sftp.trust_host_key("127.0.0.1", port, &host_key_fingerprint(&server))
        .expect("trusted");
    let o = fetch(&sftp, FetchRequest::get(url), with_key(&server));
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data, "the body differs from the file");
    let head = o.head.unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("content-length"),
        Some(LEN.to_string().as_str())
    );
    assert!(head.header("last-modified").is_some(), "{head:?}");
}

#[test]
fn a_known_host_key_of_each_algorithm_is_accepted() {
    for host_key in [HostKey::Ed25519, HostKey::Ecdsa, HostKey::Rsa] {
        let Ok((server, data)) = serve(host_key) else {
            eprintln!("NOT RUN: no sshd");
            return;
        };
        let hosts = Hosts::new(&format!("known-{host_key:?}"));
        let sftp = trusting(&server, &hosts);
        let o = fetch(
            &sftp,
            FetchRequest::get(server.url("f.bin")),
            with_key(&server),
        );
        assert_eq!(o.done, Ok(()), "{host_key:?}");
        assert!(o.data == data, "{host_key:?}");
    }
}

#[test]
fn a_changed_host_key_is_always_refused_and_never_trusted() {
    let Ok((mut server, _)) = serve(HostKey::Ed25519) else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let hosts = Hosts::new("changed");
    let sftp = trusting(&server, &hosts);
    let pinned = host_key_fingerprint(&server);
    server.restart(true).expect("restarts with a new key");
    let o = fetch(
        &sftp,
        FetchRequest::get(server.url("f.bin")),
        with_key(&server),
    );
    match &o.done {
        Err(FetchError::HostKey(problem)) => {
            assert_eq!(problem.pinned.as_deref(), Some(pinned.as_str()));
            assert_eq!(problem.fingerprint, host_key_fingerprint(&server));
        }
        other => panic!("expected a changed host key, got {other:?}"),
    }
    assert_eq!(
        sftp.trust_host_key(
            "127.0.0.1",
            server.addr().port(),
            &host_key_fingerprint(&server)
        ),
        Err(TrustError::NotSeen),
        "a changed key isn't even offered for trust"
    );
    let still = hosts.file();
    let new_key = russh::keys::PublicKey::from_openssh(&server.host_key()).unwrap();
    assert!(matches!(
        still
            .check("127.0.0.1", server.addr().port(), &new_key)
            .unwrap(),
        Verdict::Changed { .. }
    ));
}

#[test]
fn an_encrypted_key_logs_in_with_its_passphrase_and_not_without() {
    let server = server_or_skip!(vec![("f.bin".into(), vec![7; 1000])], HostKey::Ed25519);
    let hosts = Hosts::new("passphrase");
    let sftp = trusting(&server, &hosts);
    let options = |passphrase: Option<&str>| RequestOptions {
        login: Some((server.user().to_owned(), String::new())),
        ssh_key: Some(SshKeyFile {
            path: server.client_key_with_passphrase(),
            passphrase: passphrase.map(str::to_owned),
        }),
        ..RequestOptions::default()
    };
    let ok = fetch(
        &sftp,
        FetchRequest::get(server.url("f.bin")),
        options(Some(KEY_PASSPHRASE)),
    );
    assert_eq!(ok.done, Ok(()));
    assert_eq!(ok.data, vec![7; 1000]);
    let wrong = fetch(
        &sftp,
        FetchRequest::get(server.url("f.bin")),
        options(Some("wrong")),
    );
    assert_eq!(
        kind(&wrong.done),
        CurlFailure::AccessDenied,
        "{:?}",
        wrong.done
    );
}

#[test]
fn a_refused_login_fails_as_access_denied() {
    let server = server_or_skip!(vec![("f.bin".into(), vec![1])], HostKey::Ed25519);
    let hosts = Hosts::new("refused");
    let sftp = trusting(&server, &hosts);
    let mut options = with_key(&server);
    options.login = Some(("velocity-no-such-user".to_owned(), String::new()));
    let o = fetch(&sftp, FetchRequest::get(server.url("f.bin")), options);
    assert_eq!(kind(&o.done), CurlFailure::AccessDenied, "{:?}", o.done);
    let no_user = fetch(
        &sftp,
        FetchRequest::get(server.url("f.bin")),
        RequestOptions::default(),
    );
    assert_eq!(kind(&no_user.done), CurlFailure::AccessDenied);
}

#[test]
fn ranges_are_answered_as_an_http_server_would() {
    let Ok((server, data)) = serve(HostKey::Ed25519) else {
        eprintln!("NOT RUN: no sshd");
        return;
    };
    let hosts = Hosts::new("ranges");
    let sftp = trusting(&server, &hosts);
    let url = server.url("f.bin");
    let from = fetch(
        &sftp,
        FetchRequest::range(url.clone(), 1000, None),
        with_key(&server),
    );
    assert_eq!(from.done, Ok(()));
    assert!(from.data == data[1000..]);
    let head = from.head.unwrap();
    assert_eq!(head.status, 206);
    assert_eq!(
        head.header("content-range"),
        Some(format!("bytes 1000-{}/{LEN}", LEN - 1).as_str())
    );
    let ended = fetch(
        &sftp,
        FetchRequest::range(url.clone(), 1000, Some(1999)),
        with_key(&server),
    );
    assert_eq!(ended.done, Ok(()));
    assert!(ended.data == data[1000..2000], "{} bytes", ended.data.len());
    let past = fetch(
        &sftp,
        FetchRequest::range(url, LEN as u64, None),
        with_key(&server),
    );
    assert_eq!(past.done, Ok(()));
    assert_eq!(past.head.map(|h| h.status), Some(416));
    assert!(past.data.is_empty());
}

#[test]
fn a_missing_file_fails_as_not_found() {
    let server = server_or_skip!(Vec::new(), HostKey::Ed25519);
    let hosts = Hosts::new("missing");
    let sftp = trusting(&server, &hosts);
    let o = fetch(
        &sftp,
        FetchRequest::get(server.url("missing.bin")),
        with_key(&server),
    );
    assert_eq!(kind(&o.done), CurlFailure::NotFound, "{:?}", o.done);
    assert!(o.head.is_none());
}

#[test]
fn a_password_login_works_where_ci_provides_a_server() {
    let server = match PasswordServer::from_env() {
        Ok(s) => s,
        Err(why) => {
            eprintln!("NOT RUN: {why}");
            return;
        }
    };
    let hosts = Hosts::new("password");
    let sftp = SftpTransport::start(hosts.file(), SftpOptions::default()).unwrap();
    let options = RequestOptions {
        login: Some((server.user.clone(), server.password.clone())),
        ..RequestOptions::default()
    };
    // First the unknown key, as for any server; then trusted, the login.
    let refused = fetch(&sftp, FetchRequest::get(server.url()), options.clone());
    let Err(FetchError::HostKey(problem)) = &refused.done else {
        panic!("expected an unknown host key, got {:?}", refused.done);
    };
    sftp.trust_host_key(&problem.host, problem.port, &problem.fingerprint)
        .unwrap();
    let o = fetch(&sftp, FetchRequest::get(server.url()), options.clone());
    assert_eq!(o.done, Ok(()));
    use sha2_check::hex_sha256;
    assert_eq!(hex_sha256(&o.data), server.sha256);
    let wrong = RequestOptions {
        login: Some((server.user.clone(), "wrong-password".to_owned())),
        ..options
    };
    let refused = fetch(&sftp, FetchRequest::get(server.url()), wrong);
    assert_eq!(kind(&refused.done), CurlFailure::AccessDenied);
}

mod sha2_check {
    /// The SHA-256 of `data` in lower-case hex, through the system's own tool, so this crate needs
    /// no hashing dependency for one CI check.
    pub fn hex_sha256(data: &[u8]) -> String {
        let path = std::env::temp_dir().join(format!("v2-sftp-sha-{}", std::process::id()));
        std::fs::write(&path, data).unwrap();
        let out = if cfg!(windows) {
            std::process::Command::new("certutil")
                .arg("-hashfile")
                .arg(&path)
                .arg("SHA256")
                .output()
        } else {
            std::process::Command::new("sha256sum").arg(&path).output()
        }
        .unwrap();
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8_lossy(&out.stdout);
        text.split_whitespace()
            .find(|w| w.len() == 64 && w.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap_or_default()
            .to_ascii_lowercase()
    }
}
