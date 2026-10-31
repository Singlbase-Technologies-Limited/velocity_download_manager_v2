//! The downloader over HTTP/2 (#47), against the local HTTP/2 server with prior knowledge (the
//! `test-h2c` feature; PO decision on #47): the probe's fixtures moved here from #41, the split's
//! stream reset moved from #42, and the multiplex policy. Real libcurl, real sockets, real files.

use std::path::PathBuf;
use std::time::Duration;

use velocity_core::domain::FailureClass;
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, Http2Options, Http2Server, RangeMode};
use velocity_transfer::adaptive::AdaptivePolicy;
use velocity_transfer::download::{
    download, Cadence, DiskOptions, DownloadError, DownloadOptions, Mode,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const MIB: usize = 1 << 20;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-http2-{name}-{}", std::process::id()));
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

fn h2c_loop() -> CurlLoop {
    CurlLoop::start(
        &Libcurl::start().expect("libcurl starts"),
        LoopOptions {
            http2_prior_knowledge: true,
            ..LoopOptions::default()
        },
    )
    .expect("the loop starts")
}

const FIXED: DownloadOptions = DownloadOptions {
    disk: DiskOptions::DEFAULT,
    connections: 4,
    probe_bytes: 64 * 1024,
    leases: Sizes::DEFAULT,
    checkpoints: Cadence::DEFAULT,
    retry: RetryPolicy::DEFAULT,
    adaptive: None,
};

fn serve(len: usize, options: Http2Options) -> (Http2Server, Vec<u8>) {
    let data = seeded_bytes(len, 71);
    (
        Http2Server::start(vec![("f.bin".into(), data.clone())], options),
        data,
    )
}

fn in_mode(range_mode: RangeMode) -> Http2Options {
    Http2Options {
        range_mode,
        ..Http2Options::default()
    }
}

#[test]
fn the_probe_classifies_answers_over_http2_as_over_http1() {
    // #41's fixture rows over HTTP/2 (moved to #47): libcurl gives the engine the same status and
    // headers over either version, and the answers must be classified the same.
    let dir = Dir::new("probe");
    let lp = h2c_loop();

    let (s, data) = serve(1_000_000, Http2Options::default());
    let r = download(&lp, &s.url("f.bin"), &dir.file("honour"), FIXED).expect("ranges");
    assert!(std::fs::read(dir.file("honour")).unwrap() == data);
    assert_eq!(
        (r.mode, r.fetches),
        (Mode::Ranges, 5),
        "the probe, then 4 segments"
    );

    let (s, data) = serve(500_000, in_mode(RangeMode::Ignore));
    let r = download(&lp, &s.url("f.bin"), &dir.file("ignore"), FIXED).expect("one stream");
    assert!(std::fs::read(dir.file("ignore")).unwrap() == data);
    assert_eq!(
        (r.mode, r.fetches, s.counters().gets),
        (Mode::SingleStream, 1, 1)
    );

    let (s, _) = serve(500_000, in_mode(RangeMode::MalformedContentRange));
    match download(&lp, &s.url("f.bin"), &dir.file("malformed"), FIXED) {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Permanent);
            assert!(e.message.contains("Content-Range"), "{}", e.message);
        }
        other => panic!("a malformed Content-Range must be refused: {other:?}"),
    }
    assert!(!dir.file("malformed").exists());

    let (s, data) = serve(1_000_000, in_mode(RangeMode::FullBodyForNonZeroStart));
    let r =
        download(&lp, &s.url("f.bin"), &dir.file("full"), FIXED).expect("the rest as one stream");
    assert!(std::fs::read(dir.file("full")).unwrap() == data);
    assert_eq!(r.mode, Mode::RangesLost);
    assert_eq!(r.written_bytes, 1_000_000);

    let (s, data) = serve(1_000_000, in_mode(RangeMode::UnknownTotal));
    let r = download(&lp, &s.url("f.bin"), &dir.file("unknown"), FIXED).expect("sequential");
    assert!(std::fs::read(dir.file("unknown")).unwrap() == data);
    assert_eq!(r.mode, Mode::SequentialRanges);

    let (s, data) = serve(1_000_000, in_mode(RangeMode::ShortRanges(100_000)));
    let r = download(&lp, &s.url("f.bin"), &dir.file("short"), FIXED).expect("short ranges");
    assert!(std::fs::read(dir.file("short")).unwrap() == data);
    assert_eq!((r.mode, r.wasted_bytes), (Mode::Ranges, 0));

    let (s, data) = serve(500_000, in_mode(RangeMode::Multipart));
    let r = download(&lp, &s.url("f.bin"), &dir.file("multipart"), FIXED).expect("retried once");
    assert!(std::fs::read(dir.file("multipart")).unwrap() == data);
    assert_eq!(r.mode, Mode::SingleStream);
    let ranges = s.counters().ranges;
    assert!(
        ranges.len() == 2 && ranges[0].is_some() && ranges[1].is_none(),
        "{ranges:?}"
    );

    for (status, class) in [
        (404, FailureClass::Permanent),
        (503, FailureClass::Throttled),
    ] {
        let (s, _) = serve(1000, in_mode(RangeMode::Status(status)));
        let dest = dir.file(&format!("status{status}"));
        let options = DownloadOptions {
            retry: RetryPolicy {
                failures_in_a_row: 1,
                ..RetryPolicy::DEFAULT
            },
            ..FIXED
        };
        match download(&lp, &s.url("f.bin"), &dest, options) {
            Err(DownloadError::Transfer(e)) => {
                assert_eq!((e.http_status, e.class), (Some(status), class));
            }
            other => panic!("{status} must fail: {other:?}"),
        }
        assert!(!dest.exists());
    }
}

#[test]
fn a_split_over_http2_resets_only_the_stream() {
    // #42's criterion, moved to #47: the first later range is slow, so the other connection
    // finishes early and splits it. Over HTTP/2 the old request's stream is reset and the
    // connection stays: one TCP connection for the whole file.
    let (s, data) = serve(
        1_000_000,
        Http2Options {
            throttle_first_later_range: Some(300_000),
            ..Http2Options::default()
        },
    );
    let dir = Dir::new("split");
    let options = DownloadOptions {
        connections: 2,
        leases: Sizes {
            max_lease: 4 << 20,
            min_split: 32 << 10,
            align: 4 << 10,
        },
        ..FIXED
    };
    let r = download(&h2c_loop(), &s.url("f.bin"), &dir.file("f"), options).expect("split");
    assert!(std::fs::read(dir.file("f")).unwrap() == data);
    assert!(r.splits >= 1, "{r:?}");
    // The server sees the reset at its next paced send, which may come after the download ends.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while s.counters().streams_reset == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the old request's stream was never reset: {:?}",
            s.counters()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let c = s.counters();
    assert_eq!(c.connections, 1, "and the connection kept");
}

/// The engine's ramp with shorter windows, so a test runs in seconds. 4 asked for, so losing a
/// connection that helps costs at least 25%, well clear of the 10% threshold and of measurement
/// noise on a loaded machine.
const ADAPTIVE: DownloadOptions = DownloadOptions {
    connections: 4,
    adaptive: Some(AdaptivePolicy {
        // 1 s windows: each measurement averages twice the bytes of 500 ms ones, which kept
        // misjudging gains on CI's slower runners.
        min_window: Duration::from_secs(1),
        ..AdaptivePolicy::DEFAULT
    }),
    ..FIXED
};

#[test]
fn a_server_that_throttles_each_connection_gets_separate_connections() {
    // 1 MiB/s per TCP connection, however many streams share it: more streams don't help, so the
    // downloader tries connections of their own, which do. 64 MiB, so the trial of its own
    // connections settles before the file ends (#168).
    let (s, data) = serve(
        64 * MIB,
        Http2Options {
            throttle_connection_bytes_per_sec: Some(MIB as u64),
            ..Http2Options::default()
        },
    );
    let dir = Dir::new("per-tcp");
    // 8 asked for (#168): the streams start at 8 and step down while the speed holds, so they stop
    // below 8 and the trial, which starts only on "no gain", runs. Each count takes 2 windows, so
    // after a first window that isn't measured, the streams settle by about 17 s and the trial's own
    // connections by about 22 s (#221).
    let options = DownloadOptions {
        connections: 8,
        ..ADAPTIVE
    };
    let r = download(&h2c_loop(), &s.url("f.bin"), &dir.file("f"), options).expect("done");
    assert!(std::fs::read(dir.file("f")).unwrap() == data);
    let c = s.counters();
    eprintln!(
        "per TCP connection: {} connections, at most {} streams at once",
        c.connections, c.peak_answering
    );
    assert!(
        r.own_connections,
        "it chose connections of their own: {r:?}"
    );
    // The mode is the criterion. How far the trial's own ramp got varies with the machine (CI's
    // Windows runner settled at 3), so this only checks several of its own ran together.
    assert!(
        c.peak_answering >= 3,
        "several connections of their own at once: {c:?}"
    );
    assert!(
        c.connections >= 4,
        "one for multiplexing, then one each: {c:?}"
    );
}

#[test]
fn a_server_that_throttles_each_stream_keeps_one_multiplexed_connection() {
    // 1 MiB/s per stream: every added stream helps, so the downloader stays on one connection.
    let (s, data) = serve(
        32 * MIB,
        Http2Options {
            throttle_stream_bytes_per_sec: Some(MIB as u64),
            ..Http2Options::default()
        },
    );
    let dir = Dir::new("per-stream");
    let r = download(&h2c_loop(), &s.url("f.bin"), &dir.file("f"), ADAPTIVE).expect("done");
    assert!(std::fs::read(dir.file("f")).unwrap() == data);
    let c = s.counters();
    eprintln!(
        "per stream: {} connections, at most {} streams at once",
        c.connections, c.peak_answering
    );
    assert!(!r.own_connections, "{r:?}");
    assert_eq!(c.peak_answering, 4, "it kept the 4 asked for: {c:?}");
    assert_eq!(c.connections, 1, "as streams on one connection: {c:?}");
}
