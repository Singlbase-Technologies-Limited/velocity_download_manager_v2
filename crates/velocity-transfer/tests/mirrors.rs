//! Downloads from several sources at once (#55, PO decision on #55) against local servers: the
//! sources proven to serve the same file share its ranges, the faster ones more; a bad source is
//! demoted or dropped, and one that serves other bytes is caught by the piece hashes and never
//! mixed in.

use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};
use velocity_core::transfer::{HashAlgorithm, PieceHashes};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions, RequestOptions};
use velocity_test_http::{seeded_bytes, FileHeaders, ServerOptions, TestServer};
use velocity_transfer::download::{
    download_from, Cadence, Control, DownloadError, DownloadOptions, DownloadReport, Expected,
    Target,
};
use velocity_transfer::retry::RetryPolicy;
use velocity_transfer::scheduler::Sizes;
use velocity_transfer::sources::{SourceReport, SourceState};

const PIECE: usize = 64 * 1024;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-mirrors-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn file(&self) -> PathBuf {
        self.0.join("out.bin")
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

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The SHA-256 of each 64 KiB of `data`: a trusted piece list.
fn pieces(data: &[u8]) -> Expected {
    Expected {
        checksum: None,
        pieces: Some(PieceHashes {
            algorithm: HashAlgorithm::Sha256,
            length: PIECE as u64,
            hex: data.chunks(PIECE).map(hex).collect(),
        }),
    }
}

/// Four connections and leases of at most 256 KiB, so every source gets ranges; quick retries.
const OPTIONS: DownloadOptions = DownloadOptions {
    connections: 4,
    probe_bytes: 64 * 1024,
    leases: Sizes {
        max_lease: 256 << 10,
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

fn serve(data: &[u8], options: ServerOptions) -> TestServer {
    TestServer::start(vec![("f.bin".into(), data.to_vec())], options)
}

fn tagged(tag: &str) -> ServerOptions {
    ServerOptions {
        file_headers: FileHeaders {
            etag: Some(tag.to_owned()),
            ..FileHeaders::default()
        },
        ..ServerOptions::default()
    }
}

fn run(
    urls: &[String],
    dir: &Dir,
    expected: &Expected,
    control: &Control,
) -> Result<DownloadReport, DownloadError> {
    download_from(
        &curl_loop(),
        urls,
        Target::File(&dir.file()),
        OPTIONS,
        None,
        expected,
        control,
    )
}

fn source<'a>(report: &'a DownloadReport, url: &str) -> &'a SourceReport {
    report.sources.iter().find(|s| s.url == url).unwrap()
}

/// A port nothing listens on.
const DEAD: &str = "http://127.0.0.1:9/f.bin";

#[test]
fn two_mirrors_one_missing_the_file_completes_from_the_other_and_drops_it() {
    let data = seeded_bytes(1 << 20, 55);
    let good = serve(&data, ServerOptions::default());
    let other = serve(b"not this file", ServerOptions::default());
    let missing = other.url("missing.bin");
    let dir = Dir::new("missing");
    let urls = [good.url("f.bin"), missing.clone()];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    let bad = source(&report, &missing);
    assert!(matches!(bad.state, SourceState::Disabled(_)), "{bad:?}");
    assert_eq!(bad.bytes, 0);
    assert_eq!(source(&report, &urls[0]).bytes, data.len() as u64);
}

#[test]
fn a_dead_mirror_is_demoted_and_then_dropped() {
    let data = seeded_bytes(1 << 20, 56);
    let good = serve(&data, ServerOptions::default());
    let dir = Dir::new("dead");
    let urls = [good.url("f.bin"), DEAD.to_owned()];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    let dead = source(&report, DEAD);
    assert!(dead.failures >= 1, "{dead:?}");
    assert_eq!(dead.bytes, 0);
}

#[test]
fn a_mirror_with_other_bytes_is_caught_by_the_piece_hashes_and_never_mixed_in() {
    let data = seeded_bytes(1 << 20, 57);
    let wrong = seeded_bytes(1 << 20, 58);
    let good = serve(&data, ServerOptions::default());
    let bad = serve(&wrong, ServerOptions::default());
    let dir = Dir::new("other-bytes");
    let urls = [good.url("f.bin"), bad.url("f.bin")];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    // Every byte is the good file's: the bad mirror's pieces failed and came again from the other.
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    assert!(
        bad.counters().body_bytes_sent > 0,
        "the bad mirror was asked"
    );
    let bad = source(&report, &urls[1]);
    assert!(matches!(bad.state, SourceState::Disabled(_)), "{bad:?}");
}

#[test]
fn two_good_mirrors_share_the_file() {
    let data = seeded_bytes(2 << 20, 59);
    let a = serve(&data, ServerOptions::default());
    let b = serve(&data, ServerOptions::default());
    let dir = Dir::new("share");
    let urls = [a.url("f.bin"), b.url("f.bin")];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    let (from_a, from_b) = (
        source(&report, &urls[0]).bytes,
        source(&report, &urls[1]).bytes,
    );
    assert!(from_a > 0 && from_b > 0, "{from_a} and {from_b}");
    assert_eq!(
        from_a + from_b,
        data.len() as u64,
        "each byte from one source"
    );
}

#[test]
fn the_faster_mirror_serves_more() {
    let data = seeded_bytes(4 << 20, 60);
    let slow = serve(
        &data,
        ServerOptions {
            throttle_bytes_per_sec: Some(256 << 10),
            ..ServerOptions::default()
        },
    );
    let fast = serve(&data, ServerOptions::default());
    let dir = Dir::new("faster");
    let urls = [slow.url("f.bin"), fast.url("f.bin")];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    let (from_slow, from_fast) = (
        source(&report, &urls[0]).bytes,
        source(&report, &urls[1]).bytes,
    );
    assert!(
        from_fast > 2 * from_slow,
        "{from_fast} from the fast one, {from_slow} from the slow"
    );
}

#[test]
fn without_piece_hashes_a_mirror_with_another_etag_serves_no_range() {
    let data = seeded_bytes(1 << 20, 61);
    let a = serve(&data, tagged("\"a\""));
    let b = serve(&data, tagged("\"b\""));
    let dir = Dir::new("other-etag");
    let urls = [a.url("f.bin"), b.url("f.bin")];
    let report = run(&urls, &dir, &Expected::default(), &Control::default()).expect("completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    let b = source(&report, &urls[1]);
    assert_eq!(b.state, SourceState::Unproven);
    assert_eq!(b.bytes, 0);
}

#[test]
fn without_piece_hashes_a_mirror_with_the_same_strong_etag_joins() {
    let data = seeded_bytes(1 << 20, 62);
    let a = serve(&data, tagged("\"same\""));
    let b = serve(&data, tagged("\"same\""));
    let dir = Dir::new("same-etag");
    let urls = [a.url("f.bin"), b.url("f.bin")];
    let report = run(&urls, &dir, &Expected::default(), &Control::default()).expect("completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    assert!(source(&report, &urls[1]).bytes > 0);
}

#[test]
fn a_first_source_that_is_gone_hands_the_download_to_the_next() {
    let data = seeded_bytes(512 << 10, 63);
    let good = serve(&data, ServerOptions::default());
    let dir = Dir::new("first-gone");
    let urls = [DEAD.to_owned(), good.url("f.bin")];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    assert!(matches!(
        source(&report, DEAD).state,
        SourceState::Disabled(_)
    ));
}

#[test]
fn the_login_and_custom_headers_never_go_to_a_mirror_on_another_origin() {
    let data = seeded_bytes(1 << 20, 64);
    let a = serve(&data, ServerOptions::default());
    let b = serve(&data, ServerOptions::default());
    let dir = Dir::new("login");
    let urls = [a.url("f.bin"), b.url("f.bin")];
    let control = Control::default().with_request(RequestOptions {
        headers: vec![("X-Secret".to_owned(), "s3cret".to_owned())],
        login: Some(("alice".to_owned(), "hunter2".to_owned())),
        ..RequestOptions::default()
    });
    let report = run(&urls, &dir, &pieces(&data), &control).expect("it completes");
    assert!(source(&report, &urls[1]).bytes > 0, "the mirror was used");
    let mirror = b.counters().requests;
    assert!(!mirror.is_empty());
    for r in &mirror {
        assert_eq!(r.header("authorization"), None, "{r:?}");
        assert_eq!(r.header("x-secret"), None, "{r:?}");
    }
    assert!(a
        .counters()
        .requests
        .iter()
        .all(|r| r.header("x-secret") == Some("s3cret")));
}

#[test]
fn one_source_downloads_as_before() {
    let data = seeded_bytes(1 << 20, 65);
    let a = serve(&data, ServerOptions::default());
    let dir = Dir::new("one");
    let urls = [a.url("f.bin")];
    let report = run(&urls, &dir, &pieces(&data), &Control::default()).expect("it completes");
    assert_eq!(hex(&std::fs::read(dir.file()).unwrap()), hex(&data));
    assert_eq!(
        report.sources,
        [SourceReport {
            url: urls[0].clone(),
            state: SourceState::Usable,
            bytes: data.len() as u64,
            failures: 0,
        }]
    );
}
