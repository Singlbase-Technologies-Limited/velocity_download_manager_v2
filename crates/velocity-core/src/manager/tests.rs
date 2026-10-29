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

/// The user's action wins (#59): a running status the engine sent before it stopped the
/// transfer doesn't show a paused or cancelled download running again.
#[test]
fn a_late_running_status_does_not_undo_a_pause() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 1024, Some(4096));
    m.pump(T0);
    m.pause(&id).unwrap();
    // The worker reports bytes once more on its way out.
    control.emit(late_progress(&control, 2048));
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Paused);
    // The bytes are true, so they still count.
    assert_eq!(m.get(&id).unwrap().progress.completed_bytes, 2048);
    // Once resumed, the engine's running status shows again.
    m.resume(&id).unwrap();
    control.progress(TransferId(1), 3072, Some(4096));
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Downloading);
}

#[test]
fn a_late_running_status_does_not_undo_a_cancel() {
    let (mut m, control, id) = setup();
    control.progress(TransferId(1), 1024, Some(4096));
    m.pump(T0);
    m.cancel(&id).unwrap();
    control.emit(late_progress(&control, 2048));
    m.pump(T0);
    assert_eq!(status(&m, &id), DownloadStatus::Removed);
}

/// A Downloading snapshot of transfer 1 with `completed_bytes`, sent late: the fake engine's own
/// state stays as it is.
fn late_progress(control: &FakeControl, completed_bytes: u64) -> TransferEvent {
    let mut snapshot = control.snapshot(TransferId(1));
    snapshot.status = DownloadStatus::Downloading;
    snapshot.progress.completed_bytes = completed_bytes;
    TransferEvent::Changed { snapshot }
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
                    category_id: None,
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
                    category_id: None,
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
                    category_id: None,
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
                    category_id: None,
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
            category_id: None,
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
                    category_id: None,
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
                    category_id: None,
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
                category_id: None,
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

/// The engine's health as the app shows it, and "Restart transfer engine" (#39, #58).
mod engine_health {
    use super::*;
    use crate::domain::EngineHealth;

    #[test]
    fn the_engines_health_is_reported_as_the_engine_gives_it() {
        let (m, control, _id) = setup();
        assert_eq!(m.engine_health(), EngineHealth::Running);
        control.set_health(EngineHealth::Unavailable {
            code: ErrorCode::EngineMissing,
        });
        assert_eq!(
            m.engine_health(),
            EngineHealth::Unavailable {
                code: ErrorCode::EngineMissing
            }
        );
    }

    #[test]
    fn restart_engine_asks_the_engine_to_restart() {
        let (mut m, control, _id) = setup();
        m.restart_engine().unwrap();
        assert_eq!(control.log(), ["add", "restart"]);
    }

    #[test]
    fn the_health_serialises_for_the_ui() {
        let json = |h: EngineHealth| serde_json::to_value(h).unwrap();
        assert_eq!(
            json(EngineHealth::Running),
            serde_json::json!({"state": "running"})
        );
        assert_eq!(
            json(EngineHealth::Restarting { attempt: 2 }),
            serde_json::json!({"state": "restarting", "attempt": 2})
        );
        assert_eq!(
            json(EngineHealth::Unavailable {
                code: ErrorCode::EngineUnavailable
            }),
            serde_json::json!({"state": "unavailable", "code": "ENGINE_UNAVAILABLE"})
        );
    }
}

/// What the UI is sent (#58, V1's `get_snapshot`): every download in the list, those this build
/// can't act on, the engine's health and the last failure to save.
mod snapshot {
    use super::*;
    use crate::database::NewDownload;
    use crate::domain::{DownloadsSnapshot, EngineHealth, UnavailableDownload};
    use crate::transfer::Credentials;

    #[test]
    fn the_snapshot_lists_the_downloads_in_queue_order_then_the_finished_ones_newest_first() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        let named = |name: &str| TransferSpec {
            file_name: Some(name.to_owned()),
            ..spec()
        };
        let a = m.add(DownloadKind::Http, named("a"), T0).unwrap();
        let b = m.add(DownloadKind::Http, named("b"), T0 + 1).unwrap();
        let c = m.add(DownloadKind::Http, named("c"), T0 + 2).unwrap();
        let d = m.add(DownloadKind::Http, named("d"), T0 + 3).unwrap();
        control.complete(TransferId(1));
        control.complete(TransferId(2));
        m.pump(T0 + 4);
        let s: DownloadsSnapshot = m.snapshot();
        let ids: Vec<&str> = s.downloads.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [c.as_str(), d.as_str(), b.as_str(), a.as_str()]);
        assert_eq!(s.engine, EngineHealth::Running);
        assert_eq!(s.storage_error, None);
        assert!(s.unavailable.is_empty());
        assert!(s.needs_login.is_empty());
    }

    #[test]
    fn the_snapshot_names_downloads_waiting_for_a_login_and_records_it_cant_act_on() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let login = {
            let mut m = Manager::open(Box::new(FakeTransfer::new()), Arc::clone(&db)).unwrap();
            let s = TransferSpec {
                credentials: Some(Credentials {
                    user_name: "u".to_owned(),
                    password: "p".to_owned(),
                }),
                ..spec()
            };
            m.add(DownloadKind::Http, s, T0).unwrap()
        };
        let odd = NewDownload::from_transfer("d-0009", DownloadKind::Http, &spec(), T0 + 1);
        db.insert_download(&odd).unwrap();
        db.set_raw_status_for_test("d-0009", "teleporting");
        let m = Manager::open(Box::new(FakeTransfer::new()), Arc::clone(&db)).unwrap();
        let s = m.snapshot();
        assert_eq!(s.needs_login, vec![login]);
        assert_eq!(
            s.unavailable,
            vec![UnavailableDownload {
                id: "d-0009".to_owned(),
                source_url: "https://example.com/files/a.zip".to_owned(),
                file_name: Some("a.zip".to_owned()),
                directory: "downloads".to_owned(),
                created_at_ms: T0 + 1,
            }]
        );
    }

    #[test]
    fn a_failed_save_is_in_the_snapshot() {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        db.delete_download(&id).unwrap();
        control.progress(TransferId(1), 10, Some(20));
        m.pump(T0 + 1);
        assert_eq!(m.snapshot().storage_error, Some(ErrorCode::NotFound));
    }

    #[test]
    fn the_snapshot_serialises_in_camel_case() {
        let (m, _control, _id) = setup();
        let json = serde_json::to_value(m.snapshot()).unwrap();
        for key in [
            "downloads",
            "unavailable",
            "needsLogin",
            "engine",
            "storageError",
        ] {
            assert!(json.get(key).is_some(), "{key} in {json}");
        }
    }
}

/// Commands over several downloads at once (V1's batch commands, #58): each id is tried, and the
/// result says which were done and why the others weren't.
mod batch {
    use super::*;
    use crate::domain::{BatchFailure, BatchResult, KeptFile, KeptReason};
    use std::fs;

    #[test]
    fn each_id_is_tried_and_failures_carry_their_code() {
        let (mut m, control, running) = setup();
        let done = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    file_name: Some("b.zip".to_owned()),
                    ..spec()
                },
                T0,
            )
            .unwrap();
        control.complete(TransferId(2));
        m.pump(T0 + 1);
        let ids = vec![running.clone(), "d-9999".to_owned(), done.clone()];
        let result = m.batch(&ids, Manager::pause).unwrap();
        assert_eq!(
            result,
            BatchResult {
                done: vec![running],
                failed: vec![
                    BatchFailure {
                        id: "d-9999".to_owned(),
                        code: ErrorCode::NotFound
                    },
                    BatchFailure {
                        id: done,
                        code: ErrorCode::InvalidState
                    },
                ],
                kept: Vec::new(),
            }
        );
    }

    #[test]
    fn too_many_ids_or_an_overlong_one_is_refused_with_nothing_done() {
        let (mut m, control, id) = setup();
        let many = vec![id.clone(); MAX_BATCH + 1];
        assert_eq!(m.batch(&many, Manager::pause), Err(ErrorCode::InvalidInput));
        let long = vec![id, "d".repeat(65)];
        assert_eq!(m.batch(&long, Manager::pause), Err(ErrorCode::InvalidInput));
        assert_eq!(control.log(), ["add"]);
    }

    #[test]
    fn removing_several_reports_the_files_kept_and_why() {
        let dir = std::env::temp_dir().join(format!("v2-batch-remove-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut m = in_memory(fake);
        let id = m
            .add(
                DownloadKind::Http,
                TransferSpec {
                    directory: dir.clone(),
                    ..spec()
                },
                T0,
            )
            .unwrap();
        control.progress(TransferId(1), 10, Some(10));
        control.complete(TransferId(1));
        m.pump(T0 + 1);
        fs::write(dir.join("a.zip"), vec![1u8; 11]).unwrap();
        let result = m.remove_many(std::slice::from_ref(&id), true).unwrap();
        assert_eq!(result.done, vec![id]);
        assert_eq!(
            result.kept,
            vec![KeptFile {
                path: dir.join("a.zip").to_string_lossy().into_owned(),
                reason: KeptReason::TooLarge,
                code: None,
            }]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_result_serialises_in_camel_case() {
        let json = serde_json::to_value(BatchResult {
            done: vec!["d-0001".to_owned()],
            failed: vec![BatchFailure {
                id: "d-0002".to_owned(),
                code: ErrorCode::NotFound,
            }],
            kept: vec![KeptFile {
                path: "x".to_owned(),
                reason: KeptReason::Failed,
                code: Some(ErrorCode::PermissionDenied),
            }],
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "done": ["d-0001"],
                "failed": [{"id": "d-0002", "code": "NOT_FOUND"}],
                "kept": [{"path": "x", "reason": "failed", "code": "PERMISSION_DENIED"}],
            })
        );
    }
}

/// Torrents in the manager (#58, part 2): added and queued like direct downloads (PO decision on
/// #58), driven through the torrent contract, and stored without trackers.
mod torrents {
    use super::*;
    use crate::database::TorrentOrigin;
    use crate::queue::QueueSettings;
    use crate::seeding::SeedPolicy;
    use crate::torrent::bencode::{encode, Value};
    use crate::torrent::tests::{album, resume_for, sha1_hex};
    use crate::torrent_engine::fake::{FakeTorrentControl, FakeTorrentEngine};
    use crate::torrent_engine::{TorrentId, TorrentSource, HELPER_STOPPED};
    use std::fs;

    const MAGNET: &str = "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&dn=album\
                          &tr=https%3A%2F%2Ftracker.example%2Fannounce%3Fpasskey%3Dsecret-key";

    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("v2-torrents-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(dir: &std::path::Path) -> TorrentRequest {
        TorrentRequest {
            origin: TorrentOrigin::Magnet {
                uri: MAGNET.to_owned(),
            },
            directory: dir.to_path_buf(),
            trackers: Vec::new(),
            choose_files: false,
            files: None,
            seeding: SeedPolicy::Never {},
            upload_limit: 0,
            download_limit: 0,
        }
    }

    /// The album's metadata (`d4:info…e`) and its v1 info hash.
    fn metadata() -> (Vec<u8>, String) {
        let info = encode(&album());
        let mut m = b"d4:info".to_vec();
        m.extend(&info);
        m.push(b'e');
        (m, sha1_hex(&info))
    }

    fn resume_data() -> Vec<u8> {
        resume_for(
            &album(),
            vec![(
                "trackers",
                Value::List(vec![Value::List(vec![Value::str(
                    "https://tracker.example/announce?passkey=secret-key",
                )])]),
            )],
        )
    }

    struct Setup {
        m: Manager,
        torrents: FakeTorrentControl,
        direct: FakeControl,
        db: Arc<Database>,
    }

    fn with_db(db: &Arc<Database>, max: u32) -> Setup {
        let fake = FakeTransfer::new();
        let direct = fake.control();
        let lt = FakeTorrentEngine::new();
        let torrents = lt.control();
        let mut m = Manager::open(Box::new(fake), Arc::clone(db))
            .unwrap()
            .with_torrents(Box::new(lt));
        m.set_queue_settings(QueueSettings {
            max_concurrent_downloads: max,
            ..QueueSettings::default()
        })
        .unwrap();
        Setup {
            m,
            torrents,
            direct,
            db: Arc::clone(db),
        }
    }

    fn setup(max: u32) -> Setup {
        with_db(&Arc::new(Database::open_in_memory().unwrap()), max)
    }

    #[test]
    fn a_magnet_is_added_and_started_with_its_trackers_and_stored_without_them() {
        let dir = folder("magnet");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        s.m.pump(T0 + 1);
        let r = s.m.get(&id).unwrap();
        assert_eq!(r.kind, DownloadKind::Magnet);
        assert_eq!(r.status, DownloadStatus::FetchingMetadata);
        assert_eq!(
            r.file_name.as_deref(),
            Some("album"),
            "the magnet's name until the metadata's"
        );
        let spec = s.torrents.spec(TorrentId(1));
        assert_eq!(
            spec.source,
            TorrentSource::Magnet {
                uri: MAGNET.to_owned()
            },
            "this session keeps the trackers in memory"
        );
        assert_eq!(spec.directory, dir);

        let stored = s.db.torrent(&id).unwrap().unwrap();
        let options = stored.options.known().unwrap().clone();
        let TorrentOrigin::Magnet { uri } = &options.origin else {
            panic!("a magnet origin: {options:?}");
        };
        assert!(uri.starts_with("magnet:?xt=urn:btih:aaaa"), "{uri}");
        let everything = format!("{:?} {:?}", s.db.downloads().unwrap(), stored);
        assert!(
            !everything.contains("secret-key"),
            "no passkey stored: {everything}"
        );
        assert!(!everything.contains("tracker.example"), "no tracker stored");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_the_helper_reports_reaches_the_record_and_the_database() {
        let dir = folder("events");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        let (meta, hash) = metadata();
        s.torrents
            .metadata_known(TorrentId(1), "album", &hash, 32_000, meta.clone());
        s.torrents.progress(TorrentId(1), 16_000);
        s.m.pump(T0 + 1);
        let r = s.m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Downloading);
        assert_eq!(r.progress.completed_bytes, 16_000);
        assert_eq!(r.progress.total_bytes, Some(32_000));
        let d = s.db.downloads().unwrap().remove(0);
        assert_eq!(d.info_hash.as_deref(), Some(hash.as_str()));
        assert_eq!(
            s.db.torrent(&id).unwrap().unwrap().metadata,
            Some(meta),
            "its metadata is kept once known (#62)"
        );
        s.torrents.complete(TorrentId(1));
        s.m.pump(T0 + 2);
        let r = s.m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Completed);
        assert_eq!(r.completed_at_ms, Some(T0 + 2));
        assert!(s.m.queue().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn metadata_that_isnt_the_torrents_is_never_kept() {
        let dir = folder("bad-metadata");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        let (meta, _hash) = metadata();
        s.torrents
            .metadata_known(TorrentId(1), "album", &"b".repeat(40), 32_000, meta);
        s.m.pump(T0 + 1);
        assert_eq!(s.db.torrent(&id).unwrap().unwrap().metadata, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resume_data_is_put_together_stripped_of_its_trackers_and_saved() {
        let dir = folder("resume");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        s.torrents.resume_data(TorrentId(1), &resume_data());
        s.m.pump(T0 + 1);
        let saved = s.db.torrent(&id).unwrap().unwrap().resume_data.unwrap();
        assert!(!String::from_utf8_lossy(&saved).contains("secret-key"));
        let (expected, _) = crate::torrent::strip_trackers(&resume_data()).unwrap();
        assert_eq!(saved, expected);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn torrents_wait_for_a_slot_and_free_it_to_seed_or_choose_files() {
        let dir = folder("queue");
        let mut s = setup(1);
        let seeding = request(&dir);
        let a =
            s.m.add_torrent(
                TorrentRequest {
                    seeding: SeedPolicy::Unlimited {},
                    ..seeding
                },
                AddOptions::default(),
                T0,
            )
            .unwrap();
        let b = s.m.add(DownloadKind::Http, spec(), T0).unwrap();
        s.m.pump(T0 + 1);
        assert!(s.direct.log().is_empty(), "the torrent holds the one slot");
        assert_eq!(status(&s.m, &b), DownloadStatus::Queued);
        let (meta, hash) = metadata();
        s.torrents
            .metadata_known(TorrentId(1), "album", &hash, 32_000, meta);
        s.torrents.complete(TorrentId(1));
        s.m.pump(T0 + 2);
        assert_eq!(status(&s.m, &a), DownloadStatus::Seeding);
        assert_eq!(s.direct.log(), ["add"], "seeding frees the slot");

        let mut s = setup(1);
        let c =
            s.m.add_torrent(
                TorrentRequest {
                    choose_files: true,
                    ..request(&dir)
                },
                AddOptions::default(),
                T0,
            )
            .unwrap();
        s.m.add(DownloadKind::Http, spec(), T0).unwrap();
        let (meta, hash) = metadata();
        s.torrents
            .metadata_known(TorrentId(1), "album", &hash, 32_000, meta);
        s.m.pump(T0 + 1);
        assert_eq!(status(&s.m, &c), DownloadStatus::ChoosingFiles);
        assert_eq!(s.direct.log(), ["add"], "choosing files frees the slot");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_are_chosen_and_the_choice_is_stored() {
        let dir = folder("choose");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(
                TorrentRequest {
                    choose_files: true,
                    ..request(&dir)
                },
                AddOptions::default(),
                T0,
            )
            .unwrap();
        assert_eq!(s.m.choose_files(&id, vec![1]), Err(ErrorCode::InvalidState));
        let (meta, hash) = metadata();
        s.torrents
            .metadata_known(TorrentId(1), "album", &hash, 32_000, meta);
        s.m.pump(T0 + 1);
        s.m.choose_files(&id, vec![1]).unwrap();
        assert!(s.torrents.log().contains(&"set_files 1 [1]".to_owned()));
        let options =
            s.db.torrent(&id)
                .unwrap()
                .unwrap()
                .options
                .known()
                .unwrap()
                .clone();
        assert_eq!(options.files, Some(vec![1]));
        assert!(!options.choose_files, "chosen");
        assert_eq!(
            s.m.choose_files("d-9999", vec![1]),
            Err(ErrorCode::NotFound)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_seeding_policy_and_limits_change_live_and_are_stored() {
        let dir = folder("policy");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        s.m.set_seeding(&id, SeedPolicy::Ratio { ratio: 2.0 })
            .unwrap();
        s.m.set_download_limits(
            &id,
            Limits {
                max_download_bps: Some(64 * 1024),
            },
        )
        .unwrap();
        assert!(s.torrents.log().contains(&"set_seeding 1".to_owned()));
        assert!(s
            .torrents
            .log()
            .contains(&"set_limits 1 0 65536".to_owned()));
        let options =
            s.db.torrent(&id)
                .unwrap()
                .unwrap()
                .options
                .known()
                .unwrap()
                .clone();
        assert_eq!(options.seeding, SeedPolicy::Ratio { ratio: 2.0 });
        assert_eq!(options.download_limit, 64 * 1024);
        assert_eq!(
            s.m.set_seeding(&id, SeedPolicy::Ratio { ratio: 1000.0 }),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(
            s.m.set_download_connections(&id, 4),
            Err(ErrorCode::Unsupported),
            "libtorrent chooses its peers"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pause_resume_and_cancel_go_to_the_helper() {
        let dir = folder("lifecycle");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        s.m.pump(T0 + 1);
        s.m.pause(&id).unwrap();
        s.m.pump(T0 + 2);
        assert_eq!(status(&s.m, &id), DownloadStatus::Paused);
        s.m.resume(&id).unwrap();
        s.m.pump(T0 + 3);
        assert_eq!(s.torrents.log(), ["add", "pause 1", "resume 1"]);
        s.m.cancel(&id).unwrap();
        s.m.pump(T0 + 4);
        assert_eq!(status(&s.m, &id), DownloadStatus::Removed);
        assert!(s.torrents.log().contains(&"remove 1".to_owned()));
        assert!(s.m.queue().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_torrent_the_helper_lost_waits_for_it_to_come_back() {
        let dir = folder("lost");
        let mut s = setup(1);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        s.m.add(DownloadKind::Http, spec(), T0).unwrap();
        s.torrents.progress(TorrentId(1), 100);
        s.torrents.fail(
            TorrentId(1),
            TransferError::new(FailureClass::Transient, None, HELPER_STOPPED),
        );
        s.m.pump(T0 + 1);
        let r = s.m.get(&id).unwrap();
        assert_eq!(
            r.status,
            DownloadStatus::Queued,
            "not failed: it comes back"
        );
        assert!(r.error.is_some());
        assert!(s.direct.log().is_empty(), "it keeps its slot");
        // The supervisor adds it back under the same id.
        s.torrents.progress(TorrentId(1), 200);
        s.m.pump(T0 + 2);
        let r = s.m.get(&id).unwrap();
        assert_eq!(r.status, DownloadStatus::Downloading);
        assert_eq!(r.error, None);
        assert_eq!(s.torrents.log(), ["add"], "the manager adds nothing itself");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn after_a_restart_a_torrent_comes_back_from_its_resume_data_without_trackers() {
        let dir = folder("restart");
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = {
            let mut s = with_db(&db, 3);
            let id =
                s.m.add_torrent(
                    TorrentRequest {
                        origin: TorrentOrigin::File {
                            path: dir.join("album.torrent"),
                        },
                        files: Some(vec![1]),
                        upload_limit: 1024,
                        ..request(&dir)
                    },
                    AddOptions::default(),
                    T0,
                )
                .unwrap();
            s.torrents.progress(TorrentId(1), 100);
            s.torrents.resume_data(TorrentId(1), &resume_data());
            s.m.pump(T0 + 1);
            id
        };
        let mut s = with_db(&db, 3);
        s.m.pump(T0 + 2);
        assert_eq!(status(&s.m, &id), DownloadStatus::Queued);
        let log = s.torrents.log();
        assert_eq!(log.first().map(String::as_str), Some("upload 0"));
        assert_eq!(log.last().map(String::as_str), Some("add"));
        let (expected, _) = crate::torrent::strip_trackers(&resume_data()).unwrap();
        assert_eq!(s.torrents.uploaded(), expected);
        let spec = s.torrents.spec(TorrentId(1));
        assert_eq!(spec.source, TorrentSource::Resume {});
        assert!(
            spec.trackers.is_empty(),
            "trackers wait for the keychain (#70)"
        );
        assert_eq!(spec.upload_limit, 1024);
        assert_eq!(spec.directory, dir);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn after_a_restart_a_torrent_without_resume_data_comes_back_from_its_origin() {
        let dir = folder("origin");
        let db = Arc::new(Database::open_in_memory().unwrap());
        {
            let mut s = with_db(&db, 3);
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        }
        let mut s = with_db(&db, 3);
        s.m.pump(T0 + 1);
        let spec = s.torrents.spec(TorrentId(1));
        let TorrentSource::Magnet { uri } = spec.source else {
            panic!("from the magnet");
        };
        assert!(!uri.contains("tr="), "without its trackers: {uri}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removing_a_torrent_with_its_files_deletes_only_its_own() {
        let dir = folder("delete");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        let (meta, hash) = metadata();
        s.torrents
            .metadata_known(TorrentId(1), "album", &hash, 32_000, meta);
        s.torrents.complete(TorrentId(1));
        s.m.pump(T0 + 1);
        fs::create_dir_all(dir.join("album")).unwrap();
        fs::write(dir.join("album").join("a.bin"), vec![1u8; 20_000]).unwrap();
        fs::write(dir.join("album").join("b.bin"), vec![1u8; 12_000]).unwrap();
        fs::write(dir.join("other.txt"), b"not the torrent's").unwrap();
        assert_eq!(s.m.remove(&id, true), Ok(Vec::new()));
        assert!(
            !dir.join("album").exists(),
            "its files and its emptied folder"
        );
        assert!(dir.join("other.txt").exists());
        assert!(s.m.get(&id).is_none());
        assert!(!s.db.downloads().unwrap()[0].in_list);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn before_the_app_closes_every_torrents_resume_data_is_asked_for_and_saved() {
        let dir = folder("close");
        let mut s = setup(3);
        let id =
            s.m.add_torrent(request(&dir), AddOptions::default(), T0)
                .unwrap();
        s.m.pump(T0 + 1);
        // The helper answers save_resume with the data, as the real one does.
        let control = s.torrents.clone();
        let answer = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !control.log().contains(&"save_resume".to_owned()) {
                assert!(std::time::Instant::now() < deadline, "asked in time");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            control.resume_data(TorrentId(1), &resume_data());
        });
        s.m.prepare_to_close(std::time::Duration::from_secs(5), T0 + 2);
        answer.join().unwrap();
        assert!(s.db.torrent(&id).unwrap().unwrap().resume_data.is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_that_isnt_a_magnet_or_a_bad_policy_is_refused_with_nothing_stored() {
        let dir = folder("refused");
        let mut s = setup(3);
        let bad = TorrentRequest {
            origin: TorrentOrigin::Magnet {
                uri: "https://example.com/a.torrent".to_owned(),
            },
            ..request(&dir)
        };
        assert_eq!(
            s.m.add_torrent(bad, AddOptions::default(), T0).err(),
            Some(ErrorCode::InvalidUrl)
        );
        let policy = TorrentRequest {
            seeding: SeedPolicy::Time { minutes: 0 },
            ..request(&dir)
        };
        assert_eq!(
            s.m.add_torrent(policy, AddOptions::default(), T0).err(),
            Some(ErrorCode::InvalidInput)
        );
        assert!(s.db.downloads().unwrap().is_empty());
        assert!(s.torrents.log().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_snapshot_shows_the_helpers_health() {
        let s = setup(3);
        assert_eq!(s.m.snapshot().torrent_engine, EngineHealth::Running);
        s.torrents
            .set_health(EngineHealth::Restarting { attempt: 1 });
        assert_eq!(
            s.m.snapshot().torrent_engine,
            EngineHealth::Restarting { attempt: 1 }
        );
    }

    #[test]
    fn without_a_torrent_engine_a_torrent_waits_and_says_why() {
        let dir = folder("no-engine");
        let fake = FakeTransfer::new();
        let mut m = in_memory(fake);
        let id = m
            .add_torrent(request(&dir), AddOptions::default(), T0)
            .unwrap();
        m.pump(T0 + 1);
        assert_eq!(status(&m, &id), DownloadStatus::Queued);
        assert!(matches!(
            m.snapshot().torrent_engine,
            EngineHealth::Unavailable { .. }
        ));
        fs::remove_dir_all(&dir).unwrap();
    }
}

/// Categories and destinations in the manager (#31): a download's category, the folder and name
/// it is placed at, and what happens when the name is taken. The rules themselves are tested in
/// `destination` and `category`.
mod categories {
    use super::*;
    use crate::category::CategoryInput;
    use crate::destination::{Choice, FolderSettings, OnConflict, Placement};
    use crate::domain::DestinationCheck;
    use std::fs;

    /// A manager whose default folder is a fresh `dir`, with its database.
    fn in_folder(test: &str) -> (Manager, FakeControl, Arc<Database>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("v2-categories-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_section(
            &FolderSettings {
                default_directory: dir.to_string_lossy().into_owned(),
                ..FolderSettings::default()
            },
            T0,
        )
        .unwrap();
        let fake = FakeTransfer::new();
        let control = fake.control();
        let m = Manager::open(Box::new(fake), Arc::clone(&db)).unwrap();
        (m, control, db, dir)
    }

    fn with_category(category_id: &str) -> AddOptions {
        AddOptions {
            category_id: Some(category_id.to_owned()),
            ..AddOptions::default()
        }
    }

    fn stored_category(db: &Database, id: &str) -> Option<String> {
        db.downloads()
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .expect("stored")
            .category_id
    }

    fn typed(name: &str) -> Choice {
        Choice {
            file_name: Some(name.to_owned()),
            ..Choice::default()
        }
    }

    #[test]
    fn a_download_added_with_a_category_keeps_it_after_a_restart() {
        let (mut m, _control, db, dir) = in_folder("added");
        let id = m
            .add_with(DownloadKind::Http, spec(), with_category("video"), T0)
            .unwrap();
        assert_eq!(m.get(&id).unwrap().category_id.as_deref(), Some("video"));
        assert_eq!(stored_category(&db, &id).as_deref(), Some("video"));
        // An unknown category adds nothing.
        assert_eq!(
            m.add_with(DownloadKind::Http, spec(), with_category("c-404"), T0),
            Err(ErrorCode::NotFound)
        );
        assert_eq!(db.downloads().unwrap().len(), 1);
        drop(m);
        let m = Manager::open(Box::new(FakeTransfer::new()), Arc::clone(&db)).unwrap();
        assert_eq!(m.get(&id).unwrap().category_id.as_deref(), Some("video"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn changing_a_downloads_category_moves_nothing() {
        // V1's `set_download_category`; moving files is a non-goal of #31.
        let (mut m, _control, db, dir) = in_folder("set");
        let id = m.add(DownloadKind::Http, spec(), T0).unwrap();
        m.set_download_category(&id, Some("documents")).unwrap();
        let r = m.get(&id).unwrap();
        assert_eq!(r.category_id.as_deref(), Some("documents"));
        assert_eq!(r.directory, "downloads", "the folder is unchanged");
        assert_eq!(stored_category(&db, &id).as_deref(), Some("documents"));
        m.set_download_category(&id, None).unwrap();
        assert_eq!(m.get(&id).unwrap().category_id, None);
        assert_eq!(stored_category(&db, &id), None);
        assert_eq!(
            m.set_download_category(&id, Some("c-404")),
            Err(ErrorCode::NotFound)
        );
        assert_eq!(
            m.set_download_category("d-404", Some("video")),
            Err(ErrorCode::NotFound)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_deleted_categorys_downloads_are_left_without_one_and_still_save() {
        let (mut m, control, db, dir) = in_folder("deleted");
        let mine = m
            .create_category(&CategoryInput {
                name: "Mine".to_owned(),
                color: "#112233".to_owned(),
                icon: "folder".to_owned(),
                extensions: vec!["zip".to_owned()],
                directory: None,
            })
            .unwrap();
        assert_eq!(m.categories().unwrap().last(), Some(&mine));
        let id = m
            .add_with(DownloadKind::Http, spec(), with_category(&mine.id), T0)
            .unwrap();
        m.delete_category(&mine.id).unwrap();
        assert_eq!(m.get(&id).unwrap().category_id, None);
        // The next save of the record doesn't name the deleted category.
        m.pump(T0);
        control.complete(TransferId(1));
        m.pump(T0);
        assert_eq!(m.snapshot().storage_error, None);
        assert_eq!(stored_category(&db, &id), None);
        assert_eq!(
            m.delete_category("video"),
            Err(ErrorCode::InvalidState),
            "built in"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_learned_after_the_start_sets_the_automatic_category_and_keeps_the_folder() {
        // PO decision on #31: the server's name categorises the download; nothing is moved.
        let (mut m, control, db, dir) = in_folder("learned");
        let unnamed = || TransferSpec {
            urls: vec!["https://example.com/download.php?id=3".to_owned()],
            directory: dir.clone(),
            file_name: None,
            ..spec()
        };
        let id = m.add(DownloadKind::Http, unnamed(), T0).unwrap();
        m.pump(T0);
        assert_eq!(m.get(&id).unwrap().category_id, None);
        control.named(TransferId(1), "report.pdf");
        m.pump(T0);
        let r = m.get(&id).unwrap();
        assert_eq!(r.category_id.as_deref(), Some("documents"));
        assert_eq!(PathBuf::from(&r.directory), dir, "not moved");
        assert_eq!(stored_category(&db, &id).as_deref(), Some("documents"));

        // A chosen category stays.
        let chosen = m
            .add_with(DownloadKind::Http, unnamed(), with_category("music"), T0)
            .unwrap();
        m.pump(T0);
        control.named(TransferId(2), "report.pdf");
        m.pump(T0);
        assert_eq!(
            m.get(&chosen).unwrap().category_id.as_deref(),
            Some("music")
        );

        // With automatic categories off, none is set.
        db.save_section(
            &FolderSettings {
                auto_categorize: false,
                default_directory: dir.to_string_lossy().into_owned(),
                ..FolderSettings::default()
            },
            T0,
        )
        .unwrap();
        let off = m.add(DownloadKind::Http, unnamed(), T0).unwrap();
        m.pump(T0);
        control.named(TransferId(3), "report.pdf");
        m.pump(T0);
        assert_eq!(m.get(&off).unwrap().category_id, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_destination_check_shows_what_adding_would_do() {
        let (m, _control, _db, dir) = in_folder("check");
        let url = "https://example.com/files/report.pdf";
        assert_eq!(
            m.check_destination(url, &Choice::default()),
            Ok(DestinationCheck {
                directory: dir.join("Documents").to_string_lossy().into_owned(),
                directory_exists: false,
                automatic_directory: true,
                category_id: Some("documents".to_owned()),
                file_name: Some("report.pdf".to_owned()),
                file_exists: false,
                partial_download_exists: false,
                suggested_name: None,
            })
        );
        // A typed name keeps the link's file type (V1 #46).
        let check = m.check_destination(url, &typed("Annual report")).unwrap();
        assert_eq!(check.file_name.as_deref(), Some("Annual report.pdf"));
        // A link the server names: no name, no category, the default folder.
        let check = m
            .check_destination("https://example.com/download.php?id=3", &Choice::default())
            .unwrap();
        assert_eq!(
            (
                check.file_name,
                check.category_id,
                PathBuf::from(check.directory)
            ),
            (None, None, dir.clone())
        );
        assert_eq!(
            m.check_destination(url, &typed("a/b")),
            Err(ErrorCode::InvalidFilename)
        );
        let long = format!(
            "https://example.com/{}.pdf",
            "x".repeat(crate::request::MAX_URL_BYTES)
        );
        assert_eq!(
            m.check_destination(&long, &Choice::default()),
            Err(ErrorCode::InvalidUrl)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn placing_creates_the_category_subfolder_and_nothing_else() {
        let (m, _control, _db, dir) = in_folder("subfolder");
        let url = "https://example.com/files/ubuntu.iso";
        let Ok(Placement::Placed(place)) = m.place(url, &Choice::default(), OnConflict::Ask) else {
            panic!("placed")
        };
        assert_eq!(place.directory, dir.join("Disk images"));
        assert!(place.directory.is_dir(), "created");
        // A chosen folder must exist: it isn't created.
        let missing = dir.join("missing");
        let chosen = Choice {
            directory: Some(missing.to_string_lossy().into_owned()),
            ..Choice::default()
        };
        assert_eq!(
            m.place(url, &chosen, OnConflict::Ask),
            Err(ErrorCode::DestinationMissing)
        );
        assert!(!missing.exists());
        // Nor is a missing default folder.
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            m.place(url, &Choice::default(), OnConflict::Ask),
            Err(ErrorCode::DestinationMissing)
        );
        assert!(!dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_category_subfolder_is_refused() {
        let (m, _control, _db, dir) = in_folder("link");
        let elsewhere = dir.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("Disk images")).unwrap();
        assert_eq!(
            m.place(
                "https://example.com/files/ubuntu.iso",
                &Choice::default(),
                OnConflict::Ask
            ),
            Err(ErrorCode::InvalidPath)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_taken_name_is_asked_about_or_renamed_never_replaced() {
        // PO decision on #31: ask or rename only.
        let (mut m, _control, _db, dir) = in_folder("conflict");
        let documents = dir.join("Documents");
        fs::create_dir_all(&documents).unwrap();
        fs::write(documents.join("report.pdf"), b"keep me").unwrap();
        let url = "https://example.com/files/report.pdf";

        let Ok(Placement::Conflict(check)) = m.place(url, &Choice::default(), OnConflict::Ask)
        else {
            panic!("asked")
        };
        assert!(check.file_exists);
        assert_eq!(check.suggested_name.as_deref(), Some("report (1).pdf"));

        let Ok(Placement::Placed(place)) = m.place(url, &Choice::default(), OnConflict::Rename)
        else {
            panic!("renamed")
        };
        assert_eq!(place.directory, documents);
        assert_eq!(place.file_name.as_deref(), Some("report (1).pdf"));
        assert_eq!(place.category_id.as_deref(), Some("documents"));
        assert_eq!(fs::read(documents.join("report.pdf")).unwrap(), b"keep me");

        // A download waiting in the list takes its name too, before it has any file.
        let waiting = TransferSpec {
            directory: documents.clone(),
            file_name: Some("report (1).pdf".to_owned()),
            ..spec()
        };
        m.add_with(
            DownloadKind::Http,
            waiting,
            AddOptions {
                start_paused: true,
                ..AddOptions::default()
            },
            T0,
        )
        .unwrap();
        let Ok(Placement::Placed(place)) = m.place(url, &Choice::default(), OnConflict::Rename)
        else {
            panic!("renamed")
        };
        assert_eq!(place.file_name.as_deref(), Some("report (2).pdf"));

        // A free name is placed as it is, whatever the choice.
        let Ok(Placement::Placed(place)) = m.place(url, &typed("Fresh"), OnConflict::Ask) else {
            panic!("placed")
        };
        assert_eq!(place.file_name.as_deref(), Some("Fresh.pdf"));
        fs::remove_dir_all(&dir).unwrap();
    }
}

mod adding {
    //! The Add dialog's request (#60): a result for each link, conflicts, duplicates, a chosen
    //! folder, and the options that reach the engines.
    use super::*;
    use crate::destination::{FolderSettings, OnConflict};
    use crate::manager::adding::{AddOutcome, AddRequest, Duplicate, MAX_ADD};
    use crate::torrent::bencode::{encode, Value};
    use crate::torrent::tests::{file, multi, sha1_hex, torrent};
    use crate::torrent_engine::fake::{FakeTorrentControl, FakeTorrentEngine};
    use crate::torrent_engine::TorrentId;
    use crate::transfer::{Checksum, Credentials, HashAlgorithm, Header, SshKey};
    use std::fs;
    use std::path::Path;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct Setup {
        m: Manager,
        direct: FakeControl,
        torrents: FakeTorrentControl,
        db: Arc<Database>,
        dir: PathBuf,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A manager with both engines, whose default folder is a fresh folder.
    fn setup(test: &str) -> Setup {
        let dir = std::env::temp_dir().join(format!("v2-adding-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        // Downloads go in the folder itself: category subfolders are #31's tests.
        db.save_section(
            &FolderSettings {
                default_directory: dir.to_string_lossy().into_owned(),
                category_subfolders: false,
                ..FolderSettings::default()
            },
            T0,
        )
        .unwrap();
        let fake = FakeTransfer::with_capabilities(with_sftp());
        let direct = fake.control();
        let lt = FakeTorrentEngine::new();
        let torrents = lt.control();
        let m = Manager::open(Box::new(fake), Arc::clone(&db))
            .unwrap()
            .with_torrents(Box::new(lt));
        Setup {
            m,
            direct,
            torrents,
            db,
            dir,
        }
    }

    /// The sample engine, which downloads over SFTP too.
    fn with_sftp() -> Capabilities {
        let mut caps = Capabilities::sample();
        let mut sftp = caps.protocols[0];
        sftp.kind = DownloadKind::Sftp;
        caps.protocols.push(sftp);
        caps
    }

    fn links(links: &[&str]) -> AddRequest {
        AddRequest {
            links: links.iter().map(|l| (*l).to_owned()).collect(),
            ..AddRequest::default()
        }
    }

    fn added(outcome: &AddOutcome) -> String {
        match outcome {
            AddOutcome::Added { id } => id.clone(),
            other => panic!("not added: {other:?}"),
        }
    }

    /// The album, with a video as its largest file, written as a `.torrent` in `dir`; and its v1
    /// info hash.
    fn album_file(dir: &Path) -> (PathBuf, String) {
        let info = multi(
            "album",
            vec![file(&["clip.mp4"], 20_000), file(&["notes.txt"], 12_000)],
            16_384,
            2,
        );
        let hash = sha1_hex(&encode(&info));
        let bytes = torrent(
            info,
            vec![("announce", Value::str("https://tracker.example/announce"))],
        );
        let path = dir.join("album.torrent");
        fs::write(&path, bytes).unwrap();
        (path, hash)
    }

    #[test]
    fn each_link_gets_its_own_result_in_the_order_given() {
        let mut s = setup("each");
        let results =
            s.m.add_request(
                &links(&[
                    "https://example.com/a.zip",
                    "file:///etc/passwd",
                    "ftp://example.com/b.iso",
                ]),
                T0,
            )
            .unwrap();
        let sources: Vec<&str> = results.iter().map(|r| r.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                "https://example.com/a.zip",
                "file:///etc/passwd",
                "ftp://example.com/b.iso"
            ]
        );
        let a = added(&results[0].outcome);
        assert_eq!(
            results[1].outcome,
            AddOutcome::Failed {
                code: ErrorCode::UnsupportedScheme
            }
        );
        let b = added(&results[2].outcome);
        assert_eq!(s.m.get(&a).unwrap().kind, DownloadKind::Http);
        assert_eq!(s.m.get(&b).unwrap().kind, DownloadKind::Ftp);
        let folder = s.dir.to_string_lossy().into_owned();
        assert_eq!(s.m.get(&a).unwrap().directory, folder);
        assert_eq!(s.m.get(&a).unwrap().file_name.as_deref(), Some("a.zip"));
        assert_eq!(s.db.downloads().unwrap().len(), 2);
    }

    #[test]
    fn sftp_links_are_added_with_their_key_login() {
        let mut s = setup("sftp");
        let key = SshKey {
            path: s.dir.join("id_ed25519"),
            passphrase: None,
        };
        let request = AddRequest {
            credentials: Some(Credentials {
                user_name: "me".to_owned(),
                password: String::new(),
            }),
            ssh_key: Some(key.clone()),
            ..links(&["sftp://files.example/pub/a.tar"])
        };
        let results = s.m.add_request(&request, T0).unwrap();
        let id = added(&results[0].outcome);
        assert_eq!(s.m.get(&id).unwrap().kind, DownloadKind::Sftp);
        s.m.pump(T0);
        assert_eq!(s.direct.spec(TransferId(1)).ssh_key, Some(key));
    }

    #[test]
    fn a_taken_name_is_asked_about_and_then_renamed_never_replaced() {
        let mut s = setup("taken");
        fs::write(s.dir.join("a.zip"), b"mine").unwrap();
        let ask = links(&["https://example.com/a.zip"]);
        let results = s.m.add_request(&ask, T0).unwrap();
        let AddOutcome::Conflict { check } = &results[0].outcome else {
            panic!("{:?}", results[0].outcome);
        };
        assert!(check.file_exists);
        assert_eq!(check.suggested_name.as_deref(), Some("a (1).zip"));
        assert!(s.db.downloads().unwrap().is_empty(), "nothing added");
        let rename = AddRequest {
            on_conflict: OnConflict::Rename,
            ..ask
        };
        let results = s.m.add_request(&rename, T0).unwrap();
        let id = added(&results[0].outcome);
        assert_eq!(
            s.m.get(&id).unwrap().file_name.as_deref(),
            Some("a (1).zip")
        );
        assert_eq!(fs::read(s.dir.join("a.zip")).unwrap(), b"mine");
    }

    #[test]
    fn a_link_added_before_is_a_duplicate_that_can_be_added_anyway() {
        let mut s = setup("dup-link");
        let first =
            s.m.add_request(&links(&["https://example.com/a.zip"]), T0)
                .unwrap();
        let first = added(&first[0].outcome);
        let again =
            s.m.add_request(&links(&["https://example.com/a.zip"]), T0)
                .unwrap();
        assert_eq!(
            again[0].outcome,
            AddOutcome::Duplicate {
                duplicate: Duplicate {
                    existing_id: first.clone(),
                    name: Some("a.zip".to_owned()),
                    status: DownloadStatus::Queued,
                    in_list: true,
                    can_add_anyway: true,
                }
            }
        );
        let anyway = AddRequest {
            allow_duplicate: true,
            on_conflict: OnConflict::Rename,
            ..links(&["https://example.com/a.zip"])
        };
        let results = s.m.add_request(&anyway, T0).unwrap();
        let second = added(&results[0].outcome);
        assert_ne!(first, second);
    }

    #[test]
    fn a_link_only_in_history_is_still_a_duplicate() {
        let mut s = setup("dup-history");
        let first =
            s.m.add_request(&links(&["https://example.com/a.zip"]), T0)
                .unwrap();
        let first = added(&first[0].outcome);
        s.m.pump(T0);
        s.direct.complete(TransferId(1));
        s.m.pump(T0);
        s.m.remove_many(std::slice::from_ref(&first), false)
            .unwrap();
        let again =
            s.m.add_request(&links(&["https://example.com/a.zip"]), T0)
                .unwrap();
        let AddOutcome::Duplicate { duplicate } = &again[0].outcome else {
            panic!("{:?}", again[0].outcome);
        };
        assert_eq!(duplicate.existing_id, first);
        assert!(!duplicate.in_list);
        assert!(duplicate.can_add_anyway);
    }

    #[test]
    fn a_torrent_still_in_the_list_is_never_added_twice() {
        // V1's rule: two copies of one torrent can't run.
        let mut s = setup("dup-magnet");
        let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=album");
        let first = s.m.add_request(&links(&[&magnet]), T0).unwrap();
        let first = added(&first[0].outcome);
        // The same torrent by another spelling, even when the user says "Add anyway".
        let upper = format!("magnet:?xt=urn:btih:{}", HASH.to_uppercase());
        let anyway = AddRequest {
            allow_duplicate: true,
            ..links(&[&upper])
        };
        let again = s.m.add_request(&anyway, T0).unwrap();
        let AddOutcome::Duplicate { duplicate } = &again[0].outcome else {
            panic!("{:?}", again[0].outcome);
        };
        assert_eq!(duplicate.existing_id, first);
        assert!(duplicate.in_list);
        assert!(!duplicate.can_add_anyway);
        assert_eq!(s.torrents.log().len(), 1, "one torrent given to the helper");
    }

    #[test]
    fn a_torrent_only_in_history_can_be_added_anyway() {
        let mut s = setup("dup-torrent-history");
        let magnet = format!("magnet:?xt=urn:btih:{HASH}&dn=album");
        let first = s.m.add_request(&links(&[&magnet]), T0).unwrap();
        let first = added(&first[0].outcome);
        s.m.remove_many(std::slice::from_ref(&first), false)
            .unwrap();
        let again = s.m.add_request(&links(&[&magnet]), T0).unwrap();
        let AddOutcome::Duplicate { duplicate } = &again[0].outcome else {
            panic!("{:?}", again[0].outcome);
        };
        assert!(!duplicate.in_list);
        assert!(duplicate.can_add_anyway);
        let anyway = AddRequest {
            allow_duplicate: true,
            ..links(&[&magnet])
        };
        let results = s.m.add_request(&anyway, T0).unwrap();
        assert_ne!(added(&results[0].outcome), first);
    }

    #[test]
    fn a_torrent_file_is_found_by_its_info_hash_and_categorised_by_its_largest_file() {
        let mut s = setup("torrent-file");
        let source = s.dir.join("from");
        fs::create_dir_all(&source).unwrap();
        let (path, hash) = album_file(&source);
        let request = AddRequest {
            torrent_files: vec![path.to_string_lossy().into_owned()],
            ..AddRequest::default()
        };
        let results = s.m.add_request(&request, T0).unwrap();
        assert_eq!(results[0].source, path.to_string_lossy());
        let id = added(&results[0].outcome);
        let record = s.m.get(&id).unwrap();
        assert_eq!(record.kind, DownloadKind::Torrent);
        assert_eq!(record.category_id.as_deref(), Some("video"));
        // The same torrent as a magnet link is a duplicate.
        let magnet = format!("magnet:?xt=urn:btih:{hash}");
        let again = s.m.add_request(&links(&[&magnet]), T0).unwrap();
        assert!(
            matches!(&again[0].outcome, AddOutcome::Duplicate { duplicate } if duplicate.existing_id == id),
            "{:?}",
            again[0].outcome
        );
    }

    #[test]
    fn a_torrent_whose_name_is_taken_is_asked_about_and_never_renamed() {
        let mut s = setup("torrent-taken");
        let source = s.dir.join("from");
        fs::create_dir_all(&source).unwrap();
        let (path, _) = album_file(&source);
        let target = s.dir.join("target");
        fs::create_dir_all(target.join("album")).unwrap();
        let request = AddRequest {
            torrent_files: vec![path.to_string_lossy().into_owned()],
            directory: Some(target.to_string_lossy().into_owned()),
            on_conflict: OnConflict::Rename,
            ..AddRequest::default()
        };
        let results = s.m.add_request(&request, T0).unwrap();
        let AddOutcome::Conflict { check } = &results[0].outcome else {
            panic!("{:?}", results[0].outcome);
        };
        assert_eq!(check.file_name.as_deref(), Some("album"));
        assert!(check.file_exists);
        assert_eq!(
            check.suggested_name, None,
            "a torrent's files can't be renamed"
        );
        assert!(s.torrents.log().is_empty());
    }

    #[test]
    fn a_torrent_file_must_be_a_readable_torrent_file() {
        let mut s = setup("torrent-bad");
        fs::write(s.dir.join("junk.torrent"), b"not bencode").unwrap();
        let request = AddRequest {
            torrent_files: vec![
                "relative.torrent".to_owned(),
                s.dir.join("missing.torrent").to_string_lossy().into_owned(),
                s.dir.join("junk.torrent").to_string_lossy().into_owned(),
            ],
            ..AddRequest::default()
        };
        let results = s.m.add_request(&request, T0).unwrap();
        let outcomes: Vec<&AddOutcome> = results.iter().map(|r| &r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                &AddOutcome::Failed {
                    code: ErrorCode::InvalidPath
                },
                &AddOutcome::Failed {
                    code: ErrorCode::NotFound
                },
                &AddOutcome::Failed {
                    code: ErrorCode::InvalidInput
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_torrent_file_behind_a_link_is_refused() {
        let mut s = setup("torrent-link");
        let source = s.dir.join("from");
        fs::create_dir_all(&source).unwrap();
        let (path, _) = album_file(&source);
        let link = s.dir.join("link.torrent");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let request = AddRequest {
            torrent_files: vec![link.to_string_lossy().into_owned()],
            ..AddRequest::default()
        };
        let results = s.m.add_request(&request, T0).unwrap();
        assert_eq!(
            results[0].outcome,
            AddOutcome::Failed {
                code: ErrorCode::InvalidPath
            }
        );
    }

    #[test]
    fn a_missing_chosen_folder_is_created_only_when_the_user_agrees() {
        let mut s = setup("create");
        let chosen = s.dir.join("new").join("deeper");
        let request = AddRequest {
            directory: Some(chosen.to_string_lossy().into_owned()),
            ..links(&["https://example.com/a.zip"])
        };
        let results = s.m.add_request(&request, T0).unwrap();
        assert_eq!(
            results[0].outcome,
            AddOutcome::Failed {
                code: ErrorCode::DestinationMissing
            }
        );
        assert!(!chosen.exists());
        let create = AddRequest {
            create_directory: true,
            ..request
        };
        let results = s.m.add_request(&create, T0).unwrap();
        let id = added(&results[0].outcome);
        assert!(chosen.is_dir());
        assert_eq!(s.m.get(&id).unwrap().directory, chosen.to_string_lossy());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_is_never_created_through_a_link() {
        let mut s = setup("create-link");
        let outside = s.dir.join("outside");
        std::os::unix::fs::symlink(&outside, s.dir.join("dangling")).unwrap();
        let request = AddRequest {
            directory: Some(
                s.dir
                    .join("dangling")
                    .join("x")
                    .to_string_lossy()
                    .into_owned(),
            ),
            create_directory: true,
            ..links(&["https://example.com/a.zip"])
        };
        assert_eq!(s.m.add_request(&request, T0), Err(ErrorCode::InvalidPath));
        assert!(!outside.exists());
    }

    #[test]
    fn a_request_is_refused_whole_when_it_breaks_the_rules() {
        let mut s = setup("refused");
        assert_eq!(
            s.m.add_request(&AddRequest::default(), T0),
            Err(ErrorCode::InvalidInput),
            "nothing to add"
        );
        let named_two = AddRequest {
            file_name: Some("x.zip".to_owned()),
            ..links(&["https://example.com/a.zip", "https://example.com/b.zip"])
        };
        assert_eq!(
            s.m.add_request(&named_two, T0),
            Err(ErrorCode::InvalidInput),
            "a name is for one link"
        );
        let many: Vec<String> = (0..=MAX_ADD)
            .map(|i| format!("https://example.com/{i}.zip"))
            .collect();
        let too_many = AddRequest {
            links: many,
            ..AddRequest::default()
        };
        assert_eq!(s.m.add_request(&too_many, T0), Err(ErrorCode::InvalidInput));
        let bad_header = AddRequest {
            headers: vec![Header {
                name: "Bad\nName".to_owned(),
                value: "x".to_owned(),
            }],
            ..links(&["https://example.com/a.zip"])
        };
        assert_eq!(
            s.m.add_request(&bad_header, T0),
            Err(ErrorCode::InvalidInput)
        );
        let bad_checksum = AddRequest {
            checksum: Some(Checksum {
                algorithm: HashAlgorithm::Sha256,
                hex: "xyz".to_owned(),
            }),
            ..links(&["https://example.com/a.zip"])
        };
        assert_eq!(
            s.m.add_request(&bad_checksum, T0),
            Err(ErrorCode::InvalidInput)
        );
        let login_in_link = links(&["https://me:secret@example.com/a.zip"]);
        let results = s.m.add_request(&login_in_link, T0).unwrap();
        assert_eq!(
            results[0].outcome,
            AddOutcome::Failed {
                code: ErrorCode::InvalidUrl
            }
        );
        assert!(s.db.downloads().unwrap().is_empty());
    }

    #[test]
    fn the_dialogs_options_reach_the_engine() {
        let mut s = setup("options");
        let digest = "ab".repeat(32);
        let request = AddRequest {
            file_name: Some("Annual report".to_owned()),
            connections: Some(2),
            speed_limit_kib: 64,
            headers: vec![Header {
                name: "Cookie".to_owned(),
                value: "s=1".to_owned(),
            }],
            referrer: Some("https://example.com/page".to_owned()),
            credentials: Some(Credentials {
                user_name: "me".to_owned(),
                password: "pw".to_owned(),
            }),
            checksum: Some(Checksum {
                algorithm: HashAlgorithm::Sha256,
                hex: digest.to_uppercase(),
            }),
            require_encryption: true,
            ..links(&["ftp://example.com/docs/report.pdf"])
        };
        let results = s.m.add_request(&request, T0).unwrap();
        let id = added(&results[0].outcome);
        assert_eq!(
            s.m.get(&id).unwrap().file_name.as_deref(),
            Some("Annual report.pdf")
        );
        s.m.pump(T0);
        let spec = s.direct.spec(TransferId(1));
        assert_eq!(spec.max_connections, 2);
        assert_eq!(spec.limits.max_download_bps, Some(64 * 1024));
        assert_eq!(spec.headers.len(), 1);
        assert_eq!(spec.referrer.as_deref(), Some("https://example.com/page"));
        assert_eq!(spec.credentials.unwrap().user_name, "me");
        assert_eq!(spec.checksum.unwrap().hex, digest, "in lowercase");
        assert!(spec.require_encryption);
    }

    #[test]
    fn add_paused_and_choose_files_are_kept() {
        let mut s = setup("paused");
        let magnet = format!("magnet:?xt=urn:btih:{HASH}");
        let request = AddRequest {
            start_paused: true,
            choose_files: true,
            ..links(&["https://example.com/a.zip", &magnet])
        };
        let results = s.m.add_request(&request, T0).unwrap();
        for r in &results {
            let id = added(&r.outcome);
            assert_eq!(s.m.get(&id).unwrap().status, DownloadStatus::Paused);
        }
        assert!(s.direct.log().is_empty());
        let magnet_id = added(&results[1].outcome);
        s.m.resume(&magnet_id).unwrap();
        s.m.pump(T0);
        assert!(s.torrents.spec(TorrentId(1)).choose_files);
    }

    #[test]
    fn a_protocol_the_engine_lacks_is_refused_with_why_before_any_folder_is_made() {
        let s = setup("lacks");
        let mut caps = Capabilities::sample();
        caps.protocols.retain(|p| p.kind != DownloadKind::Sftp);
        // Category subfolders on: placing the link would create one.
        s.db.save_section(
            &FolderSettings {
                default_directory: s.dir.to_string_lossy().into_owned(),
                ..FolderSettings::default()
            },
            T0,
        )
        .unwrap();
        let mut m = Manager::open(
            Box::new(FakeTransfer::with_capabilities(caps)),
            Arc::clone(&s.db),
        )
        .unwrap();
        let results = m
            .add_request(&links(&["sftp://files.example/a.zip"]), T0)
            .unwrap();
        assert_eq!(
            results[0].outcome,
            AddOutcome::Failed {
                code: ErrorCode::Unsupported
            }
        );
        assert_eq!(
            fs::read_dir(&s.dir).unwrap().count(),
            0,
            "no category subfolder was made for it"
        );
    }

    #[test]
    fn duplicates_in_one_request_are_caught_too() {
        let mut s = setup("same-request");
        let results =
            s.m.add_request(
                &links(&["https://example.com/a.zip", "https://example.com/a.zip"]),
                T0,
            )
            .unwrap();
        added(&results[0].outcome);
        assert!(matches!(results[1].outcome, AddOutcome::Duplicate { .. }));
    }
}

mod torrent_links {
    //! `.torrent` links (PO decision on #60): fetched into the app's folder, kept without their
    //! trackers, then added from there.
    use super::*;
    use crate::destination::FolderSettings;
    use crate::manager::adding::{AddOutcome, AddRequest};
    use crate::naming::PART_FILE_SUFFIX;
    use crate::torrent::bencode::Value;
    use crate::torrent::tests::{file, multi, torrent};
    use crate::torrent::MAX_TORRENT_BYTES;
    use crate::torrent_engine::fake::{FakeTorrentControl, FakeTorrentEngine};
    use crate::torrent_engine::TorrentId;
    use std::fs;
    use std::sync::mpsc::{channel, Receiver};

    const LINK: &str = "https://tracker.example/files/album.torrent";

    struct Setup {
        m: Manager,
        direct: FakeControl,
        torrents: FakeTorrentControl,
        db: Arc<Database>,
        /// The app's folder for fetched `.torrent` files.
        links: PathBuf,
        dir: PathBuf,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("v2-links-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("downloads")).unwrap();
        dir
    }

    fn open(dir: &std::path::Path, db: &Arc<Database>) -> Setup {
        let fake = FakeTransfer::new();
        let direct = fake.control();
        let lt = FakeTorrentEngine::new();
        let torrents = lt.control();
        let links = dir.join("torrent-links");
        let m = Manager::open(Box::new(fake), Arc::clone(db))
            .unwrap()
            .with_torrents(Box::new(lt))
            .with_torrent_links(links.clone())
            .unwrap();
        Setup {
            m,
            direct,
            torrents,
            db: Arc::clone(db),
            links,
            dir: dir.to_path_buf(),
        }
    }

    fn setup(test: &str) -> Setup {
        let dir = folder(test);
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_section(
            &FolderSettings {
                default_directory: dir.join("downloads").to_string_lossy().into_owned(),
                category_subfolders: false,
                ..FolderSettings::default()
            },
            T0,
        )
        .unwrap();
        open(&dir, &db)
    }

    fn album_bytes() -> Vec<u8> {
        let info = multi(
            "album",
            vec![file(&["clip.mp4"], 20_000), file(&["notes.txt"], 12_000)],
            16_384,
            2,
        );
        torrent(
            info,
            vec![(
                "announce",
                Value::str("https://tracker.example/announce?passkey=secret-key"),
            )],
        )
    }

    fn fetch(s: &mut Setup, link: &str) -> Receiver<Result<PathBuf, AddOutcome>> {
        let (reply, answer) = channel();
        s.m.fetch_torrent_link(link, &AddRequest::default(), reply)
            .unwrap();
        answer
    }

    /// The engine writes the fetched file and reports it complete.
    fn fetched(s: &mut Setup, bytes: &[u8]) {
        let spec = s.direct.spec(TransferId(1));
        fs::write(spec.directory.join(spec.file_name.unwrap()), bytes).unwrap();
        s.direct
            .progress(TransferId(1), 0, Some(bytes.len() as u64));
        s.direct.complete(TransferId(1));
        s.m.pump(T0);
    }

    #[test]
    fn a_torrent_link_is_fetched_into_the_apps_folder_and_kept_without_its_trackers() {
        let mut s = setup("fetched");
        let answer = fetch(&mut s, LINK);
        let spec = s.direct.spec(TransferId(1));
        assert_eq!(spec.urls, [LINK]);
        assert_eq!(spec.directory, s.links);
        assert_eq!(spec.max_connections, 1);
        assert!(
            s.m.snapshot().downloads.is_empty(),
            "not a download in the list"
        );
        fetched(&mut s, &album_bytes());
        let kept = answer.try_recv().unwrap().unwrap();
        assert!(kept.starts_with(&s.links), "{kept:?}");
        let text = String::from_utf8_lossy(&fs::read(&kept).unwrap()).into_owned();
        assert!(!text.contains("secret-key"), "no tracker is kept: {text}");
        let names: Vec<_> = fs::read_dir(&s.links)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "only the kept file: {names:?}");
        assert!(s.direct.log().contains(&"remove 1".to_owned()));
        // Added from there, with its trackers in memory.
        let request = AddRequest {
            torrent_files: vec![kept.to_string_lossy().into_owned()],
            ..AddRequest::default()
        };
        let results = s.m.add_request(&request, T0).unwrap();
        assert!(matches!(results[0].outcome, AddOutcome::Added { .. }));
        s.m.pump(T0);
        assert_eq!(
            s.torrents.spec(TorrentId(1)).trackers,
            ["https://tracker.example/announce?passkey=secret-key"]
        );
    }

    #[test]
    fn a_fetch_larger_than_a_torrent_can_be_is_cancelled() {
        let mut s = setup("large");
        let answer = fetch(&mut s, LINK);
        s.direct
            .progress(TransferId(1), 0, Some(MAX_TORRENT_BYTES as u64 + 1));
        s.m.pump(T0);
        assert_eq!(
            answer.try_recv().unwrap(),
            Err(AddOutcome::Failed {
                code: ErrorCode::InvalidInput
            })
        );
        assert!(s.direct.log().contains(&"cancel 1".to_owned()));
    }

    #[test]
    fn a_failed_fetch_says_why() {
        let mut s = setup("failed");
        let answer = fetch(&mut s, LINK);
        let error = TransferError::new(FailureClass::Permanent, Some(404), "Not found");
        s.direct.fail(TransferId(1), error.clone());
        s.m.pump(T0);
        assert_eq!(
            answer.try_recv().unwrap(),
            Err(AddOutcome::NotFetched { error })
        );
    }

    #[test]
    fn what_was_fetched_must_be_a_torrent() {
        let mut s = setup("junk");
        let answer = fetch(&mut s, LINK);
        fetched(&mut s, b"<html>sign in</html>");
        assert_eq!(
            answer.try_recv().unwrap(),
            Err(AddOutcome::Failed {
                code: ErrorCode::InvalidInput
            })
        );
        assert_eq!(fs::read_dir(&s.links).unwrap().count(), 0, "nothing kept");
    }

    #[test]
    fn a_fetch_given_up_on_is_cancelled() {
        let mut s = setup("given-up");
        let (reply, answer) = channel();
        let fetch =
            s.m.fetch_torrent_link(LINK, &AddRequest::default(), reply)
                .unwrap();
        s.m.cancel_torrent_link(fetch);
        s.m.pump(T0);
        assert!(s.direct.log().contains(&"cancel 1".to_owned()));
        assert!(answer.try_recv().is_err(), "no answer is waited for");
    }

    #[test]
    fn only_http_and_ftp_links_without_a_login_are_fetched() {
        let mut s = setup("refused");
        for (link, code) in [
            ("sftp://host/a.torrent", ErrorCode::UnsupportedScheme),
            ("file:///a.torrent", ErrorCode::UnsupportedScheme),
            ("https://me:pw@host/a.torrent", ErrorCode::InvalidUrl),
        ] {
            let (reply, _answer) = channel();
            assert_eq!(
                s.m.fetch_torrent_link(link, &AddRequest::default(), reply),
                Err(code),
                "{link}"
            );
        }
        assert!(s.direct.log().is_empty());
    }

    #[test]
    fn leftovers_are_cleared_when_the_app_starts_and_kept_torrents_stay() {
        let mut s = setup("leftovers");
        let answer = fetch(&mut s, LINK);
        fetched(&mut s, &album_bytes());
        let kept = answer.try_recv().unwrap().unwrap();
        let request = AddRequest {
            torrent_files: vec![kept.to_string_lossy().into_owned()],
            ..AddRequest::default()
        };
        s.m.add_request(&request, T0).unwrap();
        // A fetch the app quit during, and a fetched torrent that was never added.
        fs::write(
            s.links.join(format!("fetch-9.torrent{PART_FILE_SUFFIX}")),
            b"x",
        )
        .unwrap();
        fs::write(s.links.join("ffff.torrent"), b"x").unwrap();
        fs::create_dir(s.links.join("a folder")).unwrap();
        let (dir, db) = (s.dir.clone(), Arc::clone(&s.db));
        std::mem::forget(s);
        let s = open(&dir, &db);
        let mut names: Vec<String> = fs::read_dir(&s.links)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let kept = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(names, ["a folder".to_owned(), kept]);
    }
}

mod adding_through_the_service {
    //! What the app's `add_downloads` command runs (#60): `.torrent` links fetched first, then
    //! everything added, with the results in the request's order.
    use super::*;
    use crate::destination::FolderSettings;
    use crate::manager::adding::{is_torrent_link, AddOutcome, AddRequest};
    use crate::manager::ManagerService;
    use crate::torrent::bencode::Value;
    use crate::torrent::tests::{file, multi, torrent};
    use crate::torrent_engine::fake::FakeTorrentEngine;
    use std::fs;
    use std::time::{Duration, Instant};

    const LINK: &str = "https://tracker.example/files/album.torrent?key=1";

    fn service(test: &str) -> (ManagerService, FakeControl, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("v2-service-add-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("downloads")).unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_section(
            &FolderSettings {
                default_directory: dir.join("downloads").to_string_lossy().into_owned(),
                category_subfolders: false,
                ..FolderSettings::default()
            },
            T0,
        )
        .unwrap();
        let fake = FakeTransfer::new();
        let direct = fake.control();
        let m = Manager::open(Box::new(fake), db)
            .unwrap()
            .with_torrents(Box::new(FakeTorrentEngine::new()))
            .with_torrent_links(dir.join("torrent-links"))
            .unwrap();
        (ManagerService::start(m, |_| {}), direct, dir)
    }

    /// Waits until the engine was given `n` transfers.
    fn wait_for_adds(direct: &FakeControl, n: usize) {
        let until = Instant::now() + Duration::from_secs(10);
        while direct.log().iter().filter(|l| *l == "add").count() < n {
            assert!(Instant::now() < until, "{:?}", direct.log());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn torrent_links_are_told_apart_as_v1_did() {
        assert!(is_torrent_link("https://e.com/a/album.TORRENT"));
        assert!(is_torrent_link("http://e.com/album.torrent?x=1#y"));
        assert!(!is_torrent_link(
            "https://e.com/download.php?f=album.torrent"
        ));
        assert!(!is_torrent_link("ftp://e.com/album.torrent"));
        assert!(!is_torrent_link("magnet:?xt=urn:btih:aa"));
        assert!(!is_torrent_link("https://e.com/album.torrent.zip"));
    }

    #[test]
    fn a_torrent_link_is_fetched_then_added_with_every_result_in_order() {
        let (service, direct, dir) = service("ordered");
        let request = AddRequest {
            links: vec![
                "https://example.com/a.zip".to_owned(),
                LINK.to_owned(),
                "file:///etc/passwd".to_owned(),
            ],
            ..AddRequest::default()
        };
        let adding = std::thread::spawn(move || {
            let results = service.add(request, Duration::from_secs(10));
            (service, results)
        });
        wait_for_adds(&direct, 1);
        let spec = direct.spec(TransferId(1));
        assert_eq!(spec.urls, [LINK], "the link is fetched first");
        let bytes = torrent(
            multi("album", vec![file(&["a.mp4"], 20_000)], 16_384, 2),
            vec![("announce", Value::str("https://tracker.example/announce"))],
        );
        fs::write(spec.directory.join(spec.file_name.unwrap()), bytes).unwrap();
        direct.complete(TransferId(1));
        let (service, results) = adding.join().unwrap();
        let results = results.unwrap();
        let sources: Vec<&str> = results.iter().map(|r| r.source.as_str()).collect();
        assert_eq!(
            sources,
            ["https://example.com/a.zip", LINK, "file:///etc/passwd"]
        );
        assert!(matches!(results[0].outcome, AddOutcome::Added { .. }));
        let AddOutcome::Added { id } = &results[1].outcome else {
            panic!("{:?}", results[1].outcome);
        };
        let id = id.clone();
        let kind = service.call(move |m| m.get(&id).unwrap().kind).unwrap();
        assert_eq!(kind, DownloadKind::Torrent);
        assert_eq!(
            results[2].outcome,
            AddOutcome::Failed {
                code: ErrorCode::UnsupportedScheme
            }
        );
        drop(service);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fetch_that_takes_too_long_is_cancelled_and_says_so() {
        let (service, direct, dir) = service("slow");
        let request = AddRequest {
            links: vec![LINK.to_owned()],
            ..AddRequest::default()
        };
        let results = service.add(request, Duration::from_millis(200)).unwrap();
        let AddOutcome::NotFetched { error } = &results[0].outcome else {
            panic!("{:?}", results[0].outcome);
        };
        assert!(error.retryable, "{error:?}");
        assert!(direct.log().contains(&"cancel 1".to_owned()));
        drop(service);
        let _ = fs::remove_dir_all(&dir);
    }
}
