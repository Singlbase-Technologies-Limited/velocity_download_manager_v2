//! The torrent helper as a child process (#62, ADR-0014), against real peers on loopback: a
//! seeding libtorrent session in this test, a tracker on 127.0.0.1 that names it, and the helper
//! program under its supervisor. DHT, peer exchange and local peer discovery are off, so nothing
//! leaves the machine.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;

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
        "a completed torrent seeds again only through setSeeding"
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

/// File choice (#62): a magnet fetches its metadata, waits for the choice without writing
/// anything, downloads only the chosen files, and downloads a file chosen after it completed.
#[test]
fn a_magnet_waits_for_the_file_choice_and_downloads_only_the_chosen_files() {
    let name = "v2-multi";
    let seed_dir = Dir::new("choose-seed");
    let get_dir = Dir::new("choose-get");
    let seeder = seed(name, &seed_dir.0);
    let mut engine = helper();
    let events = engine.subscribe();
    let uri = manifest()["magnets"][name].as_str().unwrap().to_owned();
    let id = engine
        .add(TorrentSpec {
            choose_files: true,
            ..spec(
                TorrentSource::Magnet { uri },
                &get_dir.0,
                vec![tracker(seeder.port)],
            )
        })
        .unwrap();
    let choosing = wait_status(&events, DownloadStatus::ChoosingFiles, 60);
    assert_eq!(choosing.progress.total_bytes, None);
    assert_eq!(
        files_under(&get_dir.0),
        Vec::<String>::new(),
        "nothing is written while choosing"
    );

    // The metadata, in chunks, is the torrent's: the app checks and keeps it.
    let meta = parse_metadata(&fetched_metadata(&engine, id)).unwrap();
    let want = &manifest()["torrents"][name];
    assert_eq!(meta.info_hash_v2.as_deref(), want["v2"].as_str());

    let a = index_of(&meta, "v2-multi/a.bin");
    let b = index_of(&meta, "v2-multi/sub/b.bin");
    engine.set_files(id, vec![a, b]).unwrap();
    let done = wait_status(&events, DownloadStatus::Completed, 120);
    assert_eq!(done.progress.completed_bytes, 150_000 + 70_000);
    assert_eq!(done.progress.total_bytes, Some(150_000 + 70_000));
    assert_eq!(
        files_under(&get_dir.0),
        ["v2-multi/a.bin", "v2-multi/sub/b.bin"],
        "only the chosen files are on disk"
    );
    for path in ["v2-multi/a.bin", "v2-multi/sub/b.bin"] {
        assert_eq!(
            digest_on_disk(&get_dir.0, path),
            content_digest(name, path),
            "{path}"
        );
    }

    // A file chosen after it completed: it downloads again and completes again.
    let c = index_of(&meta, "v2-multi/sub/deeper/c.bin");
    engine.set_files(id, vec![a, b, c]).unwrap();
    let again = wait_status(&events, DownloadStatus::Completed, 120);
    assert_eq!(again.progress.completed_bytes, 150_000 + 70_000 + 5_000);
    assert_eq!(
        digest_on_disk(&get_dir.0, "v2-multi/sub/deeper/c.bin"),
        content_digest(name, "v2-multi/sub/deeper/c.bin")
    );
    drop(seeder);
}

#[test]
fn a_torrent_file_downloads_only_its_first_choice() {
    let name = "v1-multi";
    let seed_dir = Dir::new("first-choice-seed");
    let get_dir = Dir::new("first-choice-get");
    let seeder = seed(name, &seed_dir.0);
    let meta = parse_torrent(&std::fs::read(corpus().join("v1-multi.torrent")).unwrap()).unwrap();
    let b = index_of(&meta, "v1-multi/sub/b.bin");
    let mut engine = helper();
    let events = engine.subscribe();
    engine
        .add(TorrentSpec {
            files: Some(vec![b]),
            ..spec(file(name), &get_dir.0, vec![tracker(seeder.port)])
        })
        .unwrap();
    let done = wait_status(&events, DownloadStatus::Completed, 120);
    assert_eq!(
        done.progress.completed_bytes, 70_000,
        "only the chosen file's bytes count"
    );
    // A v1 piece spans files: libtorrent keeps the parts of the files nobody chose in a part file
    // of its own, named by the info hash, never in those files.
    let parts = format!(".{}.parts", meta.info_hash.as_deref().unwrap());
    assert_eq!(
        files_under(&get_dir.0),
        [parts.as_str(), "v1-multi/sub/b.bin"]
    );
    assert_eq!(
        digest_on_disk(&get_dir.0, "v1-multi/sub/b.bin"),
        content_digest(name, "v1-multi/sub/b.bin")
    );
    drop(seeder);
}

/// A magnet's metadata comes from peers: unsafe names in it fail the torrent before any byte is
/// written, where libtorrent alone would rename them and download.
#[test]
fn a_magnet_whose_metadata_is_unsafe_fails_before_writing_anything() {
    let seed_dir = Dir::new("unsafe-seed");
    let get_dir = Dir::new("unsafe-get");
    let seeder = metadata_seeder("v2-path-traversal", &seed_dir.0);
    let v2 = manifest()["invalid"]["v2-path-traversal.torrent"]["libtorrent"]["v2"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut engine = helper();
    let events = engine.subscribe();
    let id = engine
        .add(spec(
            TorrentSource::Magnet {
                uri: format!("magnet:?xt=urn:btmh:1220{v2}"),
            },
            &get_dir.0,
            vec![tracker(seeder.port)],
        ))
        .unwrap();
    let failed = wait_status(&events, DownloadStatus::Failed, 60);
    let error = failed.error.unwrap();
    assert!(
        error.message.contains("unsafe file name"),
        "{}",
        error.message
    );
    assert!(!error.retryable);
    assert_eq!(files_under(&get_dir.0), Vec::<String>::new());
    assert_eq!(engine.set_files(id, vec![1]), Err(ErrorCode::InvalidState));
    drop(seeder);
}

#[test]
fn file_choices_are_checked() {
    let dir = Dir::new("choices");
    let mut engine = helper();
    let events = engine.subscribe();
    // A torrent file waits in Choosing files at once.
    let id = engine
        .add(TorrentSpec {
            choose_files: true,
            ..spec(file("v2-multi"), &dir.0, vec![])
        })
        .unwrap();
    wait_status(&events, DownloadStatus::ChoosingFiles, 30);
    let meta = parse_torrent(&std::fs::read(corpus().join("v2-multi.torrent")).unwrap()).unwrap();
    let pad = meta.files.iter().find(|f| f.pad).unwrap().index;
    for (bad, why) in [
        (vec![], "nothing"),
        (vec![pad], "a padding file"),
        (vec![99], "an index the torrent hasn't"),
        (vec![1, 1], "a duplicate"),
    ] {
        assert_eq!(
            engine.set_files(id, bad),
            Err(ErrorCode::InvalidInput),
            "{why}"
        );
    }
    assert_eq!(files_under(&dir.0), Vec::<String>::new());
    // Its metadata is known from the start.
    assert_eq!(
        parse_metadata(&fetched_metadata(&engine, id))
            .unwrap()
            .info_hash_v2,
        meta.info_hash_v2
    );
    // A choice and "choose later" together, or a choice for a magnet: refused.
    let both = TorrentSpec {
        choose_files: true,
        files: Some(vec![1]),
        ..spec(file("v1-single"), &dir.0, vec![])
    };
    assert_eq!(engine.add(both), Err(ErrorCode::InvalidInput));
    let uri = manifest()["magnets"]["v1-multi"]
        .as_str()
        .unwrap()
        .to_owned();
    let magnet_with_files = TorrentSpec {
        files: Some(vec![1]),
        ..spec(TorrentSource::Magnet { uri: uri.clone() }, &dir.0, vec![])
    };
    assert_eq!(engine.add(magnet_with_files), Err(ErrorCode::InvalidInput));
    // A magnet without peers: no metadata, so nothing can be chosen yet.
    let waiting = engine
        .add(spec(TorrentSource::Magnet { uri }, &dir.0, vec![]))
        .unwrap();
    assert_eq!(
        engine.set_files(waiting, vec![1]),
        Err(ErrorCode::InvalidState)
    );
    assert_eq!(
        engine.metadata(waiting, 0).unwrap_err(),
        ErrorCode::InvalidState
    );
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
    // A new helper: running, with a process of its own. Just after the kill, the old one is
    // already retired (no process) while the health may still say Running.
    while engine.health() != EngineHealth::Running
        || engine.process_id().is_none_or(|now| now == pid)
    {
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
