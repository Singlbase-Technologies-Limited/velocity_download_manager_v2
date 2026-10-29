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

use crate::database::{Database, DatabaseError, NewDownload, QueueEntry};
use crate::domain::{
    DownloadKind, DownloadRecord, DownloadStatus, DownloadsSnapshot, EngineHealth, ErrorCode,
    FailureClass, Progress, TransferError, UnavailableDownload,
};
use crate::metalink::{self, Metalink};
use crate::performance::PerformanceSettings;
use crate::queue::QueueSettings;
use crate::torrent_engine::{MissingTorrentEngine, TorrentEngine, TorrentEvent, TorrentId};
use crate::transfer::{
    lost_by_engine, DirectTransfer, Limits, Resume, TransferEvent, TransferId, TransferSpec,
};
use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

struct Download {
    record: DownloadRecord,
    /// What its engine needs.
    job: Job,
    /// The engine runs it now, so it holds a slot.
    running: bool,
    /// The user paused it, or it was added paused: it waits for a resume.
    paused: bool,
    /// It runs only while the schedule's window is open.
    scheduled: bool,
    /// When the engine's events last saved it; progress alone is saved at most every
    /// [`persist::PROGRESS_SAVE_MS`].
    saved_at_ms: Option<u64>,
    /// It came back after a restart without the login it had (#70); it waits, paused, for
    /// [`Manager::set_login`].
    needs_login: bool,
    /// When it started and how fast it went while it ran (#59).
    activity: activity::Tracker,
}

/// What a download's engine needs: the direct-download engine's or the torrent helper's.
enum Job {
    /// Both boxed, so a download stays small whichever engine it is for.
    Direct(Box<DirectJob>),
    Torrent(Box<torrents::TorrentJob>),
}

/// A direct download's part.
struct DirectJob {
    /// What the engine is given to start it, without a resume. It keeps any login in memory only,
    /// for a retry in this session (#70).
    request: TransferSpec,
    /// What the next start resumes from (#43): set for a download recovered after a restart, or
    /// retried; `None` for a new one.
    resume: Option<Resume>,
    /// `None` until the engine is given the download, and once it has forgotten the transfer.
    transfer: Option<TransferId>,
    /// The file's media type, as the engine reported it (#54).
    content_type: Option<String>,
}

impl DirectJob {
    fn new(request: TransferSpec, resume: Option<Resume>) -> Self {
        DirectJob {
            request,
            resume,
            transfer: None,
            content_type: None,
        }
    }
}

/// How a new download starts (V1's `startPaused` and `scheduled`), and its category (#31).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddOptions {
    /// Added paused: it waits for a resume.
    pub start_paused: bool,
    /// It runs only while the schedule's window is open.
    pub scheduled: bool,
    /// Its category, which must exist; `None` for none yet ([`Manager::place`] chooses one).
    pub category_id: Option<String>,
}

/// Which of a Metalink document's files to download, and where (#54).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalinkSelection {
    /// The chosen folder; each file goes into its subfolders inside it.
    pub directory: PathBuf,
    /// Indexes into [`Metalink::files`], or `None` for every file.
    pub files: Option<Vec<usize>>,
    pub limits: Limits,
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
    /// Where the records are kept, so a restart brings them back (#58).
    database: Arc<Database>,
    /// Records changed since they were last saved ([`Manager::flush`]).
    unsaved: BTreeSet<String>,
    /// The last failure to save a record.
    storage_error: Option<DatabaseError>,
    /// The queue as it was last saved.
    saved_queue: Vec<QueueEntry>,
    /// Stored records this build can't read, or whose engine it doesn't run yet.
    unavailable: Vec<crate::database::StoredDownload>,
    events: Receiver<TransferEvent>,
    /// The torrent engine: the torrent helper in the app ([`Manager::with_torrents`]).
    torrents: Box<dyn TorrentEngine>,
    torrent_events: Receiver<TorrentEvent>,
    downloads: BTreeMap<String, Download>,
    by_transfer: HashMap<TransferId, String>,
    by_torrent: HashMap<TorrentId, String>,
    next_id: u64,
    /// The unfinished downloads in queue order; the first waiting ones start first.
    order: Vec<String>,
    settings: QueueSettings,
    performance: PerformanceSettings,
    /// The local time at the last [`Manager::tick`], and whether the window was open then.
    local: Option<DateTime>,
    window_open: bool,
    /// Metalink is turned on (#54).
    metalink_enabled: bool,
    /// Downloads that completed in the events being applied, to follow when they are Metalink
    /// files.
    just_completed: Vec<String>,
    /// `.torrent` links being fetched, and the trackers of those fetched (#60).
    links: torrent_links::TorrentLinks,
}

/// Statuses a direct engine may report. The torrent-only ones (fetching metadata, choosing files,
/// seeding) are not among them.
fn is_direct_status(status: DownloadStatus) -> bool {
    !matches!(
        status,
        DownloadStatus::FetchingMetadata | DownloadStatus::ChoosingFiles | DownloadStatus::Seeding
    )
}

/// A status the engine shows only while it runs the transfer.
fn is_running_status(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Connecting
            | DownloadStatus::Downloading
            | DownloadStatus::Verifying
            | DownloadStatus::Retrying
            | DownloadStatus::Stalled
    )
}

impl Download {
    fn direct(&self) -> Option<&DirectJob> {
        match &self.job {
            Job::Direct(job) => Some(job.as_ref()),
            Job::Torrent(_) => None,
        }
    }

    fn direct_mut(&mut self) -> Option<&mut DirectJob> {
        match &mut self.job {
            Job::Direct(job) => Some(job.as_mut()),
            Job::Torrent(_) => None,
        }
    }

    /// Its transfer, while the direct-download engine has it.
    fn transfer(&self) -> Option<TransferId> {
        self.direct().and_then(|job| job.transfer)
    }

    /// Its torrent, while the helper has it.
    fn torrent(&self) -> Option<TorrentId> {
        match &self.job {
            Job::Torrent(job) => job.torrent,
            Job::Direct(_) => None,
        }
    }

    /// It holds a slot of the queue: running, and for a torrent, fetching its metadata or
    /// downloading. One choosing files or seeding runs without a slot (PO decision on #58).
    fn holds_slot(&self) -> bool {
        self.running
            && match self.job {
                Job::Direct(_) => true,
                Job::Torrent(_) => !matches!(
                    self.record.status,
                    DownloadStatus::ChoosingFiles | DownloadStatus::Seeding
                ),
            }
    }

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
    /// An empty manager; [`Manager::open`] loads what the database kept.
    fn new(mut engine: Box<dyn DirectTransfer>, database: Arc<Database>) -> Self {
        let events = engine.subscribe();
        let mut torrents: Box<dyn TorrentEngine> =
            Box::new(MissingTorrentEngine::new(ErrorCode::EngineMissing));
        let torrent_events = torrents.subscribe();
        Self {
            torrents,
            torrent_events,
            by_torrent: HashMap::new(),
            engine,
            database,
            unsaved: BTreeSet::new(),
            storage_error: None,
            saved_queue: Vec::new(),
            unavailable: Vec::new(),
            events,
            downloads: BTreeMap::new(),
            by_transfer: HashMap::new(),
            next_id: 0,
            order: Vec::new(),
            settings: QueueSettings::default(),
            performance: PerformanceSettings::default(),
            local: None,
            window_open: false,
            metalink_enabled: false,
            just_completed: Vec::new(),
            links: torrent_links::TorrentLinks::default(),
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
        let source_url = spec.urls.first().cloned().ok_or(ErrorCode::InvalidUrl)?;
        let capabilities = self.engine.capabilities();
        let supported = match kind {
            DownloadKind::Torrent | DownloadKind::Magnet => false,
            // A Metalink file's download fetches over its first source's protocol (#54).
            DownloadKind::Metalink => {
                if !self.metalink_enabled {
                    return Err(ErrorCode::ProtocolDisabled);
                }
                capabilities.metalink
                    && metalink::source_kind(&source_url)
                        .is_some_and(|k| capabilities.protocol(k).is_some())
            }
            _ => capabilities.protocol(kind).is_some(),
        };
        if !supported {
            return Err(ErrorCode::Unsupported);
        }
        let category_id = self.known_category(options.category_id.as_deref())?;
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
                category_id: category_id.clone(),
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
                retry_at_ms: None,
                started_at_ms: None,
                average_speed_bps: None,
            },
            job: Job::Direct(Box::new(DirectJob::new(spec, None))),
            running: false,
            paused,
            scheduled: options.scheduled,
            saved_at_ms: None,
            needs_login: false,
            activity: activity::Tracker::default(),
        };
        download.record.status = download.waiting_status();
        let request = &download.direct().expect("a direct download").request;
        let mut new = NewDownload::from_transfer(&id, kind, request, now_ms);
        new.status = download.record.status;
        new.category_id = category_id;
        self.database.insert_download(&new).map_err(|e| e.code)?;
        let starts_now = download.may_run(self.window_open) && self.free_slots() > 0;
        self.downloads.insert(id.clone(), download);
        self.order.push(id.clone());
        if starts_now {
            if let Err(code) = self.start(&id) {
                // Refused before it began: there is no download.
                self.downloads.remove(&id);
                self.order.retain(|o| *o != id);
                self.database.delete_download(&id).map_err(|e| e.code)?;
                return Err(code);
            }
        }
        self.flush();
        Ok(id)
    }

    /// What the transfer engine can do (#57), so the UI offers only that (#60).
    pub fn capabilities(&self) -> crate::domain::Capabilities {
        self.engine.capabilities()
    }

    /// Turns Metalink on or off (V1's `advanced.enable_metalink`, off by default; ADR-0005). Its
    /// storage and Settings page are #30's.
    pub fn set_metalink_enabled(&mut self, enabled: bool) {
        self.metalink_enabled = enabled;
    }

    /// Adds the selected files of a Metalink document, one download each, and returns their ids
    /// in document order. Each file is saved in its subfolders inside the chosen folder, which are
    /// created here ([`metalink::create_folders`]), and fetched from its sources the engine
    /// supports, best first, with the document's hashes to check. The downloads are added as with
    /// [`Manager::add_with`], so they queue as any other.
    ///
    /// Refused with nothing added: `ProtocolDisabled` while Metalink is off, `Unsupported` by an
    /// engine without Metalink or for a file with no source it can fetch, `InvalidInput` for a
    /// selection that is empty, out of range or names a file twice. When a file is refused after
    /// others were added, those are cancelled again, and the refusal is the error.
    pub fn add_metalink(
        &mut self,
        metalink: &Metalink,
        selection: &MetalinkSelection,
        options: AddOptions,
        now_ms: u64,
    ) -> Result<Vec<String>, ErrorCode> {
        if !self.metalink_enabled {
            return Err(ErrorCode::ProtocolDisabled);
        }
        let capabilities = self.engine.capabilities();
        if !capabilities.metalink {
            return Err(ErrorCode::Unsupported);
        }
        let indexes: Vec<usize> = match &selection.files {
            None => (0..metalink.files.len()).collect(),
            Some(files) => {
                let mut sorted = files.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let in_range = sorted.last().is_some_and(|&i| i < metalink.files.len());
                if sorted.len() != files.len() || !in_range {
                    return Err(ErrorCode::InvalidInput);
                }
                sorted
            }
        };
        // Every file's sources first, so an unusable one refuses the lot before anything changes.
        let mut planned = Vec::with_capacity(indexes.len());
        for &i in &indexes {
            let file = &metalink.files[i];
            let urls: Vec<String> = file
                .urls
                .iter()
                .filter(|url| {
                    metalink::source_kind(url).is_some_and(|k| capabilities.protocol(k).is_some())
                })
                .cloned()
                .collect();
            if urls.is_empty() {
                return Err(ErrorCode::Unsupported);
            }
            planned.push((file, urls));
        }
        let mut added = Vec::with_capacity(planned.len());
        for (file, urls) in planned {
            let result = metalink::create_folders(&selection.directory, &file.folders).and_then(
                |directory| {
                    let spec = TransferSpec {
                        urls,
                        directory,
                        file_name: Some(file.name.clone()),
                        credentials: None,
                        require_encryption: false,
                        ssh_key: None,
                        headers: Vec::new(),
                        referrer: None,
                        user_agent: None,
                        checksum: file.checksum.clone(),
                        pieces: file.pieces.clone(),
                        limits: selection.limits,
                        max_connections: self.connections_for_new_downloads(),
                        resume: None,
                    };
                    self.add_with(DownloadKind::Metalink, spec, options.clone(), now_ms)
                },
            );
            match result {
                Ok(id) => added.push(id),
                Err(code) => {
                    for id in &added {
                        // Each was just added and is unfinished, so it can be cancelled; one that
                        // still can't stays in the list, where the user sees it.
                        let _ = self.cancel(id);
                    }
                    return Err(code);
                }
            }
        }
        Ok(added)
    }

    /// Pauses an unfinished download: the engine pauses a running one, and its slot goes to the
    /// next waiting download; a waiting one just stays where it is in the queue.
    pub fn pause(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        if d.paused || d.record.status.is_terminal() {
            return Err(ErrorCode::InvalidState);
        }
        if d.running {
            // Its status comes from the engine's event.
            self.pause_in_engine(id)?;
        }
        let d = self.downloads.get_mut(id).expect("known");
        d.paused = true;
        if !d.running {
            d.record.status = d.waiting_status();
            self.unsaved.insert(id.to_owned());
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
        if d.needs_login {
            return Err(ErrorCode::ConfirmationRequired);
        }
        d.paused = false;
        d.record.status = d.waiting_status();
        self.unsaved.insert(id.to_owned());
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
        if d.torrent().is_some() {
            // The helper forgets it, and keeps its files, as V1 did; its status is the manager's.
            self.forget_torrent(id)?;
        }
        let d = self.downloads.get_mut(id).expect("known");
        d.running = false;
        match d.transfer() {
            Some(transfer) => self.engine.cancel(transfer),
            None => {
                d.record.status = DownloadStatus::Removed;
                self.order.retain(|o| o != id);
                self.mark(id);
                self.flush();
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
        self.flush();
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
            if d.running {
                // Its status comes from the engine's event.
                let paused = match (d.transfer(), d.torrent()) {
                    (Some(transfer), _) => self.engine.pause(transfer),
                    (None, Some(torrent)) => self.torrents.pause(torrent),
                    (None, None) => Ok(()),
                };
                if let Err(code) = paused {
                    first_error.get_or_insert(code);
                    continue;
                }
                d.running = false;
            } else {
                d.record.status = DownloadStatus::Paused;
                self.unsaved.insert(id.clone());
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
            if d.paused && !d.needs_login {
                d.paused = false;
                d.record.status = d.waiting_status();
                self.unsaved.insert(id.clone());
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

    /// Trusts the SFTP host key the user confirmed (#53), as a download's error showed it
    /// ([`crate::domain::TransferError::host_key`]). The engine pins only the key it saw with
    /// that fingerprint; the download is then retried by the caller. `InvalidInput`, with
    /// nothing sent, for an empty host or a fingerprint that isn't `SHA256:` and base64.
    pub fn trust_host_key(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        let base64 = |c: char| c.is_ascii_alphanumeric() || "+/".contains(c);
        let well_formed = fingerprint
            .strip_prefix("SHA256:")
            .is_some_and(|b| !b.is_empty() && b.chars().all(base64));
        if host.is_empty() || !well_formed {
            return Err(ErrorCode::InvalidInput);
        }
        self.engine.trust_host_key(host, port, fingerprint)
    }

    /// Whether the engine can take commands now (#39), for the app to show.
    pub fn engine_health(&self) -> EngineHealth {
        self.engine.health()
    }

    /// "Restart transfer engine" (ADR-0005): a new engine in place of the running one. What the
    /// old one was running resumes on the new one (PO decision on #58).
    pub fn restart_engine(&mut self) -> Result<(), ErrorCode> {
        self.engine.restart()
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
        let mut applied = self.apply_torrent_events(now_ms);
        while let Ok(event) = self.events.try_recv() {
            if self.apply(event, now_ms) {
                applied += 1;
            }
        }
        for id in std::mem::take(&mut self.just_completed) {
            self.follow_metalink(&id, now_ms);
        }
        self.fill();
        applied
    }

    /// Follows a completed download that is a Metalink file, as aria2's `follow-metalink` did for
    /// V1 (#54): while Metalink is on, an HTTP or FTP download named `.meta4` or `.metalink`, or
    /// that the server called `application/metalink4+xml` or `application/metalink+xml`, adds
    /// every file it lists, in its folder. The Metalink file stays on disk as a completed
    /// download. One that can't be used fails, with the reason, so the user knows its files
    /// weren't added. A download from a Metalink is never followed, so one can't list another.
    fn follow_metalink(&mut self, id: &str, now_ms: u64) {
        let Some(download) = self.downloads.get(id) else {
            return;
        };
        let record = &download.record;
        let followed = matches!(record.kind, DownloadKind::Http | DownloadKind::Ftp);
        let Some(name) = record
            .file_name
            .clone()
            .filter(|_| followed && self.metalink_enabled)
        else {
            return;
        };
        let by_name = crate::naming::extension_of(&name)
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "meta4" | "metalink"));
        let by_type = download
            .direct()
            .and_then(|job| job.content_type.as_deref())
            .is_some_and(|t| matches!(t, "application/metalink4+xml" | "application/metalink+xml"));
        if !by_name && !by_type {
            return;
        }
        let directory = PathBuf::from(&record.directory);
        let options = AddOptions {
            start_paused: false,
            scheduled: download.scheduled,
            category_id: None,
        };
        let result = metalink::read_document(&directory.join(&name))
            .map_err(|e| e.to_string())
            .and_then(|document| {
                let selection = MetalinkSelection {
                    directory,
                    files: None,
                    limits: Limits::default(),
                };
                self.add_metalink(&document, &selection, options, now_ms)
                    .map_err(|code| code.user_message().to_owned())
            });
        if let (Err(why), Some(download)) = (result, self.downloads.get_mut(id)) {
            let record = &mut download.record;
            record.status = DownloadStatus::Failed;
            record.completed_at_ms = None;
            self.unsaved.insert(id.to_owned());
            record.error = Some(TransferError::new(
                FailureClass::Permanent,
                None,
                &format!(
                    "Velocity downloaded this Metalink file but couldn't add its files: {why}"
                ),
            ));
        }
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

    /// What the UI is sent: the unfinished downloads in queue order, then the finished ones,
    /// newest first.
    pub fn snapshot(&self) -> DownloadsSnapshot {
        let mut finished: Vec<&DownloadRecord> = self
            .downloads
            .values()
            .map(|d| &d.record)
            .filter(|r| r.status.is_terminal())
            .collect();
        finished.sort_by(|a, b| (b.created_at_ms, &b.id).cmp(&(a.created_at_ms, &a.id)));
        let downloads = self
            .order
            .iter()
            .map(|id| &self.downloads[id].record)
            .chain(finished)
            .cloned()
            .collect();
        DownloadsSnapshot {
            downloads,
            unavailable: self
                .unavailable
                .iter()
                .map(|d| UnavailableDownload {
                    id: d.id.clone(),
                    source_url: d.source_url.clone(),
                    file_name: d.file_name.clone(),
                    directory: d.directory.clone(),
                    created_at_ms: d.created_at_ms,
                })
                .collect(),
            needs_login: self
                .downloads
                .iter()
                .filter(|(_, d)| d.needs_login)
                .map(|(id, _)| id.clone())
                .collect(),
            engine: self.engine_health(),
            torrent_engine: self.torrents.health(),
            storage_error: self.storage_error.as_ref().map(|e| e.code),
        }
    }

    pub fn get(&self, id: &str) -> Option<&DownloadRecord> {
        self.downloads.get(id).map(|d| &d.record)
    }

    /// Slots the limit leaves free.
    fn free_slots(&self) -> usize {
        let running = self.downloads.values().filter(|d| d.holds_slot()).count();
        (self.settings.max_concurrent_downloads as usize).saturating_sub(running)
    }

    /// Pauses running downloads that may no longer run (the window closed), then starts waiting
    /// ones in queue order while slots are free. A download the engine refuses to start fails
    /// with the reason; one it refuses to pause keeps its slot until its next event.
    fn fill(&mut self) {
        let window_open = self.window_open;
        let stopping: Vec<String> = self
            .order
            .iter()
            .filter(|id| {
                let d = &self.downloads[*id];
                d.running && !d.may_run(window_open)
            })
            .cloned()
            .collect();
        for id in stopping {
            if self.pause_in_engine(&id).is_ok() {
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
            match self.start(&id) {
                Ok(()) => {}
                // No engine now: it keeps waiting, and the next fill tries again (#58).
                Err(ErrorCode::EngineUnavailable) => break,
                Err(code) => {
                    let d = self.downloads.get_mut(&id).expect("known");
                    d.record.status = DownloadStatus::Failed;
                    d.record.error = Some(TransferError::new(
                        FailureClass::Permanent,
                        None,
                        code.user_message(),
                    ));
                    self.order.retain(|o| *o != id);
                    self.mark(&id);
                }
            }
        }
        self.flush();
    }

    /// Pauses the download in whichever engine has it.
    fn pause_in_engine(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = &self.downloads[id];
        match (d.transfer(), d.torrent()) {
            (Some(transfer), _) => self.engine.pause(transfer),
            (None, Some(torrent)) => self.torrents.pause(torrent),
            (None, None) => Ok(()),
        }
    }

    /// Gives the engine a waiting download: a new transfer while the engine doesn't know it, a
    /// resume after that. A new transfer for a download that ran before resumes from what it left
    /// on disk, under the name it was saved as.
    fn start(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).expect("known");
        let name = d.record.file_name.clone();
        let Some(job) = d.direct_mut() else {
            self.start_torrent(id)?;
            self.downloads.get_mut(id).expect("known").running = true;
            return Ok(());
        };
        match job.transfer {
            Some(transfer) => self.engine.resume(transfer)?,
            None => {
                let mut spec = job.request.clone();
                // A resume gives the name the file is saved as; a download the server never named
                // has nothing on disk to resume, and starts afresh.
                if let (Some(resume), Some(name)) = (job.resume.clone(), name) {
                    spec.file_name = Some(name);
                    spec.resume = Some(resume);
                }
                let transfer = self.engine.add(spec)?;
                job.transfer = Some(transfer);
                job.resume = None;
                self.by_transfer.insert(transfer, id.to_owned());
            }
        }
        self.downloads.get_mut(id).expect("known").running = true;
        Ok(())
    }

    fn apply(&mut self, event: TransferEvent, now_ms: u64) -> bool {
        match event {
            TransferEvent::Changed { snapshot } => {
                if self.apply_fetch(&snapshot) {
                    return true;
                }
                let Some(app_id) = self.by_transfer.get(&snapshot.id).cloned() else {
                    return false;
                };
                if snapshot.status == DownloadStatus::Failed {
                    if let Some(error) = snapshot.error.clone().filter(lost_by_engine) {
                        self.lose(&app_id, snapshot.id, error);
                        return true;
                    }
                }
                let Some(download) = self.downloads.get_mut(&app_id) else {
                    return false;
                };
                let status = if is_direct_status(snapshot.status) {
                    snapshot.status
                } else {
                    DownloadStatus::Unknown
                };
                // The engine pauses the downloads the queue holds back; they show as waiting. A
                // running status sent before the engine stopped one the user paused or cancelled
                // doesn't undo that (#59).
                let shown = if download.running {
                    status
                } else if status == DownloadStatus::Paused {
                    download.waiting_status()
                } else if is_running_status(status) {
                    download.record.status
                } else {
                    status
                };
                let before = download.record.clone();
                download.activity.observe(
                    now_ms,
                    download.running && is_running_status(shown),
                    snapshot.progress.completed_bytes,
                );
                let record = &mut download.record;
                record.started_at_ms = download.activity.totals.started_at_ms;
                record.average_speed_bps = download.activity.average_speed_bps();
                record.status = shown;
                // Not saved: a download read back at start-up isn't waiting to retry.
                record.retry_at_ms = snapshot
                    .retry_at_ms
                    .filter(|_| shown == DownloadStatus::Retrying);
                record.progress = snapshot.progress;
                record.error = snapshot.error;
                record.verification = snapshot.verification;
                if snapshot.file_name.is_some() {
                    record.file_name = snapshot.file_name;
                }
                // The server named it: its automatic category, in the folder it started in
                // (PO decision on #31).
                if before.file_name.is_none() && record.category_id.is_none() {
                    record.category_id =
                        categories::automatic(&self.database, record.file_name.as_deref());
                }
                if let (Some(content_type), Some(job)) =
                    (snapshot.content_type, download.direct_mut())
                {
                    job.content_type = Some(content_type);
                }
                let record = &mut download.record;
                if record.status == DownloadStatus::Completed && record.completed_at_ms.is_none() {
                    record.completed_at_ms = Some(now_ms);
                    self.just_completed.push(app_id.clone());
                }
                if record.status.is_terminal() {
                    download.running = false;
                    self.order.retain(|o| *o != app_id);
                }
                let record = &download.record;
                let changed = record.status != before.status
                    || record.error != before.error
                    || record.verification != before.verification
                    || record.file_name != before.file_name
                    || record.category_id != before.category_id
                    || record.completed_at_ms != before.completed_at_ms
                    || record.started_at_ms != before.started_at_ms;
                let due = download
                    .saved_at_ms
                    .is_none_or(|at| now_ms.saturating_sub(at) >= persist::PROGRESS_SAVE_MS);
                if changed || (record.progress != before.progress && due) {
                    download.saved_at_ms = Some(now_ms);
                    self.mark(&app_id);
                }
                true
            }
            TransferEvent::Removed { id } => {
                let Some(app_id) = self.by_transfer.remove(&id) else {
                    return false;
                };
                if let Some(download) = self.downloads.get_mut(&app_id) {
                    if let Some(job) = download.direct_mut() {
                        job.transfer = None;
                    }
                    download.running = false;
                }
                true
            }
            // A checkpoint changes nothing the UI shows: it is saved, to resume from after a
            // restart or a crash (#43).
            TransferEvent::Checkpoint { id, checkpoint } => {
                let Some(app_id) = self.by_transfer.get(&id).cloned() else {
                    return false;
                };
                if let Err(e) = self.database.save_checkpoint(&app_id, &checkpoint, now_ms) {
                    self.storage_error = Some(e);
                }
                true
            }
        }
    }
}

mod activity;
mod adding;
mod batch;
mod categories;
mod finished;
mod history;
mod options;
mod persist;
mod recovery;
mod service;
mod settings;
mod torrent_links;
mod torrents;

pub use adding::{
    is_torrent_link, AddOutcome, AddRequest, AddResult, Duplicate, FETCH_WAIT, MAX_ADD,
};
pub use batch::MAX_BATCH;
pub use recovery::SensitiveOptions;
pub use service::{now_ms, ManagerService, SNAPSHOT_EVERY, TICK};
pub use torrent_links::FetchReply;
pub use torrents::TorrentRequest;

#[cfg(test)]
mod tests;
