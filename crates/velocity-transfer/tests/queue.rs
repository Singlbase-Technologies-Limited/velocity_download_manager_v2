//! The manager's download queue (#50) over the real engine, in both deployment modes: real
//! libcurl, real sockets, real files. The unit tests in `velocity-core` cover each queue rule with
//! the fake engine; this checks that the limit holds against a real one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use velocity_core::domain::{DownloadKind, DownloadStatus};
use velocity_core::manager::Manager;
use velocity_core::queue::QueueSettings;
use velocity_core::transfer::{DirectTransfer, Limits, TransferSpec};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::stdio::ChildProcessEngine;

const WAIT: Duration = Duration::from_secs(60);

fn engines() -> Vec<(&'static str, Box<dyn DirectTransfer>)> {
    vec![
        (
            "in-process",
            Box::new(InProcessEngine::start().expect("the in-process engine starts")),
        ),
        (
            "child process",
            Box::new(
                ChildProcessEngine::spawn(Path::new(env!("CARGO_BIN_EXE_velocity-transfer")))
                    .expect("the child engine starts"),
            ),
        ),
    ]
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-queue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spec(url: String, destination: PathBuf) -> TransferSpec {
    TransferSpec {
        urls: vec![url],
        directory: destination.parent().expect("a folder").to_owned(),
        file_name: destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        credentials: None,
        headers: Vec::new(),
        referrer: None,
        user_agent: None,
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        // One connection each, so the server's count of answers at once is the downloads'.
        max_connections: 1,
        resume: None,
        require_encryption: false,
    }
}

/// Statuses of a download the engine is running.
fn running(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Connecting
            | DownloadStatus::Downloading
            | DownloadStatus::Retrying
            | DownloadStatus::Stalled
            | DownloadStatus::Verifying
    )
}

#[test]
fn no_more_than_the_limit_run_at_once_against_the_real_engine() {
    // Four 400 kB files at 400 kB/s each: every download takes about a second, so they overlap
    // unless the queue holds them back.
    let files: Vec<(String, Vec<u8>)> = (0..4)
        .map(|n| (format!("f{n}.bin"), seeded_bytes(400_000, 50 + n)))
        .collect();
    let server = TestServer::start(
        files.clone(),
        ServerOptions {
            throttle_bytes_per_sec: Some(400_000),
            ..ServerOptions::default()
        },
    );
    for (mode, engine) in engines() {
        let dir = Dir::new(&mode.replace(' ', "-"));
        let mut m = Manager::new(engine);
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: 2,
            ..QueueSettings::default()
        })
        .unwrap();
        let ids: Vec<String> = files
            .iter()
            .map(|(name, _)| {
                m.add(
                    DownloadKind::Http,
                    spec(server.url(name), dir.0.join(name)),
                    0,
                )
                .expect("add")
            })
            .collect();
        let deadline = Instant::now() + WAIT;
        let mut most = 0;
        loop {
            m.pump(0);
            let statuses: Vec<DownloadStatus> =
                ids.iter().map(|id| m.get(id).unwrap().status).collect();
            most = most.max(statuses.iter().filter(|s| running(**s)).count());
            if statuses.iter().all(|s| s.is_terminal()) {
                assert!(
                    statuses.iter().all(|s| *s == DownloadStatus::Completed),
                    "{mode}: {statuses:?}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "{mode}: still {statuses:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(most <= 2, "{mode}: {most} ran at once");
        assert_eq!(most, 2, "{mode}: the limit was used");
        for (name, data) in &files {
            assert!(
                std::fs::read(dir.0.join(name)).unwrap() == *data,
                "{mode}: {name}"
            );
        }
    }
    assert!(
        server.counters().peak_answering <= 2,
        "the server answered {} at once",
        server.counters().peak_answering
    );
}
