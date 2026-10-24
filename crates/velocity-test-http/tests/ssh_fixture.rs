//! The OpenSSH fixture itself (#53): the machine's `sshd`, started without administrator rights,
//! serves a file over SFTP to the system's own `sftp` client, and a restart can change its host
//! key. Where `sshd` isn't installed, each test says NOT RUN and returns.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use velocity_test_http::seeded_bytes;
use velocity_test_http::ssh::{HostKey, SshServer};

fn sftp_client() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Windows\System32\OpenSSH\sftp.exe")
    } else {
        PathBuf::from("sftp")
    }
}

/// `sftp -b` downloading `name` to `to`, trusting whatever host key the server presents.
fn system_sftp_get(server: &SshServer, name: &str, to: &std::path::Path) -> bool {
    let batch = to.with_extension("batch");
    let remote = server.url(name);
    let remote = remote.split_once(&server.addr().to_string()).unwrap().1;
    let local = to.to_string_lossy().replace('\\', "/");
    std::fs::write(&batch, format!("get \"{remote}\" \"{local}\"\n")).unwrap();
    let status = Command::new(sftp_client())
        .arg("-q")
        .arg("-b")
        .arg(&batch)
        .arg("-i")
        .arg(server.client_key())
        .args(["-P", &server.addr().port().to_string()])
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
        ])
        .arg(format!("{}@127.0.0.1", server.user()))
        .stdin(Stdio::null())
        .status()
        .expect("sftp starts");
    status.success()
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

#[test]
fn the_system_sftp_client_downloads_a_served_file() {
    let data = seeded_bytes(200_000, 53);
    let server = server_or_skip!(vec![("f.bin".into(), data.clone())], HostKey::Ed25519);
    let to = std::env::temp_dir().join(format!("v2-ssh-fixture-{}.bin", std::process::id()));
    assert!(system_sftp_get(&server, "f.bin", &to), "sftp failed");
    let got = std::fs::read(&to).unwrap();
    let _ = std::fs::remove_file(&to);
    let _ = std::fs::remove_file(to.with_extension("batch"));
    assert!(got == data, "the downloaded file differs");
    assert!(
        server.host_key().starts_with("ssh-ed25519 "),
        "{}",
        server.host_key()
    );
    assert!(server.url("f.bin").starts_with("sftp://127.0.0.1:"));
}

#[test]
fn a_restart_can_present_a_new_host_key_on_the_same_port() {
    let mut server = server_or_skip!(vec![("f.bin".into(), vec![1, 2, 3])], HostKey::Ecdsa);
    let (port, before) = (server.addr().port(), server.host_key());
    assert!(before.starts_with("ecdsa-sha2-nistp256 "), "{before}");
    server.restart(false).expect("restarts");
    assert_eq!(server.host_key(), before);
    server.restart(true).expect("restarts with a new key");
    assert_ne!(server.host_key(), before);
    assert_eq!(server.addr().port(), port);
}

#[test]
fn an_rsa_host_key_can_be_presented() {
    let server = server_or_skip!(Vec::new(), HostKey::Rsa);
    assert!(
        server.host_key().starts_with("ssh-rsa "),
        "{}",
        server.host_key()
    );
}
