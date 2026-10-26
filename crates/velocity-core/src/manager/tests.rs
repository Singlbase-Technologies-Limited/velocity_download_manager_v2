use super::*;
use crate::database::{Database, Engine, Stored, StoredDownload};
use crate::domain::{Capabilities, FailureClass, TransferError, Verification};
use crate::transfer::fake::{FakeControl, FakeTransfer};
use crate::transfer::{Limits, TransferSnapshot};
use std::path::PathBuf;
use std::sync::Arc;

const T0: u64 = 1_792_600_000_000;

fn spec() -> TransferSpec {
    TransferSpec {
        urls: vec!["https://example.com/files/a.zip".to_owned()],
        directory: PathBuf::from("downloads"),
        file_name: Some("a.zip".to_owned()),
        credentials: None,
        headers: Vec::new(),
        referrer: None,
        user_agent: None,
        checksum: None,
        pieces: None,
        limits: Limits::default(),
        max_connections: 4,
        resume: None,
        require_encryption: false,
        ssh_key: None,
    }
}

/// A manager over `fake`, saving to a database in memory.
fn in_memory(fake: FakeTransfer) -> Manager {
    Manager::new(
        Box::new(fake),
        Arc::new(Database::open_in_memory().unwrap()),
    )
}

fn setup() -> (Manager, FakeControl, String) {
    let fake = FakeTransfer::new();
    let control = fake.control();
    let mut manager = in_memory(fake);
    let id = manager.add(DownloadKind::Http, spec(), T0).unwrap();
    manager.pump(T0);
    (manager, control, id)
}

fn status(m: &Manager, id: &str) -> DownloadStatus {
    m.get(id).unwrap().status
}

#[test]
fn an_added_download_is_queued_with_an_app_id_not_the_engine_id() {
    let (m, control, id) = setup();
    let r = m.get(&id).unwrap();
    assert_eq!(r.status, DownloadStatus::Queued);
    assert_eq!(r.kind, DownloadKind::Http);
    assert_eq!(r.source_url, "https://example.com/files/a.zip");
    assert_eq!(r.file_name.as_deref(), Some("a.zip"));
    assert_eq!(r.directory, "downloads");
    assert_eq!(r.created_at_ms, T0);
    assert_ne!(id, "1");
    assert_eq!(control.log(), ["add"]);
}

#[test]
fn progress_from_the_engine_reaches_the_record() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 1024, Some(4096));
    assert_eq!(m.pump(T0 + 1), 1);
    let r = m.get(&id).unwrap();
    assert_eq!(r.status, DownloadStatus::Downloading);
    assert_eq!(r.progress.completed_bytes, 1024);
    assert_eq!(r.progress.total_bytes, Some(4096));
    assert_eq!(r.progress.connections, 4);
}

#[test]
fn status_changes_only_when_the_engine_reports_them() {
    let (mut m, _control, id) = setup();
    m.pause(&id).unwrap();
    // No fake progress: the record waits for the engine's event.
    assert_eq!(status(&m, &id), DownloadStatus::Queued);
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Paused);
}

#[test]
fn pause_then_resume_keeps_progress() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 2048, Some(4096));
    m.pause(&id).unwrap();
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Paused);
    assert_eq!(m.get(&id).unwrap().progress.completed_bytes, 2048);
    m.resume(&id).unwrap();
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Queued);
    assert_eq!(m.get(&id).unwrap().progress.completed_bytes, 2048);
    assert_eq!(control.log(), ["add", "pause 1", "resume 1"]);
}

#[test]
fn cancel_ends_the_download_as_removed() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 10, Some(100));
    m.cancel(&id).unwrap();
    m.pump(T0);
    let r = m.get(&id).unwrap();
    assert_eq!(r.status, DownloadStatus::Removed);
    assert_eq!(r.completed_at_ms, None);
}

#[test]
fn a_failure_records_the_engine_neutral_error() {
    let (mut m, control, id) = setup();
    let error = TransferError::new(FailureClass::Permanent, Some(404), "Not found");
    control.fail(TransferId(1), error.clone());
    m.pump(T0);
    let r = m.get(&id).unwrap();
    assert_eq!(r.status, DownloadStatus::Failed);
    assert_eq!(r.error.as_ref(), Some(&error));
    assert!(!r.error.as_ref().unwrap().retryable);
}

#[test]
fn completion_sets_the_completion_time_once() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 50, Some(100));
    control.complete(TransferId(1));
    m.pump(T0 + 5);
    let r = m.get(&id).unwrap();
    assert_eq!(r.status, DownloadStatus::Completed);
    assert_eq!(r.progress.completed_bytes, 100);
    assert_eq!(r.completed_at_ms, Some(T0 + 5));
    assert_eq!(r.verification, Some(Verification::SizeVerified));
    control.complete(TransferId(1));
    m.pump(T0 + 9);
    assert_eq!(m.get(&id).unwrap().completed_at_ms, Some(T0 + 5));
}

#[test]
fn commands_that_the_status_forbids_never_reach_the_engine() {
    let (mut m, control, id) = setup();
    assert_eq!(m.resume(&id), Err(ErrorCode::InvalidState));
    control.complete(TransferId(1));
    m.pump(T0);
    assert_eq!(m.pause(&id), Err(ErrorCode::InvalidState));
    assert_eq!(m.cancel(&id), Err(ErrorCode::InvalidState));
    assert_eq!(control.log(), ["add"]);
}

#[test]
fn unknown_download_ids_are_not_found() {
    let (mut m, _control, _id) = setup();
    assert_eq!(m.pause("d-missing"), Err(ErrorCode::NotFound));
    assert_eq!(m.resume("d-missing"), Err(ErrorCode::NotFound));
    assert_eq!(m.cancel("d-missing"), Err(ErrorCode::NotFound));
}

#[test]
fn an_unavailable_engine_fails_the_command_and_changes_nothing() {
    let (mut m, control, id) = setup();
    control.set_unavailable(Some(ErrorCode::EngineUnavailable));
    assert_eq!(m.pause(&id), Err(ErrorCode::EngineUnavailable));
    assert_eq!(
        m.add(DownloadKind::Http, spec(), T0),
        Err(ErrorCode::EngineUnavailable)
    );
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Queued);
}

#[test]
fn a_download_the_server_names_takes_the_name_the_engine_reports() {
    let fake = FakeTransfer::new();
    let control = fake.control();
    let mut m = in_memory(fake);
    let unnamed = TransferSpec {
        urls: vec!["https://example.com/download.php?id=3".to_owned()],
        file_name: None,
        ..spec()
    };
    let id = m.add(DownloadKind::Http, unnamed, T0).unwrap();
    m.pump(T0);
    assert_eq!(m.get(&id).unwrap().file_name, None);
    control.named(TransferId(1), "report (1).pdf");
    m.pump(T0);
    assert_eq!(
        m.get(&id).unwrap().file_name.as_deref(),
        Some("report (1).pdf")
    );
    // A later snapshot that doesn't repeat the name keeps it.
    control.emit(TransferEvent::Changed {
        snapshot: TransferSnapshot {
            file_name: None,
            ..control.snapshot(TransferId(1))
        },
    });
    m.pump(T0);
    assert_eq!(
        m.get(&id).unwrap().file_name.as_deref(),
        Some("report (1).pdf")
    );
}

#[test]
fn events_for_unknown_transfers_are_ignored() {
    let (mut m, control, id) = setup();
    let mut stray = TransferSnapshot {
        id: TransferId(42),
        status: DownloadStatus::Completed,
        progress: m.get(&id).unwrap().progress.clone(),
        wasted_bytes: 0,
        error: None,
        verification: None,
        file_name: None,
        content_type: None,
    };
    stray.progress.completed_bytes = 999;
    control.emit(TransferEvent::Changed { snapshot: stray });
    assert_eq!(m.pump(T0), 0);
    assert_eq!(status(&m, &id), DownloadStatus::Queued);
}

#[test]
fn torrent_statuses_from_a_direct_engine_become_unknown() {
    let (mut m, control, id) = setup();
    for s in [
        DownloadStatus::Seeding,
        DownloadStatus::FetchingMetadata,
        DownloadStatus::ChoosingFiles,
    ] {
        control.status(TransferId(1), s);
        m.pump(T0);
        assert_eq!(status(&m, &id), DownloadStatus::Unknown, "{s:?}");
    }
}

#[test]
fn a_confirmed_host_key_goes_to_the_engine_and_a_malformed_one_never_does() {
    let fake = FakeTransfer::new();
    let control = fake.control();
    let mut m = in_memory(fake);
    assert_eq!(
        m.trust_host_key("files.example.org", 22, "SHA256:abc+/def"),
        Ok(())
    );
    assert!(control
        .log()
        .contains(&"trust_host_key files.example.org:22 SHA256:abc+/def".to_owned()));
    for bad in ["", "abc", "MD5:aa:bb", "SHA256:", "SHA256:a b"] {
        assert_eq!(
            m.trust_host_key("files.example.org", 22, bad),
            Err(ErrorCode::InvalidInput),
            "{bad}"
        );
    }
    assert_eq!(
        m.trust_host_key("", 22, "SHA256:abc"),
        Err(ErrorCode::InvalidInput)
    );
}

#[test]
fn torrent_kinds_and_protocols_the_engine_lacks_are_refused() {
    let fake = FakeTransfer::with_capabilities(Capabilities {
        protocols: Capabilities::sample()
            .protocols
            .into_iter()
            .filter(|p| p.kind == DownloadKind::Http)
            .collect(),
        ..Capabilities::sample()
    });
    let control = fake.control();
    let mut m = in_memory(fake);
    for kind in [
        DownloadKind::Torrent,
        DownloadKind::Magnet,
        DownloadKind::Sftp,
    ] {
        assert_eq!(
            m.add(kind, spec(), T0),
            Err(ErrorCode::Unsupported),
            "{kind:?}"
        );
    }
    let mut no_url = spec();
    no_url.urls.clear();
    assert_eq!(
        m.add(DownloadKind::Http, no_url, T0),
        Err(ErrorCode::InvalidUrl)
    );
    assert!(control.log().is_empty());
}

#[test]
fn a_removed_event_detaches_the_transfer_and_keeps_the_record() {
    let (mut m, control, id) = setup();
    control.complete(TransferId(1));
    control.emit(TransferEvent::Removed { id: TransferId(1) });
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Completed);
    // Later events for the forgotten transfer don't touch the record.
    control.status(TransferId(1), DownloadStatus::Downloading);
    assert_eq!(m.pump(T0), 0);
    assert_eq!(status(&m, &id), DownloadStatus::Completed);
}

#[test]
fn progress_fields_are_copied_exactly() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 7, None);
    m.pump(T0);
    assert_eq!(
        m.get(&id).unwrap().progress,
        Progress {
            completed_bytes: 7,
            total_bytes: None,
            download_speed_bps: 500_000,
            upload_speed_bps: 0,
            connections: 4,
            connection_detail: None,
        }
    );
}

/// The queue (#50), against the fake engine. A download is named by its file, so a test can
/// see which ones the engine was given and in what order.
mod queue {
    use super::*;
    use crate::queue::{QueueSettings, ScheduleSettings};
    use jiff::civil::{date, DateTime};

    fn named(name: &str) -> TransferSpec {
        TransferSpec {
            file_name: Some(name.to_owned()),
            ..spec()
        }
    }

    /// A manager that runs at most `max` downloads at once.
    fn queue_of(max: u32) -> (Manager, FakeControl) {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: max,
            ..QueueSettings::default()
        })
        .unwrap();
        (m, control)
    }

    fn add(m: &mut Manager, name: &str) -> String {
        m.add(DownloadKind::Http, named(name), T0).unwrap()
    }

    /// The files the engine was given, in order.
    fn started(control: &FakeControl) -> Vec<String> {
        let adds = control.log().iter().filter(|c| *c == "add").count() as u64;
        (1..=adds)
            .map(|n| control.spec(TransferId(n)).file_name.unwrap())
            .collect()
    }

    /// 2026-10-19 is a Monday.
    fn local(hour: i8, minute: i8) -> DateTime {
        date(2026, 10, 19).at(hour, minute, 0, 0)
    }

    #[test]
    fn no_more_than_the_limit_run_and_the_next_starts_when_one_ends() {
        let (mut m, control) = queue_of(2);
        let ids: Vec<String> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| add(&mut m, n))
            .collect();
        m.pump(T0);
        assert_eq!(started(&control), ["a", "b"]);
        assert_eq!(status(&m, &ids[2]), DownloadStatus::Queued);
        control.complete(TransferId(1));
        m.pump(T0);
        assert_eq!(
            started(&control),
            ["a", "b", "c"],
            "a finished, so c starts"
        );
        let error = TransferError::new(FailureClass::Permanent, Some(404), "Not found");
        control.fail(TransferId(2), error);
        m.pump(T0);
        assert_eq!(
            started(&control),
            ["a", "b", "c", "d"],
            "a failure frees its slot too"
        );
    }

    #[test]
    fn a_retrying_or_stalled_download_keeps_its_slot() {
        let (mut m, control) = queue_of(1);
        add(&mut m, "a");
        add(&mut m, "b");
        m.pump(T0);
        control.status(TransferId(1), DownloadStatus::Retrying);
        m.pump(T0);
        control.status(TransferId(1), DownloadStatus::Stalled);
        m.pump(T0);
        assert_eq!(started(&control), ["a"]);
    }

    #[test]
    fn a_paused_download_gives_its_slot_back_and_a_resumed_one_waits_for_a_slot() {
        let (mut m, control) = queue_of(1);
        let a = add(&mut m, "a");
        let b = add(&mut m, "b");
        m.pump(T0);
        m.pause(&a).unwrap();
        m.pump(T0);
        assert_eq!(status(&m, &a), DownloadStatus::Paused);
        assert_eq!(started(&control), ["a", "b"], "b takes a's slot");
        m.resume(&a).unwrap();
        m.pump(T0);
        assert_eq!(status(&m, &a), DownloadStatus::Queued, "waiting for a slot");
        assert!(!control.log().contains(&"resume 1".to_owned()));
        control.complete(TransferId(2));
        m.pump(T0);
        assert!(
            control.log().contains(&"resume 1".to_owned()),
            "b finished, so a goes on"
        );
        let _ = b;
    }

    #[test]
    fn a_move_changes_which_download_starts_next() {
        let (mut m, control) = queue_of(1);
        add(&mut m, "a");
        let b = add(&mut m, "b");
        let c = add(&mut m, "c");
        let d = add(&mut m, "d");
        m.pump(T0);
        // Waiting: b, c, d.
        m.move_download(&d, MoveDirection::Top).unwrap(); // d, b, c
        m.move_download(&b, MoveDirection::Bottom).unwrap(); // d, c, b
        m.move_download(&c, MoveDirection::Down).unwrap(); // d, b, c
        m.move_download(&b, MoveDirection::Up).unwrap(); // b, d, c
        for n in 1..=3 {
            control.complete(TransferId(n));
            m.pump(T0);
        }
        assert_eq!(started(&control), ["a", "b", "d", "c"]);
    }

    #[test]
    fn only_a_waiting_or_paused_download_can_move() {
        let (mut m, _control) = queue_of(1);
        let a = add(&mut m, "a");
        let b = add(&mut m, "b");
        m.pump(T0);
        assert_eq!(
            m.move_download(&a, MoveDirection::Bottom),
            Err(ErrorCode::InvalidState)
        );
        assert_eq!(
            m.move_download("d-9999", MoveDirection::Top),
            Err(ErrorCode::NotFound)
        );
        assert_eq!(m.move_download(&b, MoveDirection::Top), Ok(()));
    }

    #[test]
    fn added_paused_or_with_start_immediately_off_it_waits_for_a_resume() {
        let (mut m, control) = queue_of(3);
        let paused = m
            .add_with(
                DownloadKind::Http,
                named("a"),
                AddOptions {
                    start_paused: true,
                    scheduled: false,
                },
                T0,
            )
            .unwrap();
        m.set_queue_settings(QueueSettings {
            start_immediately: false,
            ..QueueSettings::default()
        })
        .unwrap();
        let later = add(&mut m, "b");
        m.pump(T0);
        assert_eq!(status(&m, &paused), DownloadStatus::Paused);
        assert_eq!(status(&m, &later), DownloadStatus::Paused);
        assert!(started(&control).is_empty(), "the engine was given neither");
        m.resume(&later).unwrap();
        m.pump(T0);
        assert_eq!(started(&control), ["b"]);
    }

    #[test]
    fn a_waiting_download_can_be_cancelled_without_the_engine() {
        let (mut m, control) = queue_of(1);
        add(&mut m, "a");
        let b = add(&mut m, "b");
        m.pump(T0);
        m.cancel(&b).unwrap();
        m.pump(T0);
        assert_eq!(status(&m, &b), DownloadStatus::Removed);
        control.complete(TransferId(1));
        m.pump(T0);
        assert_eq!(
            started(&control),
            ["a"],
            "a cancelled download never starts"
        );
    }

    #[test]
    fn scheduled_downloads_start_when_the_window_opens_and_pause_when_it_closes() {
        let (mut m, control) = queue_of(3);
        m.set_queue_settings(QueueSettings {
            schedule: ScheduleSettings {
                enabled: true,
                ..ScheduleSettings::default()
            },
            ..QueueSettings::default()
        })
        .unwrap();
        m.tick(T0, local(1, 59));
        let night = m
            .add_with(
                DownloadKind::Http,
                named("night"),
                AddOptions {
                    start_paused: false,
                    scheduled: true,
                },
                T0,
            )
            .unwrap();
        let now = add(&mut m, "now");
        m.tick(T0, local(1, 59));
        assert_eq!(started(&control), ["now"], "the window is closed");
        assert_eq!(status(&m, &night), DownloadStatus::Queued);
        m.tick(T0, local(2, 0));
        assert_eq!(
            started(&control),
            ["now", "night"],
            "02:00: the window opens"
        );
        m.tick(T0, local(7, 0));
        assert!(
            control.log().contains(&"pause 2".to_owned()),
            "07:00: it closes"
        );
        assert!(
            !control.log().contains(&"pause 1".to_owned()),
            "an unscheduled one runs on"
        );
        let _ = now;
    }

    #[test]
    fn pause_all_and_resume_all_cover_running_and_waiting_downloads() {
        let (mut m, control) = queue_of(1);
        let a = add(&mut m, "a");
        let b = add(&mut m, "b");
        m.pump(T0);
        m.pause_all().unwrap();
        m.pump(T0);
        assert_eq!(status(&m, &a), DownloadStatus::Paused);
        assert_eq!(status(&m, &b), DownloadStatus::Paused);
        assert_eq!(started(&control), ["a"], "b didn't take a's slot");
        m.resume_all().unwrap();
        m.pump(T0);
        assert!(
            control.log().contains(&"resume 1".to_owned()),
            "a, first in the queue"
        );
        assert_eq!(status(&m, &b), DownloadStatus::Queued);
    }

    #[test]
    fn a_waiting_download_the_engine_refuses_fails_with_the_reason_and_frees_its_turn() {
        let (mut m, control) = queue_of(1);
        add(&mut m, "a");
        let b = add(&mut m, "b");
        m.pump(T0);
        control.set_unavailable(Some(ErrorCode::EngineUnavailable));
        control.complete(TransferId(1));
        m.pump(T0);
        let r = m.get(&b).unwrap();
        assert_eq!(r.status, DownloadStatus::Failed);
        let message = r.error.as_ref().map(|e| e.message.as_str());
        assert_eq!(message, Some(ErrorCode::EngineUnavailable.user_message()));
        control.set_unavailable(None);
        add(&mut m, "c");
        assert_eq!(control.spec(TransferId(2)).file_name.as_deref(), Some("c"));
    }

    #[test]
    fn the_queue_to_save_is_its_order_with_the_scheduled_flags() {
        let (mut m, control) = queue_of(1);
        let a = add(&mut m, "a");
        let b = add(&mut m, "b");
        let c = m
            .add_with(
                DownloadKind::Http,
                named("c"),
                AddOptions {
                    start_paused: false,
                    scheduled: true,
                },
                T0,
            )
            .unwrap();
        m.move_download(&c, MoveDirection::Top).unwrap();
        let entry = |id: &String, scheduled| QueueEntry {
            id: id.clone(),
            scheduled,
        };
        // a runs, so it stays first: a move reorders only the waiting ones.
        assert_eq!(
            m.queue(),
            [entry(&a, false), entry(&c, true), entry(&b, false)]
        );
        control.complete(TransferId(1));
        m.pump(T0);
        assert_eq!(
            m.queue(),
            [entry(&c, true), entry(&b, false)],
            "a finished and left it"
        );
    }

    #[test]
    fn a_profile_sets_the_engines_limits_and_caps_new_downloads() {
        use crate::performance::{PerformanceProfile, PerformanceSettings};
        use crate::transfer::EngineLimits;
        let (mut m, control) = queue_of(3);
        let profile = |profile| PerformanceSettings {
            profile,
            ..PerformanceSettings::default()
        };
        let asking = |name: &str, connections| TransferSpec {
            max_connections: connections,
            ..named(name)
        };
        assert_eq!(m.connections_for_new_downloads(), 8, "Balanced by default");
        m.set_performance(profile(PerformanceProfile::Maximum))
            .unwrap();
        assert_eq!(control.engine_limits().max_connections_per_host, 16);
        assert_eq!(m.connections_for_new_downloads(), 16);
        m.add(DownloadKind::Http, asking("a", 16), T0).unwrap();
        assert_eq!(
            control.spec(TransferId(1)).max_connections,
            16,
            "Maximum allows it"
        );
        m.set_performance(profile(PerformanceProfile::Conservative))
            .unwrap();
        assert_eq!(
            control.engine_limits(),
            EngineLimits {
                max_connections_per_host: 2,
                min_segment_bytes: 20 << 20
            }
        );
        m.add(DownloadKind::Http, asking("b", 16), T0).unwrap();
        assert_eq!(
            control.spec(TransferId(2)).max_connections,
            2,
            "Conservative caps it"
        );
        let mut bad = profile(PerformanceProfile::Custom);
        bad.custom.max_connections_per_server = 0;
        let log = control.log().len();
        assert_eq!(m.set_performance(bad), Err(ErrorCode::InvalidInput));
        assert_eq!(control.log().len(), log, "nothing reached the engine");
    }

    #[test]
    fn a_new_limit_applies_at_once() {
        let (mut m, control) = queue_of(1);
        for n in ["a", "b", "c", "d"] {
            add(&mut m, n);
        }
        m.pump(T0);
        assert_eq!(started(&control), ["a"]);
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: 3,
            ..QueueSettings::default()
        })
        .unwrap();
        assert_eq!(started(&control), ["a", "b", "c"], "two more at once");
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: 1,
            ..QueueSettings::default()
        })
        .unwrap();
        assert!(
            !control.log().iter().any(|c| c.starts_with("pause")),
            "a lower limit lets the running ones finish, as aria2 did for V1"
        );
        let invalid = QueueSettings {
            max_concurrent_downloads: 0,
            ..QueueSettings::default()
        };
        assert_eq!(m.set_queue_settings(invalid), Err(ErrorCode::InvalidInput));
    }
}

/// Metalink downloads (#54).
mod metalink {
    use super::*;
    use crate::metalink::parse;

    fn document() -> Metalink {
        parse(xml().as_bytes()).unwrap()
    }

    fn xml() -> String {
        format!(
            "<metalink xmlns=\"urn:ietf:params:xml:ns:metalink\">\
             <file name=\"disc/image.iso\"><size>2048</size>\
             <hash type=\"sha-256\">{a}</hash>\
             <pieces length=\"1024\" type=\"sha-256\"><hash>{b}</hash><hash>{c}</hash></pieces>\
             <url priority=\"2\">https://b.example.com/image.iso</url>\
             <url priority=\"1\">ftp://a.example.com/image.iso</url></file>\
             <file name=\"readme.txt\"><url>ftp://example.com/readme.txt</url></file>\
             </metalink>",
            a = "a".repeat(64),
            b = "b".repeat(64),
            c = "c".repeat(64),
        )
    }

    fn with_metalink() -> Capabilities {
        Capabilities {
            metalink: true,
            ..Capabilities::sample()
        }
    }

    fn manager(capabilities: Capabilities, enabled: bool) -> (Manager, FakeControl) {
        let fake = FakeTransfer::with_capabilities(capabilities);
        let control = fake.control();
        let mut m = in_memory(fake);
        m.set_metalink_enabled(enabled);
        (m, control)
    }

    fn folder(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("v2-mgr-metalink-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn selection(directory: &std::path::Path, files: Option<Vec<usize>>) -> MetalinkSelection {
        MetalinkSelection {
            directory: directory.to_path_buf(),
            files,
            limits: Limits::default(),
        }
    }

    #[test]
    fn metalink_is_refused_while_it_is_turned_off() {
        let dir = folder("off");
        let (mut m, control) = manager(with_metalink(), false);
        let all = selection(&dir, None);
        assert_eq!(
            m.add_metalink(&document(), &all, AddOptions::default(), T0),
            Err(ErrorCode::ProtocolDisabled)
        );
        // The same gate for a download added as Metalink directly.
        assert_eq!(
            m.add(DownloadKind::Metalink, spec(), T0),
            Err(ErrorCode::ProtocolDisabled)
        );
        assert!(control.log().is_empty());
        assert!(!dir.join("disc").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_engine_without_metalink_refuses_it() {
        let dir = folder("unsupported");
        let (mut m, control) = manager(Capabilities::sample(), true);
        assert_eq!(
            m.add_metalink(
                &document(),
                &selection(&dir, None),
                AddOptions::default(),
                T0
            ),
            Err(ErrorCode::Unsupported)
        );
        assert!(control.log().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_file_becomes_a_download_in_its_subfolder_with_its_sources_and_hashes() {
        let dir = folder("all");
        let (mut m, control) = manager(with_metalink(), true);
        let ids = m
            .add_metalink(
                &document(),
                &selection(&dir, None),
                AddOptions::default(),
                T0,
            )
            .unwrap();
        assert_eq!(ids.len(), 2);
        let image = m.get(&ids[0]).unwrap();
        assert_eq!(image.kind, DownloadKind::Metalink);
        assert_eq!(image.source_url, "ftp://a.example.com/image.iso");
        assert_eq!(image.file_name.as_deref(), Some("image.iso"));
        assert_eq!(PathBuf::from(&image.directory), dir.join("disc"));
        assert!(dir.join("disc").is_dir());
        let spec = control.spec(TransferId(1));
        assert_eq!(
            spec.urls,
            [
                "ftp://a.example.com/image.iso",
                "https://b.example.com/image.iso"
            ]
        );
        assert_eq!(spec.directory, dir.join("disc"));
        assert_eq!(spec.file_name.as_deref(), Some("image.iso"));
        assert_eq!(spec.checksum, document().files[0].checksum);
        assert_eq!(spec.pieces, document().files[0].pieces);
        assert_eq!(spec.max_connections, m.connections_for_new_downloads());
        assert_eq!(spec.credentials, None);
        assert!(spec.headers.is_empty());
        let readme = m.get(&ids[1]).unwrap();
        assert_eq!(readme.file_name.as_deref(), Some("readme.txt"));
        assert_eq!(PathBuf::from(&readme.directory), dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_selected_files_are_added() {
        let dir = folder("selected");
        let (mut m, _control) = manager(with_metalink(), true);
        let one = selection(&dir, Some(vec![1]));
        let ids = m
            .add_metalink(&document(), &one, AddOptions::default(), T0)
            .unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(
            m.get(&ids[0]).unwrap().file_name.as_deref(),
            Some("readme.txt")
        );
        assert!(!dir.join("disc").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_selection_that_names_no_file_or_one_twice_is_refused() {
        let dir = folder("bad-selection");
        let (mut m, control) = manager(with_metalink(), true);
        for files in [vec![], vec![2], vec![0, 0]] {
            let bad = selection(&dir, Some(files.clone()));
            assert_eq!(
                m.add_metalink(&document(), &bad, AddOptions::default(), T0),
                Err(ErrorCode::InvalidInput),
                "{files:?}"
            );
        }
        assert!(control.log().is_empty());
        assert!(m.queue().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sources_the_engine_cant_fetch_are_dropped() {
        let dir = folder("http-only");
        let mut http_only = with_metalink();
        http_only.protocols.retain(|p| p.kind == DownloadKind::Http);
        let (mut m, control) = manager(http_only, true);
        let image = selection(&dir, Some(vec![0]));
        let ids = m
            .add_metalink(&document(), &image, AddOptions::default(), T0)
            .unwrap();
        assert_eq!(
            m.get(&ids[0]).unwrap().source_url,
            "https://b.example.com/image.iso"
        );
        assert_eq!(
            control.spec(TransferId(1)).urls,
            ["https://b.example.com/image.iso"]
        );
        // readme.txt has only an FTP source.
        let readme = selection(&dir, Some(vec![1]));
        assert_eq!(
            m.add_metalink(&document(), &readme, AddOptions::default(), T0),
            Err(ErrorCode::Unsupported)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A link added as any download, which the server names `file_name`.
    fn link(m: &mut Manager, dir: &std::path::Path) -> String {
        let spec = TransferSpec {
            urls: vec!["https://example.com/get?id=1".to_owned()],
            directory: dir.to_path_buf(),
            file_name: None,
            ..spec()
        };
        m.add(DownloadKind::Http, spec, T0).unwrap()
    }

    #[test]
    fn a_finished_link_to_a_metalink_file_adds_the_files_it_lists() {
        let dir = folder("follow");
        let (mut m, control) = manager(with_metalink(), true);
        let id = link(&mut m, &dir);
        std::fs::write(dir.join("files.meta4"), xml()).unwrap();
        control.named(TransferId(1), "files.meta4");
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        // The Metalink file itself stays a completed download, on disk.
        assert_eq!(status(&m, &id), DownloadStatus::Completed);
        assert!(dir.join("files.meta4").is_file());
        let queue: Vec<String> = m.queue().into_iter().map(|e| e.id).collect();
        assert_eq!(queue.len(), 2, "{queue:?}");
        let image = m.get(&queue[0]).unwrap();
        assert_eq!(image.kind, DownloadKind::Metalink);
        assert_eq!(image.file_name.as_deref(), Some("image.iso"));
        assert_eq!(PathBuf::from(&image.directory), dir.join("disc"));
        assert_eq!(
            control.spec(TransferId(2)).pieces,
            document().files[0].pieces
        );
        assert_eq!(
            m.get(&queue[1]).unwrap().file_name.as_deref(),
            Some("readme.txt")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_is_known_as_metalink_by_its_media_type_too() {
        for media_type in ["application/metalink4+xml", "application/metalink+xml"] {
            let dir = folder("follow-type");
            let (mut m, control) = manager(with_metalink(), true);
            link(&mut m, &dir);
            std::fs::write(dir.join("get"), xml()).unwrap();
            control.named(TransferId(1), "get");
            control.typed(TransferId(1), media_type);
            control.complete(TransferId(1));
            m.pump(T0 + 1);
            assert_eq!(m.queue().len(), 2, "{media_type}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn while_metalink_is_off_a_metalink_file_is_just_a_file() {
        let dir = folder("follow-off");
        let (mut m, control) = manager(with_metalink(), false);
        let id = link(&mut m, &dir);
        std::fs::write(dir.join("files.meta4"), xml()).unwrap();
        control.named(TransferId(1), "files.meta4");
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        assert_eq!(status(&m, &id), DownloadStatus::Completed);
        assert!(m.queue().is_empty());
        assert_eq!(control.log(), ["add"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_metalink_file_that_cant_be_used_fails_with_the_reason() {
        let dir = folder("follow-bad");
        let (mut m, control) = manager(with_metalink(), true);
        let id = link(&mut m, &dir);
        std::fs::write(dir.join("files.meta4"), "<rss/>").unwrap();
        control.named(TransferId(1), "files.meta4");
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Failed);
        let error = r.error.as_ref().unwrap();
        assert_eq!(error.class, FailureClass::Permanent);
        assert!(error.message.contains("Metalink"), "{}", error.message);
        assert_eq!(r.completed_at_ms, None);
        assert!(m.queue().is_empty());
        // The file the user downloaded is kept.
        assert!(dir.join("files.meta4").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_metalink_file_listed_in_a_metalink_is_not_followed_again() {
        let dir = folder("follow-nested");
        let (mut m, control) = manager(with_metalink(), true);
        let nested = parse(
            b"<metalink xmlns=\"urn:ietf:params:xml:ns:metalink\"><file name=\"inner.meta4\">\
              <url>https://example.com/inner.meta4</url></file></metalink>",
        )
        .unwrap();
        let ids = m
            .add_metalink(&nested, &selection(&dir, None), AddOptions::default(), T0)
            .unwrap();
        std::fs::write(dir.join("inner.meta4"), xml()).unwrap();
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        assert_eq!(status(&m, &ids[0]), DownloadStatus::Completed);
        assert!(m.queue().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_refused_file_takes_back_the_files_added_before_it() {
        let dir = folder("rollback");
        let (mut m, control) = manager(with_metalink(), true);
        control.refuse_file_name("readme.txt", ErrorCode::FileExists);
        assert_eq!(
            m.add_metalink(
                &document(),
                &selection(&dir, None),
                AddOptions::default(),
                T0
            ),
            Err(ErrorCode::FileExists)
        );
        m.pump(T0);
        assert!(m.queue().is_empty());
        assert!(
            control.log().contains(&"cancel 1".to_owned()),
            "{:?}",
            control.log()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// The records the manager keeps in the database (#58): what a restart brings back.
mod persistence {
    use super::*;
    use crate::transfer::{Checksum, Credentials, HashAlgorithm, Header};

    fn stored(db: &Database, id: &str) -> StoredDownload {
        db.downloads()
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .expect("the download is stored")
    }

    fn with_database() -> (Manager, FakeControl, Arc<Database>) {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        (Manager::new(Box::new(fake), Arc::clone(&db)), control, db)
    }

    #[test]
    fn an_added_download_is_stored_with_what_it_needs_to_restart_and_no_secret() {
        let (mut m, _control, db) = with_database();
        let checksum = Checksum {
            algorithm: HashAlgorithm::Sha256,
            hex: "ab".repeat(32),
        };
        let mut s = spec();
        s.urls = vec![
            "https://example.com/files/a.zip?token=t1".to_owned(),
            "https://mirror.example.net/a.zip".to_owned(),
        ];
        s.credentials = Some(Credentials {
            user_name: "user-secret".to_owned(),
            password: "password-secret".to_owned(),
        });
        s.headers = vec![Header {
            name: "Cookie".to_owned(),
            value: "cookie-secret".to_owned(),
        }];
        s.referrer = Some("https://example.com/page?session=referrer-secret#top".to_owned());
        s.user_agent = Some("Agent/1".to_owned());
        s.checksum = Some(checksum.clone());
        s.limits.max_download_bps = Some(64 * 1024);
        let id = m.add(DownloadKind::Http, s, T0).unwrap();

        let d = stored(&db, &id);
        assert_eq!(d.engine, Stored::Known(Engine::Velocity));
        assert_eq!(d.kind, Stored::Known(DownloadKind::Http));
        assert_eq!(d.status, Stored::Known(DownloadStatus::Queued));
        assert_eq!(d.source_url, "https://example.com/files/a.zip?token=t1");
        assert_eq!(d.directory, "downloads");
        assert_eq!(d.file_name.as_deref(), Some("a.zip"));
        assert_eq!(d.created_at_ms, T0);
        assert!(d.has_sensitive_options, "it had a login and headers");
        let options = d.options.known().expect("options this build reads").clone();
        assert_eq!(
            options.urls,
            vec![
                "https://example.com/files/a.zip?token=t1".to_owned(),
                "https://mirror.example.net/a.zip".to_owned(),
            ]
        );
        assert_eq!(options.max_connections, 4);
        assert_eq!(options.max_download_bps, Some(64 * 1024));
        assert_eq!(options.user_agent.as_deref(), Some("Agent/1"));
        assert_eq!(
            options.referrer.as_deref(),
            Some("https://example.com/page"),
            "a referrer keeps no query or fragment"
        );
        assert_eq!(options.checksum, Some(checksum));
        let row = format!("{d:?}");
        for secret in [
            "user-secret",
            "password-secret",
            "cookie-secret",
            "referrer-secret",
        ] {
            assert!(!row.contains(secret), "{secret} reached the database");
        }
    }

    #[test]
    fn what_the_engine_reports_is_stored_and_completion_once() {
        let (mut m, control, db) = with_database();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        control.named(TransferId(1), "server-name.zip");
        control.progress(TransferId(1), 1024, Some(4096));
        m.pump(T0 + 1);
        let d = stored(&db, &id);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Downloading));
        assert_eq!(d.file_name.as_deref(), Some("server-name.zip"));
        assert_eq!((d.completed_bytes, d.total_bytes), (1024, Some(4096)));
        assert_eq!(d.error, None);
        assert_eq!(d.verification, None);

        control.complete(TransferId(1));
        m.pump(T0 + 2);
        let d = stored(&db, &id);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Completed));
        assert_eq!((d.completed_bytes, d.total_bytes), (4096, Some(4096)));
        assert_eq!(
            d.verification,
            Some(Stored::Known(Verification::SizeVerified))
        );
        assert_eq!(d.completed_at_ms, Some(T0 + 2));
    }

    #[test]
    fn a_failure_is_stored_with_its_error() {
        let (mut m, control, db) = with_database();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        let error = TransferError::new(FailureClass::Permanent, Some(404), "Not found");
        control.fail(TransferId(1), error.clone());
        m.pump(T0 + 1);
        let d = stored(&db, &id);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Failed));
        assert_eq!(d.error, Some(Stored::Known(error)));
    }

    #[test]
    fn a_download_the_engine_refuses_is_not_stored() {
        let (mut m, control, db) = with_database();
        control.refuse_file_name("a.zip", ErrorCode::FileExists);
        assert_eq!(
            m.add(DownloadKind::Http, spec(), T0),
            Err(ErrorCode::FileExists)
        );
        assert_eq!(db.downloads().unwrap(), Vec::new());
    }
}
