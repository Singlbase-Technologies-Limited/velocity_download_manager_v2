//! The torrent helper's test harness (#62): the corpus, seeders on loopback, a local tracker,
//! and waiting for events. Each test file uses part of it.

// Each test file is its own crate and uses only part of the harness.
#![allow(dead_code, unused_imports)]

pub use std::io::{BufRead, BufReader, Write};
pub use std::net::TcpListener;
pub use std::path::{Path, PathBuf};
pub use std::sync::mpsc::Receiver;
pub use std::thread;
pub use std::time::{Duration, Instant};

pub use sha2::{Digest, Sha256};
pub use velocity_core::domain::{DownloadStatus, ErrorCode};
pub use velocity_core::seeding::SeedPolicy;
pub use velocity_core::torrent::{parse_metadata, parse_torrent, TorrentMeta};
pub use velocity_core::torrent_engine::{
    TorrentEngine, TorrentEvent, TorrentId, TorrentSettings, TorrentSnapshot, TorrentSource,
    TorrentSpec,
};
pub use velocity_lt::{new_session, AddParams, Session, SessionConfig, UniquePtr};
pub use velocity_torrent::supervisor::{EngineHealth, SupervisedTorrentEngine, SupervisorOptions};

pub const OFF: TorrentSettings = TorrentSettings {
    dht: false,
    peer_exchange: false,
    local_peer_discovery: false,
    max_active_seeds: 0,
};

pub fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_velocity-torrent"))
}

pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/torrents")
}

pub fn manifest() -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(corpus().join("manifest.json")).unwrap()).unwrap()
}

/// Same generator as scripts/make-torrent-corpus.py.
pub fn content(seed: u64, len: usize) -> Vec<u8> {
    (0..len as u64)
        .map(|i| {
            let x = (i + seed * 7919).wrapping_mul(2654435761);
            (((x >> 9) ^ (x >> 17) ^ seed) & 0xFF) as u8
        })
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh folder for one test, deleted when dropped.
pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(test: &str) -> Self {
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
pub fn files_of(name: &str) -> Vec<(String, Vec<u8>)> {
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
pub struct Seeder {
    /// Kept alive while the test downloads from it.
    pub _session: UniquePtr<Session>,
    pub port: u16,
}

/// Writes the corpus torrent `name`'s files under `dir`, as a finished download would have.
pub fn place_files(name: &str, dir: &Path) {
    for (path, data) in files_of(name) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

pub fn seed(name: &str, dir: &Path) -> Seeder {
    place_files(name, dir);
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
pub fn tracker(peer_port: u16) -> String {
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

/// An HTTP tracker on 127.0.0.1 that answers every announce with every peer that has announced
/// to it, by the port each gave, on 127.0.0.1: a swarm on loopback. Returns its announce URL.
pub fn swarm_tracker() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", listener.local_addr().unwrap());
    thread::spawn(move || {
        let mut ports: Vec<u16> = Vec::new();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            let port = request
                .split(['?', '&', ' '])
                .find_map(|part| part.strip_prefix("port="))
                .and_then(|p| p.parse::<u16>().ok());
            if let Some(port) = port.filter(|p| !ports.contains(p)) {
                ports.push(port);
            }
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            // Never the peer itself: every peer is on 127.0.0.1, and libtorrent keeps one entry
            // per address by default, so its own would take the others' place.
            let others: Vec<u16> = ports.iter().copied().filter(|p| Some(*p) != port).collect();
            let mut body = format!("d8:intervali5e5:peers{}:", others.len() * 6).into_bytes();
            for port in &others {
                body.extend_from_slice(&[127, 0, 0, 1]);
                body.extend_from_slice(&port.to_be_bytes());
            }
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

/// A libtorrent session on loopback downloading the corpus torrent `name` into `dir`, finding
/// peers through `tracker`.
pub struct Leecher {
    pub session: UniquePtr<Session>,
    pub id: u64,
}

pub fn leech(name: &str, dir: &Path, tracker: &str) -> Leecher {
    let session = new_session(&SessionConfig {
        listen_interfaces: "127.0.0.1:0".into(),
        user_agent: "velocity-test-leecher".into(),
        ..Default::default()
    })
    .unwrap();
    let id = session
        .add(&AddParams {
            torrent: std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap(),
            save_path: dir.to_string_lossy().into_owned(),
            trackers: vec![tracker.to_owned()],
            ..Default::default()
        })
        .unwrap();
    Leecher { session, id }
}

pub fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// The next event about a torrent whose status is `want`, within `secs`.
pub fn wait_status(
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

/// The next snapshot of torrent `id` that `want` accepts, within `secs`.
pub fn wait_snapshot(
    events: &Receiver<TorrentEvent>,
    id: TorrentId,
    what: &str,
    secs: u64,
    want: impl Fn(&TorrentSnapshot) -> bool,
) -> TorrentSnapshot {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(TorrentEvent::Changed { snapshot }) if snapshot.id == id => {
                if want(&snapshot) {
                    return *snapshot;
                }
                seen.push((snapshot.status, snapshot.seed_queued));
            }
            Ok(_) => {}
            Err(_) => panic!("{id:?}: not {what} within {secs} s; saw {seen:?}"),
        }
    }
}

pub fn helper() -> SupervisedTorrentEngine {
    SupervisedTorrentEngine::start(program(), SupervisorOptions::default(), OFF)
        .expect("the helper starts")
}

pub fn spec(source: TorrentSource, dir: &Path, trackers: Vec<String>) -> TorrentSpec {
    TorrentSpec {
        source,
        directory: dir.to_path_buf(),
        trackers,
        paused: false,
        choose_files: false,
        files: None,
        seeding: SeedPolicy::Never {},
    }
}

/// Every file under `dir`, as paths relative to it.
pub fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().to_string_lossy();
                out.push(rel.replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// A torrent's whole metadata, fetched from the helper chunk by chunk.
pub fn fetched_metadata(engine: &SupervisedTorrentEngine, id: TorrentId) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let chunk = engine.metadata(id, bytes.len() as u64).unwrap();
        assert_eq!(chunk.offset, bytes.len() as u64);
        bytes.extend(chunk.bytes().unwrap());
        if bytes.len() as u64 >= chunk.total {
            return bytes;
        }
    }
}

/// The index of the file at `path` (inside the torrent's folder).
pub fn index_of(meta: &TorrentMeta, path: &str) -> u32 {
    meta.files
        .iter()
        .find(|f| f.path.join("/") == path)
        .unwrap_or_else(|| panic!("{path}"))
        .index
}

/// A libtorrent session on loopback that has `name`'s metadata and none of its data: it can give a
/// magnet's metadata to a peer, and nothing else. It wants the files, so a peer that wants nothing
/// yet still interests it and isn't dropped before the metadata is exchanged.
pub fn metadata_seeder(name: &str, dir: &Path) -> Seeder {
    let session = new_session(&SessionConfig {
        listen_interfaces: "127.0.0.1:0".into(),
        user_agent: "velocity-test-seeder".into(),
        ..Default::default()
    })
    .unwrap();
    session
        .add(&AddParams {
            torrent: std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap(),
            save_path: dir.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    wait_for("the seeder to listen", 30, || session.listen_port() != 0);
    let port = session.listen_port();
    Seeder {
        _session: session,
        port,
    }
}

/// The SHA-256 of the corpus content at `path` inside the torrent `name`.
pub fn content_digest(name: &str, path: &str) -> String {
    let (_, data) = files_of(name)
        .into_iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path}"));
    hex(&Sha256::digest(&data))
}

pub fn digest_on_disk(dir: &Path, path: &str) -> String {
    hex(&Sha256::digest(std::fs::read(dir.join(path)).unwrap()))
}

pub fn file(name: &str) -> TorrentSource {
    TorrentSource::File {
        path: corpus().join(format!("{name}.torrent")),
    }
}
