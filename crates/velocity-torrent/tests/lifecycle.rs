//! A torrent's life after it completes (#62's lifecycle part), through the torrent helper:
//! seeding under a policy and through the seeding queue. Peers are libtorrent sessions on loopback
//! found through a local tracker; DHT, peer exchange and local peer discovery are off.

mod common;

use common::*;

/// A helper with `max_active_seeds`, and its events.
fn helper_with(max_active_seeds: u32) -> (SupervisedTorrentEngine, Receiver<TorrentEvent>) {
    let mut engine = helper();
    let events = engine.subscribe();
    engine
        .set_settings(TorrentSettings {
            max_active_seeds,
            ..OFF
        })
        .unwrap();
    (engine, events)
}

/// Adds the corpus torrent `name` with its files already in `dir`: libtorrent checks them, and it
/// completes without a peer.
fn add_complete(
    engine: &mut SupervisedTorrentEngine,
    name: &str,
    dir: &Path,
    tracker: &str,
    seeding: SeedPolicy,
) -> TorrentId {
    place_files(name, dir);
    engine
        .add(TorrentSpec {
            seeding,
            ..spec(file(name), dir, vec![tracker.to_owned()])
        })
        .unwrap()
}

fn seeding(s: &TorrentSnapshot) -> bool {
    s.status == DownloadStatus::Seeding
}

fn waiting_to_seed(s: &TorrentSnapshot) -> bool {
    s.status == DownloadStatus::Completed && s.seed_queued
}

fn completed(s: &TorrentSnapshot) -> bool {
    s.status == DownloadStatus::Completed && !s.seed_queued
}

/// V1's `seeding_stops_when_the_share_ratio_is_reached`: a torrent seeds once complete, and stops
/// when it has uploaded as many bytes as it wanted.
#[test]
fn seeding_stops_when_the_share_ratio_is_reached() {
    let name = "v1-multi";
    let dir = Dir::new("ratio");
    let leech_dir = Dir::new("ratio-leech");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(0);
    let id = add_complete(
        &mut engine,
        name,
        &dir.0,
        &tracker,
        SeedPolicy::Ratio { ratio: 1.0 },
    );
    let total: u64 = files_of(name).iter().map(|(_, d)| d.len() as u64).sum();

    let started = wait_snapshot(&events, id, "seeding", 60, seeding);
    assert_eq!(started.progress.total_bytes, Some(total));
    assert!(!started.seed_queued);

    let leecher = leech(name, &leech_dir.0, &tracker);
    let stopped = wait_snapshot(&events, id, "completed", 120, completed);
    assert!(
        stopped.uploaded_bytes >= total,
        "{} of {total} bytes",
        stopped.uploaded_bytes
    );
    assert!(leecher.session.status(leecher.id).unwrap().is_seeding);
    // Stopped: it uploads nothing more.
    assert_eq!(stopped.progress.upload_speed_bps, 0);
    assert_eq!(engine.resume(id), Err(ErrorCode::InvalidState));
}

/// V1's `the_seeding_queue_limits_active_seeders_and_starts_the_next`.
#[test]
fn the_seeding_queue_limits_active_seeders_and_starts_the_next() {
    let dir = Dir::new("queue");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(1);
    let first = add_complete(
        &mut engine,
        "v1-single",
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    wait_snapshot(&events, first, "seeding", 60, seeding);
    let second = add_complete(
        &mut engine,
        "v1-multi",
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    let waiting = wait_snapshot(&events, second, "waiting to seed", 60, waiting_to_seed);
    assert_eq!(waiting.progress.upload_speed_bps, 0);

    // Stopping the first frees its slot for the second.
    engine.set_seeding(first, SeedPolicy::Never {}).unwrap();
    wait_snapshot(&events, first, "completed", 30, completed);
    wait_snapshot(&events, second, "seeding", 30, seeding);
}

/// V1's `lowering_the_seeding_limit_queues_the_newest_seeders`.
#[test]
fn lowering_the_seeding_limit_queues_the_newest_seeders() {
    let dir = Dir::new("lower");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(0);
    let oldest = add_complete(
        &mut engine,
        "v1-single",
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    wait_snapshot(&events, oldest, "seeding", 60, seeding);
    let newest = add_complete(
        &mut engine,
        "v1-multi",
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    wait_snapshot(&events, newest, "seeding", 60, seeding);

    engine
        .set_settings(TorrentSettings {
            max_active_seeds: 1,
            ..OFF
        })
        .unwrap();
    wait_snapshot(&events, newest, "back in the queue", 30, waiting_to_seed);
    let all = engine.snapshot().unwrap();
    let state = |id| all.iter().find(|s| s.id == id).unwrap().clone();
    assert!(seeding(&state(oldest)), "{:?}", state(oldest));
}

/// V1's `unlimited_seeding_can_be_paused_resumed_and_stopped`.
#[test]
fn unlimited_seeding_can_be_paused_resumed_and_stopped() {
    let dir = Dir::new("unlimited");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(0);
    let id = add_complete(
        &mut engine,
        "v1-single",
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    wait_snapshot(&events, id, "seeding", 60, seeding);
    engine.pause(id).unwrap();
    wait_snapshot(&events, id, "paused", 30, |s| {
        s.status == DownloadStatus::Paused
    });
    engine.resume(id).unwrap();
    wait_snapshot(&events, id, "seeding again", 30, seeding);
    engine.set_seeding(id, SeedPolicy::Never {}).unwrap();
    wait_snapshot(&events, id, "completed", 30, completed);
    assert_eq!(engine.resume(id), Err(ErrorCode::InvalidState));
    // A new policy makes a completed torrent seed again.
    engine.set_seeding(id, SeedPolicy::Unlimited {}).unwrap();
    wait_snapshot(&events, id, "seeding after a new policy", 30, seeding);
}

#[test]
fn policies_and_limits_outside_v1s_ranges_are_refused() {
    let dir = Dir::new("refused-policy");
    let (mut engine, _events) = helper_with(0);
    assert_eq!(
        engine.add(TorrentSpec {
            seeding: SeedPolicy::Time { minutes: 0 },
            ..spec(file("v1-single"), &dir.0, vec![])
        }),
        Err(ErrorCode::InvalidInput)
    );
    let id = engine
        .add(TorrentSpec {
            paused: true,
            ..spec(file("v1-single"), &dir.0, vec![])
        })
        .unwrap();
    assert_eq!(
        engine.set_seeding(id, SeedPolicy::Ratio { ratio: 0.0 }),
        Err(ErrorCode::InvalidInput)
    );
    assert_eq!(
        engine.set_seeding(TorrentId(999), SeedPolicy::Never {}),
        Err(ErrorCode::NotFound)
    );
    assert_eq!(
        engine.set_settings(TorrentSettings {
            max_active_seeds: 1001,
            ..OFF
        }),
        Err(ErrorCode::InvalidInput)
    );
}

/// Sends `data` to the helper in chunks and adds the torrent from it.
fn restore(
    engine: &mut SupervisedTorrentEngine,
    data: &[u8],
    spec: TorrentSpec,
) -> Result<TorrentId, ErrorCode> {
    let mut offset = 0;
    while offset < data.len() {
        let chunk = Chunk::of(data, offset as u64, CHUNK_BYTES);
        offset += chunk.bytes().unwrap().len();
        engine.upload(chunk)?;
    }
    engine.add(TorrentSpec {
        source: TorrentSource::Resume {},
        ..spec
    })
}

/// The resume data of torrent `id`, asked for now.
fn saved(engine: &SupervisedTorrentEngine, id: TorrentId) -> Vec<u8> {
    let mut all = engine.collect_resume_data(Duration::from_secs(20)).unwrap();
    all.remove(&id)
        .unwrap_or_else(|| panic!("no resume data for {id:?}"))
}

fn resumed(dir: &Path) -> TorrentSpec {
    spec(TorrentSource::Resume {}, dir, vec![])
}

/// V1's `restart_recovers_interrupted_torrent_without_duplicates`: a new helper continues a torrent
/// from its resume data without downloading it again (there is no peer), with the file choice that
/// only the resume data holds, and only once.
#[test]
fn a_new_helper_continues_from_resume_data_without_downloading_again() {
    let name = "v1-multi";
    let dir = Dir::new("restart");
    place_files(name, &dir.0);
    let meta =
        parse_torrent(&std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap()).unwrap();
    let first = meta.user_files().next().unwrap().clone();
    let data = {
        let (mut engine, events) = helper_with(0);
        let id = engine
            .add(TorrentSpec {
                files: Some(vec![first.index]),
                ..spec(file(name), &dir.0, vec![])
            })
            .unwrap();
        wait_snapshot(&events, id, "completed", 60, completed);
        saved(&engine, id)
    };

    // No tracker, no peer: only the resume data can complete it.
    let (mut engine, events) = helper_with(0);
    let id = restore(&mut engine, &data, resumed(&dir.0)).unwrap();
    let done = wait_snapshot(&events, id, "completed", 60, completed);
    assert_eq!(
        done.progress.total_bytes,
        Some(first.length),
        "its choice is kept"
    );
    assert_eq!(done.progress.completed_bytes, first.length);
    assert_eq!(done.name.as_deref(), Some(name));
    // Once only.
    assert_eq!(
        restore(&mut engine, &data, resumed(&dir.0)),
        Err(ErrorCode::FileExists)
    );
    assert_eq!(engine.snapshot().unwrap().len(), 1);
}

/// V1's `a_paused_torrent_stays_paused_after_restart`.
#[test]
fn a_paused_torrent_stays_paused_after_restart() {
    let dir = Dir::new("restart-paused");
    let paused = |source| TorrentSpec {
        paused: true,
        ..spec(source, &dir.0, vec![])
    };
    let data = {
        let (mut engine, events) = helper_with(0);
        let id = engine.add(paused(file("v1-single"))).unwrap();
        wait_snapshot(&events, id, "paused", 30, |s| {
            s.status == DownloadStatus::Paused
        });
        saved(&engine, id)
    };
    let (mut engine, events) = helper_with(0);
    let id = restore(&mut engine, &data, paused(TorrentSource::Resume {})).unwrap();
    wait_snapshot(&events, id, "paused", 30, |s| {
        s.status == DownloadStatus::Paused
    });
    thread::sleep(Duration::from_secs(1));
    let now = engine.snapshot().unwrap();
    assert_eq!(now[0].status, DownloadStatus::Paused, "{now:?}");
}

/// V1's `seeding_resumes_after_restart_without_redownloading`: the seeding totals come back with
/// the resume data, so a policy already met doesn't seed again.
#[test]
fn seeding_totals_survive_a_restart() {
    let name = "v1-multi";
    let dir = Dir::new("restart-seed");
    let leech_dir = Dir::new("restart-seed-leech");
    let tracker = swarm_tracker();
    let ratio = SeedPolicy::Ratio { ratio: 1.0 };
    let total: u64 = files_of(name).iter().map(|(_, d)| d.len() as u64).sum();
    let data = {
        let (mut engine, events) = helper_with(0);
        let id = add_complete(&mut engine, name, &dir.0, &tracker, ratio);
        wait_snapshot(&events, id, "seeding", 60, seeding);
        let _leecher = leech(name, &leech_dir.0, &tracker);
        wait_snapshot(&events, id, "completed", 120, completed);
        saved(&engine, id)
    };
    let (mut engine, events) = helper_with(0);
    let id = restore(
        &mut engine,
        &data,
        TorrentSpec {
            seeding: ratio,
            ..resumed(&dir.0)
        },
    )
    .unwrap();
    let done = wait_snapshot(&events, id, "completed", 60, completed);
    assert!(
        done.uploaded_bytes >= total,
        "{} of {total}",
        done.uploaded_bytes
    );
}

/// V1's `magnet_recovers_after_restart_using_saved_metadata`: a magnet whose metadata came from a
/// peer comes back with it, without the peer.
#[test]
fn a_magnet_comes_back_with_its_metadata_after_a_restart() {
    let name = "v2-single";
    let dir = Dir::new("restart-magnet");
    let seed_dir = Dir::new("restart-magnet-seed");
    let uri = manifest()["magnets"][name].as_str().unwrap().to_owned();
    let (data, metadata) = {
        let seeder = metadata_seeder(name, &seed_dir.0);
        let (mut engine, events) = helper_with(0);
        let id = engine
            .add(TorrentSpec {
                choose_files: true,
                ..spec(
                    TorrentSource::Magnet { uri },
                    &dir.0,
                    vec![tracker(seeder.port)],
                )
            })
            .unwrap();
        wait_snapshot(&events, id, "choosing files", 60, |s| {
            s.status == DownloadStatus::ChoosingFiles
        });
        (saved(&engine, id), fetched_metadata(&engine, id))
    };
    let (mut engine, events) = helper_with(0);
    let id = restore(&mut engine, &data, resumed(&dir.0)).unwrap();
    let back = wait_snapshot(&events, id, "choosing files", 30, |s| {
        s.status == DownloadStatus::ChoosingFiles
    });
    assert_eq!(back.name.as_deref(), Some(name));
    assert_eq!(fetched_metadata(&engine, id), metadata);
    let meta = parse_metadata(&metadata).unwrap();
    let index = meta.user_files().next().unwrap().index;
    engine.set_files(id, vec![index]).unwrap();
    wait_snapshot(&events, id, "downloading", 30, |s| {
        s.status == DownloadStatus::Downloading
    });
}

/// V1's `deleting_an_active_torrent_removes_partial_data_and_control_file`, on V2's terms: the
/// helper never deletes; once it has let go of a torrent, the core deletes the torrent's own files,
/// waiting for libtorrent to close them (Windows doesn't delete an open file), and nothing else.
#[test]
fn an_active_torrents_files_are_deleted_once_the_helper_lets_go() {
    use velocity_core::torrent_files::{delete_owned, owned_paths};
    let name = "v1-multi";
    let dir = Dir::new("delete-active");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(0);
    let id = add_complete(
        &mut engine,
        name,
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    wait_snapshot(&events, id, "seeding", 60, seeding);
    let mine = dir.0.join("mine.txt");
    std::fs::write(&mine, b"not the torrent's").unwrap();
    let meta =
        parse_torrent(&std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap()).unwrap();
    let chosen: Vec<u32> = meta.user_files().map(|f| f.index).collect();

    engine.remove(id).unwrap();
    let done = delete_owned(&owned_paths(&dir.0, &meta, &chosen), Duration::from_secs(5));

    assert!(done.kept.is_empty(), "{done:?}");
    assert_eq!(files_under(&dir.0), ["mine.txt"]);
}

/// An id is never given to two torrents: a helper takes back only ids it doesn't have and won't
/// give itself.
#[test]
fn a_torrent_comes_back_only_under_an_id_no_other_has() {
    let dir = Dir::new("restore-ids");
    let (mut engine, _events) = helper_with(0);
    let paused = |name| TorrentSpec {
        paused: true,
        ..spec(file(name), &dir.0, vec![])
    };
    let id = engine.add(paused("v1-single")).unwrap();
    assert_eq!(
        engine.restore(id, paused("v1-multi")),
        Err(ErrorCode::InvalidInput),
        "an id it has"
    );
    assert_eq!(
        engine.restore(TorrentId(id.0 + 1), paused("v1-multi")),
        Err(ErrorCode::InvalidInput),
        "an id it would give next"
    );
    assert_eq!(
        engine.restore(TorrentId(0), paused("v1-multi")),
        Err(ErrorCode::InvalidInput)
    );
    assert_eq!(engine.snapshot().unwrap().len(), 1);
}

/// V1's `engine_crash_recovers_torrent`: a crashed helper's torrent comes back under its id from
/// its latest resume data, seeding again, without a peer to download from.
#[test]
fn a_crashed_helpers_seeding_torrent_comes_back_from_its_resume_data() {
    let name = "v1-multi";
    let dir = Dir::new("crash-seed");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(0);
    let id = add_complete(
        &mut engine,
        name,
        &dir.0,
        &tracker,
        SeedPolicy::Unlimited {},
    );
    wait_snapshot(&events, id, "seeding", 60, seeding);
    wait_for("its resume data", 30, || engine.resume_data(id).is_some());
    let pid = engine.process_id().unwrap();
    // Its files can't be checked again: what is on disk is no longer the torrent's. Only the
    // resume data can bring it back as complete.
    for (path, data) in files_of(name) {
        std::fs::write(dir.0.join(path), vec![0u8; data.len()]).unwrap();
    }
    velocity_test_http::kill_process(pid).unwrap();

    let lost = wait_snapshot(&events, id, "failed", 30, |s| {
        s.status == DownloadStatus::Failed
    });
    assert!(lost.error.unwrap().retryable);
    let back = wait_snapshot(&events, id, "seeding again", 60, seeding);
    let total: u64 = files_of(name).iter().map(|(_, d)| d.len() as u64).sum();
    assert_eq!(back.progress.completed_bytes, total);
    assert_eq!(back.error, None);
    engine.set_seeding(id, SeedPolicy::Never {}).unwrap();
    wait_snapshot(&events, id, "completed", 30, completed);
}

#[test]
fn resume_data_is_checked_before_libtorrent_reads_it() {
    let dir = Dir::new("restart-refused");
    let data = {
        let (mut engine, events) = helper_with(0);
        let id = engine
            .add(TorrentSpec {
                paused: true,
                ..spec(file("v1-single"), &dir.0, vec![])
            })
            .unwrap();
        wait_snapshot(&events, id, "paused", 30, |s| {
            s.status == DownloadStatus::Paused
        });
        saved(&engine, id)
    };
    let (mut engine, _events) = helper_with(0);
    // Nothing uploaded.
    assert_eq!(engine.add(resumed(&dir.0)), Err(ErrorCode::InvalidInput));
    // A chunk out of order.
    assert_eq!(
        engine.upload(Chunk::of(&data, 1, CHUNK_BYTES)),
        Err(ErrorCode::InvalidInput)
    );
    // Only its first part.
    engine.upload(Chunk::of(&data, 0, 10)).unwrap();
    assert_eq!(engine.add(resumed(&dir.0)), Err(ErrorCode::InvalidInput));
    // Changed on its way: an entry libtorrent never writes.
    let tampered = String::from_utf8_lossy(&data).replacen("6:paused", "6:pausex", 1);
    assert_ne!(tampered.as_bytes(), data.as_slice());
    assert_eq!(
        restore(&mut engine, tampered.as_bytes(), resumed(&dir.0)),
        Err(ErrorCode::InvalidInput)
    );
    // No choice can be waited for: the resume data has it.
    assert_eq!(
        restore(
            &mut engine,
            &data,
            TorrentSpec {
                choose_files: true,
                ..resumed(&dir.0)
            }
        ),
        Err(ErrorCode::InvalidInput)
    );
    assert_eq!(engine.snapshot().unwrap(), [], "nothing was added");
    restore(&mut engine, &data, resumed(&dir.0)).unwrap();
}
