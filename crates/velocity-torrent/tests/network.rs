//! How the torrent helper reaches peers (#62's tracker and port-mapping part): limits, trackers
//! and re-announce, peers, extra trackers, and the listening port. Peers are libtorrent sessions
//! on loopback found through local trackers; DHT, peer exchange, local peer discovery and port
//! mapping are off, so nothing leaves the machine.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::*;
use velocity_core::torrent_engine::{PortMapState, TrackerState, MAX_RATE_LIMIT};

fn helper_with(settings: TorrentSettings) -> (SupervisedTorrentEngine, Receiver<TorrentEvent>) {
    let mut engine = helper();
    let events = engine.subscribe();
    engine.set_settings(settings).unwrap();
    (engine, events)
}

fn completed(s: &TorrentSnapshot) -> bool {
    s.status == DownloadStatus::Completed
}

/// A port nothing listens on now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// V1's `download_limit_and_global_upload_limit_reach_the_engine`, first half: a torrent's own
/// download limit holds it back. 257,768 bytes at 32 KiB/s take about 8 s.
#[test]
fn a_torrents_download_limit_reaches_libtorrent() {
    let name = "v1-multi";
    let seed_dir = Dir::new("limit-down-seed");
    let dir = Dir::new("limit-down");
    let seeder = seed(name, &seed_dir.0);
    let (mut engine, events) = helper_with(OFF);
    let started = Instant::now();
    let id = engine
        .add(TorrentSpec {
            download_limit: 32 * 1024,
            ..spec(file(name), &dir.0, vec![tracker(seeder.port)])
        })
        .unwrap();
    wait_snapshot(&events, id, "completed", 120, completed);
    // Generous for the rate limiter's bursts, as V1's was.
    assert!(
        started.elapsed() > Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        engine.set_limits(id, MAX_RATE_LIMIT + 1, 0),
        Err(ErrorCode::InvalidInput)
    );
    assert_eq!(
        engine.set_limits(TorrentId(999), 0, 0),
        Err(ErrorCode::NotFound)
    );
    engine.set_limits(id, 0, 0).unwrap();
}

/// How long a leecher on 127.0.0.1 takes to get `name` from the helper, connecting to the
/// helper's port itself: with several peers on one address, a tracker's list can't name the
/// helper reliably, since libtorrent keeps one entry per address.
fn leech_time(name: &str, test: &str, engine: &SupervisedTorrentEngine) -> Duration {
    let dir = Dir::new(test);
    let port = engine.network().unwrap().listen_port;
    let session = new_session(&SessionConfig {
        listen_interfaces: "127.0.0.1:0".into(),
        user_agent: "velocity-test-leecher".into(),
        ..Default::default()
    })
    .unwrap();
    let id = session
        .add(&AddParams {
            torrent: std::fs::read(corpus().join(format!("{name}.torrent"))).unwrap(),
            save_path: dir.0.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    wait_for("the leecher to listen", 30, || session.listen_port() != 0);
    let started = Instant::now();
    session.connect_peer(id, "127.0.0.1", port).unwrap();
    wait_for("the leecher to finish", 120, || {
        session.status(id).unwrap().is_seeding
    });
    started.elapsed()
}

/// V1's `download_limit_and_global_upload_limit_reach_the_engine`, second half, and its
/// `set_upload_limit`. The global limits reach libtorrent's session settings
/// (`engine::tests::the_session_follows_the_settings`), but libtorrent doesn't apply them to peers
/// on the local network, as with V1, so on loopback they can't be seen. A torrent's own upload
/// limit applies to every peer: set while it seeds, it holds a downloader back.
#[test]
fn a_torrents_upload_limit_holds_back_even_a_local_peer_and_the_global_one_doesnt() {
    let name = "v1-multi";
    let dir = Dir::new("limit-up");
    let tracker = swarm_tracker();
    let (mut engine, events) = helper_with(TorrentSettings {
        upload_limit: 32 * 1024,
        ..OFF
    });
    place_files(name, &dir.0);
    let id = engine
        .add(TorrentSpec {
            seeding: SeedPolicy::Unlimited {},
            ..spec(file(name), &dir.0, vec![tracker.clone()])
        })
        .unwrap();
    wait_snapshot(&events, id, "seeding", 60, |s| {
        s.status == DownloadStatus::Seeding
    });
    let global = leech_time(name, "limit-up-leech-1", &engine);
    assert!(global < Duration::from_secs(4), "{global:?}");

    // 257,768 bytes at 32 KiB/s take about 8 s.
    engine.set_limits(id, 32 * 1024, 0).unwrap();
    let own = leech_time(name, "limit-up-leech-2", &engine);
    assert!(own > Duration::from_secs(4), "{own:?}");
}

/// V1's `tracker_errors_are_shown_and_a_reannounce_recovers`.
#[test]
fn tracker_errors_are_shown_and_a_reannounce_recovers() {
    let name = "v2-multi";
    let seed_dir = Dir::new("tracker-error-seed");
    let dir = Dir::new("tracker-error");
    let seeder = seed(name, &seed_dir.0);
    let failing = Arc::new(AtomicBool::new(true));
    let (url, announces) = switchable_tracker(seeder.port, Arc::clone(&failing));
    let (mut engine, events) = helper_with(OFF);
    let id = engine.add(spec(file(name), &dir.0, vec![url])).unwrap();

    let mut shown = None;
    wait_for("the tracker error to show", 60, || {
        shown = engine
            .trackers(id)
            .unwrap()
            .into_iter()
            .find(|t| t.state == TrackerState::Error);
        shown.is_some()
    });
    let shown = shown.unwrap();
    assert_eq!(shown.message.as_deref(), Some("tracker offline (test)"));
    assert!(shown.fails >= 1);
    // Redacted: the passkey in the path never leaves the helper.
    assert!(!shown.url.contains("s3cretpasskey"), "{}", shown.url);
    assert!(shown.url.ends_with("/…"), "{}", shown.url);
    assert_eq!(
        engine.snapshot().unwrap()[0].progress.completed_bytes,
        0,
        "no peer without the tracker"
    );

    failing.store(false, Ordering::SeqCst);
    let before = announces.load(Ordering::SeqCst);
    engine.reannounce(id).unwrap();
    wait_snapshot(&events, id, "completed", 120, completed);
    assert!(announces.load(Ordering::SeqCst) > before);
    assert!(engine
        .trackers(id)
        .unwrap()
        .iter()
        .any(|t| t.state == TrackerState::Working));
    assert_eq!(engine.reannounce(TorrentId(999)), Err(ErrorCode::NotFound));
}

/// The peers list shows the peer a torrent downloads from.
#[test]
fn a_torrents_peers_are_listed() {
    let name = "v1-multi";
    let seed_dir = Dir::new("peers-seed");
    let dir = Dir::new("peers");
    let seeder = seed(name, &seed_dir.0);
    let (mut engine, _events) = helper_with(OFF);
    // Slow, so the peer stays connected while the test looks.
    let id = engine
        .add(TorrentSpec {
            download_limit: 16 * 1024,
            ..spec(file(name), &dir.0, vec![tracker(seeder.port)])
        })
        .unwrap();
    // A peer is listed once connected, and known as a seed once its piece list (bitfield) came.
    let mut peers = Vec::new();
    wait_for("a seed among the peers", 60, || {
        peers = engine.peers(id).unwrap();
        peers.iter().any(|p| p.seed)
    });
    let peer = &peers[0];
    assert_eq!((peer.ip.as_str(), peer.port), ("127.0.0.1", seeder.port));
    assert!(peer.seed);
    assert_eq!(peer.progress_permille, 1000);
    assert_eq!(engine.peers(TorrentId(999)), Err(ErrorCode::NotFound));
}

/// V1's `extra_trackers`: trackers from the settings reach every torrent added from then on.
#[test]
fn extra_trackers_reach_new_torrents() {
    let name = "v1-single";
    let seed_dir = Dir::new("extra-seed");
    let dir = Dir::new("extra");
    let seeder = seed(name, &seed_dir.0);
    let (mut engine, events) = helper_with(TorrentSettings {
        extra_trackers: vec![tracker(seeder.port)],
        ..OFF
    });
    let id = engine.add(spec(file(name), &dir.0, vec![])).unwrap();
    wait_snapshot(&events, id, "completed", 120, completed);
    assert_eq!(engine.trackers(id).unwrap().len(), 1);
    assert_eq!(
        engine.set_settings(TorrentSettings {
            extra_trackers: vec!["ftp://t.test/a".into()],
            ..OFF
        }),
        Err(ErrorCode::InvalidUrl)
    );
}

/// The helper listens on the port the settings name, and says when it can't.
#[test]
fn the_helper_listens_on_the_port_set_and_reports_one_it_cant_open() {
    let (mut engine, _events) = helper_with(OFF);
    let mut network = engine.network().unwrap();
    wait_for("a random port", 30, || {
        network = engine.network().unwrap();
        network.listen_port != 0
    });
    assert_eq!(network.listen_error, None);
    assert_eq!(network.port_mapping, PortMapState::Off {});

    let port = free_port();
    engine
        .set_settings(TorrentSettings {
            listen_port: port,
            ..OFF
        })
        .unwrap();
    wait_for("the port set", 30, || {
        engine.network().unwrap().listen_port == port
    });

    // Taken by someone else, on every interface. On Linux the helper can't listen there and says
    // so. Windows lets libtorrent's socket, opened for address reuse, share a port with one that
    // didn't ask for exclusive use, as Rust's listener doesn't: the helper then listens and
    // reports no error (docs/TORRENTS.md).
    #[cfg(unix)]
    {
        let taken = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let busy = taken.local_addr().unwrap().port();
        engine
            .set_settings(TorrentSettings {
                listen_port: busy,
                ..OFF
            })
            .unwrap();
        let mut error = None;
        wait_for("the listen error", 30, || {
            error = engine.network().unwrap().listen_error;
            error.is_some()
        });
        let error = error.unwrap();
        assert!(error.contains(&busy.to_string()), "{error}");
        drop(taken);
    }
    assert_eq!(
        engine.set_settings(TorrentSettings {
            listen_port: 80,
            ..OFF
        }),
        Err(ErrorCode::InvalidInput)
    );
}
