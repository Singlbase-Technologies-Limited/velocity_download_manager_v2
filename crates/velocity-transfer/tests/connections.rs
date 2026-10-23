//! Connection counts (#47) against the local test server: real libcurl, real sockets, real files.
//! The server counts the responses it sends at once, which is what a connection count means to a
//! server.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::download::{download_with, Cadence, Control, DownloadOptions};
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
        connections: 1,
        probe_bytes: 64 * 1024,
        leases: Sizes::DEFAULT,
        checkpoints: Cadence::DEFAULT,
        retry: RetryPolicy::DEFAULT,
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
