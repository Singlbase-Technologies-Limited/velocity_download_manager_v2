//! #70's acceptance test, on #71's shared scan: after downloads with an HTTP login and cookie, an
//! FTP login, and SFTP key and password logins, no secret is anywhere V2 writes or shows: the
//! database's files (WAL included), the checkpoints in it, the part and finished files, temporary
//! files, the app's log, the diagnostics export, or a child process's arguments and environment
//! (the running engine on Linux, the command it's started with everywhere). Real processes, real
//! files and the local fixtures (docs/SECURITY.md#secrets-70, docs/TESTING.md).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tracing_subscriber::layer::SubscriberExt;
use velocity_core::database::Database;
use velocity_core::domain::{DownloadKind, DownloadStatus};
use velocity_core::logs::AppLog;
use velocity_core::manager::Manager;
use velocity_core::transfer::{Credentials, Header, SshKey, TransferSpec};
use velocity_test_http::ftp::{FtpOptions, FtpServer};
use velocity_test_http::leaks::LeakScan;
use velocity_test_http::ssh::{HostKey, PasswordServer, SshServer, KEY_PASSPHRASE};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::supervisor::{SupervisedEngine, SupervisorOptions};

const WAIT: Duration = Duration::from_secs(60);
const HTTP_PASSWORD: &str = "LEAK-HTTP-PASSWORD-70";
const HTTP_COOKIE: &str = "LEAK-HTTP-COOKIE-70";
const FTP_PASSWORD: &str = "LEAK-FTP-PASSWORD-70";

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_velocity-transfer"))
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-leaks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("files")).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn spec(url: String, dir: &Path, name: &str) -> TransferSpec {
    // From JSON, as the engine's pipe carries it: a field added later takes its default.
    serde_json::from_value(serde_json::json!({
        "urls": [url],
        "directory": dir,
        "fileName": name,
        "credentials": null,
        "requireEncryption": false,
        "sshKey": null,
        "headers": [],
        "referrer": null,
        "userAgent": null,
        "checksum": null,
        "pieces": null,
        "limits": { "maxDownloadBps": null },
        "maxConnections": 4,
        "resume": null,
    }))
    .expect("a spec")
}

/// Pumps `m` until `done` holds for `id`.
fn until(m: &mut Manager, id: &str, what: &str, done: impl Fn(&Manager) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done(m) {
        m.pump(now());
        assert!(
            Instant::now() < deadline,
            "{id} never {what}: {:?}",
            m.get(id)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn status(m: &Manager, id: &str) -> Option<DownloadStatus> {
    m.get(id).map(|r| r.status)
}

/// Logs every download's record, one line each, the way the app logs what the manager reports:
/// the core logs nothing while a download runs, so the log check needs these lines to read.
fn log_records(m: &Manager) {
    for record in m.snapshot().downloads {
        tracing::info!("{record:?}");
    }
}

/// Scans the app's log and the diagnostics export, as the user would save it.
fn scan_log_and_export(scan: &mut LeakScan, log: &AppLog, m: &Manager) {
    for line in log.lines() {
        scan.text("the app's log", &line.message);
    }
    let export = m.diagnostics(log.lines(), now()).export().unwrap();
    scan.text("the diagnostics export", &export);
}

#[test]
fn http_and_ftp_logins_leave_no_trace_through_the_child_engine() {
    let log = AppLog::new();
    let _logging =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let since = SystemTime::now();
    let dir = Dir::new("child");
    let data = seeded_bytes(2_000_000, 70);
    let http = TestServer::start(
        vec![("a.bin".into(), data.clone())],
        ServerOptions {
            throttle_bytes_per_sec: Some(1_000_000),
            ..ServerOptions::default()
        },
    );
    let ftp = FtpServer::start(
        vec![("/pub/b.bin".into(), data.clone())],
        FtpOptions {
            login: Some(("ftp-user".to_owned(), FTP_PASSWORD.to_owned())),
            ..FtpOptions::default()
        },
    );
    let mut scan = LeakScan::new(&[HTTP_PASSWORD, HTTP_COOKIE, FTP_PASSWORD]);
    {
        let db = Arc::new(Database::open(&dir.0.join("velocity.sqlite3")).unwrap());
        let engine = SupervisedEngine::start(program(), SupervisorOptions::default()).unwrap();
        let pid = engine.process_id().expect("the engine runs");
        let mut m = Manager::open(Box::new(engine), Arc::clone(&db)).unwrap();
        let files = dir.0.join("files");
        let http_id = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    credentials: Some(Credentials {
                        user_name: "http-user".to_owned(),
                        password: HTTP_PASSWORD.into(),
                    }),
                    headers: vec![Header {
                        name: "Cookie".to_owned(),
                        value: format!("session={HTTP_COOKIE}").into(),
                    }],
                    ..spec(http.url("a.bin"), &files, "a.bin")
                },
                now(),
            )
            .unwrap();
        let ftp_id = m
            .add(
                DownloadKind::Ftp,
                TransferSpec {
                    credentials: Some(Credentials {
                        user_name: "ftp-user".to_owned(),
                        password: FTP_PASSWORD.into(),
                    }),
                    ..spec(ftp.url("pub/b.bin"), &files, "b.bin")
                },
                now(),
            )
            .unwrap();
        // Paused part-way, so a checkpoint is stored while the secrets are in use.
        until(&mut m, &http_id, "made progress", |m| {
            m.get(&http_id)
                .is_some_and(|r| r.progress.completed_bytes > 300_000)
        });
        m.pause(&http_id).unwrap();
        until(&mut m, &http_id, "paused", |m| {
            status(m, &http_id) == Some(DownloadStatus::Paused)
        });
        m.pump(now());
        // While the logins are in use: the files, and the engine as it runs.
        scan.files(&dir.0).process("the running engine", pid);
        m.resume(&http_id).unwrap();
        for id in [&http_id, &ftp_id] {
            until(&mut m, id, "completed", |m| {
                status(m, id) == Some(DownloadStatus::Completed)
            });
        }
        m.pump(now());
        log_records(&m);
        // With the WAL still open.
        scan.files(&dir.0);
        scan_log_and_export(&mut scan, &log, &m);
    }
    // Closed: the database's files as they stay on disk, and any temporary file written meanwhile.
    scan.files(&dir.0).temp_files(since);
    scan.assert_clean();
    assert!(!log.lines().is_empty(), "the log was captured");
}

/// A child is started with one fixed argument and a cleared environment, so no secret can be in
/// either (the login crosses its stdin pipe instead).
#[test]
fn a_child_engine_gets_no_secret_in_its_arguments_or_environment() {
    let command = velocity_child::client::child_command(&program());
    let args: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["serve"]);
    let names: Vec<_> = command
        .get_envs()
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    for name in &names {
        assert!(
            velocity_child::client::KEPT_ENV.contains(&name.as_str()),
            "{name}"
        );
    }
}

/// SFTP key and password logins, on the in-process engine with a known_hosts file of the test's
/// own (the child engine would use the user's). Where sshd isn't installed, or no password server
/// is described, that part says NOT RUN and returns, as the SFTP engine tests do.
#[test]
fn sftp_key_and_password_logins_leave_no_trace() {
    let log = AppLog::new();
    let _logging =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let since = SystemTime::now();
    let dir = Dir::new("sftp");
    let files = dir.0.join("files");
    let db = Arc::new(Database::open(&dir.0.join("velocity.sqlite3")).unwrap());
    let engine = InProcessEngine::start_with_known_hosts(dir.0.join("known_hosts")).unwrap();
    let mut m = Manager::open(Box::new(engine), Arc::clone(&db)).unwrap();
    let mut secrets = vec![KEY_PASSPHRASE.to_owned()];

    match SshServer::start(
        vec![("k.bin".into(), seeded_bytes(200_000, 71))],
        HostKey::Ed25519,
    ) {
        Ok(server) => {
            let id = m
                .add(
                    DownloadKind::Sftp,
                    TransferSpec {
                        credentials: Some(Credentials {
                            user_name: server.user().to_owned(),
                            password: Default::default(),
                        }),
                        ssh_key: Some(SshKey {
                            path: server.client_key_with_passphrase(),
                            passphrase: Some(KEY_PASSPHRASE.into()),
                        }),
                        ..spec(server.url("k.bin"), &files, "k.bin")
                    },
                    now(),
                )
                .unwrap();
            // The server's key is unknown at first: trusted as the user would, then retried.
            until(&mut m, &id, "stopped at the host key", |m| {
                status(m, &id) == Some(DownloadStatus::Failed)
            });
            let check = m
                .get(&id)
                .and_then(|r| r.error.clone())
                .and_then(|e| e.host_key)
                .expect("a host key");
            m.trust_host_key(&check.host, check.port, &check.fingerprint)
                .unwrap();
            m.retry(&id).unwrap();
            until(&mut m, &id, "completed", |m| {
                status(m, &id) == Some(DownloadStatus::Completed)
            });
        }
        Err(why) => eprintln!("SFTP key login: NOT RUN ({why})"),
    }
    match PasswordServer::from_env() {
        Ok(server) => {
            secrets.push(server.password.clone());
            let id = m
                .add(
                    DownloadKind::Sftp,
                    TransferSpec {
                        credentials: Some(Credentials {
                            user_name: server.user.clone(),
                            password: server.password.clone().into(),
                        }),
                        ..spec(server.url(), &files, "p.bin")
                    },
                    now(),
                )
                .unwrap();
            until(&mut m, &id, "stopped at the host key", |m| {
                status(m, &id) == Some(DownloadStatus::Failed)
            });
            if let Some(check) = m
                .get(&id)
                .and_then(|r| r.error.clone())
                .and_then(|e| e.host_key)
            {
                m.trust_host_key(&check.host, check.port, &check.fingerprint)
                    .unwrap();
                m.retry(&id).unwrap();
            }
            until(&mut m, &id, "completed", |m| {
                status(m, &id) == Some(DownloadStatus::Completed)
            });
        }
        Err(why) => eprintln!("SFTP password login: NOT RUN ({why})"),
    }
    m.pump(now());
    log_records(&m);
    let mut scan = LeakScan::new(&secrets);
    scan_log_and_export(&mut scan, &log, &m);
    drop(m);
    scan.files(&dir.0).temp_files(since).assert_clean();
}
