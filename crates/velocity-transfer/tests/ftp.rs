//! FTP downloads through the engine (#51), against the local FTP server: real libcurl, real
//! sockets, real files. Ported from V1's FTP integration tests: a whole file whose SHA-256 matches,
//! a cut transfer that resumes with `REST`, and a missing file that fails with its own error. Plus
//! what V2 adds: one connection, the resume check against `MDTM` (ADR-0010), and a server that
//! can't resume.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use velocity_core::domain::{FailureClass, Verification};
use velocity_core::transfer::{Checkpoint, Resume, CHECKPOINT_FORMAT};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::ftp::{FtpOptions, FtpServer};
use velocity_test_http::seeded_bytes;
use velocity_transfer::download::{
    download_resumable, download_with, Cadence, Control, DownloadError, DownloadOptions, Expected,
    Mode,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const LEN: usize = 3 * 1024 * 1024 + 7;
/// The test server's `MDTM` ([`velocity_test_http::ftp::MODIFIED`]) as an HTTP date, as the
/// libcurl layer reports it.
const MODIFIED_DATE: &str = "Thu, 02 Jan 2020 03:04:05 GMT";

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-ftp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
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

/// Short waits, so a transient failure doesn't wait out the production policy.
const FAST_RETRY: RetryPolicy = RetryPolicy {
    first_wait: Duration::from_millis(20),
    max_wait: Duration::from_millis(200),
    ..RetryPolicy::DEFAULT
};

/// What the engine uses for FTP, from options that ask for more.
fn options() -> DownloadOptions {
    DownloadOptions {
        connections: 8,
        probe_bytes: 64 * 1024,
        leases: Sizes::DEFAULT,
        checkpoints: Cadence::DEFAULT,
        retry: FAST_RETRY,
        adaptive: None,
    }
    .one_connection()
}

fn serve(options: FtpOptions) -> (FtpServer, Vec<u8>) {
    let data = seeded_bytes(LEN, 151);
    (
        FtpServer::start(vec![("/pub/f.bin".into(), data.clone())], options),
        data,
    )
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// A part file holding the first `kept` bytes, and a checkpoint that says so.
fn resumable(
    server: &FtpServer,
    dest: &Path,
    data: &[u8],
    kept: usize,
    date: Option<&str>,
) -> Resume {
    std::fs::write(part_of(dest), &data[..kept]).unwrap();
    Resume::Checkpoint {
        checkpoint: Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: Checkpoint::source_of(&server.url("pub/f.bin")),
            total: data.len() as u64,
            etag: None,
            last_modified: date.map(str::to_owned),
            content_encoding: None,
            durable: vec![(0, kept as u64)],
        },
    }
}

#[test]
fn the_ftp_options_are_one_connection_and_one_stream() {
    let o = options();
    assert_eq!(o.connections, 1);
    assert_eq!(o.adaptive, None);
    // The probe and a resume's lease reach the end of any file.
    assert_eq!(o.probe_bytes, u64::MAX);
    assert_eq!(o.leases.max_lease, u64::MAX);
}

#[test]
fn a_file_downloads_in_one_transfer_and_matches_its_sha256() {
    let (server, data) = serve(FtpOptions::default());
    let dir = Dir::new("whole");
    let dest = dir.file("out.bin");
    let report = download_with(
        &curl_loop(),
        &server.url("pub/f.bin"),
        &dest,
        options(),
        &Control::default(),
    )
    .expect("it downloads");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    assert_eq!(report.total, LEN as u64);
    assert_eq!(report.fetches, 1, "{report:?}");
    assert_eq!(report.wasted_bytes, 0);
    assert_eq!(report.verification, Verification::SizeVerified);
    let record = server.record();
    assert_eq!(
        (record.retrs, record.rests.clone()),
        (1, vec![]),
        "{record:?}"
    );
}

#[test]
fn a_cut_transfer_resumes_with_rest_and_still_matches() {
    const CUT: u64 = 1_000_000;
    let (server, data) = serve(FtpOptions {
        cut_first: Some(CUT),
        ..FtpOptions::default()
    });
    let dir = Dir::new("cut");
    let dest = dir.file("out.bin");
    let report = download_with(
        &curl_loop(),
        &server.url("pub/f.bin"),
        &dest,
        options(),
        &Control::default(),
    )
    .expect("it resumes after the cut");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    let record = server.record();
    assert!(record.cut);
    assert_eq!(record.retrs, 2, "{record:?}");
    assert_eq!(
        record.rests,
        vec![CUT],
        "the rest came from where the cut was"
    );
    assert_eq!(report.wasted_bytes, 0, "{report:?}");
}

#[test]
fn a_paused_download_resumes_from_its_checkpoint_with_rest() {
    let (server, data) = serve(FtpOptions {
        throttle_bytes_per_sec: Some(1_000_000),
        ..FtpOptions::default()
    });
    let dir = Dir::new("pause");
    let dest = dir.file("out.bin");
    let points = Arc::new(Mutex::new(Vec::<Checkpoint>::new()));
    let p = Arc::clone(&points);
    let control = Control::default().with_checkpoints(move |c| p.lock().unwrap().push(c));
    let fast = DownloadOptions {
        checkpoints: Cadence {
            bytes: 64 << 10,
            interval: Duration::from_millis(50),
        },
        ..options()
    };
    let durable = |c: &Checkpoint| c.durable.iter().map(|(a, b)| b - a).sum::<u64>();
    let url = server.url("pub/f.bin");
    let lp = curl_loop();
    let paused = std::thread::scope(|scope| {
        let run = scope.spawn(|| download_with(&lp, &url, &dest, fast, &control));
        let deadline = Instant::now() + Duration::from_secs(30);
        while points.lock().unwrap().last().map_or(0, durable) < 300_000 {
            assert!(Instant::now() < deadline, "no checkpoint in time");
            std::thread::sleep(Duration::from_millis(10));
        }
        control.pause();
        run.join().unwrap()
    });
    assert!(matches!(paused, Err(DownloadError::Paused)), "{paused:?}");
    let checkpoint = points
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("a checkpoint");
    assert_eq!(checkpoint.last_modified.as_deref(), Some(MODIFIED_DATE));
    let kept = durable(&checkpoint);
    assert_eq!(checkpoint.durable, vec![(0, kept)]);

    let report = download_resumable(
        &lp,
        &url,
        &dest,
        fast,
        Some(&Resume::Checkpoint { checkpoint }),
        &Expected::default(),
        &Control::default(),
    )
    .expect("the download resumes");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    assert_eq!(report.resumed_bytes, kept);
    assert_eq!(
        server.record().rests,
        vec![kept],
        "one REST, at the checkpoint"
    );
    // Checked against MDTM, a strong date (ADR-0010).
    assert_eq!(report.verification, Verification::SizeVerified);
}

#[test]
fn a_resume_without_a_modification_time_is_kept_on_its_size_but_not_verified() {
    let (server, data) = serve(FtpOptions {
        no_mdtm: true,
        ..FtpOptions::default()
    });
    let dir = Dir::new("no-mdtm");
    let dest = dir.file("out.bin");
    let resume = resumable(&server, &dest, &data, 500_000, None);
    let report = download_resumable(
        &curl_loop(),
        &server.url("pub/f.bin"),
        &dest,
        options(),
        Some(&resume),
        &Expected::default(),
        &Control::default(),
    )
    .expect("it resumes");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    assert_eq!(report.resumed_bytes, 500_000);
    assert_eq!(server.record().rests, vec![500_000]);
    assert_eq!(report.verification, Verification::NotVerified);
}

#[test]
fn a_resume_of_a_file_whose_modification_time_changed_starts_over() {
    let (server, old) = serve(FtpOptions::default());
    let new = seeded_bytes(LEN, 152);
    server.replace("/pub/f.bin", new.clone(), "20240607080910");
    let dir = Dir::new("changed");
    let dest = dir.file("out.bin");
    let resume = resumable(&server, &dest, &old, 500_000, Some(MODIFIED_DATE));
    let report = download_resumable(
        &curl_loop(),
        &server.url("pub/f.bin"),
        &dest,
        options(),
        Some(&resume),
        &Expected::default(),
        &Control::default(),
    )
    .expect("it starts over");
    assert!(
        std::fs::read(&dest).unwrap() == new,
        "none of the old bytes were kept"
    );
    assert_eq!(report.resumed_bytes, 0);
    let record = server.record();
    // The resume asked from its checkpoint, saw another date before any byte, and started over
    // from byte 0, without REST.
    assert_eq!(record.rests, vec![500_000], "{record:?}");
}

#[test]
fn a_server_that_cannot_resume_sends_the_file_again_from_the_start() {
    let (server, data) = serve(FtpOptions {
        no_rest: true,
        ..FtpOptions::default()
    });
    let dir = Dir::new("no-rest");
    let dest = dir.file("out.bin");
    let resume = resumable(&server, &dest, &data, 500_000, Some(MODIFIED_DATE));
    let report = download_resumable(
        &curl_loop(),
        &server.url("pub/f.bin"),
        &dest,
        options(),
        Some(&resume),
        &Expected::default(),
        &Control::default(),
    )
    .expect("it downloads without REST");
    assert_eq!(sha256(&std::fs::read(&dest).unwrap()), sha256(&data));
    assert_eq!(report.mode, Mode::RangesLost);
    let record = server.record();
    assert_eq!(record.retrs, 1, "only the whole file was sent: {record:?}");
}

#[test]
fn a_missing_file_fails_permanently_as_not_found() {
    let (server, _) = serve(FtpOptions::default());
    let dir = Dir::new("missing");
    let dest = dir.file("out.bin");
    let result = download_with(
        &curl_loop(),
        &server.url("pub/missing.bin"),
        &dest,
        options(),
        &Control::default(),
    );
    match result {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Permanent, "{e:?}");
            assert!(e.message.contains("wasn't found"), "{}", e.message);
        }
        other => panic!("expected a transfer failure, got {other:?}"),
    }
    assert!(!dest.exists() && !part_of(&dest).exists());
}
