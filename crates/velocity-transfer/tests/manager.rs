//! The download manager on the real engine (#58, L2): a supervised `velocity-transfer serve`
//! child, a database file, and the local test server. Each direct-download command of
//! docs/PARITY.md, the completion gate (#46), a crashed engine, an engine left unavailable, and
//! the app starting again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use velocity_core::database::Database;
use velocity_core::domain::{
    DownloadKind, DownloadStatus, EngineHealth, ErrorCode, KeptReason, Verification,
};
use velocity_core::manager::Manager;
use velocity_core::naming::PART_FILE_SUFFIX;
use velocity_core::transfer::{Checksum, HashAlgorithm, Limits, TransferSpec};
use velocity_test_http::{kill_process, seeded_bytes, FirstStatuses, ServerOptions, TestServer};
use velocity_transfer::supervisor::{SupervisedEngine, SupervisorOptions};

const WAIT: Duration = Duration::from_secs(60);

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_velocity-transfer"))
}

/// Fast restarts, so the tests don't wait out the production backoff.
fn fast(max_restarts: usize) -> SupervisorOptions {
    SupervisorOptions {
        max_restarts,
        restart_window: Duration::from_secs(300),
        first_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
        ..SupervisorOptions::default()
    }
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-manager-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("files")).unwrap();
        Dir(p)
    }

    fn files(&self) -> PathBuf {
        self.0.join("files")
    }

    fn db(&self) -> PathBuf {
        self.0.join("velocity.db")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app: a manager over the database in `dir`, on a supervised engine.
struct App {
    manager: Manager,
}

impl App {
    fn open(dir: &Dir, options: SupervisorOptions) -> (App, Arc<Database>) {
        let db = Arc::new(Database::open(&dir.db()).expect("the database opens"));
        let engine = SupervisedEngine::start(program(), options).expect("the engine starts");
        let manager = Manager::open(Box::new(engine), Arc::clone(&db)).expect("the manager opens");
        (App { manager }, db)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Pumps the manager until `done` holds for the download `id`.
    fn until(&mut self, id: &str, what: &str, done: impl Fn(&Manager, &str) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.manager.pump(Self::now());
            if done(&self.manager, id) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{id} never {what}: {:?}",
                self.manager.get(id)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn until_status(&mut self, id: &str, status: DownloadStatus) {
        self.until(id, &format!("{status:?}"), |m, id| {
            m.get(id).is_some_and(|r| r.status == status)
        });
    }

    fn add(&mut self, spec: TransferSpec) -> String {
        self.manager
            .add(DownloadKind::Http, spec, Self::now())
            .expect("added")
    }
}

fn spec(url: String, dir: &Path, name: &str) -> TransferSpec {
    TransferSpec {
        urls: vec![url],
        directory: dir.to_path_buf(),
        file_name: Some(name.to_owned()),
        credentials: None,
        headers: Vec::new(),
        referrer: None,
        user_agent: None,
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        max_connections: 2,
        resume: None,
        require_encryption: false,
        ssh_key: None,
    }
}

fn part(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{PART_FILE_SUFFIX}"))
}

const SIZE: usize = 2_000_000;

fn data() -> Vec<u8> {
    seeded_bytes(SIZE, 58)
}

fn fast_server() -> TestServer {
    TestServer::start(vec![("f.bin".into(), data())], ServerOptions::default())
}

/// About 10 s for the file: slow enough to pause, crash or close part way.
fn slow_server() -> TestServer {
    TestServer::start(
        vec![("f.bin".into(), data())],
        ServerOptions {
            throttle_bytes_per_sec: Some(100_000),
            ..ServerOptions::default()
        },
    )
}

fn bytes_so_far(m: &Manager, id: &str) -> u64 {
    m.get(id).map_or(0, |r| r.progress.completed_bytes)
}

#[test]
fn a_download_completes_with_its_bytes_and_is_stored() {
    let server = fast_server();
    let dir = Dir::new("complete");
    let (mut app, db) = App::open(&dir, fast(5));
    let id = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
    app.until_status(&id, DownloadStatus::Completed);
    assert_eq!(std::fs::read(dir.files().join("f.bin")).unwrap(), data());
    let r = app.manager.get(&id).unwrap();
    assert_eq!(r.verification, Some(Verification::SizeVerified));
    assert_eq!(r.progress.completed_bytes, SIZE as u64);
    let stored = db.downloads().unwrap().remove(0);
    assert_eq!(stored.completed_bytes, SIZE as u64);
    assert!(stored.completed_at_ms.is_some());
}

#[test]
fn pause_and_resume_continue_the_same_file() {
    let server = slow_server();
    let dir = Dir::new("pause");
    let (mut app, _db) = App::open(&dir, fast(5));
    let id = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
    app.until(&id, "got bytes", |m, id| bytes_so_far(m, id) > 0);
    app.manager.pause(&id).unwrap();
    app.until_status(&id, DownloadStatus::Paused);
    let paused_at = bytes_so_far(&app.manager, &id);
    app.manager.resume(&id).unwrap();
    app.until_status(&id, DownloadStatus::Completed);
    assert!(paused_at > 0);
    assert_eq!(std::fs::read(dir.files().join("f.bin")).unwrap(), data());
}

#[test]
fn cancel_discards_the_part_file_and_retry_starts_over() {
    let server = slow_server();
    let dir = Dir::new("cancel");
    let (mut app, db) = App::open(&dir, fast(5));
    let id = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
    app.until(&id, "got bytes", |m, id| bytes_so_far(m, id) > 0);
    app.manager.cancel(&id).unwrap();
    app.until_status(&id, DownloadStatus::Removed);
    assert!(
        !part(&dir.files(), "f.bin").exists(),
        "the part file is gone"
    );
    assert_eq!(db.checkpoint(&id).unwrap(), None);

    app.manager.retry(&id).unwrap();
    assert_eq!(bytes_so_far(&app.manager, &id), 0, "it starts over");
    app.until_status(&id, DownloadStatus::Completed);
    assert_eq!(std::fs::read(dir.files().join("f.bin")).unwrap(), data());
}

#[test]
fn a_failed_download_is_retried_by_hand_and_retry_failed_retries_them_all() {
    let server = TestServer::start(
        vec![("f.bin".into(), data()), ("g.bin".into(), data())],
        ServerOptions {
            first_statuses: Some(FirstStatuses {
                status: 404,
                count: 2,
                retry_after: None,
            }),
            ..ServerOptions::default()
        },
    );
    let dir = Dir::new("retry");
    let (mut app, _db) = App::open(&dir, fast(5));
    let a = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
    app.until_status(&a, DownloadStatus::Failed);
    let b = app.add(spec(server.url("g.bin"), &dir.files(), "g.bin"));
    app.until_status(&b, DownloadStatus::Failed);
    let error = app.manager.get(&a).unwrap().error.clone().unwrap();
    assert_eq!(error.http_status, Some(404));
    assert!(!error.retryable);

    app.manager.retry(&a).unwrap();
    app.until_status(&a, DownloadStatus::Completed);
    app.manager.retry_failed().unwrap();
    app.until_status(&b, DownloadStatus::Completed);
    assert_eq!(std::fs::read(dir.files().join("g.bin")).unwrap(), data());
}

#[test]
fn pause_all_and_resume_all_cover_every_download() {
    let server = slow_server();
    let dir = Dir::new("all");
    let (mut app, _db) = App::open(&dir, fast(5));
    let a = app.add(spec(server.url("f.bin"), &dir.files(), "a.bin"));
    let b = app.add(spec(server.url("f.bin"), &dir.files(), "b.bin"));
    app.until(&a, "got bytes", |m, id| bytes_so_far(m, id) > 0);
    app.manager.pause_all().unwrap();
    app.until_status(&a, DownloadStatus::Paused);
    app.until_status(&b, DownloadStatus::Paused);
    app.manager.resume_all().unwrap();
    app.until(&a, "resumed", |m, id| {
        m.get(id)
            .is_some_and(|r| r.status != DownloadStatus::Paused)
    });
    app.until(&b, "resumed", |m, id| {
        m.get(id)
            .is_some_and(|r| r.status != DownloadStatus::Paused)
    });
    for id in [&a, &b] {
        app.manager.cancel(id).unwrap();
        app.until_status(id, DownloadStatus::Removed);
    }
}

#[test]
fn remove_and_clear_completed_keep_or_delete_only_the_downloads_own_file() {
    let server = fast_server();
    let dir = Dir::new("remove");
    let (mut app, db) = App::open(&dir, fast(5));
    let keep = app.add(spec(server.url("f.bin"), &dir.files(), "keep.bin"));
    let delete = app.add(spec(server.url("f.bin"), &dir.files(), "delete.bin"));
    let cleared = app.add(spec(server.url("f.bin"), &dir.files(), "cleared.bin"));
    for id in [&keep, &delete, &cleared] {
        app.until_status(id, DownloadStatus::Completed);
    }
    let bystander = dir.files().join("other.bin");
    std::fs::write(&bystander, b"not a download").unwrap();

    assert_eq!(app.manager.remove(&keep, false), Ok(Vec::new()));
    assert!(dir.files().join("keep.bin").exists());
    assert_eq!(app.manager.remove(&delete, true), Ok(Vec::new()));
    assert!(!dir.files().join("delete.bin").exists());
    assert_eq!(app.manager.clear_completed(), Ok(1));
    assert!(dir.files().join("cleared.bin").exists());
    assert!(bystander.exists());
    assert!(app.manager.snapshot().downloads.is_empty());
    assert!(db
        .downloads()
        .unwrap()
        .iter()
        .all(|d| !d.in_list && d.in_history));

    // A file that is no longer the download's own is kept and reported.
    let other = app.add(spec(server.url("f.bin"), &dir.files(), "grown.bin"));
    app.until_status(&other, DownloadStatus::Completed);
    std::fs::write(dir.files().join("grown.bin"), vec![0u8; SIZE + 1]).unwrap();
    let result = app.manager.remove_many(&[other], true).unwrap();
    assert_eq!(result.kept.len(), 1);
    assert_eq!(result.kept[0].reason, KeptReason::TooLarge);
    assert!(dir.files().join("grown.bin").exists());
}

#[test]
fn a_download_is_never_completed_before_its_checksum_is_proven() {
    let server = fast_server();
    let dir = Dir::new("integrity");
    let (mut app, _db) = App::open(&dir, fast(5));
    let right: String = Sha256::digest(data())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let wrong = "0".repeat(64);
    let checked = |name: &str, hex: &str| TransferSpec {
        checksum: Some(Checksum {
            algorithm: HashAlgorithm::Sha256,
            hex: hex.to_owned(),
        }),
        ..spec(server.url("f.bin"), &dir.files(), name)
    };
    let good = app.add(checked("good.bin", &right));
    let bad = app.add(checked("bad.bin", &wrong));
    app.until_status(&good, DownloadStatus::Completed);
    assert_eq!(
        app.manager.get(&good).unwrap().verification,
        Some(Verification::Verified)
    );
    // Watch the bad one to its end: it must never show Completed on the way.
    let deadline = Instant::now() + WAIT;
    loop {
        app.manager.pump(App::now());
        let status = app.manager.get(&bad).unwrap().status;
        assert_ne!(
            status,
            DownloadStatus::Completed,
            "a mismatch never completes"
        );
        if status == DownloadStatus::Failed {
            break;
        }
        assert!(Instant::now() < deadline, "the bad download never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !dir.files().join("bad.bin").exists(),
        "no file under its name"
    );
}

#[test]
fn a_crashed_engine_comes_back_and_the_download_resumes_and_completes() {
    let server = slow_server();
    let dir = Dir::new("crash");
    let db = Arc::new(Database::open(&dir.db()).unwrap());
    let engine = SupervisedEngine::start(program(), fast(5)).unwrap();
    let pid = engine.process_id().unwrap();
    let mut app = App {
        manager: Manager::open(Box::new(engine), Arc::clone(&db)).unwrap(),
    };
    let id = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
    app.until(&id, "has a checkpoint", |_, id| {
        db.checkpoint(id).unwrap().is_some()
    });
    kill_process(pid).unwrap();
    app.until(&id, "was lost and waits", |m, id| {
        m.get(id).is_some_and(|r| {
            r.status == DownloadStatus::Queued
                && r.error
                    .as_ref()
                    .is_some_and(|e| e.message.contains("engine"))
        })
    });
    app.until(&id, "resumed", |m, id| {
        m.get(id)
            .is_some_and(|r| r.status == DownloadStatus::Downloading)
    });
    app.until_status(&id, DownloadStatus::Completed);
    assert_eq!(std::fs::read(dir.files().join("f.bin")).unwrap(), data());
}

#[test]
fn an_unavailable_engine_shows_it_and_restart_engine_brings_the_downloads_back() {
    let server = slow_server();
    let dir = Dir::new("unavailable");
    let db = Arc::new(Database::open(&dir.db()).unwrap());
    let engine = SupervisedEngine::start(program(), fast(0)).unwrap();
    let pid = engine.process_id().unwrap();
    let mut app = App {
        manager: Manager::open(Box::new(engine), Arc::clone(&db)).unwrap(),
    };
    let id = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
    app.until(&id, "got bytes", |m, id| bytes_so_far(m, id) > 0);
    // With no restart allowed, one crash leaves the engine unavailable.
    kill_process(pid).unwrap();
    app.until(&id, "waits for an engine", |m, _| {
        matches!(m.engine_health(), EngineHealth::Unavailable { .. })
    });
    assert_eq!(
        app.manager.engine_health(),
        EngineHealth::Unavailable {
            code: ErrorCode::EngineUnavailable
        }
    );
    assert_eq!(
        app.manager.snapshot().engine,
        app.manager.engine_health(),
        "the UI is told"
    );
    for _ in 0..5 {
        app.manager.pump(App::now());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        app.manager.get(&id).unwrap().status,
        DownloadStatus::Queued,
        "it waits, it doesn't fail"
    );

    app.manager.restart_engine().unwrap();
    app.until_status(&id, DownloadStatus::Completed);
    assert_eq!(std::fs::read(dir.files().join("f.bin")).unwrap(), data());
}

#[test]
fn after_the_app_starts_again_an_unfinished_download_resumes_from_its_checkpoint() {
    let server = slow_server();
    let dir = Dir::new("restart");
    let id = {
        let (mut app, db) = App::open(&dir, fast(5));
        let id = app.add(spec(server.url("f.bin"), &dir.files(), "f.bin"));
        app.until(&id, "has a checkpoint", |_, id| {
            db.checkpoint(id).unwrap().is_some()
        });
        id
        // The app closes: the manager and its engine are dropped.
    };
    let (mut app, db) = App::open(&dir, fast(5));
    let checkpoint = db.checkpoint(&id).unwrap();
    assert!(checkpoint.is_some(), "kept across the restart");
    let r = app.manager.get(&id).expect("it came back");
    assert!(r.progress.completed_bytes > 0);
    app.manager.pump(App::now());
    app.until_status(&id, DownloadStatus::Completed);
    assert_eq!(std::fs::read(dir.files().join("f.bin")).unwrap(), data());
    assert!(!part(&dir.files(), "f.bin").exists());
}

/// Categories and destinations on the real engine (#31): the manager places a download, the
/// engine names and saves it, and the record and the files show both.
mod categories {
    use super::*;
    use velocity_core::destination::{Choice, FolderSettings, OnConflict, Placement};
    use velocity_core::manager::AddOptions;
    use velocity_test_http::FileHeaders;

    /// The app, with `dir.files()` as its default folder.
    fn open(dir: &Dir) -> (App, Arc<Database>) {
        let (app, db) = App::open(dir, fast(5));
        db.save_section(
            &FolderSettings {
                default_directory: dir.files().to_string_lossy().into_owned(),
                ..FolderSettings::default()
            },
            App::now(),
        )
        .unwrap();
        (app, db)
    }

    /// Adds `url` where the manager placed it.
    fn add_placed(app: &mut App, url: String, placement: Placement) -> String {
        let Placement::Placed(place) = placement else {
            panic!("not placed: {placement:?}")
        };
        let mut s = spec(url, &place.directory, "unused");
        s.file_name = place.file_name;
        let options = AddOptions {
            category_id: place.category_id,
            ..AddOptions::default()
        };
        app.manager
            .add_with(DownloadKind::Http, s, options, App::now())
            .expect("added")
    }

    #[test]
    fn a_name_the_server_gives_sets_the_category_and_the_file_stays_in_its_folder() {
        // PO decision on #31; the name is the server's `Content-Disposition` (V1 #46).
        let server = TestServer::start(
            vec![("get".into(), data())],
            ServerOptions {
                file_headers: FileHeaders {
                    content_disposition: Some("attachment; filename=\"report.pdf\"".to_owned()),
                    ..FileHeaders::default()
                },
                ..ServerOptions::default()
            },
        );
        let dir = Dir::new("server-named");
        let (mut app, db) = open(&dir);
        let url = server.url("get");
        let placement = app
            .manager
            .place(&url, &Choice::default(), OnConflict::Ask)
            .unwrap();
        // No extension in the link: the server names it, and the default folder is used.
        let Placement::Placed(place) = &placement else {
            panic!("placed")
        };
        assert_eq!(
            (&place.directory, &place.file_name, &place.category_id),
            (&dir.files(), &None, &None)
        );
        let id = add_placed(&mut app, url, placement);
        app.until_status(&id, DownloadStatus::Completed);
        let r = app.manager.get(&id).unwrap();
        assert_eq!(r.file_name.as_deref(), Some("report.pdf"));
        assert_eq!(r.category_id.as_deref(), Some("documents"));
        assert_eq!(PathBuf::from(&r.directory), dir.files(), "not moved");
        assert_eq!(
            std::fs::read(dir.files().join("report.pdf")).unwrap(),
            data()
        );
        assert!(!dir.files().join("Documents").exists());
        let stored = db.downloads().unwrap().remove(0);
        assert_eq!(stored.category_id.as_deref(), Some("documents"));
    }

    #[test]
    fn an_automatic_category_chooses_its_folder_and_a_v1_download_there_is_never_written_over() {
        let server = TestServer::start(
            vec![("ubuntu.iso".into(), data())],
            ServerOptions::default(),
        );
        let dir = Dir::new("category-folder");
        let (mut app, _db) = open(&dir);
        let url = server.url("ubuntu.iso");
        let images = dir.files().join("Disk images");

        // V1 (aria2) is downloading the same file into the same folder.
        std::fs::create_dir(&images).unwrap();
        std::fs::write(images.join("ubuntu.iso"), b"V1's partial file").unwrap();
        std::fs::write(images.join("ubuntu.iso.aria2"), b"V1's control file").unwrap();

        let Placement::Conflict(check) = app
            .manager
            .place(&url, &Choice::default(), OnConflict::Ask)
            .unwrap()
        else {
            panic!("asked")
        };
        assert!(check.file_exists && check.partial_download_exists);
        assert_eq!(check.category_id.as_deref(), Some("images"));

        let placement = app
            .manager
            .place(&url, &Choice::default(), OnConflict::Rename)
            .unwrap();
        let id = add_placed(&mut app, url, placement);
        app.until_status(&id, DownloadStatus::Completed);
        let r = app.manager.get(&id).unwrap();
        assert_eq!(r.category_id.as_deref(), Some("images"));
        assert_eq!(r.file_name.as_deref(), Some("ubuntu (1).iso"));
        assert_eq!(
            std::fs::read(images.join("ubuntu (1).iso")).unwrap(),
            data()
        );
        assert_eq!(
            std::fs::read(images.join("ubuntu.iso")).unwrap(),
            b"V1's partial file"
        );
        assert_eq!(
            std::fs::read(images.join("ubuntu.iso.aria2")).unwrap(),
            b"V1's control file"
        );
    }

    #[test]
    fn a_missing_category_folder_is_created_when_placed() {
        let server = TestServer::start(vec![("song.mp3".into(), data())], ServerOptions::default());
        let dir = Dir::new("created");
        let (mut app, _db) = open(&dir);
        let url = server.url("song.mp3");
        let placement = app
            .manager
            .place(&url, &Choice::default(), OnConflict::Ask)
            .unwrap();
        let id = add_placed(&mut app, url, placement);
        app.until_status(&id, DownloadStatus::Completed);
        assert_eq!(
            std::fs::read(dir.files().join("Music").join("song.mp3")).unwrap(),
            data()
        );
    }
}
