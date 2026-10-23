//! Retries at the engine (#48): a download that still fails after its own retries is started
//! again, at most `download_retries` times, and its snapshots show Retrying and Stalled. In the
//! app's process only, with short waits; the child process uses the production policy, which
//! the contract doesn't carry (the values become settings in #30).

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use velocity_core::domain::{DownloadStatus, FailureClass};
use velocity_core::transfer::{
    DirectTransfer, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};
use velocity_test_http::{seeded_bytes, Freeze, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::retry::RetryPolicy;

const WAIT: Duration = Duration::from_secs(30);

const FAST: RetryPolicy = RetryPolicy {
    first_wait: Duration::from_millis(50),
    max_wait: Duration::from_millis(200),
    throttled_wait: Duration::from_millis(100),
    max_retry_after: Duration::from_secs(600),
    failures_in_a_row: 5,
    stalled_after: Duration::from_millis(500),
    reset_after: Duration::from_secs(1),
    download_retries: 2,
    first_download_wait: Duration::from_millis(200),
    first_throttled_download_wait: Duration::from_millis(300),
    max_download_wait: Duration::from_secs(15 * 60),
};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-engine-retry-{name}-{}", std::process::id()));
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

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

fn spec(url: String, destination: PathBuf, connections: u32) -> TransferSpec {
    TransferSpec {
        urls: vec![url],
        destination,
        credentials: None,
        headers: Vec::new(),
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        max_connections: connections,
        resume: None,
    }
}

fn engine(policy: RetryPolicy) -> InProcessEngine {
    InProcessEngine::start_with(policy).expect("the engine starts")
}

/// Every snapshot of `id` until one satisfies `done`.
fn snapshots_until(
    events: &Receiver<TransferEvent>,
    id: TransferId,
    done: impl Fn(&TransferSnapshot) -> bool,
) -> Vec<TransferSnapshot> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if let TransferEvent::Changed { snapshot } =
            events.recv_timeout(left).expect("an event in time")
        {
            if snapshot.id == id {
                let stop = done(&snapshot);
                seen.push(snapshot);
                if stop {
                    return seen;
                }
            }
        }
    }
}

fn finished(s: &TransferSnapshot) -> bool {
    matches!(
        s.status,
        DownloadStatus::Completed | DownloadStatus::Failed | DownloadStatus::Removed
    )
}

const LEN: usize = 300_000;

fn serve(options: ServerOptions) -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(LEN, 49);
    (
        TestServer::start(vec![("f.bin".into(), data.clone())], options),
        data,
    )
}

#[test]
fn a_failed_download_is_started_again_and_finishes() {
    // The first run fails after its 5 tries; the restart gets through.
    let (s, data) = serve(ServerOptions {
        drop_first: FAST.failures_in_a_row,
        ..ServerOptions::default()
    });
    let dir = Dir::new("restart");
    let dest = dir.0.join("out.bin");
    let mut engine = engine(FAST);
    let events = engine.subscribe();
    let id = engine.add(spec(s.url("f.bin"), dest.clone(), 4)).unwrap();
    let seen = snapshots_until(&events, id, finished);
    assert_eq!(seen.last().unwrap().status, DownloadStatus::Completed);
    assert!(std::fs::read(&dest).unwrap() == data);
    let waiting = seen
        .iter()
        .find(|s| {
            s.status == DownloadStatus::Retrying
                && s.error
                    .as_ref()
                    .is_some_and(|e| e.message.contains("Retrying automatically (1 of 2)"))
        })
        .unwrap_or_else(|| panic!("a restart wait in {seen:?}"));
    assert_eq!(
        waiting.error.as_ref().unwrap().class,
        FailureClass::Transient
    );
    assert_eq!(
        seen.last().unwrap().error,
        None,
        "a finished download has no error"
    );
    assert_eq!(s.counters().gets, u64::from(FAST.failures_in_a_row) + 1);
}

#[test]
fn a_download_that_keeps_failing_fails_after_its_restarts() {
    let (s, _) = serve(ServerOptions {
        drop_first: 1000,
        ..ServerOptions::default()
    });
    let dir = Dir::new("keeps-failing");
    let dest = dir.0.join("out.bin");
    let mut engine = engine(FAST);
    let events = engine.subscribe();
    let id = engine.add(spec(s.url("f.bin"), dest.clone(), 4)).unwrap();
    let last = snapshots_until(&events, id, finished).pop().unwrap();
    assert_eq!(last.status, DownloadStatus::Failed);
    let error = last.error.expect("an error");
    assert!(
        error.retryable,
        "the user may still retry by hand: {error:?}"
    );
    assert!(
        !error.message.contains("automatically"),
        "{}",
        error.message
    );
    // Three runs (the first and two restarts), each giving up after its 5 tries.
    let runs = 1 + u64::from(FAST.download_retries);
    assert_eq!(s.counters().gets, runs * u64::from(FAST.failures_in_a_row));
    assert!(!dest.exists() && !part_of(&dest).exists());
}

#[test]
fn a_permanent_failure_is_never_started_again() {
    let (s, _) = serve(ServerOptions::default());
    let dir = Dir::new("permanent");
    let dest = dir.0.join("out.bin");
    let mut engine = engine(FAST);
    let events = engine.subscribe();
    let id = engine.add(spec(s.url("gone.bin"), dest, 4)).unwrap();
    let seen = snapshots_until(&events, id, finished);
    assert_eq!(seen.last().unwrap().status, DownloadStatus::Failed);
    assert!(
        seen.iter().all(|s| s.status != DownloadStatus::Retrying),
        "{seen:?}"
    );
    assert_eq!(s.counters().gets, 1);
}

#[test]
fn cancelling_while_waiting_to_start_again_removes_it() {
    let (s, _) = serve(ServerOptions {
        drop_first: 1000,
        ..ServerOptions::default()
    });
    let dir = Dir::new("cancel-restart");
    let dest = dir.0.join("out.bin");
    let mut engine = engine(RetryPolicy {
        first_download_wait: Duration::from_secs(30),
        ..FAST
    });
    let events = engine.subscribe();
    let id = engine.add(spec(s.url("f.bin"), dest.clone(), 4)).unwrap();
    snapshots_until(&events, id, |s| {
        s.error
            .as_ref()
            .is_some_and(|e| e.message.contains("automatically"))
    });
    let asked = Instant::now();
    engine.cancel(id).expect("cancel");
    let last = snapshots_until(&events, id, finished).pop().unwrap();
    assert_eq!(last.status, DownloadStatus::Removed);
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "{:?}",
        asked.elapsed()
    );
    assert_eq!(s.counters().gets, u64::from(FAST.failures_in_a_row));
    assert!(!part_of(&dest).exists());
}

#[test]
fn a_stall_shows_in_the_snapshot() {
    let dir = Dir::new("stall");
    let dest = dir.0.join("out.bin");
    let mut engine = engine(FAST);
    let events = engine.subscribe();
    // One connection, so a frozen segment stalls the whole download. The engine's probe is
    // 4 MiB, so the file is larger than that, for a later range to freeze.
    let big = seeded_bytes(8 << 20, 50);
    let s = TestServer::start(
        vec![("f.bin".into(), big.clone())],
        ServerOptions {
            freeze_later_range: Some(Freeze {
                after: 16 * 1024,
                hold: Duration::from_secs(20),
            }),
            ..ServerOptions::default()
        },
    );
    let id = engine.add(spec(s.url("f.bin"), dest.clone(), 1)).unwrap();
    let seen = snapshots_until(&events, id, finished);
    assert_eq!(seen.last().unwrap().status, DownloadStatus::Completed);
    assert!(std::fs::read(&dest).unwrap() == big);
    assert!(
        seen.iter().any(|s| s.status == DownloadStatus::Stalled),
        "{:?}",
        seen.iter().map(|s| s.status).collect::<Vec<_>>()
    );
}
