//! What a download sends and how its file is named (#40), against the local server, in both
//! engine modes: the login and custom headers go only to the first URL's origin, the user agent and
//! referrer with every request, and a file without a name of its own takes the server's
//! `Content-Disposition` name, or the final URL's, sanitised, in its folder, never replacing a file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use velocity_core::domain::DownloadStatus;
use velocity_core::request::default_user_agent;
use velocity_core::transfer::{
    Credentials, DirectTransfer, Header, Limits, TransferEvent, TransferId, TransferSnapshot,
    TransferSpec,
};
use velocity_test_http::{seeded_bytes, FileHeaders, SeenRequest, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::stdio::ChildProcessEngine;

const WAIT: Duration = Duration::from_secs(30);
/// `alice:hunter2` as HTTP Basic.
const BASIC: &str = "Basic YWxpY2U6aHVudGVyMg==";

fn engines() -> Vec<(&'static str, Box<dyn DirectTransfer>)> {
    vec![
        (
            "in-process",
            Box::new(InProcessEngine::start().expect("the in-process engine starts")),
        ),
        (
            "child process",
            Box::new(
                ChildProcessEngine::spawn(Path::new(env!("CARGO_BIN_EXE_velocity-transfer")))
                    .expect("the child engine starts"),
            ),
        ),
    ]
}

/// A fresh folder, with a folder inside it to download into, so that a name that climbs out of
/// the download folder would land somewhere this test can see.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-metadata-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("downloads")).unwrap();
        Dir(p)
    }

    fn downloads(&self) -> PathBuf {
        self.0.join("downloads")
    }

    /// The names in the download folder, sorted.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.downloads())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A download into `directory`, named by the server.
fn unnamed(url: String, directory: PathBuf) -> TransferSpec {
    TransferSpec {
        urls: vec![url],
        directory,
        file_name: None,
        credentials: None,
        headers: Vec::new(),
        referrer: None,
        user_agent: None,
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        max_connections: 4,
        resume: None,
        require_encryption: false,
        ssh_key: None,
    }
}

/// Waits for the transfer to complete or fail, and returns its last snapshot.
fn finished(events: &Receiver<TransferEvent>, id: TransferId) -> TransferSnapshot {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left).expect("an event in time") {
            TransferEvent::Changed { snapshot }
                if snapshot.id == id
                    && matches!(
                        snapshot.status,
                        DownloadStatus::Completed | DownloadStatus::Failed
                    ) =>
            {
                return snapshot
            }
            _ => {}
        }
    }
}

fn serve_named(name: &str, data: &[u8], disposition: Option<&str>) -> TestServer {
    TestServer::start(
        vec![(name.into(), data.to_vec())],
        ServerOptions {
            file_headers: FileHeaders {
                content_disposition: disposition.map(str::to_owned),
                ..FileHeaders::default()
            },
            ..ServerOptions::default()
        },
    )
}

fn completed(mode: &str, done: &TransferSnapshot) {
    assert_eq!(
        done.status,
        DownloadStatus::Completed,
        "{mode}: {:?}",
        done.error
    );
}

#[test]
fn a_script_link_takes_its_utf8_name_from_content_disposition() {
    let data = seeded_bytes(300_000, 40);
    let server = serve_named(
        "download.php",
        &data,
        Some("attachment; filename=\"rates.pdf\"; filename*=UTF-8''%E2%82%AC%20rates.pdf"),
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("utf8-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let id = engine
            .add(unnamed(server.url("download.php"), dir.downloads()))
            .expect("add");
        let done = finished(&events, id);
        completed(mode, &done);
        assert_eq!(done.file_name.as_deref(), Some("€ rates.pdf"), "{mode}");
        assert_eq!(dir.names(), ["€ rates.pdf"], "{mode}: no part file left");
        assert!(
            std::fs::read(dir.downloads().join("€ rates.pdf")).unwrap() == data,
            "{mode}"
        );
    }
}

#[test]
fn a_malicious_name_is_sanitised_and_stays_in_its_folder() {
    let data = seeded_bytes(50_000, 41);
    for (disposition, saved) in [
        ("attachment; filename=\"../../escape.txt\"", "escape.txt"),
        ("attachment; filename*=UTF-8''..%5C..%5Cwin.txt", "win.txt"),
        ("attachment; filename=\"CON.txt\"", "_CON.txt"),
        ("attachment; filename=\"a.zip.vdmpart\"", "a.zip.vdmpart_"),
    ] {
        let server = serve_named("get", &data, Some(disposition));
        for (mode, mut engine) in engines() {
            let dir = Dir::new(&format!("evil-{}", mode.replace(' ', "-")));
            let events = engine.subscribe();
            let id = engine
                .add(unnamed(server.url("get"), dir.downloads()))
                .expect("add");
            let done = finished(&events, id);
            completed(mode, &done);
            assert_eq!(
                done.file_name.as_deref(),
                Some(saved),
                "{mode}: {disposition}"
            );
            assert_eq!(dir.names(), [saved], "{mode}: {disposition}");
            // Nothing reached the folder above the download folder.
            let above: Vec<_> = std::fs::read_dir(&dir.0)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(above, ["downloads"], "{mode}: {disposition}");
        }
    }
}

#[test]
fn without_content_disposition_the_final_url_names_the_file() {
    let data = seeded_bytes(80_000, 42);
    let server = TestServer::start(
        vec![("real.iso".into(), data.clone())],
        ServerOptions {
            redirects: vec![("/files/12345".into(), "/real.iso".into())],
            ..ServerOptions::default()
        },
    );
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("final-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let id = engine
            .add(unnamed(server.url("files/12345"), dir.downloads()))
            .expect("add");
        let done = finished(&events, id);
        completed(mode, &done);
        assert_eq!(done.file_name.as_deref(), Some("real.iso"), "{mode}");
        assert!(
            std::fs::read(dir.downloads().join("real.iso")).unwrap() == data,
            "{mode}"
        );
    }
}

#[test]
fn a_taken_name_gets_a_free_one_and_the_old_file_is_untouched() {
    let data = seeded_bytes(60_000, 43);
    let server = serve_named("get", &data, Some("attachment; filename=report.pdf"));
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("taken-{}", mode.replace(' ', "-")));
        std::fs::write(dir.downloads().join("report.pdf"), b"the user's own file").unwrap();
        let events = engine.subscribe();
        let id = engine
            .add(unnamed(server.url("get"), dir.downloads()))
            .expect("add");
        let done = finished(&events, id);
        completed(mode, &done);
        assert_eq!(done.file_name.as_deref(), Some("report (1).pdf"), "{mode}");
        assert_eq!(dir.names(), ["report (1).pdf", "report.pdf"], "{mode}");
        assert_eq!(
            std::fs::read(dir.downloads().join("report.pdf")).unwrap(),
            b"the user's own file",
            "{mode}"
        );
    }
}

#[test]
fn an_empty_file_is_named_too() {
    let server = serve_named("get", &[], Some("attachment; filename=empty.txt"));
    for (mode, mut engine) in engines() {
        let dir = Dir::new(&format!("empty-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let id = engine
            .add(unnamed(server.url("get"), dir.downloads()))
            .expect("add");
        let done = finished(&events, id);
        completed(mode, &done);
        assert_eq!(done.file_name.as_deref(), Some("empty.txt"), "{mode}");
        assert_eq!(dir.names(), ["empty.txt"], "{mode}");
    }
}

fn with_secrets(url: String, directory: PathBuf) -> TransferSpec {
    TransferSpec {
        file_name: Some("out.bin".to_owned()),
        credentials: Some(Credentials {
            user_name: "alice".to_owned(),
            password: "hunter2".into(),
        }),
        headers: vec![
            Header {
                name: "Cookie".to_owned(),
                value: "session=secret".into(),
            },
            Header {
                name: "X-Custom".to_owned(),
                value: "custom-value".into(),
            },
        ],
        referrer: Some("https://example.com/page".to_owned()),
        ..unnamed(url, directory)
    }
}

fn assert_secrets(mode: &str, requests: &[SeenRequest], expected: bool) {
    assert!(
        !requests.is_empty(),
        "{mode}: no request reached the server"
    );
    for seen in requests {
        let (auth, cookie, custom) = (
            seen.header("Authorization"),
            seen.header("Cookie"),
            seen.header("X-Custom"),
        );
        if expected {
            assert_eq!(auth, Some(BASIC), "{mode}: {}", seen.path);
            assert_eq!(cookie, Some("session=secret"), "{mode}: {}", seen.path);
            assert_eq!(custom, Some("custom-value"), "{mode}: {}", seen.path);
        } else {
            assert_eq!(
                (auth, cookie, custom),
                (None, None, None),
                "{mode}: {}",
                seen.path
            );
        }
        // Not secrets: they go with every request.
        assert_eq!(
            seen.header("User-Agent"),
            Some(default_user_agent().as_str()),
            "{mode}"
        );
        assert_eq!(
            seen.header("Referer"),
            Some("https://example.com/page"),
            "{mode}"
        );
    }
}

#[test]
fn a_cross_origin_redirect_drops_the_login_and_custom_headers() {
    let data = seeded_bytes(600_000, 44);
    let mirror = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    for (mode, mut engine) in engines() {
        let origin = TestServer::start(
            Vec::new(),
            ServerOptions {
                redirects: vec![("/f.bin".into(), mirror.url("f.bin"))],
                ..ServerOptions::default()
            },
        );
        let before = mirror.counters().requests.len();
        let dir = Dir::new(&format!("cross-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let id = engine
            .add(with_secrets(origin.url("f.bin"), dir.downloads()))
            .expect("add");
        let done = finished(&events, id);
        completed(mode, &done);
        assert!(
            std::fs::read(dir.downloads().join("out.bin")).unwrap() == data,
            "{mode}"
        );
        assert_secrets(mode, &origin.counters().requests, true);
        assert_secrets(mode, &mirror.counters().requests[before..], false);
    }
}

#[test]
fn the_login_and_headers_reach_the_first_origin_with_every_request() {
    let data = seeded_bytes(600_000, 45);
    let server = TestServer::start(
        vec![("f.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    for (mode, mut engine) in engines() {
        let before = server.counters().requests.len();
        let dir = Dir::new(&format!("same-{}", mode.replace(' ', "-")));
        let events = engine.subscribe();
        let id = engine
            .add(with_secrets(server.url("f.bin"), dir.downloads()))
            .expect("add");
        let done = finished(&events, id);
        completed(mode, &done);
        assert!(
            std::fs::read(dir.downloads().join("out.bin")).unwrap() == data,
            "{mode}"
        );
        assert_secrets(mode, &server.counters().requests[before..], true);
    }
}

#[test]
fn the_files_media_type_is_reported_without_its_parameters() {
    let data = seeded_bytes(50_000, 54);
    for (sent, reported) in [
        (
            Some("Application/Metalink4+XML; charset=utf-8"),
            Some("application/metalink4+xml"),
        ),
        (Some("text/plain"), Some("text/plain")),
        (Some("not a media type"), None),
        (None, None),
    ] {
        let server = TestServer::start(
            vec![("files".into(), data.clone())],
            ServerOptions {
                file_headers: FileHeaders {
                    content_type: sent.map(str::to_owned),
                    ..FileHeaders::default()
                },
                ..ServerOptions::default()
            },
        );
        for (mode, mut engine) in engines() {
            let dir = Dir::new(&format!("type-{}", mode.replace(' ', "-")));
            let events = engine.subscribe();
            let id = engine
                .add(unnamed(server.url("files"), dir.downloads()))
                .unwrap();
            let done = finished(&events, id);
            completed(mode, &done);
            assert_eq!(done.content_type.as_deref(), reported, "{mode}: {sent:?}");
        }
    }
}
