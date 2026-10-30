//! Crash trials (#43, L4): a real `velocity-transfer serve` engine is killed (`kill -9`,
//! `taskkill /F`) at a random moment mid-download, and a new engine resumes from the last
//! checkpoint the first one sent. Every trial must:
//!
//! - leave nothing that looks finished before the rename;
//! - finish with exactly the source's bytes;
//! - fetch again at most what wasn't durable yet, plus what was in flight.
//!
//! The app itself is killed too (#74): a process running the manager over a database file, with
//! its own child engine, dies at a random moment, and a new app on the same database brings the
//! download back and finishes it with the source's bytes.
//!
//! `VELOCITY_CRASH_TRIALS` sets how many trials run: 3 by default, for CI; the full run is 100,
//! by hand (PO decision on #43). `VELOCITY_CRASH_SEED` repeats a run; the seed is printed.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use velocity_core::database::Database;
use velocity_core::domain::{DownloadKind, DownloadStatus};
use velocity_core::manager::Manager;
use velocity_core::transfer::{
    Checkpoint, Limits, Resume, TransferEvent, TransferId, TransferSpec,
};
use velocity_test_http::{kill_process, process_exists, seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::stdio::ChildProcessEngine;
use velocity_transfer::supervisor::{SupervisedEngine, SupervisorOptions};

const LEN: usize = 6_000_000;
/// Per connection: the probe's 4 MiB take about 4 s, so most kills land in the probe and some in
/// the parallel segments after it.
const RATE: u64 = 1_000_000;
const CONNECTIONS: u32 = 4;
/// The engine's checkpoint cadence (`Cadence::DEFAULT`): at most this long between checkpoints.
const INTERVAL: Duration = Duration::from_secs(2);
/// What can be in flight when the engine dies, besides what wasn't durable yet: per connection,
/// libcurl's 1 MiB buffer and two queued chunks, plus the 256 KiB a progress event may lag.
const IN_FLIGHT: u64 = CONNECTIONS as u64 * (3 << 20) + (256 << 10);
const WAIT: Duration = Duration::from_secs(60);

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_velocity-transfer"))
}

fn trials() -> u32 {
    std::env::var("VELOCITY_CRASH_TRIALS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

fn seed() -> u64 {
    std::env::var("VELOCITY_CRASH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64)
        })
}

/// xorshift64, enough to spread the kill times.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-crash-{name}-{}", std::process::id()));
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

fn spec(url: String, destination: &Path, resume: Option<Resume>) -> TransferSpec {
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
        max_connections: CONNECTIONS,
        resume,
        require_encryption: false,
        ssh_key: None,
        trust: Default::default(),
    }
}

fn durable_len(c: &Checkpoint) -> u64 {
    c.durable.iter().map(|(a, b)| b - a).sum()
}

/// Waits for transfer `id` to finish, and returns how.
fn finish(events: &Receiver<TransferEvent>, id: TransferId) -> (DownloadStatus, String) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events
            .recv_timeout(left)
            .expect("the resumed transfer finishes in time")
        {
            TransferEvent::Changed { snapshot }
                if snapshot.id == id
                    && matches!(
                        snapshot.status,
                        DownloadStatus::Completed | DownloadStatus::Failed
                    ) =>
            {
                return (snapshot.status, format!("{:?}", snapshot.error))
            }
            _ => {}
        }
    }
}

#[test]
fn a_killed_engine_resumes_from_its_checkpoint_and_finishes_the_right_file() {
    let data = seeded_bytes(LEN, 47);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions {
            throttle_bytes_per_sec: Some(RATE),
            ..ServerOptions::default()
        },
    );
    let (n, seed) = (trials(), seed());
    eprintln!("crash trials: {n}, seed {seed}");
    let mut rng = Rng(seed | 1);
    let (mut killed, mut renamed, mut most_twice) = (0, 0, 0u64);
    for trial in 0..n {
        let context = format!("trial {trial} of {n}, seed {seed}");
        let dir = Dir::new(&format!("trial-{trial}"));
        let dest = dir.0.join("out.bin");
        let kill_at = Duration::from_millis(300 + rng.below(4_500));
        let sent_before = server.counters().body_bytes_sent;

        // The first engine, killed at `kill_at`.
        let first = ChildProcessEngine::spawn(&program()).expect("the engine starts");
        let events = first.subscribe();
        let id = first
            .add(spec(server.url("f.bin"), &dest, None))
            .expect("add");
        let started = Instant::now();
        let mut last: Option<Checkpoint> = None;
        let mut last_at = started;
        let mut written = 0;
        let mut completed = false;
        loop {
            let left = kill_at.saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            match events.recv_timeout(left) {
                Ok(TransferEvent::Checkpoint { checkpoint, .. }) => {
                    last = Some(checkpoint);
                    last_at = Instant::now();
                }
                Ok(TransferEvent::Changed { snapshot }) if snapshot.id == id => {
                    written = written.max(snapshot.progress.completed_bytes);
                    match snapshot.status {
                        DownloadStatus::Completed => {
                            completed = true;
                            break;
                        }
                        DownloadStatus::Failed => panic!("{context}: {:?}", snapshot.error),
                        _ => {}
                    }
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => panic!("{context}: the engine stopped"),
            }
        }
        if completed {
            // It finished before the kill: nothing to resume.
            drop(first);
            assert!(std::fs::read(&dest).unwrap() == data, "{context}");
            continue;
        }
        kill_process(first.process_id()).expect("kill");
        killed += 1;
        // The engine's events end when it is gone.
        let deadline = Instant::now() + WAIT;
        loop {
            match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(TransferEvent::Checkpoint { checkpoint, .. }) => last = Some(checkpoint),
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    panic!("{context}: the killed engine's events never ended")
                }
            }
        }
        drop(first);
        // The kill can land just after the rename, before the Completed event arrived. Then the
        // file must be the whole, right one; before the rename, nothing may look finished.
        if dest.exists() {
            assert!(
                std::fs::read(&dest).unwrap() == data,
                "{context}: a file at the destination that isn't the source"
            );
            renamed += 1;
            continue;
        }
        // Checkpoints came at the cadence: what wasn't durable yet is at most one interval's
        // worth at the server's rate, plus a progress event's lag.
        let durable = last.as_ref().map_or(0, durable_len);
        let not_durable = written.saturating_sub(durable);
        let one_interval = (INTERVAL + Duration::from_millis(500)).as_secs_f64()
            * (RATE * u64::from(CONNECTIONS)) as f64;
        assert!(
            (not_durable as f64) <= one_interval + f64::from(256 << 10),
            "{context}: {not_durable} bytes weren't durable yet at the kill ({:?} after the last checkpoint)",
            started.elapsed().saturating_sub(last_at.duration_since(started))
        );

        // A new engine resumes from the last checkpoint the first one sent.
        let resume = match last {
            Some(checkpoint) => Resume::Checkpoint { checkpoint },
            None => Resume::DiscardPartFile {},
        };
        let second = ChildProcessEngine::spawn(&program()).expect("the engine starts");
        let events = second.subscribe();
        let id = second
            .add(spec(server.url("f.bin"), &dest, Some(resume)))
            .expect("add");
        let (status, error) = finish(&events, id);
        drop(second);
        assert_eq!(status, DownloadStatus::Completed, "{context}: {error}");
        assert!(
            std::fs::read(&dest).unwrap() == data,
            "{context}: the file differs from the source"
        );
        // Fetched twice: at most what wasn't durable yet, and what was in flight.
        let sent = server.counters().body_bytes_sent - sent_before;
        let twice = sent.saturating_sub(LEN as u64);
        assert!(
            twice <= not_durable + IN_FLIGHT,
            "{context}: {twice} bytes were fetched twice; {not_durable} weren't durable at the kill"
        );
        most_twice = most_twice.max(twice);
    }
    eprintln!(
        "crash trials: {n} passed ({killed} killed, {renamed} of them just after the rename), at most {most_twice} bytes fetched twice"
    );
}

/// Where the app process of a kill trial keeps its database and its download, and the source.
const APP_TRIAL: &str = "VELOCITY_APP_TRIAL";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The app, as one process: the manager over `<dir>/velocity.sqlite3` with a supervised child
/// engine. It adds the download if the database has none yet, prints its engine's process id,
/// and pumps until it is killed.
fn run_app(dir: &Path, url: &str) -> ! {
    let db = Arc::new(Database::open(&dir.join("velocity.sqlite3")).expect("the database"));
    let engine = SupervisedEngine::start(program(), SupervisorOptions::default()).expect("engine");
    println!(
        "engine pid {}",
        engine.process_id().expect("an engine runs")
    );
    let mut m = Manager::open(Box::new(engine), Arc::clone(&db)).expect("the manager");
    if m.snapshot().downloads.is_empty() {
        m.add(
            DownloadKind::Http,
            spec(url.to_owned(), &dir.join("out.bin"), None),
            now_ms(),
        )
        .expect("add");
    }
    loop {
        m.pump(now_ms());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The app process of [`a_killed_app_brings_the_download_back_and_finishes_the_right_file`]: run
/// only by that test, which starts this test binary again with [`APP_TRIAL`] set.
#[test]
#[ignore = "the app process of the app kill trials, started by them"]
fn app_process_for_the_kill_trials() {
    let trial = std::env::var(APP_TRIAL).expect("started by the app kill trials");
    let (dir, url) = trial.split_once('|').expect("<dir>|<url>");
    run_app(Path::new(dir), url);
}

/// Starts the app process for `dir` and returns it with its engine's process id.
fn start_app(dir: &Path, url: &str) -> (std::process::Child, u32) {
    let mut app = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "app_process_for_the_kill_trials",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(APP_TRIAL, format!("{}|{url}", dir.display()))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the app starts");
    let stdout = app.stdout.take().expect("piped");
    let mut lines = std::io::BufReader::new(stdout).lines();
    let pid = loop {
        let line = lines
            .next()
            .expect("the app names its engine")
            .expect("readable");
        if let Some(pid) = line.strip_prefix("engine pid ") {
            break pid.trim().parse().expect("a process id");
        }
    };
    // The rest of its output is drained, so it never blocks on a full pipe.
    std::thread::spawn(move || lines.for_each(drop));
    (app, pid)
}

#[test]
fn a_killed_app_brings_the_download_back_and_finishes_the_right_file() {
    let data = seeded_bytes(LEN, 74);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions {
            throttle_bytes_per_sec: Some(RATE),
            ..ServerOptions::default()
        },
    );
    let (n, seed) = (trials(), seed());
    eprintln!("app kill trials: {n}, seed {seed}");
    let mut rng = Rng(seed | 1);
    let mut renamed = 0;
    for trial in 0..n {
        let context = format!("app trial {trial} of {n}, seed {seed}");
        let dir = Dir::new(&format!("app-{trial}"));
        let dest = dir.0.join("out.bin");
        let kill_at = Duration::from_millis(300 + rng.below(4_500));

        let (mut app, engine_pid) = start_app(&dir.0, &server.url("f.bin"));
        std::thread::sleep(kill_at);
        app.kill().expect("the app is killed");
        app.wait().expect("the app is gone");
        // Its engine notices the app went away and stops; the next app starts its own.
        let deadline = Instant::now() + WAIT;
        while process_exists(engine_pid).expect("checked") {
            assert!(
                Instant::now() < deadline,
                "{context}: the orphaned engine never stopped"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        // The kill can land just after the rename: then the file must be the whole, right one.
        if dest.exists() {
            assert!(std::fs::read(&dest).unwrap() == data, "{context}");
            renamed += 1;
            continue;
        }

        // The app starts again on the same database.
        let db = Arc::new(Database::open(&dir.0.join("velocity.sqlite3")).expect("reopened"));
        let engine = SupervisedEngine::start(program(), SupervisorOptions::default()).unwrap();
        let mut m = Manager::open(Box::new(engine), Arc::clone(&db)).expect("the manager");
        let id = match m.snapshot().downloads.as_slice() {
            [one] => one.id.clone(),
            other => panic!("{context}: the download didn't come back once: {other:?}"),
        };
        let deadline = Instant::now() + WAIT;
        loop {
            m.pump(now_ms());
            let record = m.get(&id).expect("listed");
            match record.status {
                DownloadStatus::Completed => break,
                DownloadStatus::Failed => panic!("{context}: {:?}", record.error),
                _ => {}
            }
            assert!(Instant::now() < deadline, "{context}: {record:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(m);
        assert!(
            std::fs::read(&dest).unwrap() == data,
            "{context}: the file differs from the source"
        );
    }
    eprintln!("app kill trials: {n} passed ({renamed} killed just after the rename)");
}
