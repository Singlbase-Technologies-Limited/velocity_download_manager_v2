//! The download manager with a real torrent helper (#58, L2): a supervised `velocity-torrent
//! serve` child, a database file, and a libtorrent seeder on loopback. A torrent downloads through
//! the manager and keeps its metadata and its resume data without trackers; it comes back after
//! the app starts again and after the helper crashes; and removing it with its files deletes only
//! its own.

mod common;

use std::sync::Arc;

use common::*;
use velocity_core::database::{Database, TorrentOrigin};
use velocity_core::domain::{DownloadKind, DownloadStatus, EngineHealth};
use velocity_core::manager::{AddOptions, Manager, TorrentRequest};
use velocity_core::torrent::parse_metadata;
use velocity_core::transfer::MissingEngine;

const NAME: &str = "v1-multi";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The app: a manager over `db` with a real helper that finds `peers` through a tracker in its
/// settings (trackers given with a torrent aren't kept across a restart, #70). Returns the helper's
/// process id too.
fn app(db: &Arc<Database>, tracker: &str) -> (Manager, u32) {
    let helper = SupervisedTorrentEngine::start(
        program(),
        SupervisorOptions {
            first_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(40),
            ..SupervisorOptions::default()
        },
        TorrentSettings {
            extra_trackers: vec![tracker.to_owned()],
            ..OFF
        },
    )
    .expect("the helper starts");
    let pid = helper.process_id().expect("a helper runs");
    let direct = MissingEngine::new(ErrorCode::EngineMissing, PathBuf::from("velocity-transfer"));
    let manager = Manager::open(Box::new(direct), Arc::clone(db))
        .expect("the manager opens")
        .with_torrents(Box::new(helper));
    (manager, pid)
}

fn request(dir: &Path, seeding: SeedPolicy) -> TorrentRequest {
    TorrentRequest {
        origin: TorrentOrigin::File {
            path: corpus().join(format!("{NAME}.torrent")),
        },
        directory: dir.to_path_buf(),
        trackers: Vec::new(),
        choose_files: false,
        files: None,
        seeding,
        upload_limit: 0,
        download_limit: 0,
    }
}

/// Pumps the manager until `done` holds for `id`.
fn until(m: &mut Manager, id: &str, what: &str, done: impl Fn(&Manager) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        m.pump(now());
        if done(m) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{id} never {what}: {:?}",
            m.get(id)
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn status_is(id: &str, status: DownloadStatus) -> impl Fn(&Manager) -> bool + '_ {
    move |m| m.get(id).is_some_and(|r| r.status == status)
}

fn every_file_is_right(dir: &Path) {
    for (path, _) in files_of(NAME) {
        assert_eq!(digest_on_disk(dir, &path), content_digest(NAME, &path), "{path}");
    }
}

#[test]
fn a_torrent_downloads_through_the_manager_and_keeps_its_metadata_and_resume_data() {
    let seed_dir = Dir::new("manager-seed");
    let dir = Dir::new("manager-download");
    let data = Dir::new("manager-db");
    let seeder = seed(NAME, &seed_dir.0);
    let tracker = tracker(seeder.port);
    let db = Arc::new(Database::open(&data.0.join("velocity.db")).unwrap());
    let (mut m, _pid) = app(&db, &tracker);
    let id = m
        .add_torrent(request(&dir.0, SeedPolicy::Never {}), AddOptions::default(), now())
        .unwrap();
    until(&mut m, &id, "completed", status_is(&id, DownloadStatus::Completed));
    every_file_is_right(&dir.0);
    let r = m.get(&id).unwrap();
    assert_eq!(r.kind, DownloadKind::Torrent);
    assert_eq!(r.file_name.as_deref(), Some(NAME));

    let stored = db.torrent(&id).unwrap().unwrap();
    let metadata = stored.metadata.expect("the metadata is kept");
    let meta = parse_metadata(&metadata).unwrap();
    assert_eq!(meta.name, NAME);
    let row = db.downloads().unwrap().remove(0);
    assert_eq!(row.info_hash, meta.info_hash, "its hash is kept");
    m.prepare_to_close(Duration::from_secs(10), now());
    let resume = db
        .torrent(&id)
        .unwrap()
        .unwrap()
        .resume_data
        .expect("its resume data is kept");
    let text = String::from_utf8_lossy(&resume);
    assert!(!text.contains("announce"), "no tracker in the stored resume data");
}

#[test]
fn a_seeding_torrent_comes_back_from_its_resume_data_after_the_app_starts_again() {
    let seed_dir = Dir::new("manager-restart-seed");
    let dir = Dir::new("manager-restart");
    let data = Dir::new("manager-restart-db");
    let seeder = seed(NAME, &seed_dir.0);
    let tracker = tracker(seeder.port);
    let db = Arc::new(Database::open(&data.0.join("velocity.db")).unwrap());
    let id = {
        let (mut m, _pid) = app(&db, &tracker);
        let id = m
            .add_torrent(
                request(&dir.0, SeedPolicy::Unlimited {}),
                AddOptions::default(),
                now(),
            )
            .unwrap();
        until(&mut m, &id, "seeding", status_is(&id, DownloadStatus::Seeding));
        m.prepare_to_close(Duration::from_secs(10), now());
        assert!(db.torrent(&id).unwrap().unwrap().resume_data.is_some());
        id
        // The app closes: the manager and its helper are dropped.
    };
    let (mut m, _pid) = app(&db, &tracker);
    assert!(m.get(&id).is_some(), "it came back");
    until(
        &mut m,
        &id,
        "seeding again from its resume data",
        status_is(&id, DownloadStatus::Seeding),
    );
    every_file_is_right(&dir.0);
}

#[test]
fn a_torrent_comes_back_on_its_own_when_the_helper_crashes() {
    let seed_dir = Dir::new("manager-crash-seed");
    let dir = Dir::new("manager-crash");
    let data = Dir::new("manager-crash-db");
    let seeder = seed(NAME, &seed_dir.0);
    let tracker = tracker(seeder.port);
    let db = Arc::new(Database::open(&data.0.join("velocity.db")).unwrap());
    let (mut m, pid) = app(&db, &tracker);
    let id = m
        .add_torrent(
            request(&dir.0, SeedPolicy::Unlimited {}),
            AddOptions::default(),
            now(),
        )
        .unwrap();
    until(&mut m, &id, "seeding", status_is(&id, DownloadStatus::Seeding));
    velocity_test_http::kill_process(pid).unwrap();
    until(&mut m, &id, "noticed the crash", |m| {
        m.torrent_helper_health() != EngineHealth::Running
            || m.get(&id).is_some_and(|r| r.error.is_some())
    });
    until(&mut m, &id, "seeding again", |m| {
        m.get(&id)
            .is_some_and(|r| r.status == DownloadStatus::Seeding && r.error.is_none())
    });
    assert_ne!(
        m.get(&id).unwrap().status,
        DownloadStatus::Failed,
        "it never failed"
    );
}

#[test]
fn removing_a_torrent_with_its_files_deletes_only_its_own() {
    let seed_dir = Dir::new("manager-remove-seed");
    let dir = Dir::new("manager-remove");
    let data = Dir::new("manager-remove-db");
    let seeder = seed(NAME, &seed_dir.0);
    let tracker = tracker(seeder.port);
    let db = Arc::new(Database::open(&data.0.join("velocity.db")).unwrap());
    let (mut m, _pid) = app(&db, &tracker);
    let id = m
        .add_torrent(request(&dir.0, SeedPolicy::Never {}), AddOptions::default(), now())
        .unwrap();
    until(&mut m, &id, "completed", status_is(&id, DownloadStatus::Completed));
    std::fs::write(dir.0.join("not-the-torrents.txt"), b"keep me").unwrap();
    assert_eq!(m.remove(&id, true), Ok(Vec::new()));
    assert_eq!(files_under(&dir.0), vec!["not-the-torrents.txt".to_owned()]);
    assert!(m.get(&id).is_none());
}
