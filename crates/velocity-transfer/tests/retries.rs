//! Retries, waits and stalls (#48) against the local test server: real libcurl, real sockets, real
//! files. The acceptance fixtures of #48, and report section 5.4's politeness and stall rows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use velocity_core::domain::{DownloadStatus, FailureClass};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, FirstStatuses, Freeze, ServerOptions, TestServer};
use velocity_transfer::download::{
    download_with, Cadence, Control, DiskOptions, DownloadError, DownloadOptions, DownloadReport,
    Mode,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-retries-{name}-{}", std::process::id()));
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

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

fn curl_loop() -> CurlLoop {
    CurlLoop::start(
        &Libcurl::start().expect("libcurl starts"),
        LoopOptions::default(),
    )
    .expect("the loop starts")
}

/// Short waits, so the tests take seconds; `Retry-After` is honoured in full, as in production.
const FAST: RetryPolicy = RetryPolicy {
    first_wait: Duration::from_millis(100),
    max_wait: Duration::from_millis(800),
    throttled_wait: Duration::from_millis(200),
    max_retry_after: Duration::from_secs(600),
    failures_in_a_row: 5,
    stalled_after: Duration::from_millis(500),
    reset_after: Duration::from_secs(1),
    ..RetryPolicy::DEFAULT
};

const OPTIONS: DownloadOptions = DownloadOptions {
    disk: DiskOptions::DEFAULT,
    connections: 4,
    probe_bytes: 64 * 1024,
    leases: Sizes::DEFAULT,
    checkpoints: Cadence::DEFAULT,
    retry: FAST,
    adaptive: None,
};

const LEN: usize = 1_000_000;

fn serve(options: ServerOptions) -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(LEN, 48);
    (
        TestServer::start(vec![("f.bin".into(), data.clone())], options),
        data,
    )
}

/// A control that records every status the download reports.
fn recording() -> (Control, Arc<Mutex<Vec<DownloadStatus>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&seen);
    let control = Control::default().with_status(move |status, _| s.lock().unwrap().push(status));
    (control, seen)
}

fn run(
    url: &str,
    dest: &Path,
    options: DownloadOptions,
    control: &Control,
) -> Result<DownloadReport, DownloadError> {
    download_with(&curl_loop(), url, dest, options, control)
}

#[test]
fn a_429_with_retry_after_5_gets_no_request_before_5_s() {
    let (s, data) = serve(ServerOptions {
        first_statuses: Some(FirstStatuses {
            status: 429,
            count: 1,
            retry_after: Some("5".into()),
        }),
        ..ServerOptions::default()
    });
    let dir = Dir::new("retry-after");
    let dest = dir.file("out.bin");
    let (control, seen) = recording();
    run(&s.url("f.bin"), &dest, OPTIONS, &control).expect("the download succeeds after the wait");
    assert!(std::fs::read(&dest).unwrap() == data);
    let times = s.counters().request_times;
    let waited = times[1] - times[0];
    assert!(
        waited >= Duration::from_secs(5),
        "the retry came after {waited:?}"
    );
    assert!(
        seen.lock().unwrap().contains(&DownloadStatus::Retrying),
        "{:?}",
        seen.lock().unwrap()
    );
}

#[test]
fn a_reset_storm_backs_off_within_its_bounds_and_recovers() {
    let (s, data) = serve(ServerOptions {
        drop_first: 3,
        ..ServerOptions::default()
    });
    let dir = Dir::new("storm");
    let dest = dir.file("out.bin");
    run(&s.url("f.bin"), &dest, OPTIONS, &Control::default()).expect("the download recovers");
    assert!(std::fs::read(&dest).unwrap() == data);
    let c = s.counters();
    assert_eq!(c.dropped, 3);
    // Each retry waits at most its doubling bound (plus scheduling slack); full jitter may make
    // it shorter, which the policy's unit tests cover.
    for (i, gap) in c.request_times.windows(2).take(3).enumerate() {
        let bound = (FAST.first_wait * 2u32.pow(i as u32)).min(FAST.max_wait);
        let gap = gap[1] - gap[0];
        assert!(
            gap <= bound + Duration::from_millis(500),
            "retry {}: {gap:?}",
            i + 1
        );
    }
}

#[test]
fn a_reset_storm_that_does_not_end_fails_after_five_tries() {
    let (s, _) = serve(ServerOptions {
        drop_first: 1000,
        ..ServerOptions::default()
    });
    let dir = Dir::new("endless-storm");
    let dest = dir.file("out.bin");
    match run(&s.url("f.bin"), &dest, OPTIONS, &Control::default()) {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Transient, "{e:?}");
            assert!(e.message.contains("interrupted"), "{}", e.message);
        }
        other => panic!("expected a transient failure, got {other:?}"),
    }
    assert_eq!(s.counters().gets, u64::from(FAST.failures_in_a_row));
    assert!(!dest.exists() && !part_of(&dest).exists());
}

#[test]
fn a_slow_but_healthy_server_is_never_reset() {
    // Report 5.4: 20 KiB/s, slow but steady. The server sends a 16 KiB piece every 0.8 s, so the
    // reset limit here (3 s) is well above the gaps; production waits 30 s.
    let small = seeded_bytes(80 * 1024, 48);
    let s = TestServer::start(
        vec![("f.bin".into(), small.clone())],
        ServerOptions {
            throttle_bytes_per_sec: Some(20 * 1024),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("slow");
    let dest = dir.file("out.bin");
    let (control, seen) = recording();
    let options = DownloadOptions {
        probe_bytes: 1 << 20,
        retry: RetryPolicy {
            stalled_after: Duration::from_secs(2),
            reset_after: Duration::from_secs(3),
            ..FAST
        },
        ..OPTIONS
    };
    let report = run(&s.url("f.bin"), &dest, options, &control).expect("the download succeeds");
    assert!(std::fs::read(&dest).unwrap() == small);
    assert_eq!(s.counters().gets, 1, "one request, never reset");
    assert_eq!(report.wasted_bytes, 0);
    assert!(
        !seen.lock().unwrap().contains(&DownloadStatus::Stalled),
        "{:?}",
        seen.lock().unwrap()
    );
}

#[test]
fn a_frozen_connection_shows_stalled_then_is_reset_and_retried() {
    // Report 5.4: a frozen socket is reset (30 s in production; 1 s here), and its lease is
    // fetched again. A stall isn't a sign that ranges fail, so the download keeps its ranges.
    let (s, data) = serve(ServerOptions {
        freeze_later_range: Some(Freeze {
            after: 16 * 1024,
            hold: Duration::from_secs(20),
        }),
        ..ServerOptions::default()
    });
    let dir = Dir::new("frozen");
    let dest = dir.file("out.bin");
    let (control, seen) = recording();
    let options = DownloadOptions {
        connections: 1,
        ..OPTIONS
    };
    let started = Instant::now();
    let report = run(&s.url("f.bin"), &dest, options, &control).expect("the download recovers");
    assert!(std::fs::read(&dest).unwrap() == data);
    assert!(s.counters().frozen);
    assert_eq!(report.mode, Mode::Ranges);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    let seen = seen.lock().unwrap();
    let stalled = seen.iter().position(|s| *s == DownloadStatus::Stalled);
    let again = seen.iter().rposition(|s| *s == DownloadStatus::Downloading);
    assert!(
        matches!((stalled, again), (Some(a), Some(b)) if a < b),
        "Stalled, then Downloading again: {seen:?}"
    );
}

#[test]
fn a_host_that_does_not_resolve_fails_as_transient() {
    let dir = Dir::new("dns");
    let dest = dir.file("out.bin");
    let options = DownloadOptions {
        retry: RetryPolicy {
            failures_in_a_row: 2,
            // The engine's own stall times, not FAST's 1 s: a name lookup counts as silence, and
            // on a loaded or WSL machine it can take longer than 1 s (a lookup took 2.2 s there).
            // With 1 s, the request was reset before the lookup failed (#46).
            stalled_after: RetryPolicy::DEFAULT.stalled_after,
            reset_after: RetryPolicy::DEFAULT.reset_after,
            ..FAST
        },
        ..OPTIONS
    };
    match run(
        "http://velocity-v2-test.invalid/f.bin",
        &dest,
        options,
        &Control::default(),
    ) {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Transient, "{e:?}");
            assert!(e.retryable);
            assert!(e.message.contains("could not be resolved"), "{}", e.message);
        }
        other => panic!("expected a transient failure, got {other:?}"),
    }
    assert!(!part_of(&dest).exists());
}

#[test]
fn a_404_is_permanent_with_its_message_and_never_retried() {
    let (s, _) = serve(ServerOptions::default());
    let dir = Dir::new("404");
    let dest = dir.file("out.bin");
    match run(&s.url("missing.bin"), &dest, OPTIONS, &Control::default()) {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(
                (e.class, e.http_status),
                (FailureClass::Permanent, Some(404))
            );
            assert!(!e.retryable);
            assert!(e.message.contains("not found"), "{}", e.message);
        }
        other => panic!("expected a permanent failure, got {other:?}"),
    }
    assert_eq!(s.counters().gets, 1);
    assert!(!part_of(&dest).exists());
}

#[test]
fn cancelling_during_a_retry_wait_ends_at_once() {
    let (s, _) = serve(ServerOptions {
        first_statuses: Some(FirstStatuses {
            status: 503,
            count: 1,
            retry_after: Some("30".into()),
        }),
        ..ServerOptions::default()
    });
    let dir = Dir::new("cancel-wait");
    let dest = dir.file("out.bin");
    let control = Control::default();
    let url = s.url("f.bin");
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        let download = scope.spawn(|| run(&url, &dest, OPTIONS, &control));
        std::thread::sleep(Duration::from_millis(500));
        control.cancel();
        download.join().unwrap()
    });
    assert!(
        matches!(result, Err(DownloadError::Cancelled)),
        "{result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(s.counters().gets, 1, "no request after the cancel");
    assert!(!part_of(&dest).exists());
}
