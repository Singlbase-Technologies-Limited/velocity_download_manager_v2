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
    Manager::open(
        Box::new(fake),
        Arc::new(Database::open_in_memory().unwrap()),
    )
    .unwrap()
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

    /// Since #58, an engine that is unavailable leaves waiting downloads waiting
    /// (`engine_loss::while_the_engine_is_unavailable_waiting_downloads_keep_waiting`); any other
    /// refusal still fails the download.
    #[test]
    fn a_waiting_download_the_engine_refuses_fails_with_the_reason_and_frees_its_turn() {
        let (mut m, control) = queue_of(1);
        add(&mut m, "a");
        let b = add(&mut m, "b");
        let c = add(&mut m, "c");
        m.pump(T0);
        control.refuse_file_name("b", ErrorCode::FileExists);
        control.complete(TransferId(1));
        m.pump(T0);
        let r = m.get(&b).unwrap();
        assert_eq!(r.status, DownloadStatus::Failed);
        let message = r.error.as_ref().map(|e| e.message.as_str());
        assert_eq!(message, Some(ErrorCode::FileExists.user_message()));
        assert_eq!(control.spec(TransferId(2)).file_name.as_deref(), Some("c"));
        assert_eq!(
            m.queue().into_iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![c]
        );
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
    use super::super::persist::PROGRESS_SAVE_MS;
    use super::*;
    use crate::transfer::{Checkpoint, Checksum, Credentials, HashAlgorithm, Header};

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
        (
            Manager::open(Box::new(fake), Arc::clone(&db)).unwrap(),
            control,
            db,
        )
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
    fn progress_alone_is_saved_at_most_every_few_seconds_and_a_status_change_at_once() {
        let (mut m, control, db) = with_database();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        control.progress(TransferId(1), 1000, Some(100_000));
        m.pump(T0 + 1);
        assert_eq!(stored(&db, &id).completed_bytes, 1000, "the status changed");
        control.progress(TransferId(1), 2000, Some(100_000));
        m.pump(T0 + 1 + PROGRESS_SAVE_MS - 1);
        assert_eq!(
            stored(&db, &id).completed_bytes,
            1000,
            "too soon to save again"
        );
        control.progress(TransferId(1), 3000, Some(100_000));
        m.pump(T0 + 1 + PROGRESS_SAVE_MS);
        assert_eq!(stored(&db, &id).completed_bytes, 3000);
        control.progress(TransferId(1), 4000, Some(100_000));
        m.pump(T0 + 2 + PROGRESS_SAVE_MS);
        control.status(TransferId(1), DownloadStatus::Stalled);
        m.pump(T0 + 3 + PROGRESS_SAVE_MS);
        let d = stored(&db, &id);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Stalled));
        assert_eq!(d.completed_bytes, 4000, "saved with the status change");
    }

    #[test]
    fn what_the_manager_decides_itself_is_stored() {
        let (mut m, _control, db) = with_database();
        m.set_queue_settings(crate::queue::QueueSettings {
            max_concurrent_downloads: 1,
            ..crate::queue::QueueSettings::default()
        })
        .unwrap();
        let _running = m.add(DownloadKind::Http, spec(), T0).unwrap();
        let waiting = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    file_name: Some("b.zip".to_owned()),
                    ..spec()
                },
                T0,
            )
            .unwrap();
        let other = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    file_name: Some("c.zip".to_owned()),
                    ..spec()
                },
                T0,
            )
            .unwrap();
        m.pause(&waiting).unwrap();
        assert_eq!(
            stored(&db, &waiting).status,
            Stored::Known(DownloadStatus::Paused)
        );
        m.resume(&waiting).unwrap();
        assert_eq!(
            stored(&db, &waiting).status,
            Stored::Known(DownloadStatus::Queued)
        );
        m.cancel(&other).unwrap();
        assert_eq!(
            stored(&db, &other).status,
            Stored::Known(DownloadStatus::Removed)
        );
        m.pause_all().unwrap();
        assert_eq!(
            stored(&db, &waiting).status,
            Stored::Known(DownloadStatus::Paused)
        );
    }

    #[test]
    fn the_queue_is_stored_whenever_it_changes() {
        let (mut m, control, db) = with_database();
        m.set_queue_settings(crate::queue::QueueSettings {
            max_concurrent_downloads: 1,
            ..crate::queue::QueueSettings::default()
        })
        .unwrap();
        let a = m.add(DownloadKind::Http, spec(), T0).unwrap();
        let b = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    file_name: Some("b.zip".to_owned()),
                    ..spec()
                },
                T0,
            )
            .unwrap();
        let c = m
            .add_with(
                DownloadKind::Http,
                TransferSpec {
                    file_name: Some("c.zip".to_owned()),
                    ..spec()
                },
                AddOptions {
                    start_paused: false,
                    scheduled: true,
                },
                T0,
            )
            .unwrap();
        assert_eq!(db.queue().unwrap(), m.queue());
        m.move_download(&c, MoveDirection::Top).unwrap();
        assert_eq!(db.queue().unwrap(), m.queue());
        assert_eq!(db.queue().unwrap()[1].id, c);
        assert!(db.queue().unwrap()[1].scheduled);
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        let ids: Vec<String> = db.queue().unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![c, b], "a finished download leaves the queue");
        assert!(!ids.contains(&a));
    }

    #[test]
    fn checkpoints_are_stored_and_dropped_once_the_download_completes() {
        let (mut m, control, db) = with_database();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        let checkpoint = Checkpoint {
            format: crate::transfer::CHECKPOINT_FORMAT,
            source: "https://example.com/files/a.zip".to_owned(),
            total: 4096,
            etag: Some("\"v1\"".to_owned()),
            last_modified: None,
            content_encoding: None,
            durable: vec![(0, 1024)],
        };
        control.emit(TransferEvent::Checkpoint {
            id: TransferId(1),
            checkpoint: checkpoint.clone(),
        });
        m.pump(T0 + 1);
        assert_eq!(db.checkpoint(&id), Ok(Some(Stored::Known(checkpoint))));
        control.progress(TransferId(1), 1024, Some(4096));
        control.complete(TransferId(1));
        m.pump(T0 + 2);
        assert_eq!(db.checkpoint(&id), Ok(None));
    }

    #[test]
    fn a_failed_download_keeps_its_checkpoint_for_a_retry_and_a_cancelled_one_loses_it() {
        let (mut m, control, db) = with_database();
        let failed = m.add(DownloadKind::Http, spec(), T0).unwrap();
        let cancelled = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    file_name: Some("b.zip".to_owned()),
                    ..spec()
                },
                T0,
            )
            .unwrap();
        for n in [1, 2] {
            control.emit(TransferEvent::Checkpoint {
                id: TransferId(n),
                checkpoint: Checkpoint {
                    format: crate::transfer::CHECKPOINT_FORMAT,
                    source: "https://example.com/files/a.zip".to_owned(),
                    total: 4096,
                    etag: None,
                    last_modified: None,
                    content_encoding: None,
                    durable: vec![(0, 1024)],
                },
            });
        }
        m.pump(T0 + 1);
        control.fail(
            TransferId(1),
            TransferError::new(FailureClass::Transient, Some(503), "Unavailable"),
        );
        m.cancel(&cancelled).unwrap();
        m.pump(T0 + 2);
        assert!(db.checkpoint(&failed).unwrap().is_some());
        assert_eq!(db.checkpoint(&cancelled), Ok(None));
    }

    #[test]
    fn a_failed_save_is_reported_and_the_download_goes_on() {
        let (mut m, control, db) = with_database();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        // The record is gone from under the manager: its saves now fail.
        db.delete_download(&id).unwrap();
        control.progress(TransferId(1), 1024, Some(4096));
        m.pump(T0 + 1);
        assert_eq!(status(&m, &id), DownloadStatus::Downloading);
        assert_eq!(m.storage_error().map(|e| e.code), Some(ErrorCode::NotFound));
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

/// Bringing the downloads back when the app starts (#58): the records, the queue and the
/// checkpoints the database kept.
mod recovery {
    use super::*;
    use crate::database::{NewDownload, QueueEntry};
    use crate::queue::QueueSettings;
    use crate::transfer::{Checkpoint, Credentials, Resume, CHECKPOINT_FORMAT};

    fn named(name: &str) -> TransferSpec {
        TransferSpec {
            file_name: Some(name.to_owned()),
            ..spec()
        }
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: "https://example.com/files/a.zip".to_owned(),
            total: 4096,
            etag: Some("\"v1\"".to_owned()),
            last_modified: None,
            content_encoding: None,
            durable: vec![(0, 1024)],
        }
    }

    /// The app starting again over `db`, with a new engine.
    fn restart(db: &Arc<Database>, max: u32) -> (Manager, FakeControl) {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = Manager::open(Box::new(fake), Arc::clone(db)).expect("opens");
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: max,
            ..QueueSettings::default()
        })
        .unwrap();
        (m, control)
    }

    #[test]
    fn downloads_come_back_with_their_ids_statuses_and_progress_and_unfinished_ones_resume() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let (done, running, paused) = {
            let (mut m, control) = restart(&db, 1);
            let done = m.add(DownloadKind::Http, named("done.zip"), T0).unwrap();
            control.progress(TransferId(1), 4096, Some(4096));
            control.complete(TransferId(1));
            m.pump(T0 + 1);
            let running = m
                .add(
                    DownloadKind::Http,
                    TransferSpec {
                        file_name: None,
                        ..spec()
                    },
                    T0 + 2,
                )
                .unwrap();
            let paused = m
                .add(DownloadKind::Http, named("paused.zip"), T0 + 3)
                .unwrap();
            m.pause(&paused).unwrap();
            control.named(TransferId(2), "server.zip");
            control.progress(TransferId(2), 1024, Some(4096));
            control.emit(TransferEvent::Checkpoint {
                id: TransferId(2),
                checkpoint: checkpoint(),
            });
            m.pump(T0 + 4);
            (done, running, paused)
        };

        let (mut m, control) = restart(&db, 1);
        let r = m.get(&done).unwrap();
        assert_eq!(r.status, DownloadStatus::Completed);
        assert_eq!(r.verification, Some(Verification::SizeVerified));
        assert_eq!(r.completed_at_ms, Some(T0 + 1));
        assert_eq!(r.progress.completed_bytes, 4096);
        assert_eq!(r.created_at_ms, T0);

        assert_eq!(
            control.log(),
            ["add"],
            "only the unfinished, unpaused download starts"
        );
        let started = control.spec(TransferId(1));
        assert_eq!(
            started.resume,
            Some(Resume::Checkpoint {
                checkpoint: checkpoint()
            })
        );
        assert_eq!(
            started.file_name.as_deref(),
            Some("server.zip"),
            "a resume gives the name it was saved as"
        );
        assert_eq!(started.urls, spec().urls);
        assert_eq!(started.max_connections, 4);
        let r = m.get(&running).unwrap();
        assert_eq!(r.progress.completed_bytes, 1024);
        assert_eq!(r.progress.total_bytes, Some(4096));

        assert_eq!(status(&m, &paused), DownloadStatus::Paused);
        m.resume(&paused).unwrap();
        assert_eq!(
            status(&m, &paused),
            DownloadStatus::Queued,
            "it waits for the slot"
        );

        let next = m.add(DownloadKind::Http, named("new.zip"), T0 + 5).unwrap();
        assert_eq!(next, "d-0004", "ids are never given twice");
    }

    #[test]
    fn the_queue_order_and_schedule_come_back() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let paused = |scheduled| AddOptions {
            start_paused: true,
            scheduled,
        };
        let order = {
            let (mut m, _control) = restart(&db, 1);
            let a = m
                .add_with(DownloadKind::Http, named("a.zip"), paused(false), T0)
                .unwrap();
            let b = m
                .add_with(DownloadKind::Http, named("b.zip"), paused(false), T0)
                .unwrap();
            let c = m
                .add_with(DownloadKind::Http, named("c.zip"), paused(true), T0)
                .unwrap();
            m.move_download(&c, MoveDirection::Top).unwrap();
            m.move_download(&a, MoveDirection::Bottom).unwrap();
            vec![c, b, a]
        };
        let (m, control) = restart(&db, 1);
        let ids: Vec<String> = m.queue().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, order);
        assert!(m.queue()[0].scheduled);
        assert!(control.log().is_empty(), "paused, so nothing starts");
        for id in &order {
            assert_eq!(status(&m, id), DownloadStatus::Paused);
        }
    }

    #[test]
    fn an_unfinished_download_without_a_usable_checkpoint_starts_over() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        {
            let (mut m, control) = restart(&db, 1);
            m.add(DownloadKind::Http, spec(), T0).unwrap();
            control.progress(TransferId(1), 1024, Some(4096));
            m.pump(T0 + 1);
        }
        let (_m, control) = restart(&db, 1);
        assert_eq!(
            control.spec(TransferId(1)).resume,
            Some(Resume::DiscardPartFile {})
        );
    }

    #[test]
    fn a_failed_download_comes_back_failed_and_is_not_started() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = {
            let (mut m, control) = restart(&db, 1);
            let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
            control.fail(
                TransferId(1),
                TransferError::new(FailureClass::Permanent, Some(404), "Not found"),
            );
            m.pump(T0 + 1);
            id
        };
        let (m, control) = restart(&db, 1);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Failed);
        assert_eq!(r.error.as_ref().and_then(|e| e.http_status), Some(404));
        assert!(control.log().is_empty());
        assert!(m.queue().is_empty());
    }

    #[test]
    fn a_download_that_had_a_login_comes_back_paused_and_resumes_only_with_it() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let login = Credentials {
            user_name: "user".to_owned(),
            password: "secret".to_owned(),
        };
        let id = {
            let (mut m, _control) = restart(&db, 1);
            let s = TransferSpec {
                credentials: Some(login.clone()),
                ..spec()
            };
            m.add(DownloadKind::Http, s, T0).unwrap()
        };
        let (mut m, control) = restart(&db, 1);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Paused);
        assert!(
            r.error
                .as_ref()
                .is_some_and(|e| e.message.contains("login")),
            "the note says why: {:?}",
            r.error
        );
        assert!(m.needs_login(&id));
        assert_eq!(m.resume(&id), Err(ErrorCode::ConfirmationRequired));
        assert_eq!(m.resume_all(), Ok(()));
        assert_eq!(
            status(&m, &id),
            DownloadStatus::Paused,
            "resume all skips it"
        );
        assert!(control.log().is_empty());

        let login_again = SensitiveOptions {
            credentials: Some(login.clone()),
            ..SensitiveOptions::default()
        };
        m.set_login(&id, login_again).unwrap();
        m.resume(&id).unwrap();
        assert_eq!(control.spec(TransferId(1)).credentials, Some(login));
        assert_eq!(m.get(&id).unwrap().error, None, "the note is gone");
        assert!(!m.needs_login(&id));
    }

    #[test]
    fn a_record_this_build_cant_read_is_listed_as_unavailable_and_never_started() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let new = NewDownload::from_transfer("d-0007", DownloadKind::Http, &spec(), T0);
        db.insert_download(&new).unwrap();
        db.save_queue(&[QueueEntry {
            id: "d-0007".to_owned(),
            scheduled: false,
        }])
        .unwrap();
        db.set_raw_status_for_test("d-0007", "teleporting");
        let (mut m, control) = restart(&db, 1);
        assert!(m.get("d-0007").is_none());
        let unavailable = m.unavailable();
        assert_eq!(unavailable.len(), 1);
        assert_eq!(unavailable[0].id, "d-0007");
        assert_eq!(
            unavailable[0].status,
            Stored::Unknown("teleporting".to_owned())
        );
        assert!(control.log().is_empty());
        assert_eq!(m.pause("d-0007"), Err(ErrorCode::NotFound));
        let next = m.add(DownloadKind::Http, spec(), T0).unwrap();
        assert_eq!(next, "d-0008", "its id isn't given again either");
    }
}

/// Retrying by hand (V1's `retry_downloads` and `retry_failed`, #58). The engine retries a
/// retryable failure by itself first (#48); these are the user's retries, on top of none.
mod retry {
    use super::*;
    use crate::queue::QueueSettings;
    use crate::transfer::{Checkpoint, Credentials, Resume, CHECKPOINT_FORMAT};

    fn named(name: &str) -> TransferSpec {
        TransferSpec {
            file_name: Some(name.to_owned()),
            ..spec()
        }
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: "https://example.com/files/a.zip".to_owned(),
            total: 4096,
            etag: None,
            last_modified: None,
            content_encoding: None,
            durable: vec![(0, 2048)],
        }
    }

    fn not_found() -> TransferError {
        TransferError::new(FailureClass::Permanent, Some(404), "Not found")
    }

    #[test]
    fn a_failed_download_is_queued_again_and_resumes_from_its_checkpoint() {
        let (mut m, control, id) = setup();
        control.emit(TransferEvent::Checkpoint {
            id: TransferId(1),
            checkpoint: checkpoint(),
        });
        control.progress(TransferId(1), 2048, Some(4096));
        control.fail(TransferId(1), not_found());
        m.pump(T0 + 1);

        m.retry(&id).unwrap();
        assert_eq!(control.log(), ["add", "remove 1", "add"]);
        let again = control.spec(TransferId(2));
        assert_eq!(
            again.resume,
            Some(Resume::Checkpoint {
                checkpoint: checkpoint()
            })
        );
        assert_eq!(again.file_name.as_deref(), Some("a.zip"));
        m.pump(T0 + 2);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Queued);
        assert_eq!(r.error, None);

        control.progress(TransferId(2), 3000, Some(4096));
        m.pump(T0 + 3);
        assert_eq!(m.get(&id).unwrap().progress.completed_bytes, 3000);
    }

    #[test]
    fn a_cancelled_download_starts_over() {
        let (mut m, control, id) = setup();
        control.progress(TransferId(1), 2048, Some(4096));
        m.cancel(&id).unwrap();
        m.pump(T0 + 1);
        assert_eq!(status(&m, &id), DownloadStatus::Removed);

        m.retry(&id).unwrap();
        assert_eq!(
            control.spec(TransferId(2)).resume,
            Some(Resume::DiscardPartFile {})
        );
        m.pump(T0 + 2);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Queued);
        assert_eq!(r.progress.completed_bytes, 0);
    }

    #[test]
    fn a_retried_download_waits_for_a_slot_at_the_end_of_the_queue() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: 1,
            ..QueueSettings::default()
        })
        .unwrap();
        let failed = m.add(DownloadKind::Http, named("a.zip"), T0).unwrap();
        control.fail(TransferId(1), not_found());
        m.pump(T0 + 1);
        let b = m.add(DownloadKind::Http, named("b.zip"), T0).unwrap();
        let c = m.add(DownloadKind::Http, named("c.zip"), T0).unwrap();
        m.retry(&failed).unwrap();
        let ids: Vec<String> = m.queue().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![b, c, failed.clone()]);
        assert_eq!(status(&m, &failed), DownloadStatus::Queued);
        assert_eq!(
            control.log().iter().filter(|c| *c == "add").count(),
            2,
            "only b runs"
        );
    }

    #[test]
    fn only_a_failed_or_removed_download_can_be_retried() {
        let (mut m, control, id) = setup();
        assert_eq!(m.retry(&id), Err(ErrorCode::InvalidState), "it's queued");
        control.progress(TransferId(1), 4096, Some(4096));
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        assert_eq!(m.retry(&id), Err(ErrorCode::InvalidState), "it's complete");
        assert_eq!(m.retry("d-9999"), Err(ErrorCode::NotFound));
        assert_eq!(control.log(), ["add"]);
    }

    #[test]
    fn a_retry_works_when_the_engine_no_longer_knows_the_transfer() {
        let (mut m, control, id) = setup();
        control.fail(TransferId(1), not_found());
        m.pump(T0 + 1);
        // The engine forgot it, as a restarted engine does.
        control.emit(TransferEvent::Removed { id: TransferId(1) });
        m.pump(T0 + 2);
        m.retry(&id).unwrap();
        assert_eq!(control.log(), ["add", "add"]);
    }

    #[test]
    fn retry_failed_retries_every_failed_download_and_nothing_else() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        let a = m.add(DownloadKind::Http, named("a.zip"), T0).unwrap();
        let b = m.add(DownloadKind::Http, named("b.zip"), T0).unwrap();
        let done = m.add(DownloadKind::Http, named("done.zip"), T0).unwrap();
        let removed = m.add(DownloadKind::Http, named("removed.zip"), T0).unwrap();
        control.fail(TransferId(1), not_found());
        control.fail(
            TransferId(2),
            TransferError::new(FailureClass::Transient, None, "Connection reset"),
        );
        control.complete(TransferId(3));
        m.cancel(&removed).unwrap();
        m.pump(T0 + 1);

        m.retry_failed().unwrap();
        assert_eq!(status(&m, &a), DownloadStatus::Queued);
        assert_eq!(status(&m, &b), DownloadStatus::Queued);
        assert_eq!(status(&m, &done), DownloadStatus::Completed);
        assert_eq!(status(&m, &removed), DownloadStatus::Removed);
    }

    #[test]
    fn after_a_restart_a_download_that_had_a_login_is_retried_only_with_it() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let login = Credentials {
            user_name: "user".to_owned(),
            password: "secret".to_owned(),
        };
        let id = {
            let fake = FakeTransfer::new();
            let control = fake.control();
            let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
            let s = TransferSpec {
                credentials: Some(login.clone()),
                ..spec()
            };
            let id = m.add(DownloadKind::Http, s, T0).unwrap();
            control.fail(TransferId(1), not_found());
            m.pump(T0 + 1);
            id
        };
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        assert_eq!(status(&m, &id), DownloadStatus::Failed);
        assert!(m.needs_login(&id));
        assert_eq!(m.retry(&id), Err(ErrorCode::ConfirmationRequired));
        assert_eq!(m.retry_failed(), Err(ErrorCode::ConfirmationRequired));
        assert!(control.log().is_empty());
        m.set_login(
            &id,
            SensitiveOptions {
                credentials: Some(login.clone()),
                ..SensitiveOptions::default()
            },
        )
        .unwrap();
        m.retry(&id).unwrap();
        assert_eq!(control.spec(TransferId(1)).credentials, Some(login));
    }

    #[test]
    fn a_retry_is_stored() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        control.fail(TransferId(1), not_found());
        m.pump(T0 + 1);
        m.retry(&id).unwrap();
        let d = db.downloads().unwrap().remove(0);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Queued));
        assert_eq!(d.error, None);
        assert_eq!(db.queue().unwrap()[0].id, id);
    }
}

/// Removing downloads and clearing completed ones (V1's `remove_downloads` and `clear_completed`,
/// #58): they leave the list and stay in the history; files go only when asked, and only the
/// download's own (PO decision on #58).
mod removal {
    use super::*;
    use crate::naming::PART_FILE_SUFFIX;
    use crate::torrent_files::Kept;
    use std::fs;

    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("v2-removal-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn in_folder(dir: &std::path::Path, name: &str) -> TransferSpec {
        TransferSpec {
            directory: dir.to_path_buf(),
            file_name: Some(name.to_owned()),
            ..spec()
        }
    }

    /// A manager with a database, and a completed 4096-byte `a.zip` in `dir`.
    fn completed_in(dir: &std::path::Path) -> (Manager, FakeControl, Arc<Database>, String) {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        let id = m
            .add(DownloadKind::Http, in_folder(dir, "a.zip"), T0)
            .unwrap();
        control.progress(TransferId(1), 4096, Some(4096));
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        fs::write(dir.join("a.zip"), vec![7u8; 4096]).unwrap();
        (m, control, db, id)
    }

    #[test]
    fn a_removed_download_leaves_the_list_stays_in_the_history_and_keeps_its_file() {
        let dir = folder("keep");
        let (mut m, control, db, id) = completed_in(&dir);
        assert_eq!(m.remove(&id, false), Ok(Vec::new()));
        assert!(m.get(&id).is_none());
        assert!(
            control.log().contains(&"remove 1".to_owned()),
            "the engine forgets it"
        );
        let d = db.downloads().unwrap().remove(0);
        assert!(!d.in_list);
        assert!(d.in_history);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Completed));
        assert!(dir.join("a.zip").exists());
        assert_eq!(m.remove(&id, false), Err(ErrorCode::NotFound));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removing_with_files_deletes_the_finished_file_and_its_part_file() {
        let dir = folder("delete");
        let (mut m, _control, _db, id) = completed_in(&dir);
        fs::write(dir.join(format!("a.zip{PART_FILE_SUFFIX}")), b"left over").unwrap();
        fs::write(dir.join("other.txt"), b"not the download's").unwrap();
        assert_eq!(m.remove(&id, true), Ok(Vec::new()));
        assert!(!dir.join("a.zip").exists());
        assert!(!dir.join(format!("a.zip{PART_FILE_SUFFIX}")).exists());
        assert!(dir.join("other.txt").exists());
        assert!(dir.exists(), "the folder is never removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_larger_than_the_download_is_kept_and_reported() {
        let dir = folder("larger");
        let (mut m, _control, _db, id) = completed_in(&dir);
        // Something else now has the name: it isn't the download's file.
        fs::write(dir.join("a.zip"), vec![7u8; 5000]).unwrap();
        assert_eq!(
            m.remove(&id, true),
            Ok(vec![(dir.join("a.zip"), Kept::TooLarge)])
        );
        assert!(dir.join("a.zip").exists());
        assert!(m.get(&id).is_none(), "the download is removed all the same");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_removed_unfinished_download_is_cancelled_and_never_comes_back() {
        let dir = folder("unfinished");
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        let id = m
            .add(DownloadKind::Http, in_folder(&dir, "a.zip"), T0)
            .unwrap();
        control.progress(TransferId(1), 1024, Some(4096));
        m.pump(T0 + 1);
        assert_eq!(m.remove(&id, false), Ok(Vec::new()));
        assert!(control.log().contains(&"cancel 1".to_owned()));
        m.pump(T0 + 2);
        assert!(m.get(&id).is_none());
        let d = db.downloads().unwrap().remove(0);
        assert_eq!(d.status, Stored::Known(DownloadStatus::Removed));
        assert_eq!(db.queue(), Ok(Vec::new()));
        assert_eq!(db.checkpoint(&id), Ok(None));
        drop(m);

        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        m.pump(T0 + 3);
        assert!(m.get(&id).is_none());
        assert!(control.log().is_empty());
        let next = m
            .add(DownloadKind::Http, in_folder(&dir, "b.zip"), T0)
            .unwrap();
        assert_eq!(next, "d-0002", "a hidden download's id isn't given again");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_waiting_download_the_engine_never_had_loses_its_part_file_when_removed() {
        let dir = folder("waiting");
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = {
            let fake = FakeTransfer::new();
            let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
            m.add_with(
                DownloadKind::Http,
                in_folder(&dir, "a.zip"),
                AddOptions {
                    start_paused: true,
                    scheduled: false,
                },
                T0,
            )
            .unwrap()
        };
        // What an earlier run left on disk.
        let part = dir.join(format!("a.zip{PART_FILE_SUFFIX}"));
        fs::write(&part, b"partial").unwrap();
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        assert_eq!(m.remove(&id, false), Ok(Vec::new()));
        assert!(!part.exists());
        assert!(control.log().is_empty(), "the engine never had it");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_engine_being_unavailable_stops_the_removal_of_a_running_download() {
        let (mut m, control, id) = setup();
        control.set_unavailable(Some(ErrorCode::EngineUnavailable));
        assert_eq!(m.remove(&id, false), Err(ErrorCode::EngineUnavailable));
        assert!(m.get(&id).is_some(), "it is still there, still running");
    }

    #[test]
    fn clear_completed_hides_completed_downloads_only_and_keeps_their_files() {
        let dir = folder("clear");
        let (mut m, control, db, done) = completed_in(&dir);
        let failed = m
            .add(DownloadKind::Http, in_folder(&dir, "b.zip"), T0)
            .unwrap();
        let running = m
            .add(DownloadKind::Http, in_folder(&dir, "c.zip"), T0)
            .unwrap();
        control.fail(
            TransferId(2),
            TransferError::new(FailureClass::Permanent, Some(404), "Not found"),
        );
        m.pump(T0 + 2);
        assert_eq!(m.clear_completed(), Ok(1));
        assert!(m.get(&done).is_none());
        assert!(m.get(&failed).is_some());
        assert!(m.get(&running).is_some());
        assert!(dir.join("a.zip").exists());
        let hidden: Vec<String> = db
            .downloads()
            .unwrap()
            .into_iter()
            .filter(|d| !d.in_list)
            .map(|d| d.id)
            .collect();
        assert_eq!(hidden, vec![done]);
        fs::remove_dir_all(&dir).unwrap();
    }
}

/// The engine stopping (#39, #58): what a crash, a hang or a restart by hand loses comes back on
/// its own once the engine runs again (PO decision on #58), and nothing waiting fails meanwhile.
mod engine_loss {
    use super::*;
    use crate::transfer::{Checkpoint, Resume, CHECKPOINT_FORMAT, ENGINE_STOPPED};

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: "https://example.com/files/a.zip".to_owned(),
            total: 4096,
            etag: None,
            last_modified: None,
            content_encoding: None,
            durable: vec![(0, 2048)],
        }
    }

    fn lost() -> TransferError {
        TransferError::new(FailureClass::Transient, None, ENGINE_STOPPED)
    }

    #[test]
    fn a_download_the_engine_lost_waits_and_resumes_from_its_checkpoint_once_it_runs_again() {
        let (mut m, control, id) = setup();
        control.emit(TransferEvent::Checkpoint {
            id: TransferId(1),
            checkpoint: checkpoint(),
        });
        control.progress(TransferId(1), 2048, Some(4096));
        m.pump(T0 + 1);
        // The supervisor reports it lost, and has no engine for a while.
        control.fail(TransferId(1), lost());
        control.set_unavailable(Some(ErrorCode::EngineUnavailable));
        m.pump(T0 + 2);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Queued, "it waits, it didn't fail");
        assert_eq!(r.error, Some(lost()), "with the reason shown");
        assert_eq!(r.progress.completed_bytes, 2048);
        assert_eq!(m.queue()[0].id, id);

        control.set_unavailable(None);
        m.pump(T0 + 3);
        assert_eq!(
            control.log().last().map(String::as_str),
            Some("add"),
            "added again once the engine answers"
        );
        let again = control.spec(TransferId(2));
        assert_eq!(
            again.resume,
            Some(Resume::Checkpoint {
                checkpoint: checkpoint()
            })
        );
        control.progress(TransferId(2), 3000, Some(4096));
        m.pump(T0 + 4);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Downloading);
        assert_eq!(r.error, None);
    }

    #[test]
    fn an_ordinary_retryable_failure_stays_failed() {
        let (mut m, control, id) = setup();
        control.fail(
            TransferId(1),
            TransferError::new(FailureClass::Transient, None, "Connection reset"),
        );
        m.pump(T0 + 1);
        assert_eq!(status(&m, &id), DownloadStatus::Failed);
        assert_eq!(
            control.log(),
            ["add"],
            "the engine already retried it (#48)"
        );
    }

    #[test]
    fn a_paused_download_the_engine_lost_stays_paused() {
        let (mut m, control, id) = setup();
        m.pause(&id).unwrap();
        control.fail(TransferId(1), lost());
        m.pump(T0 + 1);
        assert_eq!(status(&m, &id), DownloadStatus::Paused);
        m.resume(&id).unwrap();
        // The supervisor's record of the lost transfer is dropped (`remove 1`).
        assert_eq!(control.log(), ["add", "pause 1", "remove 1", "add"]);
    }

    #[test]
    fn while_the_engine_is_unavailable_waiting_downloads_keep_waiting() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        let id = m
            .add_with(
                DownloadKind::Http,
                spec(),
                AddOptions {
                    start_paused: true,
                    scheduled: false,
                },
                T0,
            )
            .unwrap();
        control.set_unavailable(Some(ErrorCode::EngineUnavailable));
        m.resume(&id).unwrap();
        m.pump(T0 + 1);
        let r = m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Queued);
        assert_eq!(r.error, None);
        control.set_unavailable(None);
        m.pump(T0 + 2);
        assert_eq!(control.log().last().map(String::as_str), Some("add"));
        assert!(m.get(&id).unwrap().error.is_none());
    }
}

/// A download's own speed limit and connections (V1's `set_download_speed_limit` and
/// `set_download_connections`; #47, #49, #58): live while it runs, kept for when it starts, and
/// stored.
mod per_download {
    use super::*;

    fn paused_one(m: &mut Manager) -> String {
        m.add_with(
            DownloadKind::Http,
            spec(),
            AddOptions {
                start_paused: true,
                scheduled: false,
            },
            T0,
        )
        .unwrap()
    }

    #[test]
    fn a_speed_limit_applies_to_a_running_download_at_once_and_is_stored() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        let limit = Limits {
            max_download_bps: Some(64 * 1024),
        };
        m.set_download_limits(&id, limit).unwrap();
        assert_eq!(control.limits(TransferId(1)), limit);
        let stored = db.downloads().unwrap().remove(0);
        assert_eq!(
            stored.options.known().unwrap().max_download_bps,
            Some(64 * 1024)
        );
        m.set_download_limits(&id, Limits::default()).unwrap();
        assert_eq!(control.limits(TransferId(1)), Limits::default());
    }

    #[test]
    fn a_waiting_download_takes_its_limit_when_it_starts() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        let id = paused_one(&mut m);
        let limit = Limits {
            max_download_bps: Some(2048),
        };
        m.set_download_limits(&id, limit).unwrap();
        assert!(control.log().is_empty());
        m.resume(&id).unwrap();
        assert_eq!(control.spec(TransferId(1)).limits, limit);
    }

    #[test]
    fn a_limit_below_one_kib_per_second_is_refused_with_nothing_sent() {
        let (mut m, control, id) = setup();
        let too_low = Limits {
            max_download_bps: Some(1000),
        };
        assert_eq!(
            m.set_download_limits(&id, too_low),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(control.log(), ["add"]);
    }

    #[test]
    fn connections_change_live_and_the_engine_decides_how_many_apply() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        // The sample engine allows 8 per host.
        assert_eq!(m.set_download_connections(&id, 2), Ok(2));
        assert_eq!(control.spec(TransferId(1)).max_connections, 2);
        assert_eq!(m.set_download_connections(&id, 50), Ok(8));
        let stored = db.downloads().unwrap().remove(0);
        assert_eq!(stored.options.known().unwrap().max_connections, 8);
        assert_eq!(
            m.set_download_connections(&id, 0),
            Err(ErrorCode::InvalidInput)
        );
    }

    #[test]
    fn a_waiting_download_starts_with_its_connections_capped_by_the_profile() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        let id = paused_one(&mut m);
        assert_eq!(m.set_download_connections(&id, 3), Ok(3));
        let cap = m.connections_for_new_downloads();
        assert_eq!(m.set_download_connections(&id, 99), Ok(cap));
        m.resume(&id).unwrap();
        assert_eq!(control.spec(TransferId(1)).max_connections, cap);
    }

    #[test]
    fn a_finished_download_takes_neither() {
        let (mut m, control, id) = setup();
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        assert_eq!(
            m.set_download_limits(&id, Limits::default()),
            Err(ErrorCode::InvalidState)
        );
        assert_eq!(
            m.set_download_connections(&id, 2),
            Err(ErrorCode::InvalidState)
        );
    }
}
