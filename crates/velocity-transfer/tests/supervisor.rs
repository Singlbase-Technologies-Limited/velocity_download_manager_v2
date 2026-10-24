//! The engine supervisor (#39) with a real `velocity-transfer serve` child: crashes are noticed
//! and restarted with a cap, the transfers a crash lost are reported as retryable failures, and a
//! capped engine stays unavailable until it is restarted by hand.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use velocity_core::domain::{DownloadStatus, ErrorCode};
use velocity_core::transfer::{
    DirectTransfer, EngineLimits, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};
use velocity_test_http::{
    freeze_process, kill_process, process_exists, seeded_bytes, ServerOptions, TestServer,
};
use velocity_transfer::supervisor::{EngineHealth, SupervisedEngine, SupervisorOptions};

const WAIT: Duration = Duration::from_secs(30);

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_velocity-transfer"))
}

/// Fast restarts, so the tests don't wait out the production backoff.
fn fast(max_restarts: usize) -> SupervisorOptions {
    SupervisorOptions {
        max_restarts,
        restart_window: Duration::from_secs(300),
        first_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
        ..SupervisorOptions::default()
    }
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-supervisor-{name}-{}", std::process::id()));
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
        max_connections: 2,
        resume: None,
        require_encryption: false,
    }
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

fn slow_server() -> TestServer {
    TestServer::start(
        vec![("big.bin".into(), seeded_bytes(4_000_000, 39))],
        ServerOptions {
            throttle_bytes_per_sec: Some(200_000),
            ..ServerOptions::default()
        },
    )
}

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

fn wait_health(
    health: &Receiver<EngineHealth>,
    want: impl Fn(EngineHealth) -> bool,
) -> EngineHealth {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let h = health.recv_timeout(left).expect("a health change in time");
        if want(h) {
            return h;
        }
    }
}

/// Kills the running engine and waits until the supervisor has a new one running.
fn crash_and_recover(engine: &SupervisedEngine, health: &Receiver<EngineHealth>) {
    let pid = engine.process_id().expect("an engine is running");
    kill_process(pid).expect("kill");
    wait_health(health, |h| matches!(h, EngineHealth::Restarting { .. }));
    wait_health(health, |h| h == EngineHealth::Running);
    assert_ne!(engine.process_id(), Some(pid), "a new engine process");
}

#[test]
fn a_missing_engine_program_is_reported_as_missing() {
    let outcome = SupervisedEngine::start(
        PathBuf::from("does-not-exist/velocity-transfer"),
        SupervisorOptions::default(),
    );
    assert_eq!(outcome.err(), Some(ErrorCode::EngineMissing));
}

#[test]
fn a_crash_restarts_the_engine_and_fails_its_transfers_as_retryable() {
    let server = slow_server();
    let dir = Dir::new("crash");
    let dest = dir.0.join("out.bin");
    let mut engine = SupervisedEngine::start(program(), fast(5)).expect("the engine starts");
    assert_eq!(engine.health(), EngineHealth::Running);
    let health = engine.subscribe_health();
    let events = engine.subscribe();
    let lost = engine
        .add(spec(server.url("big.bin"), dest.clone()))
        .expect("add");
    wait_for(&events, lost, |s| s == DownloadStatus::Downloading);

    crash_and_recover(&engine, &health);

    let failed = wait_for(&events, lost, |s| s == DownloadStatus::Failed);
    let error = failed.error.expect("an error");
    assert!(error.retryable, "{error:?}");
    assert!(error.message.contains("engine stopped"), "{error:?}");
    assert!(
        part_of(&dest).exists(),
        "the part file is kept for checkpoints (#43)"
    );

    // The lost transfer stays known until it is removed, and is never given to another transfer.
    let snapshot = engine.snapshot().expect("snapshot");
    assert!(snapshot
        .iter()
        .any(|s| s.id == lost && s.status == DownloadStatus::Failed));
    assert_eq!(engine.cancel(lost), Err(ErrorCode::InvalidState));
    let next = engine
        .add(spec(server.url("big.bin"), dir.0.join("next.bin")))
        .expect("the new engine takes transfers");
    assert!(next > lost, "{next:?} after {lost:?}");
    engine.remove(lost).expect("remove the lost transfer");
    assert!(engine.snapshot().unwrap().iter().all(|s| s.id != lost));
    engine.cancel(next).expect("cancel");
    wait_for(&events, next, |s| s == DownloadStatus::Removed);
}

#[test]
fn the_restart_cap_leaves_the_engine_unavailable_until_restarted_by_hand() {
    let server = slow_server();
    let dir = Dir::new("cap");
    let mut engine = SupervisedEngine::start(program(), fast(2)).expect("the engine starts");
    let health = engine.subscribe_health();
    crash_and_recover(&engine, &health);
    crash_and_recover(&engine, &health);

    // The third crash in the window is one more than the cap allows.
    kill_process(engine.process_id().unwrap()).expect("kill");
    assert_eq!(
        wait_health(&health, |h| matches!(h, EngineHealth::Unavailable { .. })),
        EngineHealth::Unavailable {
            code: ErrorCode::EngineUnavailable
        }
    );
    assert_eq!(engine.process_id(), None);
    assert_eq!(
        engine
            .add(spec(server.url("big.bin"), dir.0.join("a.bin")))
            .err(),
        Some(ErrorCode::EngineUnavailable)
    );
    assert_eq!(engine.snapshot().err(), Some(ErrorCode::EngineUnavailable));

    engine.restart();
    wait_health(&health, |h| h == EngineHealth::Running);
    let events = engine.subscribe();
    let id = engine
        .add(spec(server.url("big.bin"), dir.0.join("b.bin")))
        .expect("a restarted engine takes transfers");
    wait_for(&events, id, |s| s == DownloadStatus::Downloading);
    engine.cancel(id).expect("cancel");
    wait_for(&events, id, |s| s == DownloadStatus::Removed);
}

#[test]
fn a_restart_by_hand_stops_the_old_engine_and_fails_its_transfers_as_retryable() {
    let server = slow_server();
    let dir = Dir::new("manual");
    let dest = dir.0.join("out.bin");
    let mut engine = SupervisedEngine::start(program(), fast(5)).expect("the engine starts");
    let health = engine.subscribe_health();
    let events = engine.subscribe();
    let id = engine
        .add(spec(server.url("big.bin"), dest.clone()))
        .expect("add");
    wait_for(&events, id, |s| s == DownloadStatus::Downloading);
    let old = engine.process_id();

    engine.restart();
    let failed = wait_for(&events, id, |s| s == DownloadStatus::Failed);
    let error = failed.error.expect("an error");
    assert!(
        error.retryable && error.message.contains("restarted"),
        "{error:?}"
    );
    wait_health(&health, |h| h == EngineHealth::Running);
    assert_ne!(engine.process_id(), old);
    // The old engine was stopped, not left running: the server soon sends nothing more. Since
    // #43 it pauses its transfers and keeps their part files, so the app can resume them.
    assert_server_stops(&server);
    assert!(
        part_of(&dest).exists(),
        "the part file is kept for a resume"
    );
}

/// The server sends nothing more once the bytes it had under way are counted. (It counts a chunk
/// before writing it, and on Windows a write into a socket its peer just closed can still
/// succeed.)
fn assert_server_stops(server: &TestServer) {
    std::thread::sleep(Duration::from_millis(300));
    let sent = server.counters().body_bytes_sent;
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        server.counters().body_bytes_sent,
        sent,
        "the old engine still downloads"
    );
}

#[test]
fn stopping_the_supervisor_stops_its_engine_and_keeps_the_part_file() {
    let server = slow_server();
    let dir = Dir::new("stop");
    let dest = dir.0.join("out.bin");
    let mut engine = SupervisedEngine::start(program(), fast(5)).expect("the engine starts");
    let events = engine.subscribe();
    let id = engine
        .add(spec(server.url("big.bin"), dest.clone()))
        .expect("add");
    wait_for(&events, id, |s| s == DownloadStatus::Downloading);
    drop(engine);
    // A graceful stop pauses the transfer (#43): the part file stays for a resume, nothing looks
    // finished, and the engine stopped downloading.
    assert!(part_of(&dest).exists() && !dest.exists());
    assert_server_stops(&server);
}

/// What the engine process really got: its command line and environment, read from `/proc`.
#[cfg(target_os = "linux")]
#[test]
fn the_engine_process_gets_one_argument_and_no_inherited_environment() {
    let engine = SupervisedEngine::start(program(), fast(5)).expect("the engine starts");
    let pid = engine.process_id().unwrap();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    let args: Vec<&[u8]> = cmdline
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .collect();
    assert_eq!(args.len(), 2, "{args:?}");
    assert_eq!(args[1], b"serve");
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    for var in environ.split(|b| *b == 0).filter(|v| !v.is_empty()) {
        let name = String::from_utf8_lossy(var.split(|b| *b == b'=').next().unwrap());
        assert!(
            ["SSL_CERT_FILE", "SSL_CERT_DIR"].contains(&name.as_ref()),
            "the engine inherited {name}"
        );
    }
}

#[test]
fn a_frozen_engine_is_killed_and_restarted_and_its_transfers_fail_as_retryable() {
    // #48: an engine that stops answering without exiting is treated like a crash. Checks every
    // 200 ms, each answered within 300 ms, two missed in a row: noticed within about a second.
    let server = slow_server();
    let dir = Dir::new("frozen");
    let dest = dir.0.join("out.bin");
    let options = SupervisorOptions {
        check_every: Duration::from_millis(200),
        check_timeout: Duration::from_millis(300),
        missed_checks: 2,
        ..fast(5)
    };
    let mut engine = SupervisedEngine::start(program(), options).expect("the engine starts");
    let health = engine.subscribe_health();
    let events = engine.subscribe();
    let lost = engine
        .add(spec(server.url("big.bin"), dest.clone()))
        .expect("add");
    wait_for(&events, lost, |s| s == DownloadStatus::Downloading);
    let pid = engine.process_id().expect("an engine is running");

    let frozen = freeze_process(pid).expect("freeze the engine");
    let failed = wait_for(&events, lost, |s| s == DownloadStatus::Failed);
    let error = failed.error.expect("an error");
    assert!(error.retryable, "{error:?}");
    assert!(error.message.contains("stopped responding"), "{error:?}");
    // On Windows a killed process may not finish exiting while the test is its debugger, so the
    // test lets go once the supervisor has given up on it. The supervisor kills it either way.
    drop(frozen);

    wait_health(&health, |h| matches!(h, EngineHealth::Restarting { .. }));
    wait_health(&health, |h| h == EngineHealth::Running);
    let new_pid = engine.process_id().expect("a new engine runs");
    assert_ne!(new_pid, pid, "a new engine process");
    // The frozen engine is gone, not left running beside its successor.
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_exists(pid).expect("look for the process") {
        assert!(
            Instant::now() < deadline,
            "the frozen engine {pid} still runs"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The new engine takes transfers.
    let next = engine
        .add(spec(server.url("big.bin"), dir.0.join("next.bin")))
        .expect("the new engine takes transfers");
    assert!(next.0 > lost.0);
}

#[test]
fn the_engine_limits_apply_at_once_and_survive_an_engine_restart() {
    // #57: the profile's limits reach the running engine, show in its capabilities, and are
    // given to a new engine after a crash.
    let mut engine = SupervisedEngine::start(program(), fast(5)).expect("the engine starts");
    let health = engine.subscribe_health();
    let maximum = EngineLimits {
        max_connections_per_host: 16,
        ..EngineLimits::DEFAULT
    };
    engine
        .set_engine_limits(maximum)
        .expect("set_engine_limits");
    assert_eq!(
        engine.capabilities().max_connections_per_host,
        16,
        "at once"
    );
    crash_and_recover(&engine, &health);
    assert_eq!(
        engine.capabilities().max_connections_per_host,
        16,
        "after a restart"
    );
}

#[test]
fn the_global_limit_survives_an_engine_restart() {
    // #49: the supervisor gives a new engine the global limit the old one had. 512 KiB at
    // 256 KiB/s takes about 2 s.
    let server = TestServer::start(
        vec![("small.bin".into(), seeded_bytes(512 * 1024, 49))],
        ServerOptions::default(),
    );
    let dir = Dir::new("global-limit");
    let mut engine = SupervisedEngine::start(program(), fast(5)).expect("the engine starts");
    let health = engine.subscribe_health();
    let events = engine.subscribe();
    engine
        .set_global_limits(Limits {
            max_download_bps: Some(256 * 1024),
        })
        .expect("set_global_limits");
    crash_and_recover(&engine, &health);
    let started = Instant::now();
    let id = engine
        .add(spec(server.url("small.bin"), dir.0.join("out.bin")))
        .expect("the new engine takes transfers");
    let done = wait_for(&events, id, |s| {
        matches!(s, DownloadStatus::Completed | DownloadStatus::Failed)
    });
    assert_eq!(done.status, DownloadStatus::Completed, "{:?}", done.error);
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1700), "{took:?}");
}
