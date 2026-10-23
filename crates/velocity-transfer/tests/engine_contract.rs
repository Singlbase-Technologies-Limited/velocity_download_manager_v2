//! The prototype engine behind the `DirectTransfer` contract (#36), in both deployment modes
//! (#37, #38): in the app's process, and as a child process speaking the framed protocol over
//! stdio (#39). One test function per behaviour runs against both, so they can't drift apart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use velocity_core::domain::{
    ConnectionDetail, ConnectionLimit, DownloadKind, DownloadStatus, ErrorCode,
    ProtocolCapabilities, Verification,
};
use velocity_core::transfer::{
    Checkpoint, Checksum, DirectTransfer, EngineLimits, HashAlgorithm, Limits, PieceHashes,
    TransferEvent, TransferId, TransferSnapshot, TransferSpec,
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
        pieces: None,
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
        // Only HTTP(S) in the prototype: segmented (#42), resumed (#43) against a strong validator
        // (#45), over HTTP/2 when the server offers it (#47).
        assert_eq!(
            c.protocols,
            vec![ProtocolCapabilities {
                kind: DownloadKind::Http,
                segmented: true,
                resume: true,
                validator_resume: true,
                http2: true,
            }],
            "{mode}"
        );
        // Live connection changes came with #47.
        assert!(c.live_connection_changes, "{mode}");
        assert!(!c.metalink, "{mode}");
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
        assert_eq!(
            done.verification,
            Some(Verification::SizeVerified),
            "{mode}: complete without a checksum"
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
fn checks_a_checksum_before_reporting_completion() {
    use sha2::{Digest, Sha256};
    let data = seeded_bytes(400_000, 46);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    let digest: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("checksum-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();

        let dest = dir.0.join("ok.bin");
        let mut ok = spec(server.url("f.bin"), dest.clone());
        // Uppercase, as a user may paste it.
        ok.checksum = Some(Checksum {
            algorithm: HashAlgorithm::Sha256,
            hex: digest.clone(),
        });
        let id = engine.add(ok).expect("add");
        let done = wait_for(&events, id, |s| {
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        assert_eq!(done.status, DownloadStatus::Completed, "{mode}: {done:?}");
        assert_eq!(done.verification, Some(Verification::Verified), "{mode}");
        assert!(std::fs::read(&dest).unwrap() == data, "{mode}");

        let dest = dir.0.join("bad.bin");
        let mut bad = spec(server.url("f.bin"), dest.clone());
        bad.checksum = Some(Checksum {
            algorithm: HashAlgorithm::Sha256,
            hex: "0".repeat(64),
        });
        let id = engine.add(bad).expect("add");
        let seen = std::cell::RefCell::new(Vec::new());
        let done = wait_for(&events, id, |s| {
            seen.borrow_mut().push(s);
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        let seen = seen.into_inner();
        assert_eq!(done.status, DownloadStatus::Failed, "{mode}: {done:?}");
        assert_eq!(done.verification, None, "{mode}");
        let message = done.error.map(|e| e.message).unwrap_or_default();
        assert!(message.contains("SHA-256 checksum"), "{mode}: {message}");
        assert!(
            !seen.contains(&DownloadStatus::Retrying),
            "{mode}: not restarted on its own: {seen:?}"
        );
        assert!(!dest.exists(), "{mode}: not renamed");
        assert!(part_of(&dest).exists(), "{mode}: kept for inspection");
    }
}

#[test]
fn checks_piece_hashes_and_refuses_a_malformed_list() {
    use sha2::{Digest, Sha256};
    const PIECE: usize = 64 * 1024;
    let data = seeded_bytes(5 * PIECE + 100, 54);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    let hex: Vec<String> = data
        .chunks(PIECE)
        .map(|c| {
            Sha256::digest(c)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .collect();
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("pieces-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let dest = dir.0.join("ok.bin");
        let mut ok = spec(server.url("f.bin"), dest.clone());
        ok.pieces = Some(PieceHashes {
            algorithm: HashAlgorithm::Sha256,
            length: PIECE as u64,
            hex: hex.clone(),
        });
        let id = engine.add(ok).expect("add");
        let done = wait_for(&events, id, |s| {
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        assert_eq!(done.status, DownloadStatus::Completed, "{mode}: {done:?}");
        assert_eq!(done.verification, Some(Verification::Verified), "{mode}");
        assert!(std::fs::read(&dest).unwrap() == data, "{mode}");

        let mut bad = spec(server.url("f.bin"), dir.0.join("bad.bin"));
        bad.pieces = Some(PieceHashes {
            algorithm: HashAlgorithm::Sha256,
            length: PIECE as u64,
            hex: vec!["not hex".to_owned(); 6],
        });
        assert_eq!(engine.add(bad), Err(ErrorCode::InvalidInput), "{mode}");
    }
}

#[test]
fn engine_limits_set_the_per_server_cap_live_and_are_checked() {
    // #57: the profile's per-server cap. A slow download keeps running while the cap changes.
    let server = TestServer::start(
        vec![("f.bin".into(), seeded_bytes(2_000_000, 57))],
        ServerOptions {
            throttle_bytes_per_sec: Some(200_000),
            ..ServerOptions::default()
        },
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("engine-limits-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        assert_eq!(
            engine.capabilities().max_connections_per_host,
            8,
            "{mode}: Balanced"
        );
        let id = engine
            .add(spec(server.url("f.bin"), dir.0.join("f.bin")))
            .expect("add");
        assert_eq!(
            engine.set_connections(id, 16),
            Ok(8),
            "{mode}: at most 8 by default"
        );
        let maximum = EngineLimits {
            max_connections_per_host: 16,
            ..EngineLimits::DEFAULT
        };
        engine
            .set_engine_limits(maximum)
            .expect("set_engine_limits");
        assert_eq!(engine.capabilities().max_connections_per_host, 16, "{mode}");
        assert_eq!(
            engine.set_connections(id, 16),
            Ok(16),
            "{mode}: Maximum allows 16"
        );
        let conservative = EngineLimits {
            max_connections_per_host: 2,
            min_segment_bytes: 20 << 20,
        };
        engine
            .set_engine_limits(conservative)
            .expect("set_engine_limits");
        let held = wait_for_snapshot(&events, id, |s| {
            s.progress.connection_detail.is_some_and(|d| {
                d.allowed <= 2 && d.limited_by == Some(ConnectionLimit::PerHostCap)
            })
        });
        assert_eq!(
            held.progress.connection_detail.unwrap().requested,
            16,
            "{mode}"
        );
        let too_many = EngineLimits {
            max_connections_per_host: 17,
            ..EngineLimits::DEFAULT
        };
        assert_eq!(
            engine.set_engine_limits(too_many),
            Err(ErrorCode::InvalidInput),
            "{mode}"
        );
        engine.cancel(id).expect("cancel");
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
        let mut short = spec("https://example.com/a".into(), PathBuf::from("a"));
        short.checksum = Some(Checksum {
            algorithm: HashAlgorithm::Sha256,
            hex: "ab".repeat(31),
        });
        assert_eq!(engine.add(short), Err(ErrorCode::InvalidInput), "{mode}");
    }
}

#[test]
fn a_child_whose_parent_goes_away_stops_and_keeps_its_part_file() {
    // #43 changes #39's behaviour: when the app goes away, the engine pauses its transfers and
    // keeps their part files, so the app can resume them from their checkpoints when it starts
    // again, instead of cancelling and deleting them.
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
        part_of(&dest).exists(),
        "the part file is kept for a resume"
    );
    assert!(!dest.exists(), "nothing looks finished");
    // The child stopped: once the last chunk the server had under way is counted, it sends
    // nothing more. (The test server counts a chunk before writing it, and on Windows a write into
    // a socket its peer just closed can still succeed.)
    std::thread::sleep(Duration::from_millis(300));
    let sent = server.counters().body_bytes_sent;
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        server.counters().body_bytes_sent,
        sent,
        "the child still downloads"
    );
}

/// The last checkpoint event for `id` among `events` received so far, after waiting for `status`.
fn wait_with_checkpoint(
    events: &Receiver<TransferEvent>,
    id: TransferId,
    status: DownloadStatus,
) -> (TransferSnapshot, Option<Checkpoint>) {
    let deadline = Instant::now() + WAIT;
    let mut last = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left).expect("an event in time") {
            TransferEvent::Checkpoint { id: of, checkpoint } if of == id => last = Some(checkpoint),
            TransferEvent::Changed { snapshot }
                if snapshot.id == id && snapshot.status == status =>
            {
                return (snapshot, last)
            }
            _ => {}
        }
    }
}

#[test]
fn a_paused_transfer_keeps_its_part_file_and_resumes_from_its_checkpoint() {
    let data = seeded_bytes(4_000_000, 33);
    let server = TestServer::start(
        vec![("big.bin".into(), data.clone())],
        ServerOptions {
            throttle_bytes_per_sec: Some(300_000),
            ..ServerOptions::default()
        },
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("pause-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("out.bin");
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("big.bin"), dest.clone()))
            .expect("add");
        wait_for(&events, id, |s| s == DownloadStatus::Downloading);
        engine.pause(id).expect("pause");
        let (paused, checkpoint) = wait_with_checkpoint(&events, id, DownloadStatus::Paused);
        let checkpoint = checkpoint.expect("a checkpoint at the pause");
        assert!(!checkpoint.durable.is_empty(), "{mode}");
        assert!(part_of(&dest).exists() && !dest.exists(), "{mode}");
        assert_eq!(
            engine.pause(id),
            Err(ErrorCode::InvalidState),
            "{mode}: already paused"
        );
        assert!(paused.progress.completed_bytes > 0, "{mode}");

        engine.resume(id).expect("resume");
        let done = wait_for(&events, id, |s| {
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        assert_eq!(
            done.status,
            DownloadStatus::Completed,
            "{mode}: {:?}",
            done.error
        );
        assert!(
            std::fs::read(&dest).unwrap() == data,
            "{mode}: the file differs from the source"
        );
        assert_eq!(
            engine.resume(id),
            Err(ErrorCode::InvalidState),
            "{mode}: nothing to resume"
        );
    }
}

#[test]
fn a_paused_transfer_can_be_cancelled_and_its_part_file_goes() {
    let server = TestServer::start(
        vec![("big.bin".into(), seeded_bytes(4_000_000, 35))],
        ServerOptions {
            throttle_bytes_per_sec: Some(300_000),
            ..ServerOptions::default()
        },
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("pause-cancel-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("out.bin");
        let events = engine.subscribe();
        let id = engine
            .add(spec(server.url("big.bin"), dest.clone()))
            .expect("add");
        wait_for(&events, id, |s| s == DownloadStatus::Downloading);
        engine.pause(id).expect("pause");
        wait_for(&events, id, |s| s == DownloadStatus::Paused);
        engine.cancel(id).expect("cancel");
        wait_for(&events, id, |s| s == DownloadStatus::Removed);
        assert!(!part_of(&dest).exists() && !dest.exists(), "{mode}");
    }
}

/// How long it takes transfer `id` to complete, from now.
fn time_to_complete(events: &Receiver<TransferEvent>, id: TransferId) -> Duration {
    let started = Instant::now();
    let done = wait_for(events, id, |s| {
        matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
    });
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    started.elapsed()
}

#[test]
fn speed_limits_are_checked_and_applied_live() {
    let _speed = one_speed_test_at_a_time();
    // #49: a limit in the spec paces the transfer, set_limits changes it while it runs, and the
    // global limit applies to every transfer. 512 KiB at 256 KiB/s takes about 2 s.
    let data = seeded_bytes(512 * 1024, 32);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    let slow = Limits {
        max_download_bps: Some(256 * 1024),
    };
    let too_low = Limits {
        max_download_bps: Some(Limits::MIN_BPS - 1),
    };
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("limits-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();

        let mut limited = spec(server.url("f.bin"), dir.0.join("limited.bin"));
        limited.limits = slow;
        let id = engine.add(limited).expect("add");
        let took = time_to_complete(&events, id);
        assert!(took >= Duration::from_millis(1700), "{mode}: {took:?}");

        // Lifted while it runs: it finishes long before 2 s.
        let mut lifted = spec(server.url("f.bin"), dir.0.join("lifted.bin"));
        lifted.limits = slow;
        let id = engine.add(lifted).expect("add");
        wait_for(&events, id, |s| s == DownloadStatus::Downloading);
        engine
            .set_limits(id, Limits::default())
            .expect("set_limits");
        let took = time_to_complete(&events, id);
        assert!(took < Duration::from_millis(1500), "{mode}: {took:?}");

        engine.set_global_limits(slow).expect("set_global_limits");
        let id = engine
            .add(spec(server.url("f.bin"), dir.0.join("global.bin")))
            .expect("add");
        let took = time_to_complete(&events, id);
        assert!(took >= Duration::from_millis(1700), "{mode}: {took:?}");
        engine
            .set_global_limits(Limits::default())
            .expect("lift the global limit");

        assert_eq!(
            engine.set_limits(id, too_low),
            Err(ErrorCode::InvalidInput),
            "{mode}"
        );
        assert_eq!(
            engine.set_global_limits(too_low),
            Err(ErrorCode::InvalidInput),
            "{mode}"
        );
        let mut bad = spec(server.url("f.bin"), dir.0.join("bad.bin"));
        bad.limits = too_low;
        assert_eq!(engine.add(bad), Err(ErrorCode::InvalidInput), "{mode}");
    }
}

/// Waits for a snapshot of `id` that satisfies `done`.
fn wait_for_snapshot(
    events: &Receiver<TransferEvent>,
    id: TransferId,
    done: impl Fn(&TransferSnapshot) -> bool,
) -> TransferSnapshot {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left).expect("an event in time") {
            TransferEvent::Changed { snapshot } if snapshot.id == id && done(&snapshot) => {
                return snapshot
            }
            _ => {}
        }
    }
}

/// Polls `done` every 20 ms; panics with `what` after `timeout`.
fn wait_until(timeout: Duration, what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn connections_change_live_within_the_cap() {
    let _speed = one_speed_test_at_a_time();
    // #47: 1 MiB/s per connection, so each added connection helps, and a 48 MiB file is still
    // running while the engine ramps up to a raised count, one connection per 2 s window.
    let data = seeded_bytes(48 << 20, 47);
    for (mode, mut engine) in engines() {
        let server = TestServer::start(
            vec![("f.bin".into(), data.clone())],
            ServerOptions {
                throttle_bytes_per_sec: Some(1 << 20),
                ..ServerOptions::default()
            },
        );
        let cap = engine.capabilities().max_connections_per_host;
        let dir = Dir::new(&format!("connections-{}", mode.replace(' ', "-")));
        let dest = dir.0.join("f.bin");
        let events = engine.subscribe();
        let mut one = spec(server.url("f.bin"), dest.clone());
        one.max_connections = 1;
        let id = engine.add(one).expect("add");

        // The probe, then one range at a time.
        wait_until(WAIT, "a range follows the probe", || {
            server.counters().gets >= 2
        });
        assert_eq!(engine.set_connections(id, 4), Ok(4), "{mode}");
        // Raised, it ramps up to 4 rather than jumping (PO decision on #47), and says so.
        wait_for_snapshot(&events, id, |s| {
            s.progress.connection_detail.is_some_and(|d| {
                d.requested == 4 && d.limited_by == Some(ConnectionLimit::RampingUp)
            })
        });
        wait_until(
            Duration::from_secs(20),
            &format!("{mode}: 4 responses run at once"),
            || server.counters().answering == 4,
        );
        wait_for_snapshot(&events, id, |s| s.progress.connections == 4);

        assert_eq!(engine.set_connections(id, 1000), Ok(cap), "{mode}");
        wait_for_snapshot(&events, id, |s| {
            s.progress
                .connection_detail
                .is_some_and(|d| d.requested == cap)
        });
        assert_eq!(
            engine.set_connections(TransferId(999), 2),
            Err(ErrorCode::NotFound),
            "{mode}"
        );

        let done = wait_for(&events, id, |s| {
            matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
        });
        assert_eq!(
            done.status,
            DownloadStatus::Completed,
            "{mode}: {:?}",
            done.error
        );
        assert!(
            std::fs::read(&dest).unwrap() == data,
            "{mode}: the file is the source"
        );
        let c = server.counters();
        assert!(
            c.peak_answering <= cap,
            "{mode}: {} over the cap",
            c.peak_answering
        );
        let from_zero = c
            .ranges
            .iter()
            .filter(|r| r.as_deref().is_some_and(|r| r.starts_with("bytes=0-")))
            .count();
        assert_eq!(
            from_zero, 1,
            "{mode}: only the probe asked for byte 0: no restart"
        );
    }
}

/// Reads events until `until`, keeping each transfer's latest snapshot.
fn latest_until(
    events: &Receiver<TransferEvent>,
    latest: &mut HashMap<TransferId, TransferSnapshot>,
    until: Instant,
) {
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(TransferEvent::Changed { snapshot }) => {
                latest.insert(snapshot.id, snapshot);
            }
            Ok(_) => {}
            Err(_) => return,
        }
    }
}

/// Waits until every one of `ids` has ended, and checks each completed. `latest` holds the
/// snapshots read so far, which may already include some of the ends.
fn all_completed(
    events: &Receiver<TransferEvent>,
    latest: &mut HashMap<TransferId, TransferSnapshot>,
    ids: &[TransferId],
    mode: &str,
) {
    let deadline = Instant::now() + WAIT;
    let ended = |s: Option<&TransferSnapshot>| {
        s.is_some_and(|s| matches!(s.status, DownloadStatus::Completed | DownloadStatus::Failed))
    };
    while !ids.iter().all(|id| ended(latest.get(id))) {
        assert!(Instant::now() < deadline, "{mode}: {latest:?}");
        latest_until(events, latest, Instant::now() + Duration::from_millis(50));
    }
    for id in ids {
        let done = &latest[id];
        assert_eq!(
            done.status,
            DownloadStatus::Completed,
            "{mode}: {:?}",
            done.error
        );
    }
}

/// The tests that measure speed hold this for their whole run, so they run one at a time: at once,
/// on a 2-vCPU machine, they slowed each other enough to fail (#154). The others still overlap
/// them; a failed one doesn't block the rest.
static SPEED_TESTS: Mutex<()> = Mutex::new(());

fn one_speed_test_at_a_time() -> MutexGuard<'static, ()> {
    SPEED_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Jain's fairness index: 1 when all are equal, 1/n when one gets everything.
fn jain(xs: &[f64]) -> f64 {
    let sum: f64 = xs.iter().sum();
    let squares: f64 = xs.iter().map(|x| x * x).sum();
    sum * sum / (xs.len() as f64 * squares)
}

#[test]
fn downloads_from_one_host_share_its_cap_fairly() {
    let _speed = one_speed_test_at_a_time();
    // #47: 4 downloads each ask for 8 from one host with a cap of 8, so each is allowed 2. At
    // 2 MiB/s per connection, a 48 MiB file takes about 12 s, so each is still running when the
    // last one has ramped up to its 2, however far apart they started (#154).
    let data = seeded_bytes(48 << 20, 50);
    for (mode, mut engine) in engines() {
        let server = TestServer::start(
            vec![("f.bin".into(), data.clone())],
            ServerOptions {
                throttle_bytes_per_sec: Some(2 << 20),
                ..ServerOptions::default()
            },
        );
        let cap = engine.capabilities().max_connections_per_host;
        let dir = Dir::new(&format!("fair-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let ids: Vec<TransferId> = (0..4)
            .map(|i| {
                let mut s = spec(server.url("f.bin"), dir.0.join(format!("f{i}.bin")));
                s.max_connections = cap;
                engine.add(s).expect("add")
            })
            .collect();
        let shared = ConnectionDetail {
            requested: cap,
            allowed: cap / 4,
            limited_by: Some(ConnectionLimit::PerHostCap),
        };
        let mut latest = HashMap::new();
        let deadline = Instant::now() + WAIT;
        // Settled once each downloads on its 2: the allowance alone is set before a download has
        // connected, and measuring then compared downloads still connecting (#154).
        while !ids.iter().all(|id| {
            latest.get(id).is_some_and(|s: &TransferSnapshot| {
                s.progress.connection_detail == Some(shared)
                    && s.status == DownloadStatus::Downloading
                    && s.progress.connections == shared.allowed
            })
        }) {
            assert!(Instant::now() < deadline, "{mode}: {latest:?}");
            latest_until(
                &events,
                &mut latest,
                Instant::now() + Duration::from_millis(50),
            );
        }

        // Settled: measure each download's bytes over 2 s.
        latest_until(
            &events,
            &mut latest,
            Instant::now() + Duration::from_millis(500),
        );
        let before: Vec<u64> = ids
            .iter()
            .map(|id| latest[id].progress.completed_bytes)
            .collect();
        latest_until(
            &events,
            &mut latest,
            Instant::now() + Duration::from_secs(2),
        );
        let rates: Vec<f64> = ids
            .iter()
            .zip(&before)
            .map(|(id, b)| (latest[id].progress.completed_bytes - b) as f64)
            .collect();
        let index = jain(&rates);
        eprintln!("{mode}: bytes in 2 s {rates:?}, Jain's index {index:.3}");
        assert!(
            index >= 0.9,
            "{mode}: Jain's index {index:.3} for {rates:?}"
        );

        all_completed(&events, &mut latest, &ids, mode);
        let peak = server.counters().peak_answering;
        assert!(
            peak <= cap,
            "{mode}: {peak} responses at once, over the cap of {cap}"
        );
    }
}

#[test]
fn a_lower_profile_cap_bounds_many_small_downloads() {
    // #57: each running download keeps one connection, so with many small ones at once it is
    // libcurl's own per-host cap that bounds a host. It must follow the profile's.
    let data = seeded_bytes(256 * 1024, 58);
    for (mode, mut engine) in engines() {
        let server = TestServer::start(
            vec![("f.bin".into(), data.clone())],
            ServerOptions::default(),
        );
        let dir = Dir::new(&format!("small-capped-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let conservative = EngineLimits {
            max_connections_per_host: 2,
            min_segment_bytes: 20 << 20,
        };
        engine
            .set_engine_limits(conservative)
            .expect("set_engine_limits");
        let ids: Vec<TransferId> = (0..10)
            .map(|i| {
                engine
                    .add(spec(server.url("f.bin"), dir.0.join(format!("f{i}.bin"))))
                    .expect("add")
            })
            .collect();
        all_completed(&events, &mut HashMap::new(), &ids, mode);
        let used = server.counters().connections;
        assert!(
            used <= 2,
            "{mode}: 10 files at once used {used} connections, over the cap of 2"
        );
    }
}

#[test]
fn small_files_reuse_connections() {
    // #47: 40 files of 256 KiB from one host, smaller than the probe, so one GET each. One after
    // another, they reuse one keep-alive connection; added at once, the per-host cap bounds the
    // connections and the rest reuse them.
    let data = seeded_bytes(256 * 1024, 51);
    for (mode, mut engine) in engines() {
        let server = TestServer::start(
            vec![("f.bin".into(), data.clone())],
            ServerOptions::default(),
        );
        let cap = engine.capabilities().max_connections_per_host;
        let dir = Dir::new(&format!("reuse-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        for i in 0..40 {
            let id = engine
                .add(spec(server.url("f.bin"), dir.0.join(format!("one{i}.bin"))))
                .expect("add");
            time_to_complete(&events, id);
        }
        let one_by_one = server.counters().connections;
        assert_eq!(
            one_by_one, 1,
            "{mode}: 40 files one by one used {one_by_one} connections"
        );

        let ids: Vec<TransferId> = (0..40)
            .map(|i| {
                engine
                    .add(spec(server.url("f.bin"), dir.0.join(format!("all{i}.bin"))))
                    .expect("add")
            })
            .collect();
        all_completed(&events, &mut HashMap::new(), &ids, mode);
        let at_once = server.counters().connections - one_by_one;
        assert!(
            at_once <= u64::from(cap),
            "{mode}: 40 files at once used {at_once} new connections, over the cap of {cap}"
        );
        assert_eq!(server.counters().gets, 80, "{mode}: one GET per file");
    }
}
