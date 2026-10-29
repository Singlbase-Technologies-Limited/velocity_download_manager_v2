//! Connection counts (#47) against the local test server: real libcurl, real sockets, real files.
//! The server counts the responses it sends at once, which is what a connection count means to a
//! server.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::adaptive::{AdaptivePolicy, Tuning};
use velocity_transfer::download::{
    download_with, Cadence, Control, DiskOptions, DownloadOptions, DownloadReport,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const MIB: usize = 1 << 20;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-connections-{name}-{}", std::process::id()));
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

/// These tests measure speed, and on a 2-vCPU machine running them at once stretched the ramp's
/// windows from 1 s to almost 3 s (#154). Each holds this for its whole run, so they run one at a
/// time; a failed one doesn't block the rest.
static SERIAL: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
fn the_connection_count_follows_live_changes_without_a_restart() {
    let _serial = one_at_a_time();
    // 1 MiB/s per connection, so a 16 MiB file is still running while the count changes.
    let data = seeded_bytes(16 * MIB, 47);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions {
            throttle_bytes_per_sec: Some(MIB as u64),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("live");
    let dest = dir.0.join("f.bin");
    let options = DownloadOptions {
        disk: DiskOptions::DEFAULT,
        connections: 1,
        probe_bytes: 64 * 1024,
        leases: Sizes::DEFAULT,
        checkpoints: Cadence::DEFAULT,
        retry: RetryPolicy::DEFAULT,
        adaptive: None,
    };
    let (lp, control) = (curl_loop(), Arc::new(Control::default()));
    let (url, d, c, l) = (
        server.url("f.bin"),
        dest.clone(),
        Arc::clone(&control),
        Arc::clone(&lp),
    );
    let download = std::thread::spawn(move || download_with(&l, &url, &d, options, &c));
    let answering = || server.counters().answering;

    // The probe, then one range at a time.
    wait_until(Duration::from_secs(10), "a range follows the probe", || {
        server.counters().gets >= 2
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        server.counters().peak_answering,
        1,
        "one connection at first"
    );

    control.set_connections(4);
    wait_until(Duration::from_secs(5), "4 responses run at once", || {
        answering() == 4
    });

    control.set_connections(1);
    wait_until(Duration::from_secs(5), "only 1 response runs", || {
        answering() == 1
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(answering(), 1, "the lower count holds");

    control.set_connections(8);
    let report = download.join().unwrap().expect("the download completes");
    assert!(
        std::fs::read(&dest).unwrap() == data,
        "the file is the source"
    );
    let c = server.counters();
    assert!(
        c.peak_answering <= 8,
        "never more than asked: {}",
        c.peak_answering
    );
    let from_zero = c
        .ranges
        .iter()
        .filter(|r| r.as_deref().is_some_and(|r| r.starts_with("bytes=0-")))
        .count();
    assert_eq!(from_zero, 1, "only the probe asked for byte 0: no restart");
    assert_eq!(report.wasted_bytes, 0, "no byte was dropped");
}

/// A download of `data` from a server throttled per connection, run on its own thread.
fn start_throttled(
    data: &[u8],
    rate: u64,
    options: DownloadOptions,
    control: &Arc<Control>,
    name: &str,
) -> (TestServer, Dir, std::thread::JoinHandle<()>) {
    let server = TestServer::start(
        vec![("f.bin".into(), data.to_vec())],
        ServerOptions {
            throttle_bytes_per_sec: Some(rate),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new(name);
    let (lp, url, dest, c) = (
        curl_loop(),
        server.url("f.bin"),
        dir.0.join("f.bin"),
        Arc::clone(control),
    );
    let download = std::thread::spawn(move || {
        download_with(&lp, &url, &dest, options, &c).expect("the download completes");
    });
    (server, dir, download)
}

const EIGHT: DownloadOptions = DownloadOptions {
    disk: DiskOptions::DEFAULT,
    connections: 8,
    probe_bytes: 64 * 1024,
    leases: Sizes::DEFAULT,
    checkpoints: Cadence::DEFAULT,
    retry: RetryPolicy::DEFAULT,
    adaptive: None,
};

#[test]
fn the_downloader_reports_its_requests_and_when_the_file_is_too_small_for_more() {
    let _serial = one_at_a_time();
    // 2 MiB in 8 leases of about 256 KiB, at 256 KiB/s each: once the first ends, the rest is
    // too small to split (a split needs twice the 1 MiB minimum).
    let control = Arc::new(Control::default());
    let (_server, _dir, download) = start_throttled(
        &seeded_bytes(2 * MIB, 48),
        256 * 1024,
        EIGHT,
        &control,
        "report",
    );
    wait_until(Duration::from_secs(5), "8 requests run", || {
        control.connections_now().active == 8
    });
    wait_until(
        Duration::from_secs(10),
        "the file is too small for more",
        || {
            let now = control.connections_now();
            now.file_bound && now.active < 8
        },
    );
    assert_eq!(control.connections_now().throttled_to, None);
    download.join().unwrap();
}

#[test]
fn the_engine_allowance_holds_a_download_below_its_ask() {
    let _serial = one_at_a_time();
    let control = Arc::new(Control::default());
    control.allow(2);
    let (server, _dir, download) = start_throttled(
        &seeded_bytes(16 * MIB, 49),
        MIB as u64,
        EIGHT,
        &control,
        "allow",
    );
    wait_until(Duration::from_secs(5), "2 responses run", || {
        server.counters().answering == 2
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        server.counters().peak_answering,
        2,
        "never more than allowed"
    );
    control.allow(4);
    wait_until(Duration::from_secs(5), "4 responses run", || {
        server.counters().answering == 4
    });
    control.allow(8);
    download.join().unwrap();
}

/// Eight connections asked for, with the engine's adaptive ramp (#47).
const ADAPTIVE: DownloadOptions = DownloadOptions {
    // The engine's own policy, with its 2 s windows. Shorter ones, 500 ms and then 1 s, misjudged
    // gains on loaded machines: a flat 5th connection once measured as a 10% gain (#154).
    adaptive: Some(AdaptivePolicy::DEFAULT),
    ..EIGHT
};

/// A server throttled per connection and, optionally, for all connections together.
fn throttled_server(data: &[u8], per_connection: u64, total: Option<u64>) -> TestServer {
    TestServer::start(
        vec![("f.bin".into(), data.to_vec())],
        ServerOptions {
            throttle_bytes_per_sec: Some(per_connection),
            throttle_total_bytes_per_sec: total,
            ..ServerOptions::default()
        },
    )
}

/// Runs a download of `server`'s file on its own thread.
fn run(
    server: &TestServer,
    dir: &Dir,
    options: DownloadOptions,
    control: &Arc<Control>,
) -> std::thread::JoinHandle<(PathBuf, DownloadReport)> {
    let (lp, url, dest, c) = (
        curl_loop(),
        server.url("f.bin"),
        dir.0.join("f.bin"),
        Arc::clone(control),
    );
    std::thread::spawn(move || {
        let report = download_with(&lp, &url, &dest, options, &c).expect("the download completes");
        (dest, report)
    })
}

#[test]
fn adaptive_connections_ramp_up_while_each_helps() {
    let _serial = one_at_a_time();
    // Per-connection throttle: every connection adds 1 MiB/s, so the ramp reaches the 4 asked
    // for. 4, so each added connection adds at least 33%, well clear of the 10% threshold and of
    // measurement noise on a loaded machine; the 8th would add only 14%.
    let data = seeded_bytes(48 * MIB, 52);
    let server = throttled_server(&data, MIB as u64, None);
    let dir = Dir::new("ramp-up");
    let control = Arc::new(Control::default());
    let four = DownloadOptions {
        connections: 4,
        ..ADAPTIVE
    };
    let download = run(&server, &dir, four, &control);
    wait_until(Duration::from_secs(10), "a range follows the probe", || {
        server.counters().gets >= 2
    });
    assert_eq!(
        control.connections_now().tuned,
        Some((1, Tuning::RampingUp)),
        "it starts with one connection"
    );
    wait_until(Duration::from_secs(30), "4 responses run", || {
        server.counters().answering == 4
    });
    assert_eq!(control.connections_now().tuned, None, "at the 4 asked for");
    let (dest, report) = download.join().unwrap();
    // Counted by the engine: the server may count a stopped request's tail as one more (#154).
    assert_eq!(report.peak_requests, 4, "never more than asked");
    assert!(
        std::fs::read(dest).unwrap() == data,
        "the file is the source"
    );
}

#[test]
fn adaptive_connections_stop_below_the_cap_when_more_stop_helping() {
    let _serial = one_at_a_time();
    // Per-IP throttle: 1 MiB/s per connection but 1.5 MiB/s in all, so 2 connections fill it and
    // a 3rd brings nothing. A correct ramp keeps 2 and peaks at 3, the one it tried. The checks
    // below allow 4, one window's headroom: on CI's 2-vCPU runners, noise sometimes passes for a
    // 10% gain once, which with 2.5 MiB/s in all (3 that help) took the peak to 5 (#159). Two
    // noisy windows in a row would still fail it. 24 MiB, about 16 s, so the download is still
    // running when the ramp decides, however slowly a loaded machine lets it measure (#154).
    let data = seeded_bytes(24 * MIB, 53);
    let server = throttled_server(&data, MIB as u64, Some(3 * MIB as u64 / 2));
    let dir = Dir::new("per-ip");
    let control = Arc::new(Control::default());
    let download = run(&server, &dir, ADAPTIVE, &control);
    wait_until(
        Duration::from_secs(30),
        "more connections stop helping",
        || matches!(control.connections_now().tuned, Some((_, Tuning::NoGain))),
    );
    let (kept, _) = control.connections_now().tuned.unwrap();
    assert!(
        kept <= 4,
        "kept {kept}, more than the 2 that help, one to try and one noisy window"
    );
    let (dest, report) = download.join().unwrap();
    // Counted by the engine over the whole download: the server may count a stopped request's
    // tail as one more (#154).
    let peak = report.peak_requests;
    eprintln!("per-IP: kept {kept} connections, at most {peak} requests at once");
    assert!(
        peak <= 4,
        "{peak} requests at once, well below the 8 asked for"
    );
    assert!(
        std::fs::read(dest).unwrap() == data,
        "the file is the source"
    );
}
