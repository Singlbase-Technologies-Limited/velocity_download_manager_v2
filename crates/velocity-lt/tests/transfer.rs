//! Real transfers between two libtorrent sessions on loopback, for every corpus
//! format. No tracker, DHT or public network: the leecher is told the seeder's port.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use velocity_lt::{new_session, AddParams, Session, SessionConfig};

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

/// A fresh folder for one test, deleted when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-lt-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn session() -> cxx::UniquePtr<Session> {
    new_session(&SessionConfig {
        listen_interfaces: "127.0.0.1:0".into(),
        user_agent: "velocity-test".into(),
        peer_fingerprint: "-VL0100-".into(),
        ..Default::default()
    })
    .unwrap()
}

fn wait_listening(s: &Session) -> u16 {
    let t = Instant::now();
    loop {
        let p = s.listen_port();
        if p != 0 {
            return p;
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "session never listened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Writes the torrent's content (and, for v1/hybrid, zero padding files as a v1
/// client would have them) under `dir`.
fn write_content(dir: &Path, name: &str) {
    let m = manifest();
    let t = &m["torrents"][name];
    let single = t["singleFile"].as_bool().unwrap();
    for c in t["content"].as_array().unwrap() {
        let rel = c["path"].as_str().unwrap();
        let p = if single {
            dir.join(name)
        } else {
            dir.join(name).join(rel)
        };
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let data = content(
            c["seed"].as_u64().unwrap(),
            c["length"].as_u64().unwrap() as usize,
        );
        std::fs::write(p, data).unwrap();
    }
}

fn wait<F: FnMut() -> bool>(what: &str, secs: u64, mut f: F) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn transfer(name: &str) {
    let m = manifest();
    let entry = &m["torrents"][name];
    let bytes = std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap();
    let seed_dir = Dir::new(&format!("{name}-seed"));
    let leech_dir = Dir::new(&format!("{name}-leech"));
    write_content(seed_dir.path(), name);

    let a = session();
    let b = session();
    let a_port = wait_listening(&a);
    wait_listening(&b);

    let sid = a
        .add(&AddParams {
            torrent: bytes.clone(),
            save_path: seed_dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    wait("seeder checks its files", 30, || {
        a.status(sid).unwrap().is_seeding
    });

    let st = a.status(sid).unwrap();
    assert_eq!(
        st.info_hash_v1,
        entry["v1"].as_str().unwrap_or(""),
        "{name} v1 hash"
    );
    assert_eq!(
        st.info_hash_v2,
        entry["v2"].as_str().unwrap_or(""),
        "{name} v2 hash"
    );

    let lid = b
        .add(&AddParams {
            torrent: bytes,
            save_path: leech_dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    // Padding files are never wanted.
    let files = b.files(lid).unwrap();
    let want: Vec<bool> = entry["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["pad"].as_bool().unwrap())
        .collect();
    assert_eq!(
        files.iter().map(|f| f.pad).collect::<Vec<_>>(),
        want,
        "{name} pad layout"
    );

    b.connect_peer(lid, "127.0.0.1", a_port).unwrap();
    wait("leecher finishes", 60, || {
        b.status(lid).unwrap().is_finished
    });

    let st = b.status(lid).unwrap();
    let user_total: u64 = entry["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["length"].as_u64().unwrap())
        .sum();
    assert_eq!(
        st.total_wanted as u64, user_total,
        "{name}: padding is not wanted"
    );
    assert_eq!(st.total_wanted_done, st.total_wanted);
    // Finished means every piece passed its hash check, not that every write reached
    // the files; the content is read only once libtorrent says it is flushed.
    b.flush(lid).unwrap();
    wait("leecher flushes its files", 30, || {
        b.pop_alerts()
            .iter()
            .any(|a| a.kind == "cache_flushed" && a.id == lid)
    });

    let single = entry["singleFile"].as_bool().unwrap();
    for c in entry["content"].as_array().unwrap() {
        let rel = c["path"].as_str().unwrap();
        let p = if single {
            leech_dir.path().join(name)
        } else {
            leech_dir.path().join(name).join(rel)
        };
        let got = std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(
            hex(&Sha256::digest(&got)),
            c["sha256"].as_str().unwrap(),
            "{name}/{rel}"
        );
    }
    assert!(b.remove(lid));
    assert!(a.remove(sid));
    // Removing never deletes data.
    assert!(leech_dir.path().join(name).exists());
}

#[test]
fn v1_single_and_multi_file_transfer() {
    transfer("v1-single");
    transfer("v1-multi");
}

#[test]
fn v2_only_single_and_multi_file_transfer() {
    transfer("v2-single");
    transfer("v2-multi");
}

#[test]
fn hybrid_single_and_multi_file_transfer() {
    transfer("hybrid-single");
    transfer("hybrid-multi");
}

#[test]
fn magnet_learns_metadata_from_a_peer() {
    let m = manifest();
    let name = "v2-multi";
    let seed_dir = Dir::new("magnet-seed");
    write_content(seed_dir.path(), name);
    let a = session();
    let b = session();
    let a_port = wait_listening(&a);
    wait_listening(&b);
    let sid = a
        .add(&AddParams {
            torrent: std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap(),
            save_path: seed_dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    wait("seeder ready", 30, || a.status(sid).unwrap().is_seeding);

    let leech = Dir::new("magnet-leech");
    let lid = b
        .add(&AddParams {
            magnet: m["magnets"][name].as_str().unwrap().into(),
            save_path: leech.path().to_string_lossy().into_owned(),
            paused: false,
            dont_download: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        b.metadata(lid).is_err(),
        "no metadata before a peer sent it"
    );
    b.connect_peer(lid, "127.0.0.1", a_port).unwrap();
    wait("metadata arrives", 30, || {
        b.status(lid).unwrap().has_metadata
    });
    let md = b.metadata(lid).unwrap();
    assert!(md.starts_with(b"d4:infod") && md.ends_with(b"e"));
    // Metadata only: nothing is wanted and no payload is written, even while
    // connected to a seeder.
    std::thread::sleep(Duration::from_secs(2));
    let st = b.status(lid).unwrap();
    assert_eq!(st.total_wanted, 0);
    assert_eq!(st.total_done, 0);
    assert!(b.files(lid).unwrap().iter().all(|f| f.priority == 0));
    assert!(
        std::fs::read_dir(leech.path()).unwrap().next().is_none(),
        "the metadata-only fetch wrote files"
    );
    let alerts = b.pop_alerts();
    assert!(
        alerts
            .iter()
            .any(|a| a.kind == "metadata_received" && a.id == lid),
        "{:?}",
        alerts.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_ids_and_bad_input_are_errors_not_crashes() {
    let s = session();
    assert!(s.status(42).is_err());
    assert!(!s.remove(42));
    assert!(s
        .add(&AddParams {
            torrent: b"not bencode".to_vec(),
            save_path: "/tmp".into(),
            ..Default::default()
        })
        .is_err());
    assert!(s
        .add(&AddParams {
            magnet: "magnet:?dn=nothing".into(),
            save_path: "/tmp".into(),
            ..Default::default()
        })
        .is_err());
    let bytes = std::fs::read(corpus().join("v1-single.torrent")).unwrap();
    let dir = Dir::new("duplicate");
    let p = AddParams {
        torrent: bytes,
        save_path: dir.path().to_string_lossy().into_owned(),
        paused: true,
        ..Default::default()
    };
    s.add(&p).unwrap();
    let e = s.add(&p).unwrap_err();
    assert_eq!(e.what(), velocity_lt::DUPLICATE_TORRENT);
    assert!(!velocity_lt::version().is_empty());
}
