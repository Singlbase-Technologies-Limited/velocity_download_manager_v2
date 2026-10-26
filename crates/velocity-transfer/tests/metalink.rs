//! Metalink end to end (#54): the manager, the in-process engine and the local server. A link to a
//! Metalink file is followed; the file it lists is saved in its subfolder from its best source,
//! and with one wrong byte in one piece, only that piece is fetched again.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use velocity_core::database::Database;
use velocity_core::domain::{DownloadKind, DownloadStatus, Verification};
use velocity_core::manager::Manager;
use velocity_core::transfer::{Limits, TransferSpec};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;

const WAIT: Duration = Duration::from_secs(60);
const PIECE: usize = 64 * 1024;
const LEN: usize = 16 * PIECE;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-metalink-e2e-{name}-{}", std::process::id()));
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

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A Metalink 4 document for `data` at `url`: its SHA-256, its 64 KiB pieces' SHA-256, and a
/// source that doesn't exist, ranked after the real one.
fn document(url: &str, data: &[u8]) -> String {
    let pieces: String = data
        .chunks(PIECE)
        .map(|c| format!("<hash>{}</hash>", hex(c)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <metalink xmlns=\"urn:ietf:params:xml:ns:metalink\">\
         <file name=\"disc/image.iso\"><size>{len}</size>\
         <hash type=\"sha-256\">{whole}</hash>\
         <pieces length=\"{PIECE}\" type=\"sha-256\">{pieces}</pieces>\
         <url priority=\"9\">http://127.0.0.1:9/missing.iso</url>\
         <url priority=\"1\">{url}</url>\
         </file></metalink>",
        len = data.len(),
        whole = hex(data),
    )
}

/// How many GETs asked for exactly `bytes=a-(b−1)`.
fn asked_for(server: &TestServer, a: usize, b: usize) -> usize {
    let range = format!("bytes={a}-{}", b - 1);
    server
        .counters()
        .ranges
        .iter()
        .filter(|r| r.as_deref() == Some(range.as_str()))
        .count()
}

/// Pumps the manager until download `id` exists and has finished.
fn finished(m: &mut Manager, id: &str) -> DownloadStatus {
    let deadline = Instant::now() + WAIT;
    loop {
        m.pump(0);
        if let Some(status) = m.get(id).map(|r| r.status) {
            if status.is_terminal() {
                return status;
            }
        }
        assert!(Instant::now() < deadline, "{id} didn't finish in time");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_followed_metalink_downloads_its_file_and_fetches_only_a_bad_piece_again() {
    let data = seeded_bytes(LEN, 54);
    // The data comes first, so the bad byte (piece 8, once) is in it, never in the document.
    let server = TestServer::start(
        vec![("image.iso".into(), data.clone())],
        ServerOptions {
            corrupt_once: Some(7 * PIECE as u64 + 100),
            ..ServerOptions::default()
        },
    );
    let documents = TestServer::start(
        vec![(
            "files.meta4".into(),
            document(&server.url("image.iso"), &data).into_bytes(),
        )],
        ServerOptions::default(),
    );
    let dir = Dir::new("follow");
    let mut m = Manager::new(
        Box::new(InProcessEngine::start().expect("the in-process engine starts")),
        Arc::new(Database::open_in_memory().unwrap()),
    );
    m.set_metalink_enabled(true);
    let link = TransferSpec {
        urls: vec![documents.url("files.meta4")],
        directory: dir.0.clone(),
        file_name: None,
        credentials: None,
        headers: Vec::new(),
        referrer: None,
        user_agent: None,
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        max_connections: 1,
        resume: None,
        require_encryption: false,
        ssh_key: None,
    };
    let id = m.add(DownloadKind::Http, link, 0).unwrap();
    assert_eq!(finished(&mut m, &id), DownloadStatus::Completed);

    // The Metalink file added the next download.
    let image = format!("d-{:04}", 2);
    let status = finished(&mut m, &image);
    let record = m.get(&image).unwrap().clone();
    assert_eq!(status, DownloadStatus::Completed, "{:?}", record.error);
    assert_eq!(record.kind, DownloadKind::Metalink);
    assert_eq!(record.source_url, server.url("image.iso"));
    assert_eq!(record.verification, Some(Verification::Verified));
    let saved = std::fs::read(dir.0.join("disc").join("image.iso")).unwrap();
    assert_eq!(hex(&saved), hex(&data));
    assert!(server.counters().corrupted, "the bad byte was sent");
    assert_eq!(
        asked_for(&server, 7 * PIECE, 8 * PIECE),
        1,
        "the bad piece is fetched again on its own"
    );
    let sent = server.counters().body_bytes_sent;
    assert!(
        sent <= (LEN + PIECE) as u64,
        "{sent} bytes for {LEN} and one piece"
    );
}
