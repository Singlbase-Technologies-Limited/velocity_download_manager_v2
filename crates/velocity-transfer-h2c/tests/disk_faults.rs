//! A full disk and a refused write mid-download (#74), through the manager and the engine, with the
//! engine's test-only `test-faults` feature making the part file's writes fail (PO decision on
//! #74: simulated, no real small drive). Each must:
//!
//! - fail the download with its own message, as permanent: retrying on its own wouldn't help;
//! - keep the part file and a checkpoint of what reached the disk;
//! - resume once the user has fixed it and retries, fetching only what was missing, and finish
//!   with the source's bytes.
//!
//! The write that meets the fault lands its first bytes and then fails, as an interrupted write
//! does: those bytes aren't counted, and the resume fetches them again.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use velocity_core::database::Database;
use velocity_core::domain::{DownloadKind, DownloadStatus, FailureClass};
use velocity_core::manager::Manager;
use velocity_core::transfer::{Limits, TransferSpec};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::faults;

const LEN: usize = 6_000_000;
/// Where a fault strikes: inside the engine's 4 MiB probe, or past it among the parallel segments,
/// and never on a chunk boundary, so the failing write is cut part-way.
const IN_THE_PROBE: u64 = 3_000_123;
const IN_THE_SEGMENTS: u64 = 5_000_123;
const WAIT: Duration = Duration::from_secs(60);

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-disk-faults-{name}-{}", std::process::id()));
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

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn spec(url: String, dir: &Dir, name: &str) -> TransferSpec {
    TransferSpec {
        urls: vec![url],
        directory: dir.0.clone(),
        file_name: Some(name.to_owned()),
        credentials: None,
        headers: Vec::new(),
        referrer: None,
        user_agent: None,
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        max_connections: 4,
        resume: None,
        require_encryption: false,
        ssh_key: None,
        trust: Default::default(),
    }
}

/// Pumps `m` until download `id` has `status`.
fn until(m: &mut Manager, id: &str, status: DownloadStatus) {
    let deadline = Instant::now() + WAIT;
    loop {
        m.pump(now());
        let record = m.get(id).expect("listed");
        if record.status == status {
            return;
        }
        assert!(Instant::now() < deadline, "never {status:?}: {:?}", record);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs one fault: the download fails with `message`, keeps what reached the disk, and resumes
/// once the fault is cleared.
fn fails_then_resumes(name: &str, fail_after: u64, kind: io::ErrorKind, message: &str) {
    let data = seeded_bytes(LEN, 74);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    let dir = Dir::new(name);
    let db = Arc::new(Database::open(&dir.0.join("velocity.sqlite3")).unwrap());
    let engine = InProcessEngine::start().expect("the engine starts");
    let mut m = Manager::open(Box::new(engine), Arc::clone(&db)).unwrap();

    faults::fail_writes_after(name, fail_after, kind);
    let id = m
        .add(
            DownloadKind::Http,
            spec(server.url("f.bin"), &dir, name),
            now(),
        )
        .unwrap();
    until(&mut m, &id, DownloadStatus::Failed);
    let error = m.get(&id).unwrap().error.clone().expect("an error");
    assert!(error.message.starts_with(message), "{error:?}");
    assert_eq!(error.class, FailureClass::Permanent, "{error:?}");

    let part = dir.0.join(format!("{name}.vdmpart"));
    assert!(part.exists(), "the part file is kept");
    let checkpoint = db.checkpoint(&id).unwrap().expect("a checkpoint is kept");
    let checkpoint = checkpoint.known().expect("a checkpoint this build reads");
    let durable: u64 = checkpoint.durable.iter().map(|(a, b)| b - a).sum();
    assert!(
        durable > 0 && durable <= fail_after,
        "{durable} bytes recorded as on disk"
    );

    // The user frees space, or gives the permission back, and retries.
    faults::clear(name);
    let sent_before = server.counters().body_bytes_sent;
    m.retry(&id).unwrap();
    until(&mut m, &id, DownloadStatus::Completed);
    assert!(
        std::fs::read(dir.0.join(name)).unwrap() == data,
        "the file is the source's"
    );
    let missing = LEN as u64 - durable;
    let sent = server.counters().body_bytes_sent - sent_before;
    assert!(
        sent <= missing + (1 << 20),
        "{sent} bytes fetched again, {missing} were missing: it resumed rather than started over"
    );
}

#[test]
fn a_full_disk_fails_keeps_the_part_and_resumes_once_space_is_freed() {
    fails_then_resumes(
        "full.bin",
        IN_THE_PROBE,
        io::ErrorKind::StorageFull,
        "There is not enough disk space.",
    );
}

#[test]
fn a_refused_write_fails_keeps_the_part_and_resumes_once_allowed() {
    fails_then_resumes(
        "refused.bin",
        IN_THE_SEGMENTS,
        io::ErrorKind::PermissionDenied,
        "Velocity isn't allowed to write to that location.",
    );
}
