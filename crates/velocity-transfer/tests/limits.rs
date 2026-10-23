//! Speed limits (#49) against the local test server: real libcurl, real sockets, real files.
//!
//! Each test measures the bytes written over a window, from the downloader's own progress
//! callback, which sees every write. The issue's tolerance is ±10% over 30 s; every run checks
//! ±10% over 10 s, and `VELOCITY_LIMIT_SECONDS=30` runs the full window by hand (PO decision on
//! #49).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use velocity_curl::{CurlLoop, Libcurl, LoopOptions, RateLimit};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::download::{
    download_with, Cadence, Control, DownloadError, DownloadOptions,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const MIB: u64 = 1 << 20;
const TOLERANCE: f64 = 0.10;

/// The measurement window: 10 s, or `VELOCITY_LIMIT_SECONDS`.
fn window() -> Duration {
    let secs = std::env::var("VELOCITY_LIMIT_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(10);
    Duration::from_secs(secs)
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-limits-{name}-{}", std::process::id()));
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

fn curl_loop() -> Arc<CurlLoop> {
    Arc::new(
        CurlLoop::start(
            &Libcurl::start().expect("libcurl starts"),
            LoopOptions::default(),
        )
        .expect("the loop starts"),
    )
}

const OPTIONS: DownloadOptions = DownloadOptions {
    connections: 4,
    probe_bytes: 1 << 20,
    leases: Sizes::DEFAULT,
    checkpoints: Cadence::DEFAULT,
    retry: RetryPolicy::DEFAULT,
};

/// A running download whose written bytes are counted as they are written.
struct Running {
    written: Arc<AtomicU64>,
    control: Arc<Control>,
    thread: std::thread::JoinHandle<Result<(), DownloadError>>,
}

impl Running {
    fn start(lp: &Arc<CurlLoop>, url: String, dest: PathBuf, limits: Vec<RateLimit>) -> Self {
        let written = Arc::new(AtomicU64::new(0));
        let w = Arc::clone(&written);
        let control = Arc::new(
            Control::with_progress(move |bytes, _| w.store(bytes, Ordering::Release))
                .with_limits(limits),
        );
        let (lp, c) = (Arc::clone(lp), Arc::clone(&control));
        let thread =
            std::thread::spawn(move || download_with(&lp, &url, &dest, OPTIONS, &c).map(|_| ()));
        Running {
            written,
            control,
            thread,
        }
    }

    fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Cancels the download and waits for it to end.
    fn stop(self) {
        self.control.cancel();
        let _ = self.thread.join();
    }
}

/// The bytes per second all `downloads` wrote together over `window`, after `settle`.
fn measure(downloads: &[&Running], settle: Duration, window: Duration) -> f64 {
    std::thread::sleep(settle);
    let total = || downloads.iter().map(|d| d.written()).sum::<u64>();
    let (before, started) = (total(), Instant::now());
    std::thread::sleep(window);
    (total() - before) as f64 / started.elapsed().as_secs_f64()
}

fn assert_near(rate: f64, expected: f64, what: &str) {
    let off = (rate - expected) / expected;
    // Printed for the record; `--nocapture` shows it.
    eprintln!(
        "{what}: {:.3} MiB/s, expected {:.2} ({:+.1}%)",
        rate / MIB as f64,
        expected / MIB as f64,
        off * 100.0
    );
    assert!(
        off.abs() <= TOLERANCE,
        "{what}: {:.2} MiB/s, expected {:.2} MiB/s ({:+.1}%)",
        rate / MIB as f64,
        expected / MIB as f64,
        off * 100.0
    );
}

/// A file large enough that a download at `bps` is still running after `seconds`.
fn serve_for(bps: u64, seconds: f64) -> TestServer {
    let len = (bps as f64 * (seconds + 5.0)) as usize;
    TestServer::start(
        vec![("f.bin".into(), seeded_bytes(len, 49))],
        ServerOptions::default(),
    )
}

#[test]
fn a_global_limit_holds_across_three_downloads() {
    // #49's acceptance fixture: 4 MiB/s shared by 3 downloads, within ±10%.
    let global_bps = 4 * MIB;
    let window = window();
    let server = serve_for(global_bps, window.as_secs_f64());
    let dir = Dir::new("global");
    let lp = curl_loop();
    let global = RateLimit::new(Some(global_bps));
    let downloads: Vec<Running> = (0..3)
        .map(|i| {
            let own = RateLimit::new(None);
            Running::start(
                &lp,
                server.url("f.bin"),
                dir.0.join(format!("out-{i}.bin")),
                vec![own, global.clone()],
            )
        })
        .collect();
    let all: Vec<&Running> = downloads.iter().collect();
    let rate = measure(&all, Duration::from_secs(1), window);
    let shares: Vec<u64> = downloads.iter().map(Running::written).collect();
    for d in downloads {
        d.stop();
    }
    assert_near(rate, global_bps as f64, "three downloads together");
    // Shared, not taken by one: each got some of it.
    for (i, share) in shares.iter().enumerate() {
        assert!(*share > 0, "download {i} got nothing: {shares:?}");
    }
}

#[test]
fn a_changed_limit_takes_effect_within_2_s() {
    let server = serve_for(4 * MIB, 6.0);
    let dir = Dir::new("change");
    let lp = curl_loop();
    let own = RateLimit::new(Some(MIB / 2));
    let d = Running::start(
        &lp,
        server.url("f.bin"),
        dir.0.join("out.bin"),
        vec![own.clone()],
    );
    let before = measure(&[&d], Duration::from_secs(1), Duration::from_secs(2));
    own.set(Some(4 * MIB));
    // Within 2 s of the change, the download runs at the new limit.
    let after = measure(&[&d], Duration::ZERO, Duration::from_secs(2));
    d.stop();
    assert_near(before, (MIB / 2) as f64, "before the change");
    assert_near(after, (4 * MIB) as f64, "in the 2 s after the change");
}

#[test]
fn a_download_limit_holds_under_a_higher_global_one_and_the_rest_goes_to_the_others() {
    // 4 MiB/s in all; one download is limited to 1 MiB/s, so the other gets the remaining 3.
    let window = Duration::from_secs(5);
    let server = serve_for(4 * MIB, window.as_secs_f64());
    let dir = Dir::new("per-download");
    let lp = curl_loop();
    let global = RateLimit::new(Some(4 * MIB));
    let slow = Running::start(
        &lp,
        server.url("f.bin"),
        dir.0.join("slow.bin"),
        vec![RateLimit::new(Some(MIB)), global.clone()],
    );
    let fast = Running::start(
        &lp,
        server.url("f.bin"),
        dir.0.join("fast.bin"),
        vec![RateLimit::new(None), global.clone()],
    );
    std::thread::sleep(Duration::from_secs(1));
    let (s0, f0, started) = (slow.written(), fast.written(), Instant::now());
    std::thread::sleep(window);
    let secs = started.elapsed().as_secs_f64();
    let slow_rate = (slow.written() - s0) as f64 / secs;
    let fast_rate = (fast.written() - f0) as f64 / secs;
    slow.stop();
    fast.stop();
    assert_near(slow_rate, MIB as f64, "the limited download");
    assert_near(fast_rate, (3 * MIB) as f64, "the other download");
}
