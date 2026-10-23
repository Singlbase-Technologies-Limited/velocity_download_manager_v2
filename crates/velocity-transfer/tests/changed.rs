//! Changed-object detection (#45, research report sections 5.3 and 5.4) against the local test
//! server: real libcurl, real sockets, real files. The server's file changes after a set number of
//! GETs. With one connection, a download asks in a known order: the probe from byte 0, then one
//! segment for the rest, so every request can be checked.

use std::path::{Path, PathBuf};
use std::time::Duration;

use velocity_core::transfer::{Checkpoint, Resume, CHECKPOINT_FORMAT};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_test_http::{
    seeded_bytes, Change, Counters, FileHeaders, RangeMode, ServerOptions, TestServer,
};
use velocity_transfer::download::{
    download, download_resumable, Cadence, Control, DownloadError, DownloadOptions, Mode,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;

const LEN: usize = 300_000;
const PROBE: &str = "bytes=0-65535";
const REST: &str = "bytes=65536-299999";
const MODIFIED: &str = "Sun, 06 Nov 1994 08:49:37 GMT";
const LATER: &str = "Sun, 06 Nov 1994 09:00:00 GMT";

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-changed-{name}-{}", std::process::id()));
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

/// One connection, so the requests come in a known order.
const OPTIONS: DownloadOptions = DownloadOptions {
    connections: 1,
    probe_bytes: 64 * 1024,
    leases: Sizes::DEFAULT,
    checkpoints: Cadence::DEFAULT,
    retry: RetryPolicy {
        first_wait: Duration::from_millis(20),
        max_wait: Duration::from_millis(200),
        ..RetryPolicy::DEFAULT
    },
    adaptive: None,
};

fn etag(tag: &str) -> FileHeaders {
    FileHeaders {
        etag: Some(tag.to_owned()),
        ..FileHeaders::default()
    }
}

/// A version of the file: its bytes, from `seed`.
fn version(seed: u64) -> Vec<u8> {
    seeded_bytes(LEN, seed)
}

fn serve(first: FileHeaders, changes: Vec<Change>, range_mode: RangeMode) -> TestServer {
    TestServer::start(
        vec![("f.bin".into(), version(1))],
        ServerOptions {
            range_mode,
            file_headers: first,
            changes,
            ..ServerOptions::default()
        },
    )
}

/// Each GET's `Range` and `If-Range`, in order.
fn asked(c: &Counters) -> Vec<(Option<&str>, Option<&str>)> {
    c.ranges
        .iter()
        .zip(&c.if_ranges)
        .map(|(r, i)| (r.as_deref(), i.as_deref()))
        .collect()
}

#[test]
fn a_file_that_changes_mid_download_starts_over_once_and_ends_as_the_new_file() {
    // Report 5.4: "content changes mid-download with a new strong ETag: one restart; final file
    // equals the new content".
    let v2 = version(2);
    let s = serve(
        etag("\"v1\""),
        vec![Change {
            after: 1,
            data: v2.clone(),
            headers: etag("\"v2\""),
        }],
        RangeMode::Honour,
    );
    let dir = Dir::new("new-etag");
    let dest = dir.file("out.bin");
    let report = download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it starts over");
    assert!(
        std::fs::read(&dest).unwrap() == v2,
        "the file is the new one"
    );
    assert_eq!(report.mode, Mode::Ranges);
    assert_eq!(
        asked(&s.counters()),
        [
            (Some(PROBE), None),
            // If-Range "v1" no longer holds: the server answers the new file as 200, refused.
            (Some(REST), Some("\"v1\"")),
            (Some(PROBE), None),
            (Some(REST), Some("\"v2\"")),
        ]
    );
}

#[test]
fn a_dropped_etag_counts_as_a_changed_file() {
    // Report 5.4: "ETag dropped mid-download: treated as changed". The bytes are the same, but the
    // server can no longer show it, so the download starts over, now without a validator.
    let s = serve(
        etag("\"v1\""),
        vec![Change {
            after: 1,
            data: version(1),
            headers: FileHeaders::default(),
        }],
        RangeMode::Honour,
    );
    let dir = Dir::new("dropped");
    let dest = dir.file("out.bin");
    download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it starts over");
    assert!(std::fs::read(&dest).unwrap() == version(1));
    assert_eq!(
        asked(&s.counters()),
        [
            (Some(PROBE), None),
            (Some(REST), Some("\"v1\"")),
            (Some(PROBE), None),
            (Some(REST), None),
        ]
    );
}

#[test]
fn a_weak_etag_is_never_sent_in_if_range() {
    // RFC 9110 §13.1.5: no weak tag in If-Range, and no date while an entity tag exists.
    let s = serve(
        FileHeaders {
            etag: Some("W/\"v1\"".to_owned()),
            last_modified: Some(MODIFIED.to_owned()),
            date: Some(LATER.to_owned()),
        },
        Vec::new(),
        RangeMode::Honour,
    );
    let dir = Dir::new("weak");
    let dest = dir.file("out.bin");
    download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it downloads");
    assert!(std::fs::read(&dest).unwrap() == version(1));
    assert_eq!(
        asked(&s.counters()),
        [(Some(PROBE), None), (Some(REST), None)]
    );
}

#[test]
fn a_strong_last_modified_is_sent_in_if_range_without_an_etag() {
    let s = serve(
        FileHeaders {
            etag: None,
            last_modified: Some(MODIFIED.to_owned()),
            date: Some(LATER.to_owned()),
        },
        Vec::new(),
        RangeMode::Honour,
    );
    let dir = Dir::new("dated");
    let dest = dir.file("out.bin");
    download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it downloads");
    assert!(std::fs::read(&dest).unwrap() == version(1));
    assert_eq!(
        asked(&s.counters()),
        [(Some(PROBE), None), (Some(REST), Some(MODIFIED))]
    );
}

#[test]
fn a_second_change_fails_the_download_as_retryable() {
    // Report 5.3: after one start-over, another change fails with "The file changed on the server
    // during download", which the user may retry.
    let s = serve(
        etag("\"v1\""),
        vec![
            Change {
                after: 1,
                data: version(2),
                headers: etag("\"v2\""),
            },
            Change {
                after: 3,
                data: version(3),
                headers: etag("\"v3\""),
            },
        ],
        RangeMode::Honour,
    );
    let dir = Dir::new("twice");
    let dest = dir.file("out.bin");
    let result = download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS);
    let Err(DownloadError::Changed(e)) = result else {
        panic!("a second change fails the download: {result:?}");
    };
    assert!(e.retryable, "{e:?}");
    assert!(
        e.message
            .contains("The file changed on the server during download"),
        "{e:?}"
    );
    assert_eq!(s.counters().gets, 4, "one start-over, no more");
    assert!(!dest.exists());
    assert!(
        !part_of(&dest).exists(),
        "no bytes of either version are kept"
    );
}

#[test]
fn a_resume_of_a_file_whose_etag_changed_starts_over() {
    // The part file holds bytes of "v1"; the server now has "v2" at the same size, which only the
    // validator shows.
    let v2 = version(2);
    let s_data = TestServer::start(
        vec![("f.bin".into(), v2.clone())],
        ServerOptions {
            file_headers: etag("\"v2\""),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("resume");
    let dest = dir.file("out.bin");
    std::fs::write(part_of(&dest), &version(1)[..100_000]).unwrap();
    let checkpoint = Checkpoint {
        format: CHECKPOINT_FORMAT,
        source: s_data.url("f.bin"),
        total: LEN as u64,
        etag: Some("\"v1\"".to_owned()),
        last_modified: None,
        durable: vec![(0, 100_000)],
    };
    let report = download_resumable(
        &curl_loop(),
        &s_data.url("f.bin"),
        &dest,
        OPTIONS,
        Some(&Resume::Checkpoint { checkpoint }),
        &Control::default(),
    )
    .expect("it starts over");
    assert!(
        std::fs::read(&dest).unwrap() == v2,
        "none of the old bytes were kept"
    );
    assert_eq!(report.resumed_bytes, 0);
    assert_eq!(
        asked(&s_data.counters())[0],
        (Some("bytes=100000-299999"), Some("\"v1\"")),
        "the resume asks with the recorded ETag"
    );
}

#[test]
fn a_single_stream_after_ranges_were_lost_is_checked_too() {
    // Ranges stop working, so the rest comes as one whole-file stream that fills the gaps: if that
    // stream is another file, its bytes must not fill them.
    let v2 = version(2);
    let s = serve(
        etag("\"v1\""),
        vec![Change {
            after: 2,
            data: v2.clone(),
            headers: etag("\"v2\""),
        }],
        RangeMode::FullBodyForNonZeroStart,
    );
    let dir = Dir::new("gaps");
    let dest = dir.file("out.bin");
    download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it starts over");
    assert!(std::fs::read(&dest).unwrap() == v2, "no mix of versions");
    let c = s.counters();
    let ranges: Vec<_> = c.ranges.iter().map(Option::as_deref).collect();
    assert_eq!(
        ranges,
        [
            Some(PROBE),
            Some(REST), // a 200 with the same ETag: ranges lost
            None,       // the gap stream, already "v2": refused, start over
            Some(PROBE),
            Some(REST),
            None,
        ]
    );
}
