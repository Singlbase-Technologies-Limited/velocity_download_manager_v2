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
    download, download_resumable, Cadence, Control, DownloadError, DownloadOptions, Expected, Mode,
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
            content_encoding: None,
            content_disposition: None,
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
            content_encoding: None,
            content_disposition: None,
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
        content_encoding: None,
        durable: vec![(0, 100_000)],
    };
    let report = download_resumable(
        &curl_loop(),
        &s_data.url("f.bin"),
        &dest,
        OPTIONS,
        Some(&Resume::Checkpoint { checkpoint }),
        &Expected::default(),
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

fn encoded(encoding: &str) -> FileHeaders {
    FileHeaders {
        content_encoding: Some(encoding.to_owned()),
        ..FileHeaders::default()
    }
}

#[test]
fn a_file_with_an_encoding_nobody_asked_for_is_saved_as_sent() {
    // PO decision on #45: a .tar.gz served with Content-Encoding: gzip is stored byte for byte,
    // never decoded, with ranges while every answer has the same encoding.
    let s = serve(encoded("gzip"), Vec::new(), RangeMode::Honour);
    let dir = Dir::new("gzip");
    let dest = dir.file("out.bin");
    let report = download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it downloads");
    assert!(
        std::fs::read(&dest).unwrap() == version(1),
        "stored as sent"
    );
    assert_eq!(report.mode, Mode::Ranges);
}

#[test]
fn an_encoding_that_changes_between_answers_starts_over_as_one_stream() {
    // Report 5.4: "Content-Encoding changes between responses: ranges stop; restart as a single
    // stream". Ranges count encoded bytes, so none of the earlier ones are kept.
    let s = serve(
        FileHeaders::default(),
        vec![Change {
            after: 1,
            data: version(1),
            headers: encoded("gzip"),
        }],
        RangeMode::Honour,
    );
    let dir = Dir::new("encoding-changed");
    let dest = dir.file("out.bin");
    let report = download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it starts over");
    assert!(std::fs::read(&dest).unwrap() == version(1));
    assert_eq!(report.mode, Mode::SingleStream);
    assert_eq!(
        asked(&s.counters()),
        [(Some(PROBE), None), (Some(REST), None), (None, None)]
    );
}

#[test]
fn a_resume_needs_the_encoding_its_checkpoint_recorded() {
    let s = serve(encoded("gzip"), Vec::new(), RangeMode::Honour);
    let resume = |name: &str, encoding: Option<&str>| {
        let dir = Dir::new(name);
        let dest = dir.file("out.bin");
        std::fs::write(part_of(&dest), &version(1)[..100_000]).unwrap();
        let checkpoint = Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: s.url("f.bin"),
            total: LEN as u64,
            etag: None,
            last_modified: None,
            content_encoding: encoding.map(str::to_owned),
            durable: vec![(0, 100_000)],
        };
        let report = download_resumable(
            &curl_loop(),
            &s.url("f.bin"),
            &dest,
            OPTIONS,
            Some(&Resume::Checkpoint { checkpoint }),
            &Expected::default(),
            &Control::default(),
        )
        .expect("it finishes");
        assert!(std::fs::read(&dest).unwrap() == version(1), "{name}");
        report
    };
    let same = resume("resume-gzip", Some("gzip"));
    assert_eq!((same.resumed_bytes, same.mode), (100_000, Mode::Ranges));
    // Saved before the server applied an encoding: the old bytes can't be trusted.
    let other = resume("resume-identity", None);
    assert_eq!((other.resumed_bytes, other.mode), (0, Mode::SingleStream));
}

#[test]
fn a_link_that_stops_working_after_the_first_answer_has_expired() {
    // Report 5.3: a signed link refused part way needs a fresh link from the browser.
    let s = serve(FileHeaders::default(), Vec::new(), RangeMode::Status(403));
    let dir = Dir::new("expired");
    let dest = dir.file("out.bin");
    std::fs::write(part_of(&dest), &version(1)[..100_000]).unwrap();
    let checkpoint = Checkpoint {
        format: CHECKPOINT_FORMAT,
        source: s.url("f.bin"),
        total: LEN as u64,
        etag: None,
        last_modified: None,
        content_encoding: None,
        durable: vec![(0, 100_000)],
    };
    let result = download_resumable(
        &curl_loop(),
        &s.url("f.bin"),
        &dest,
        OPTIONS,
        Some(&Resume::Checkpoint { checkpoint }),
        &Expected::default(),
        &Control::default(),
    );
    let Err(DownloadError::Transfer(e)) = result else {
        panic!("a 403 fails the download: {result:?}");
    };
    assert_eq!(
        e.message,
        "The download link expired (403). Get a fresh link from the browser."
    );
    assert!(!e.retryable);
    assert_eq!(s.counters().gets, 1, "never retried");
}

#[test]
fn a_retried_first_answer_of_another_file_starts_over() {
    // The first answer is cut off without giving ranges, so it is asked for again and only the
    // missing bytes are written: if the file changed in between, those must not be its bytes.
    let v2 = version(2);
    let s = TestServer::start(
        vec![("f.bin".into(), version(1))],
        ServerOptions {
            range_mode: RangeMode::Ignore,
            file_headers: etag("\"v1\""),
            changes: vec![Change {
                after: 1,
                data: v2.clone(),
                headers: etag("\"v2\""),
            }],
            cut_first: Some(100_000),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("probe-retry");
    let dest = dir.file("out.bin");
    let report = download(&curl_loop(), &s.url("f.bin"), &dest, OPTIONS).expect("it starts over");
    assert!(std::fs::read(&dest).unwrap() == v2, "no mix of versions");
    assert_eq!(report.mode, Mode::SingleStream);
    assert_eq!(
        s.counters().gets,
        3,
        "the cut answer, the refused retry, the new start"
    );
}
