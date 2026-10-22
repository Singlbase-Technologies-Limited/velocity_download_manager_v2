//! The prototype engine behind the `DirectTransfer` contract (#36), in both deployment modes
//! (#37, #38): in the app's process, and as a child process speaking the framed protocol over
//! stdio (#39). One test function per behaviour runs against both, so they can't drift apart.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use velocity_core::domain::{DownloadKind, DownloadStatus, ErrorCode};
use velocity_core::transfer::{
    DirectTransfer, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::stdio::ChildProcessEngine;

const WAIT: Duration = Duration::from_secs(30);

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
        let p = std::env::temp_dir().join(format!("v2-engine-{name}-{}", std::process::id()));
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
        destination,
        credentials: None,
        headers: Vec::new(),
        checksum: None,
        limits: Limits::default(),
        max_connections: 4,
        resume: None,
    }
}

/// Waits for a snapshot of `id` whose status satisfies `done`.
fn wait_for(
    events: &Receiver<TransferEvent>,
    id: TransferId,
    done: impl Fn(DownloadStatus) -> bool,
) -> TransferSnapshot {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left).expect("an event in time") {
            TransferEvent::Changed { snapshot } if snapshot.id == id && done(snapshot.status) => {
                return snapshot
            }
            _ => {}
        }
    }
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

#[test]
fn reports_its_capabilities_honestly() {
    for (mode, engine) in engines() {
        let c = engine.capabilities();
        assert_eq!(
            c.protocols,
            vec![DownloadKind::Http],
            "{mode}: only HTTP(S) in the prototype"
        );
        assert!(
            !c.live_connection_changes && !c.validator_resume && !c.metalink,
            "{mode}"
        );
        assert!(c.max_connections_per_host >= 1, "{mode}");
    }
}

#[test]
fn downloads_a_file_and_reports_completion() {
    let data = seeded_bytes(800_000, 31);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("ok-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("out.bin");
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("f.bin"), dest.clone()))
            .expect("add");
        let done = wait_for(&events, id, |s| {
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        assert_eq!(
            done.status,
            DownloadStatus::Completed,
            "{mode}: {:?}",
            done.error
        );
        assert_eq!(
            (done.progress.completed_bytes, done.progress.total_bytes),
            (800_000, Some(800_000)),
            "{mode}"
        );
        assert!(
            std::fs::read(&dest).unwrap() == data,
            "{mode}: the file differs from the source"
        );
        let snapshots = engine.snapshot().expect("snapshot");
        assert!(
            snapshots
                .iter()
                .any(|s| s.id == id && s.status == DownloadStatus::Completed),
            "{mode}"
        );
    }
}

#[test]
fn reports_a_failure_with_its_status() {
    let server = TestServer::start(vec![], ServerOptions::default());
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("fail-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("out.bin");
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("missing.bin"), dest.clone()))
            .expect("add");
        let done = wait_for(&events, id, |s| {
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        assert_eq!(done.status, DownloadStatus::Failed, "{mode}");
        assert_eq!(
            done.error.as_ref().and_then(|e| e.http_status),
            Some(404),
            "{mode}"
        );
        assert!(
            !dest.exists() && !part_of(&dest).exists(),
            "{mode}: nothing left behind"
        );
    }
}

#[test]
fn cancel_ends_as_removed_and_deletes_the_part_file() {
    let data = seeded_bytes(4_000_000, 32);
    let server = TestServer::start(
        vec![("big.bin".into(), data)],
        ServerOptions {
            throttle_bytes_per_sec: Some(300_000),
            ..ServerOptions::default()
        },
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("cancel-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("out.bin");
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("big.bin"), dest.clone()))
            .expect("add");
        wait_for(&events, id, |s| s == DownloadStatus::Downloading);
        engine.cancel(id).expect("cancel");
        let done = wait_for(&events, id, |s| {
            matches!(
                s,
                DownloadStatus::Removed | DownloadStatus::Completed | DownloadStatus::Failed
            )
        });
        assert_eq!(done.status, DownloadStatus::Removed, "{mode}");
        assert!(
            !dest.exists() && !part_of(&dest).exists(),
            "{mode}: the part file is deleted"
        );
    }
}

#[test]
fn remove_forgets_a_finished_transfer() {
    let server = TestServer::start(
        vec![("s.bin".into(), seeded_bytes(1000, 1))],
        ServerOptions::default(),
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("remove-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("out.bin");
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("s.bin"), dest.clone()))
            .expect("add");
        wait_for(&events, id, |s| s == DownloadStatus::Completed);
        engine.remove(id).expect("remove");
        let deadline = Instant::now() + WAIT;
        loop {
            match events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("the Removed event")
            {
                TransferEvent::Removed { id: removed } if removed == id => break,
                _ => {}
            }
        }
        assert!(
            engine
                .snapshot()
                .expect("snapshot")
                .iter()
                .all(|s| s.id != id),
            "{mode}"
        );
        assert!(dest.exists(), "{mode}: remove keeps the file");
    }
}

#[test]
fn refuses_bad_commands_with_their_codes() {
    for (mode, mut engine) in engines() {
        assert_eq!(
            engine.pause(TransferId(999)),
            Err(ErrorCode::NotFound),
            "{mode}"
        );
        assert_eq!(
            engine.add(spec("ftp://example.com/a".into(), PathBuf::from("a"))),
            Err(ErrorCode::UnsupportedScheme),
            "{mode}"
        );
        assert_eq!(
            engine.add(spec("not a url".into(), PathBuf::from("a"))),
            Err(ErrorCode::InvalidUrl),
            "{mode}"
        );
        let mut empty = spec("https://example.com/a".into(), PathBuf::from("a"));
        empty.urls.clear();
        assert_eq!(engine.add(empty), Err(ErrorCode::InvalidInput), "{mode}");
    }
}

#[test]
fn a_child_whose_parent_goes_away_cancels_and_cleans_up() {
    let server = TestServer::start(
        vec![("big.bin".into(), seeded_bytes(4_000_000, 34))],
        ServerOptions {
            throttle_bytes_per_sec: Some(300_000),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("orphan");
    let dest = dir.0.join("out.bin");
    let engine = ChildProcessEngine::spawn(Path::new(env!("CARGO_BIN_EXE_velocity-transfer")))
        .expect("the child engine starts");
    let events = engine.subscribe();
    let id = engine
        .add(spec(server.url("big.bin"), dest.clone()))
        .expect("add");
    wait_for(&events, id, |s| s == DownloadStatus::Downloading);
    assert!(part_of(&dest).exists(), "the download is under way");
    // Dropping the client closes the child's stdin and waits for it to exit.
    drop(engine);
    assert!(
        !dest.exists() && !part_of(&dest).exists(),
        "the child removed its part file before exiting"
    );
}

#[test]
fn pause_is_unsupported_until_checkpoints_exist() {
    let data = seeded_bytes(4_000_000, 33);
    let server = TestServer::start(
        vec![("big.bin".into(), data)],
        ServerOptions {
            throttle_bytes_per_sec: Some(300_000),
            ..ServerOptions::default()
        },
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("pause-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("big.bin"), dir.0.join("out.bin")))
            .expect("add");
        wait_for(&events, id, |s| s == DownloadStatus::Downloading);
        assert_eq!(
            engine.pause(id),
            Err(ErrorCode::Unsupported),
            "{mode}: no pause without checkpoints (#43)"
        );
        engine.cancel(id).expect("cancel");
        wait_for(&events, id, |s| s == DownloadStatus::Removed);
    }
}
