//! The torrent helper as a child process (#62, ADR-0014), against real peers on loopback: a
//! seeding libtorrent session in this test, a tracker on 127.0.0.1 that names it, and the helper
//! program under its supervisor. DHT, peer exchange and local peer discovery are off, so nothing
//! leaves the machine.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use velocity_core::domain::{DownloadStatus, ErrorCode};
use velocity_core::torrent_engine::{
    TorrentEngine, TorrentEvent, TorrentSettings, TorrentSnapshot, TorrentSource, TorrentSpec,
};
use velocity_lt::{new_session, AddParams, Session, SessionConfig, UniquePtr};
use velocity_torrent::supervisor::{EngineHealth, SupervisedTorrentEngine, SupervisorOptions};

const OFF: TorrentSettings = TorrentSettings {
    dht: false,
    peer_exchange: false,
    local_peer_discovery: false,
};

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_velocity-torrent"))
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/torrents")
}

fn manifest() -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(corpus().join("manifest.json")).unwrap()).unwrap()
}

/// Same generator as scripts/make-torrent-corpus.py.
fn content(seed: u64, len: usize) -> Vec<u8> {
    (0..len as u64)
        .map(|i| {
            let x = (i + seed * 7919).wrapping_mul(2654435761);
            (((x >> 9) ^ (x >> 17) ^ seed) & 0xFF) as u8
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh folder for one test, deleted when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-torrent-{test}-{}", std::process::id()));
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

/// The corpus torrent `name`'s files, under `dir`.
fn files_of(name: &str) -> Vec<(String, Vec<u8>)> {
    let m = manifest();
    let t = &m["torrents"][name];
    let single = t["singleFile"].as_bool().unwrap();
    t["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let rel = c["path"].as_str().unwrap();
            let path = if single {
                name.to_owned()
            } else {
                format!("{name}/{rel}")
            };
            let data = content(
                c["seed"].as_u64().unwrap(),
                c["length"].as_u64().unwrap() as usize,
            );
            (path, data)
        })
        .collect()
}

/// A libtorrent session on loopback seeding the corpus torrent `name` from `dir`.
struct Seeder {
    /// Kept alive while the test downloads from it.
    _session: UniquePtr<Session>,
    port: u16,
}

fn seed(name: &str, dir: &Path) -> Seeder {
    for (path, data) in files_of(name) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
    let session = new_session(&SessionConfig {
        listen_interfaces: "127.0.0.1:0".into(),
        user_agent: "velocity-test-seeder".into(),
        ..Default::default()
    })
    .unwrap();
    let id = session
        .add(&AddParams {
            torrent: std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap(),
            save_path: dir.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    wait_for("the seeder to check its files", 30, || {
        session.status(id).unwrap().is_seeding
    });
    let port = session.listen_port();
    assert_ne!(port, 0);
    Seeder {
        _session: session,
        port,
    }
}

/// An HTTP tracker on 127.0.0.1 that answers every announce with one peer: `peer_port` on
/// 127.0.0.1. Returns its announce URL.
fn tracker(peer_port: u16) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let mut body = b"d8:intervali60e5:peers6:".to_vec();
            body.extend_from_slice(&[127, 0, 0, 1]);
            body.extend_from_slice(&peer_port.to_be_bytes());
            body.push(b'e');
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    url
}

fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// The next event about a torrent whose status is `want`, within `secs`.
fn wait_status(
    events: &Receiver<TorrentEvent>,
    want: DownloadStatus,
    secs: u64,
) -> TorrentSnapshot {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(TorrentEvent::Changed { snapshot }) => {
                if snapshot.status == want {
                    return *snapshot;
                }
                seen.push(snapshot.status);
            }
            Ok(_) => {}
            Err(_) => panic!("no {want:?} within {secs} s; saw {seen:?}"),
        }
    }
}

fn helper() -> SupervisedTorrentEngine {
    SupervisedTorrentEngine::start(program(), SupervisorOptions::default(), OFF)
        .expect("the helper starts")
}

fn spec(source: TorrentSource, dir: &Path, trackers: Vec<String>) -> TorrentSpec {
    TorrentSpec {
        source,
        directory: dir.to_path_buf(),
        trackers,
        paused: false,
    }
}

fn file(name: &str) -> TorrentSource {
    TorrentSource::File {
        path: corpus().join(format!("{name}.torrent")),
    }
}

/// Downloads the corpus torrent `name` through the helper, from a seeder found through the local
/// tracker, and checks every file's digest.
fn downloads(name: &str, source: impl FnOnce() -> TorrentSource) {
    let seed_dir = Dir::new(&format!("{name}-seed"));
    let get_dir = Dir::new(&format!("{name}-get"));
    let seeder = seed(name, &seed_dir.0);
    let mut engine = helper();
    let events = engine.subscribe();
    let id = engine
        .add(spec(source(), &get_dir.0, vec![tracker(seeder.port)]))
        .expect("the torrent is added");

    let done = wait_status(&events, DownloadStatus::Completed, 120);
    assert_eq!(done.id, id);
    let want = &manifest()["torrents"][name];
    assert_eq!(
        done.info_hash_v1.as_deref(),
        want["v1"].as_str(),
        "{name} v1"
    );
    assert_eq!(
        done.info_hash_v2.as_deref(),
        want["v2"].as_str(),
        "{name} v2"
    );
    let total: u64 = files_of(name).iter().map(|(_, d)| d.len() as u64).sum();
    assert_eq!(
        done.progress.completed_bytes, total,
        "padding is never counted"
    );
    assert_eq!(done.progress.total_bytes, Some(total));
    assert_eq!(
        done.progress.upload_speed_bps, 0,
        "a completed torrent doesn't seed"
    );
    // Completed means flushed: every file is on disk with its content.
    for (path, data) in files_of(name) {
        let got = std::fs::read(get_dir.0.join(&path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(
            hex(&Sha256::digest(&got)),
            hex(&Sha256::digest(&data)),
            "{path}"
        );
    }
    assert_eq!(
        engine.resume(id),
        Err(ErrorCode::InvalidState),
        "seeding comes with the seeding policies"
    );
    drop(seeder);
}

#[test]
fn a_v1_torrent_downloads_through_the_helper() {
    downloads("v1-multi", || file("v1-multi"));
}

#[test]
fn a_v2_only_torrent_downloads_through_the_helper() {
    downloads("v2-multi", || file("v2-multi"));
}

#[test]
fn a_hybrid_torrent_downloads_through_the_helper() {
    downloads("hybrid-single", || file("hybrid-single"));
}

#[test]
fn a_v2_magnet_learns_its_metadata_and_downloads() {
    let uri = manifest()["magnets"]["v2-single"]
        .as_str()
        .unwrap()
        .to_owned();
    downloads("v2-single", || TorrentSource::Magnet { uri });
}

#[test]
fn bad_input_is_refused_before_libtorrent_sees_it() {
    let dir = Dir::new("refused");
    let mut engine = helper();
    let refused = |engine: &mut SupervisedTorrentEngine, spec| engine.add(spec).unwrap_err();
    for (what, bad, code) in [
        (
            "a missing folder",
            spec(file("v1-single"), &dir.0.join("none"), vec![]),
            ErrorCode::DestinationMissing,
        ),
        (
            "a relative folder",
            spec(file("v1-single"), Path::new("relative"), vec![]),
            ErrorCode::InvalidPath,
        ),
        (
            "a tracker that isn't http, https or udp",
            spec(file("v1-single"), &dir.0, vec!["ftp://t.example/a".into()]),
            ErrorCode::InvalidUrl,
        ),
        (
            "a corrupt piece layer",
            spec(file("v2-piece-layer-corrupt"), &dir.0, vec![]),
            ErrorCode::InvalidInput,
        ),
        (
            "a name that escapes the folder",
            spec(file("v2-path-traversal"), &dir.0, vec![]),
            ErrorCode::InvalidFilename,
        ),
        (
            "a missing torrent file",
            spec(file("none"), &dir.0, vec![]),
            ErrorCode::NotFound,
        ),
        (
            "a magnet link without a hash",
            spec(
                TorrentSource::Magnet {
                    uri: "magnet:?dn=x".into(),
                },
                &dir.0,
                vec![],
            ),
            ErrorCode::InvalidUrl,
        ),
    ] {
        assert_eq!(refused(&mut engine, bad), code, "{what}");
    }
    assert_eq!(engine.snapshot().unwrap(), [], "nothing was added");
    // The same torrent twice is a duplicate.
    let paused = TorrentSpec {
        paused: true,
        ..spec(file("v1-single"), &dir.0, vec![])
    };
    engine.add(paused.clone()).unwrap();
    assert_eq!(engine.add(paused), Err(ErrorCode::FileExists));
}

#[test]
fn a_torrent_can_be_paused_resumed_and_removed_without_losing_its_files() {
    let dir = Dir::new("lifecycle");
    let mut engine = helper();
    let events = engine.subscribe();
    let id = engine
        .add(TorrentSpec {
            paused: true,
            ..spec(file("v1-single"), &dir.0, vec![])
        })
        .unwrap();
    let paused = wait_status(&events, DownloadStatus::Paused, 30);
    assert_eq!(paused.name.as_deref(), Some("v1-single"));
    engine.resume(id).unwrap();
    // No peers: it waits to download, it doesn't fail.
    wait_status(&events, DownloadStatus::Downloading, 30);
    engine.pause(id).unwrap();
    wait_status(&events, DownloadStatus::Paused, 30);
    let kept = dir.0.join("kept.txt");
    std::fs::write(&kept, b"mine").unwrap();
    engine.remove(id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(TorrentEvent::Removed { id: gone }) if gone == id => break,
            Ok(_) => {}
            Err(_) => panic!("no removed event"),
        }
    }
    assert_eq!(engine.pause(id), Err(ErrorCode::NotFound));
    assert!(kept.exists(), "removing a torrent deletes nothing");
    assert!(engine.version().unwrap().starts_with("2.0.15"));
}

/// Dropping the supervisor stops the helper before it returns, so the helper's files can be
/// removed straight after; on Windows an open file can't be.
#[test]
fn dropping_the_engine_stops_the_helper_before_it_returns() {
    let dir = Dir::new("drop");
    let mut engine = helper();
    let events = engine.subscribe();
    engine
        .add(TorrentSpec {
            paused: true,
            ..spec(file("v1-multi"), &dir.0, vec![])
        })
        .unwrap();
    wait_status(&events, DownloadStatus::Paused, 30);
    let pid = engine.process_id().unwrap();
    drop(engine);
    assert_eq!(velocity_test_http::process_exists(pid), Ok(false));
    std::fs::remove_dir_all(&dir.0).expect("nothing holds the torrent's files");
}

#[test]
fn a_crashed_helper_is_restarted_and_its_torrents_fail_as_retryable() {
    let dir = Dir::new("crash");
    let mut engine = helper();
    let events = engine.subscribe();
    let health = engine.subscribe_health();
    let first = engine
        .add(TorrentSpec {
            paused: true,
            ..spec(file("v1-single"), &dir.0, vec![])
        })
        .unwrap();
    wait_status(&events, DownloadStatus::Paused, 30);
    let pid = engine.process_id().unwrap();
    velocity_test_http::kill_process(pid).unwrap();

    let lost = wait_status(&events, DownloadStatus::Failed, 30);
    assert_eq!(lost.id, first);
    let error = lost.error.unwrap();
    assert!(error.retryable, "{error:?}");
    assert_eq!(error.message, "the torrent helper stopped unexpectedly");
    let deadline = Instant::now() + Duration::from_secs(30);
    while engine.health() != EngineHealth::Running || engine.process_id() == Some(pid) {
        let _ = health.recv_timeout(Duration::from_millis(100));
        assert!(Instant::now() < deadline, "the helper never came back");
    }
    // The new helper got the settings and assigns ids after the lost one's.
    let second = engine
        .add(TorrentSpec {
            paused: true,
            ..spec(file("v1-multi"), &dir.0, vec![])
        })
        .unwrap();
    assert!(second > first, "{second:?} after {first:?}");
    assert_eq!(engine.pause(first), Err(ErrorCode::InvalidState));
    let all = engine.snapshot().unwrap();
    assert_eq!(
        all.iter().map(|s| s.id).collect::<Vec<_>>(),
        [first, second]
    );
    engine.remove(first).unwrap();
}
