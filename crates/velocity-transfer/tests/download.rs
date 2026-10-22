//! The prototype engine against the local test server (#37): real libcurl, real sockets, real
//! files. Each fixture row of research report section 5.4 that the prototype covers has a test.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use velocity_core::domain::FailureClass;
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, RangeMode, ServerOptions, TestServer};
use velocity_transfer::download::{
    download, download_with, Control, DownloadError, DownloadOptions, Mode,
};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-download-{name}-{}", std::process::id()));
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

fn serve(len: usize, range_mode: RangeMode) -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(len, 21);
    (
        TestServer::start(
            vec![("f.bin".into(), data.clone())],
            ServerOptions {
                range_mode,
                ..ServerOptions::default()
            },
        ),
        data,
    )
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

const OPTIONS: DownloadOptions = DownloadOptions {
    connections: 4,
    probe_bytes: 64 * 1024,
};

#[test]
fn downloads_with_several_connections_when_ranges_work() {
    let (s, data) = serve(1_000_000, RangeMode::Honour);
    let dir = Dir::new("ranges");
    let dest = dir.file("out.bin");
    let report =
        download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("the download succeeds");
    assert!(
        std::fs::read(&dest).unwrap() == data,
        "the file differs from the source"
    );
    assert_eq!(report.mode, Mode::Ranges);
    assert_eq!(
        (report.total, report.written_bytes, report.wasted_bytes),
        (1_000_000, 1_000_000, 0)
    );
    // The probe finishes first, then the rest runs as four segments at once: five fetches, but
    // never more than four connections.
    assert_eq!(report.fetches, 5, "the probe, then four segments");
    assert_eq!(s.counters().gets, 5);
    assert!(
        s.counters().connections <= 4,
        "{} connections for a cap of 4",
        s.counters().connections
    );
    assert_eq!(
        s.counters().heads,
        0,
        "no HEAD: the probe is the first segment"
    );
    assert!(
        !part_of(&dest).exists(),
        "the part file is gone after finalisation"
    );
}

#[test]
fn a_server_that_ignores_ranges_costs_exactly_one_get() {
    // Report 5.4: ranges ignored → a single stream with zero extra GETs.
    let (s, data) = serve(500_000, RangeMode::Ignore);
    let dir = Dir::new("ignore");
    let dest = dir.file("out.bin");
    let report =
        download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("the download succeeds");
    assert!(std::fs::read(&dest).unwrap() == data);
    assert_eq!(report.mode, Mode::SingleStream);
    assert_eq!((report.fetches, s.counters().gets), (1, 1));
    assert_eq!((report.written_bytes, report.wasted_bytes), (500_000, 0));
}

#[test]
fn a_malformed_content_range_is_refused_and_leaves_nothing_behind() {
    // Report 5.4: malformed Content-Range rejected.
    let (s, _) = serve(500_000, RangeMode::MalformedContentRange);
    let dir = Dir::new("malformed");
    let dest = dir.file("out.bin");
    match download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS) {
        Err(DownloadError::Transfer(e)) => {
            assert_eq!(e.class, FailureClass::Permanent);
            assert!(e.message.contains("Content-Range"), "{}", e.message);
        }
        other => panic!("expected a refused probe, got {other:?}"),
    }
    assert!(
        !dest.exists() && !part_of(&dest).exists(),
        "no file may be left behind"
    );
}

#[test]
fn a_200_for_a_later_range_is_never_written_and_the_rest_comes_as_one_stream() {
    // Report 5.4: a 200 for a non-zero range is never written. The prototype then fetches the
    // file as one stream and writes only the bytes it doesn't have yet.
    let (s, data) = serve(1_000_000, RangeMode::FullBodyForNonZeroStart);
    let dir = Dir::new("lost");
    let dest = dir.file("out.bin");
    let report =
        download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("the download recovers");
    assert!(
        std::fs::read(&dest).unwrap() == data,
        "the file differs from the source"
    );
    assert_eq!(report.mode, Mode::RangesLost);
    // Every byte written exactly once: nothing from the refused 200s.
    assert_eq!(report.written_bytes, 1_000_000);
    // The single stream re-sent the probe's 64 KiB; they were dropped, not written again.
    assert_eq!(report.wasted_bytes, 64 * 1024);
}

#[test]
fn an_empty_file_completes_with_zero_bytes() {
    let (s, _) = serve(0, RangeMode::Honour);
    let dir = Dir::new("empty");
    let dest = dir.file("out.bin");
    let report =
        download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("an empty file downloads");
    assert_eq!(report.mode, Mode::Empty);
    assert_eq!(std::fs::read(&dest).unwrap().len(), 0);
}

#[test]
fn a_file_smaller_than_the_probe_takes_one_get() {
    let (s, data) = serve(1000, RangeMode::Honour);
    let dir = Dir::new("small");
    let dest = dir.file("out.bin");
    let report =
        download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("the download succeeds");
    assert!(std::fs::read(&dest).unwrap() == data);
    assert_eq!(
        (report.mode, report.fetches, s.counters().gets),
        (Mode::Ranges, 1, 1)
    );
}

#[test]
fn an_existing_destination_is_never_replaced() {
    let (s, _) = serve(10_000, RangeMode::Honour);
    let dir = Dir::new("exists");
    let dest = dir.file("out.bin");
    std::fs::write(&dest, b"keep me").unwrap();
    assert!(matches!(
        download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS),
        Err(DownloadError::FileExists)
    ));
    assert_eq!(std::fs::read(&dest).unwrap(), b"keep me");
    assert!(!part_of(&dest).exists());
}

#[test]
fn an_error_status_fails_with_it_and_leaves_nothing_behind() {
    let (s, _) = serve(10, RangeMode::Honour);
    let dir = Dir::new("missing");
    let dest = dir.file("out.bin");
    match download(&curl_loop(), &s.url("nope.bin"), &dest, OPTIONS) {
        Err(DownloadError::Transfer(e)) => assert_eq!(
            (e.class, e.http_status),
            (FailureClass::Permanent, Some(404))
        ),
        other => panic!("expected a 404, got {other:?}"),
    }
    assert!(!dest.exists() && !part_of(&dest).exists());
}

#[test]
fn one_connection_downloads_in_order() {
    let (s, data) = serve(300_000, RangeMode::Honour);
    let dir = Dir::new("one");
    let dest = dir.file("out.bin");
    let report = download(
        &curl_loop(),
        &s.url("f.bin"),
        &dest,
        DownloadOptions {
            connections: 1,
            probe_bytes: 64 * 1024,
        },
    )
    .expect("the download succeeds");
    assert!(std::fs::read(&dest).unwrap() == data);
    assert_eq!(report.fetches, 2, "the probe, then the rest as one segment");
}

#[test]
fn a_slow_writer_holds_back_the_network_instead_of_queueing_the_file() {
    // Rule (transfer-engine.md, "Memory"): buffers and queues are bounded. The progress callback
    // runs on the writer's thread, so pausing it once makes the writer slow. While it is paused,
    // the server must stop sending once the engine's bounded buffers and the sockets' own buffers
    // are full (#38 found the file queued whole, so the server sent all of it during the pause).
    //
    // The test checks that the server stops, not how far ahead it got: the sockets' buffers
    // belong to the kernel, and their size depends on the system. Linux grows each one up to
    // `tcp_rmem` and `tcp_wmem` (6 and 4 MiB on WSL2), so four connections can hold about 40 MiB.
    const LEN: usize = 128 << 20;
    const PAUSE_AT: u64 = 8 << 20;
    let data = seeded_bytes(LEN, 22);
    let server = Arc::new(TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    ));
    // The server's byte count after 1 s and after 2 s of the pause.
    let sent = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
    let (s, counts) = (Arc::clone(&server), Arc::clone(&sent));
    let paused = std::sync::atomic::AtomicBool::new(false);
    let control = Control::with_progress(move |written, _| {
        if written >= PAUSE_AT && !paused.swap(true, Ordering::Relaxed) {
            for count in counts.iter() {
                std::thread::sleep(Duration::from_secs(1));
                count.store(s.counters().body_bytes_sent, Ordering::Relaxed);
            }
        }
    });
    let dir = Dir::new("slow-writer");
    let dest = dir.file("out.bin");
    download_with(&curl_loop(), &server.url("f.bin"), &dest, OPTIONS, &control)
        .expect("the download succeeds");
    assert!(std::fs::read(&dest).unwrap() == data);
    let [first, second] = [0, 1].map(|i| sent[i].load(Ordering::Relaxed));
    assert!(
        second < LEN as u64,
        "the server sent the whole file while the writer was paused"
    );
    // Once every buffer is full, nothing more is sent until the writer takes some bytes.
    assert!(
        second - first < 1 << 20,
        "the server sent {} KiB more between 1 s and 2 s of the pause ({} MiB in all)",
        (second - first) >> 10,
        second >> 20
    );
}
