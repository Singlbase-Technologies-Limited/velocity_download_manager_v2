//! The download manager, first slice (#36): direct downloads through the `DirectTransfer`
//! contract. It keeps the app's records and ids, guards commands by status, and applies the
//! engine's events to the records. Persistence (#26), the queue and settings (#30) come later.
//!
//! Engine events are untrusted (AGENTS.md): events for unknown transfers are ignored, and a
//! status a direct engine must never report becomes `Unknown`.

use crate::domain::{DownloadKind, DownloadRecord, DownloadStatus, ErrorCode, Progress};
use crate::transfer::{DirectTransfer, TransferEvent, TransferId, TransferSpec};
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Receiver;

struct Download {
    record: DownloadRecord,
    /// `None` once the engine has forgotten the transfer.
    transfer: Option<TransferId>,
}

/// Owns the download records and drives one direct-download engine.
pub struct Manager {
    engine: Box<dyn DirectTransfer>,
    events: Receiver<TransferEvent>,
    downloads: BTreeMap<String, Download>,
    by_transfer: HashMap<TransferId, String>,
    next_id: u64,
}

/// Statuses a direct engine may report. The torrent-only ones (fetching metadata, choosing files,
/// seeding) are not among them.
fn is_direct_status(status: DownloadStatus) -> bool {
    !matches!(
        status,
        DownloadStatus::FetchingMetadata | DownloadStatus::ChoosingFiles | DownloadStatus::Seeding
    )
}

impl Manager {
    pub fn new(mut engine: Box<dyn DirectTransfer>) -> Self {
        let events = engine.subscribe();
        Self {
            engine,
            events,
            downloads: BTreeMap::new(),
            by_transfer: HashMap::new(),
            next_id: 0,
        }
    }

    /// Starts a direct download and returns its app id. The record starts as Queued; later
    /// statuses come only from the engine's events (see [`Manager::pump`]).
    pub fn add(
        &mut self,
        kind: DownloadKind,
        spec: TransferSpec,
        now_ms: u64,
    ) -> Result<String, ErrorCode> {
        if matches!(kind, DownloadKind::Torrent | DownloadKind::Magnet)
            || !self.engine.capabilities().protocols.contains(&kind)
        {
            return Err(ErrorCode::Unsupported);
        }
        let source_url = spec.urls.first().cloned().ok_or(ErrorCode::InvalidUrl)?;
        let file_name = spec
            .destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let directory = spec
            .destination
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let transfer = self.engine.add(spec)?;

        self.next_id += 1;
        let id = format!("d-{:04}", self.next_id);
        let record = DownloadRecord {
            id: id.clone(),
            kind,
            source_url,
            file_name,
            directory,
            category_id: None,
            status: DownloadStatus::Queued,
            progress: Progress {
                completed_bytes: 0,
                total_bytes: None,
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: None,
            },
            error: None,
            created_at_ms: now_ms,
            completed_at_ms: None,
        };
        self.by_transfer.insert(transfer, id.clone());
        self.downloads.insert(
            id.clone(),
            Download {
                record,
                transfer: Some(transfer),
            },
        );
        Ok(id)
    }

    pub fn pause(&mut self, id: &str) -> Result<(), ErrorCode> {
        let transfer = self.transfer_in(id, |s| !s.is_terminal() && s != DownloadStatus::Paused)?;
        self.engine.pause(transfer)
    }

    pub fn resume(&mut self, id: &str) -> Result<(), ErrorCode> {
        let transfer = self.transfer_in(id, |s| s == DownloadStatus::Paused)?;
        self.engine.resume(transfer)
    }

    /// Stops an unfinished download; it ends as Removed once the engine confirms.
    pub fn cancel(&mut self, id: &str) -> Result<(), ErrorCode> {
        let transfer = self.transfer_in(id, |s| !s.is_terminal())?;
        self.engine.cancel(transfer)
    }

    /// Applies every pending engine event to the records, and returns how many were applied.
    pub fn pump(&mut self, now_ms: u64) -> usize {
        let mut applied = 0;
        while let Ok(event) = self.events.try_recv() {
            if self.apply(event, now_ms) {
                applied += 1;
            }
        }
        applied
    }

    pub fn get(&self, id: &str) -> Option<&DownloadRecord> {
        self.downloads.get(id).map(|d| &d.record)
    }

    /// The engine's transfer for `id`, if the record's status allows the command.
    fn transfer_in(
        &self,
        id: &str,
        allowed: impl Fn(DownloadStatus) -> bool,
    ) -> Result<TransferId, ErrorCode> {
        let download = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        match download.transfer {
            Some(transfer) if allowed(download.record.status) => Ok(transfer),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    fn apply(&mut self, event: TransferEvent, now_ms: u64) -> bool {
        match event {
            TransferEvent::Changed { snapshot } => {
                let Some(download) = self
                    .by_transfer
                    .get(&snapshot.id)
                    .and_then(|id| self.downloads.get_mut(id))
                else {
                    return false;
                };
                let record = &mut download.record;
                record.status = if is_direct_status(snapshot.status) {
                    snapshot.status
                } else {
                    DownloadStatus::Unknown
                };
                record.progress = snapshot.progress;
                record.error = snapshot.error;
                if record.status == DownloadStatus::Completed && record.completed_at_ms.is_none() {
                    record.completed_at_ms = Some(now_ms);
                }
                true
            }
            TransferEvent::Removed { id } => {
                let Some(app_id) = self.by_transfer.remove(&id) else {
                    return false;
                };
                if let Some(download) = self.downloads.get_mut(&app_id) {
                    download.transfer = None;
                }
                true
            }
            // A checkpoint changes nothing the UI shows. Saving it in the database, to resume
            // after a crash, comes with the manager's persistence (#58).
            TransferEvent::Checkpoint { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Capabilities, FailureClass, TransferError};
    use crate::transfer::fake::{FakeControl, FakeTransfer};
    use crate::transfer::{Limits, TransferSnapshot};
    use std::path::PathBuf;

    const T0: u64 = 1_792_600_000_000;

    fn spec() -> TransferSpec {
        TransferSpec {
            urls: vec!["https://example.com/files/a.zip".to_owned()],
            destination: PathBuf::from("downloads").join("a.zip"),
            credentials: None,
            headers: Vec::new(),
            checksum: None,
            limits: Limits::default(),
            max_connections: 4,
            resume: None,
        }
    }

    fn setup() -> (Manager, FakeControl, String) {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let mut manager = Manager::new(Box::new(fake));
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
    fn events_for_unknown_transfers_are_ignored() {
        let (mut m, control, id) = setup();
        let mut stray = TransferSnapshot {
            id: TransferId(42),
            status: DownloadStatus::Completed,
            progress: m.get(&id).unwrap().progress.clone(),
            wasted_bytes: 0,
            error: None,
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
    fn torrent_kinds_and_protocols_the_engine_lacks_are_refused() {
        let fake = FakeTransfer::with_capabilities(Capabilities {
            protocols: vec![DownloadKind::Http],
            ..Capabilities::sample()
        });
        let control = fake.control();
        let mut m = Manager::new(Box::new(fake));
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
}
