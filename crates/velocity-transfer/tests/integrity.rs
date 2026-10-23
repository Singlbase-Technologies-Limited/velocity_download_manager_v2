//! Whole-file integrity (#46, research report section 5.3) against the local test server: a
//! checksum is checked before the file is renamed into place, a wrong one fails and keeps the part
//! file, and the hash is right however the segments arrive.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use velocity_core::domain::{DownloadStatus, FailureClass};
use velocity_core::transfer::{Checkpoint, Checksum, HashAlgorithm, Resume};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, RangeMode, ServerOptions, TestServer};
use velocity_transfer::download::{
    download_resumable, Cadence, Control, DownloadError, DownloadOptions, DownloadReport, Mode,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-integrity-{name}-{}", std::process::id()));
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

fn curl_loop() -> CurlLoop {
    CurlLoop::start(
        &Libcurl::start().expect("libcurl starts"),
        LoopOptions::default(),
    )
    .expect("the loop starts")
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

fn sha256(bytes: &[u8]) -> Checksum {
    let hex = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Checksum {
        algorithm: HashAlgorithm::Sha256,
        hex,
    }
}

/// The SHA-256 of other bytes: a checksum the download can never match.
fn wrong() -> Checksum {
    sha256(b"another file")
}

/// Two connections and small leases; the first range after the probe is slowed, so the
/// segments after it complete first and the prefix from byte 0 is covered only at the end.
const OPTIONS: DownloadOptions = DownloadOptions {
    connections: 2,
    probe_bytes: 64 * 1024,
    leases: Sizes {
        max_lease: 4 << 20,
        min_split: 32 << 10,
        align: 4 << 10,
    },
    checkpoints: Cadence::DEFAULT,
    retry: RetryPolicy {
        first_wait: Duration::from_millis(20),
        max_wait: Duration::from_millis(200),
        ..RetryPolicy::DEFAULT
    },
    adaptive: None,
};

fn out_of_order(data: &[u8]) -> TestServer {
    TestServer::start(
        vec![("f.bin".into(), data.to_vec())],
        ServerOptions {
            throttle_first_later_range: Some(300_000),
            ..ServerOptions::default()
        },
    )
}

/// Runs a download and records the statuses it reported and the checkpoints it sent.
struct Run {
    result: Result<DownloadReport, DownloadError>,
    statuses: Vec<DownloadStatus>,
    checkpoints: Vec<Checkpoint>,
}

fn run(
    url: &str,
    dest: &Path,
    options: DownloadOptions,
    resume: Option<&Resume>,
    checksum: Option<&Checksum>,
) -> Run {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let checkpoints = Arc::new(Mutex::new(Vec::new()));
    let (s, c) = (Arc::clone(&statuses), Arc::clone(&checkpoints));
    let control = Control::with_progress(|_, _| {})
        .with_status(move |status| s.lock().unwrap().push(status))
        .with_checkpoints(move |checkpoint| c.lock().unwrap().push(checkpoint));
    let result = download_resumable(&curl_loop(), url, dest, options, resume, checksum, &control);
    let statuses = statuses.lock().unwrap().clone();
    let checkpoints = checkpoints.lock().unwrap().clone();
    Run {
        result,
        statuses,
        checkpoints,
    }
}

#[test]
fn a_correct_checksum_completes_a_download_whose_segments_arrive_out_of_order() {
    const LEN: usize = 1_000_000;
    let data = seeded_bytes(LEN, 46);
    let server = out_of_order(&data);
    let dir = Dir::new("correct");
    let dest = dir.0.join("out.bin");
    let run = run(
        &server.url("f.bin"),
        &dest,
        OPTIONS,
        None,
        Some(&sha256(&data)),
    );
    let report = run.result.expect("the download completes");
    assert_eq!(report.mode, Mode::Ranges);
    assert!(
        report.splits >= 1,
        "the slow range was split, so later bytes landed first: {report:?}"
    );
    assert!(std::fs::read(&dest).unwrap() == data);
    assert!(!part_of(&dest).exists());
    assert!(
        run.statuses.contains(&DownloadStatus::Verifying),
        "{:?}",
        run.statuses
    );
}

#[test]
fn a_wrong_checksum_fails_keeps_the_part_file_and_renames_nothing() {
    const LEN: usize = 300_000;
    let data = seeded_bytes(LEN, 47);
    let server = out_of_order(&data);
    let dir = Dir::new("wrong");
    let dest = dir.0.join("out.bin");
    let run = run(&server.url("f.bin"), &dest, OPTIONS, None, Some(&wrong()));
    let Err(DownloadError::ChecksumMismatch(e)) = run.result else {
        panic!("expected a checksum mismatch, got {:?}", run.result);
    };
    assert!(e.message.contains("SHA-256 checksum"), "{}", e.message);
    // Worth a retry by hand, which downloads the file again; the engine doesn't restart it.
    assert_eq!(
        (e.class, e.retryable),
        (FailureClass::Transient, true),
        "{e:?}"
    );
    assert!(
        !dest.exists(),
        "a file that fails its checksum isn't renamed"
    );
    assert!(
        std::fs::read(part_of(&dest)).unwrap() == data,
        "the part file is kept, whole, for inspection"
    );
    assert!(
        run.statuses.contains(&DownloadStatus::Verifying),
        "{:?}",
        run.statuses
    );
}

#[test]
fn after_a_wrong_checksum_a_retry_downloads_the_file_again() {
    // With checkpoints, the failure's last one trusts no byte, so a resume from it fetches the
    // whole file again instead of checking the same bytes a second time (report 5.3: "A retry
    // re-downloads once").
    const LEN: usize = 300_000;
    let data = seeded_bytes(LEN, 48);
    let server = out_of_order(&data);
    let dir = Dir::new("retry");
    let dest = dir.0.join("out.bin");
    let options = DownloadOptions {
        checkpoints: Cadence {
            bytes: 64 * 1024,
            interval: Duration::from_secs(60),
        },
        ..OPTIONS
    };
    let failed = run(&server.url("f.bin"), &dest, options, None, Some(&wrong()));
    assert!(
        matches!(failed.result, Err(DownloadError::ChecksumMismatch(_))),
        "{:?}",
        failed.result
    );
    assert!(failed.checkpoints.len() >= 2, "{:?}", failed.checkpoints);
    let last = failed.checkpoints.last().unwrap().clone();
    assert!(last.durable.is_empty(), "{last:?}");
    assert_eq!(last.total, LEN as u64);

    let resume = Resume::Checkpoint { checkpoint: last };
    let retried = run(
        &server.url("f.bin"),
        &dest,
        options,
        Some(&resume),
        Some(&sha256(&data)),
    );
    let report = retried.result.expect("the retry completes");
    assert_eq!(
        report.resumed_bytes, 0,
        "no byte of the failed file is trusted"
    );
    assert_eq!(report.written_bytes, LEN as u64, "every byte fetched again");
    assert!(std::fs::read(&dest).unwrap() == data);
}

#[test]
fn a_checksum_is_checked_on_a_single_stream_and_on_an_empty_file() {
    let data = seeded_bytes(200_000, 49);
    let server = TestServer::start(
        vec![
            ("f.bin".into(), data.clone()),
            ("empty.bin".into(), Vec::new()),
        ],
        ServerOptions {
            range_mode: RangeMode::Ignore,
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("stream");
    let url = server.url("f.bin");

    let ok = dir.0.join("ok.bin");
    let report = run(&url, &ok, OPTIONS, None, Some(&sha256(&data)))
        .result
        .expect("the stream completes");
    assert_eq!(report.mode, Mode::SingleStream);
    assert!(std::fs::read(&ok).unwrap() == data);

    let bad = dir.0.join("bad.bin");
    let result = run(&url, &bad, OPTIONS, None, Some(&wrong())).result;
    assert!(
        matches!(result, Err(DownloadError::ChecksumMismatch(_))),
        "{result:?}"
    );
    assert!(!bad.exists());

    let empty = dir.0.join("empty.bin");
    let result = run(
        &server.url("empty.bin"),
        &empty,
        OPTIONS,
        None,
        Some(&sha256(b"")),
    );
    result
        .result
        .expect("an empty file matches the digest of nothing");
    assert_eq!(std::fs::read(&empty).unwrap(), b"");
}
