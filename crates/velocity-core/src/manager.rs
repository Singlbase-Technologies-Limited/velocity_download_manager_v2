//! The download manager (#36, #50): direct downloads through the `DirectTransfer` contract. It
//! keeps the app's records and ids, guards commands by status, applies the engine's events to the
//! records, and owns the queue: how many downloads run at once, in what order, and when.
//!
//! - **The queue (#50):** a download is given to the engine only when it may run and a slot is
//!   free. Running downloads (connecting, downloading, retrying, stalled, verifying) hold a slot;
//!   a paused, finished or waiting one doesn't. Waiting downloads start in queue order, which
//!   [`Manager::move_download`] changes. A lower limit lets running downloads finish, as aria2 did
//!   for V1.
//! - **The schedule:** a scheduled download runs only while the window is open
//!   ([`crate::queue::ScheduleSettings`]); [`Manager::tick`] reads the local time.
//! - **Statuses:** the engine reports those of the downloads it runs. The manager sets them only
//!   for downloads the engine isn't running: Paused when the user paused one, Queued when it waits
//!   for a slot or the schedule.
//!
//! Saving the queue is the database's (`downloads.queue_position`, `scheduled`); loading it when
//! the app starts is #58. Engine events are untrusted (AGENTS.md): events for unknown transfers
//! are ignored, and a status a direct engine must never report becomes `Unknown`.

use crate::database::QueueEntry;
use crate::domain::{
    DownloadKind, DownloadRecord, DownloadStatus, ErrorCode, FailureClass, Progress, TransferError,
};
use crate::performance::PerformanceSettings;
use crate::queue::QueueSettings;
use crate::transfer::{DirectTransfer, TransferEvent, TransferId, TransferSpec};
use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Receiver;

struct Download {
    record: DownloadRecord,
    /// What the engine is given when the download first starts; `None` once it has been.
    spec: Option<TransferSpec>,
    /// `None` until the engine is given the download, and once it has forgotten the transfer.
    transfer: Option<TransferId>,
    /// The engine runs it now, so it holds a slot.
    running: bool,
    /// The user paused it, or it was added paused: it waits for a resume.
    paused: bool,
    /// It runs only while the schedule's window is open.
    scheduled: bool,
}

/// How a new download starts (V1's `startPaused` and `scheduled`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AddOptions {
    /// Added paused: it waits for a resume.
    pub start_paused: bool,
    /// It runs only while the schedule's window is open.
    pub scheduled: bool,
}

/// Where [`Manager::move_download`] moves a download among the waiting ones (V1's
/// `MoveDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveDirection {
    Top,
    Up,
    Down,
    Bottom,
}

/// Owns the download records, the queue, and one direct-download engine.
pub struct Manager {
    engine: Box<dyn DirectTransfer>,
    events: Receiver<TransferEvent>,
    downloads: BTreeMap<String, Download>,
    by_transfer: HashMap<TransferId, String>,
    next_id: u64,
    /// The unfinished downloads in queue order; the first waiting ones start first.
    order: Vec<String>,
    settings: QueueSettings,
    performance: PerformanceSettings,
    /// The local time at the last [`Manager::tick`], and whether the window was open then.
    local: Option<DateTime>,
    window_open: bool,
}

/// Statuses a direct engine may report. The torrent-only ones (fetching metadata, choosing files,
/// seeding) are not among them.
fn is_direct_status(status: DownloadStatus) -> bool {
    !matches!(
        status,
        DownloadStatus::FetchingMetadata | DownloadStatus::ChoosingFiles | DownloadStatus::Seeding
    )
}

impl Download {
    /// It may take a slot: not paused, not finished, and not waiting for the schedule.
    fn may_run(&self, window_open: bool) -> bool {
        !self.paused && !self.record.status.is_terminal() && (!self.scheduled || window_open)
    }

    /// The status of a download the engine isn't running.
    fn waiting_status(&self) -> DownloadStatus {
        if self.paused {
            DownloadStatus::Paused
        } else {
            DownloadStatus::Queued
        }
    }
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
            order: Vec::new(),
            settings: QueueSettings::default(),
            performance: PerformanceSettings::default(),
            local: None,
            window_open: false,
        }
    }

    /// Adds a direct download and returns its app id: [`Manager::add_with`] with the default
    /// options.
    pub fn add(
        &mut self,
        kind: DownloadKind,
        spec: TransferSpec,
        now_ms: u64,
    ) -> Result<String, ErrorCode> {
        self.add_with(kind, spec, AddOptions::default(), now_ms)
    }

    /// Adds a direct download at the end of the queue and returns its app id. It is added paused
    /// when `options` say so, or when new downloads don't start at once and it isn't scheduled
    /// (V1's rule). When it may run and a slot is free, the engine is given it now, and an engine
    /// refusal is this call's error; otherwise it waits as Queued or Paused.
    pub fn add_with(
        &mut self,
        kind: DownloadKind,
        mut spec: TransferSpec,
        options: AddOptions,
        now_ms: u64,
    ) -> Result<String, ErrorCode> {
        // The profile caps what a download asks for (#57).
        spec.max_connections = spec
            .max_connections
            .min(self.connections_for_new_downloads());
        if matches!(kind, DownloadKind::Torrent | DownloadKind::Magnet)
            || self.engine.capabilities().protocol(kind).is_none()
        {
            return Err(ErrorCode::Unsupported);
        }
        let source_url = spec.urls.first().cloned().ok_or(ErrorCode::InvalidUrl)?;
        // No name yet when the server names the file; the engine reports it (#40).
        let file_name = spec.file_name.clone();
        let directory = spec.directory.to_string_lossy().into_owned();
        let paused =
            options.start_paused || (!self.settings.start_immediately && !options.scheduled);

        self.next_id += 1;
        let id = format!("d-{:04}", self.next_id);
        let mut download = Download {
            record: DownloadRecord {
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
                verification: None,
                created_at_ms: now_ms,
                completed_at_ms: None,
            },
            spec: Some(spec),
            transfer: None,
            running: false,
            paused,
            scheduled: options.scheduled,
        };
        download.record.status = download.waiting_status();
        let starts_now = download.may_run(self.window_open) && self.free_slots() > 0;
        self.downloads.insert(id.clone(), download);
        self.order.push(id.clone());
        if starts_now {
            if let Err(code) = self.start(&id) {
                // Refused before it began: there is no download.
                self.downloads.remove(&id);
                self.order.retain(|o| *o != id);
                return Err(code);
            }
        }
        Ok(id)
    }

    /// Pauses an unfinished download: the engine pauses a running one, and its slot goes to the
    /// next waiting download; a waiting one just stays where it is in the queue.
    pub fn pause(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        if d.paused || d.record.status.is_terminal() {
            return Err(ErrorCode::InvalidState);
        }
        if let (true, Some(transfer)) = (d.running, d.transfer) {
            // Its status comes from the engine's event.
            self.engine.pause(transfer)?;
        }
        let d = self.downloads.get_mut(id).expect("known");
        d.paused = true;
        if !d.running {
            d.record.status = d.waiting_status();
        }
        d.running = false;
        self.fill();
        Ok(())
    }

    /// Resumes a paused download: it waits as Queued until it may run and a slot is free.
    pub fn resume(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?;
        if !d.paused {
            return Err(ErrorCode::InvalidState);
        }
        d.paused = false;
        d.record.status = d.waiting_status();
        self.fill();
        Ok(())
    }

    /// Stops an unfinished download. One the engine knows ends as Removed once the engine confirms;
    /// one it was never given is Removed at once.
    pub fn cancel(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?;
        if d.record.status.is_terminal() {
            return Err(ErrorCode::InvalidState);
        }
        match d.transfer {
            Some(transfer) => self.engine.cancel(transfer),
            None => {
                d.spec = None;
                d.record.status = DownloadStatus::Removed;
                self.order.retain(|o| o != id);
                Ok(())
            }
        }
    }

    /// Moves a waiting or paused download among the waiting ones: running downloads keep their
    /// slots, as with aria2's `changePosition` in V1.
    pub fn move_download(&mut self, id: &str, direction: MoveDirection) -> Result<(), ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        if d.running || d.record.status.is_terminal() {
            return Err(ErrorCode::InvalidState);
        }
        // The waiting downloads in order, and the places in the queue they occupy.
        let slots: Vec<usize> = (0..self.order.len())
            .filter(|&i| !self.downloads[&self.order[i]].running)
            .collect();
        let mut waiting: Vec<String> = slots.iter().map(|&i| self.order[i].clone()).collect();
        let at = waiting
            .iter()
            .position(|w| w == id)
            .expect("a waiting download is in the queue");
        let to = match direction {
            MoveDirection::Top => 0,
            MoveDirection::Up => at.saturating_sub(1),
            MoveDirection::Down => (at + 1).min(waiting.len() - 1),
            MoveDirection::Bottom => waiting.len() - 1,
        };
        let moved = waiting.remove(at);
        waiting.insert(to, moved);
        for (slot, w) in slots.into_iter().zip(waiting) {
            self.order[slot] = w;
        }
        Ok(())
    }

    /// Pauses every unfinished download that isn't paused yet, all before any slot is refilled, so
    /// no waiting download starts in between. Every one is tried; the first engine refusal is
    /// returned, and a download the engine refused to pause keeps running.
    pub fn pause_all(&mut self) -> Result<(), ErrorCode> {
        let mut first_error = None;
        for id in &self.order {
            let d = self
                .downloads
                .get_mut(id)
                .expect("queued downloads are known");
            if d.paused {
                continue;
            }
            if let (true, Some(transfer)) = (d.running, d.transfer) {
                // Its status comes from the engine's event.
                if let Err(code) = self.engine.pause(transfer) {
                    first_error.get_or_insert(code);
                    continue;
                }
                d.running = false;
            } else {
                d.record.status = DownloadStatus::Paused;
            }
            d.paused = true;
        }
        self.fill();
        first_error.map_or(Ok(()), Err)
    }

    /// Resumes every paused download; they start in queue order as slots allow.
    pub fn resume_all(&mut self) -> Result<(), ErrorCode> {
        for id in &self.order {
            let d = self
                .downloads
                .get_mut(id)
                .expect("queued downloads are known");
            if d.paused {
                d.paused = false;
                d.record.status = d.waiting_status();
            }
        }
        self.fill();
        Ok(())
    }

    /// Applies new queue settings at once: a higher limit starts more downloads now, and a lower
    /// one lets the running ones finish. `InvalidInput` for settings out of bounds.
    pub fn set_queue_settings(&mut self, settings: QueueSettings) -> Result<(), ErrorCode> {
        settings.check()?;
        self.window_open = self
            .local
            .is_some_and(|local| settings.schedule.is_open(local));
        self.settings = settings;
        self.fill();
        Ok(())
    }

    /// Applies a performance profile (#57): the engine gets its limits, and new downloads ask
    /// for at most its connections. `InvalidInput`, with nothing sent, for settings out of bounds.
    pub fn set_performance(&mut self, settings: PerformanceSettings) -> Result<(), ErrorCode> {
        settings.check()?;
        self.engine.set_engine_limits(settings.engine_limits())?;
        self.performance = settings;
        Ok(())
    }

    /// The connections a new download asks for under the current profile: its per-server cap.
    pub fn connections_for_new_downloads(&self) -> u32 {
        self.performance.engine_limits().max_connections_per_host
    }

    /// Applies the engine's pending events and the schedule at local time `local`: when the
    /// window opens, scheduled downloads may start; when it closes, running ones pause. Returns
    /// how many events were applied. The app calls it regularly; tests pass any clock.
    pub fn tick(&mut self, now_ms: u64, local: DateTime) -> usize {
        self.local = Some(local);
        self.window_open = self.settings.schedule.is_open(local);
        self.pump(now_ms)
    }

    /// Applies every pending engine event to the records, starts what the freed slots allow, and
    /// returns how many events were applied.
    pub fn pump(&mut self, now_ms: u64) -> usize {
        let mut applied = 0;
        while let Ok(event) = self.events.try_recv() {
            if self.apply(event, now_ms) {
                applied += 1;
            }
        }
        self.fill();
        applied
    }

    /// The unfinished downloads in queue order, as the database saves them
    /// ([`crate::database::Database::save_queue`]).
    pub fn queue(&self) -> Vec<QueueEntry> {
        self.order
            .iter()
            .map(|id| QueueEntry {
                id: id.clone(),
                scheduled: self.downloads[id].scheduled,
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&DownloadRecord> {
        self.downloads.get(id).map(|d| &d.record)
    }

    /// Slots the limit leaves free.
    fn free_slots(&self) -> usize {
        let running = self.downloads.values().filter(|d| d.running).count();
        (self.settings.max_concurrent_downloads as usize).saturating_sub(running)
    }

    /// Pauses running downloads that may no longer run (the window closed), then starts waiting
    /// ones in queue order while slots are free. A download the engine refuses to start fails
    /// with the reason; one it refuses to pause keeps its slot until its next event.
    fn fill(&mut self) {
        let window_open = self.window_open;
        let stopping: Vec<(String, TransferId)> = self
            .order
            .iter()
            .filter_map(|id| {
                let d = &self.downloads[id];
                (d.running && !d.may_run(window_open)).then_some((id.clone(), d.transfer?))
            })
            .collect();
        for (id, transfer) in stopping {
            if self.engine.pause(transfer).is_ok() {
                self.downloads.get_mut(&id).expect("known").running = false;
            }
        }
        let waiting: Vec<String> = self
            .order
            .iter()
            .filter(|id| {
                let d = &self.downloads[*id];
                !d.running && d.may_run(window_open)
            })
            .cloned()
            .collect();
        for id in waiting {
            if self.free_slots() == 0 {
                break;
            }
            if let Err(code) = self.start(&id) {
                let d = self.downloads.get_mut(&id).expect("known");
                d.record.status = DownloadStatus::Failed;
                d.record.error = Some(TransferError::new(
                    FailureClass::Permanent,
                    None,
                    code.user_message(),
                ));
                d.spec = None;
                self.order.retain(|o| *o != id);
            }
        }
    }

    /// Gives the engine a waiting download: a new transfer the first time, a resume after that.
    fn start(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).expect("known");
        match (d.transfer, &d.spec) {
            (Some(transfer), _) => self.engine.resume(transfer)?,
            (None, Some(spec)) => {
                let transfer = self.engine.add(spec.clone())?;
                d.transfer = Some(transfer);
                d.spec = None;
                self.by_transfer.insert(transfer, id.to_owned());
            }
            (None, None) => return Err(ErrorCode::InvalidState),
        }
        self.downloads.get_mut(id).expect("known").running = true;
        Ok(())
    }

    fn apply(&mut self, event: TransferEvent, now_ms: u64) -> bool {
        match event {
            TransferEvent::Changed { snapshot } => {
                let Some(app_id) = self.by_transfer.get(&snapshot.id).cloned() else {
                    return false;
                };
                let Some(download) = self.downloads.get_mut(&app_id) else {
                    return false;
                };
                let status = if is_direct_status(snapshot.status) {
                    snapshot.status
                } else {
                    DownloadStatus::Unknown
                };
                // The engine pauses the downloads the queue holds back; they show as waiting.
                let shown = if status == DownloadStatus::Paused && !download.running {
                    download.waiting_status()
                } else {
                    status
                };
                let record = &mut download.record;
                record.status = shown;
                record.progress = snapshot.progress;
                record.error = snapshot.error;
                record.verification = snapshot.verification;
                if snapshot.file_name.is_some() {
                    record.file_name = snapshot.file_name;
                }
                if record.status == DownloadStatus::Completed && record.completed_at_ms.is_none() {
                    record.completed_at_ms = Some(now_ms);
                }
                if record.status.is_terminal() {
                    download.running = false;
                    self.order.retain(|o| *o != app_id);
                }
                true
            }
            TransferEvent::Removed { id } => {
                let Some(app_id) = self.by_transfer.remove(&id) else {
                    return false;
                };
                if let Some(download) = self.downloads.get_mut(&app_id) {
                    download.transfer = None;
                    download.running = false;
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
    use crate::domain::{Capabilities, FailureClass, TransferError, Verification};
    use crate::transfer::fake::{FakeControl, FakeTransfer};
    use crate::transfer::{Limits, TransferSnapshot};
    use std::path::PathBuf;

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
        let mut m = Manager::new(Box::new(fake));
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
            protocols: Capabilities::sample()
                .protocols
                .into_iter()
                .filter(|p| p.kind == DownloadKind::Http)
                .collect(),
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
            let mut m = Manager::new(Box::new(fake));
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
}
