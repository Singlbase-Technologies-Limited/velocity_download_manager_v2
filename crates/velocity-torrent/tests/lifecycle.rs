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
