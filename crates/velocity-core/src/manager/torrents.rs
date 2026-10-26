//! Torrents in the manager (#58): added and queued like direct downloads, driven through the
//! torrent contract (`TorrentEngine`, the torrent helper in the app), and stored without trackers.
//!
//! - **The queue (PO decision on #58):** a torrent takes a slot while it fetches its metadata or
//!   downloads, as in V1, where aria2 counted torrents. Choosing files, seeding (which has its own
//!   limit, `maxActiveSeeds`) and paused don't take one.
//! - **What is stored** (`database::torrents`): the magnet link without its trackers, or the
//!   `.torrent` file's path; the file choice, the seeding policy and the limits; the metadata, once
//!   known and checked against the torrent's info hash (#62); and libtorrent's resume data without
//!   its trackers ([`crate::torrent::strip_trackers`]). Tracker addresses can carry a private
//!   tracker's passkey: they stay in memory, and wait for the keychain (#70) to survive a restart
//!   (PO decision on #58).
//! - **Starting again:** a torrent comes back from its resume data when there is some, and
//!   otherwise from where it came from.
//! - **A helper that stops:** its supervisor adds the torrents back under the same ids itself
//!   (#62), so a torrent it reports lost ([`crate::torrent_engine::lost_by_helper`]) waits, with
//!   the reason shown, and keeps its slot. The manager never adds it again too.
//! - **Deleting a torrent's files** is the manager's decision, with the rules of
//!   [`crate::torrent_files`]; the helper never deletes.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{AddOptions, Download, Job, Manager};
use crate::database::{Engine, NewDownload, StoredOptions, TorrentOptions, TorrentOrigin};
use crate::domain::{DownloadKind, DownloadRecord, DownloadStatus, ErrorCode, Progress};
use crate::seeding::SeedPolicy;
use crate::torrent::{
    check_tracker, parse_magnet, parse_metadata, strip_trackers, MagnetInfo, TorrentMeta,
    MAX_RESUME_BYTES, MAX_TORRENT_BYTES,
};
use crate::torrent_engine::{
    lost_by_helper, Chunk, TorrentEngine, TorrentEvent, TorrentId, TorrentSnapshot, TorrentSource,
    TorrentSpec, CHUNK_BYTES, MAX_RATE_LIMIT,
};
use crate::torrent_files::{delete_owned, owned_paths, Kept};

/// A torrent to add.
#[derive(Clone, Debug, PartialEq)]
pub struct TorrentRequest {
    /// The `.torrent` file (an absolute path the helper reads and checks) or the magnet link.
    pub origin: TorrentOrigin,
    /// The folder its files go in: an existing absolute folder, which the helper checks.
    pub directory: PathBuf,
    /// Trackers to use beside the torrent's own; kept in memory only.
    pub trackers: Vec<String>,
    /// Wait in Choosing files until [`Manager::choose_files`].
    pub choose_files: bool,
    /// For a `.torrent` file, the files to download, by index; `None` for every file.
    pub files: Option<Vec<u32>>,
    pub seeding: SeedPolicy,
    /// Bytes per second; 0 for no limit.
    pub upload_limit: u64,
    pub download_limit: u64,
}

/// A torrent's part of a download.
pub(super) struct TorrentJob {
    /// How it is added again, as stored.
    pub(super) options: TorrentOptions,
    /// Where it came from this session, with any trackers in it; `None` after a restart.
    origin: Option<TorrentOrigin>,
    /// Extra trackers given this session; empty after a restart (#70).
    trackers: Vec<String>,
    /// `None` until the helper is given the torrent, and once it has forgotten it.
    pub(super) torrent: Option<TorrentId>,
    /// Resume data being put together from the helper's chunks: its total and the bytes so far.
    incoming: Option<(u64, Vec<u8>)>,
    /// How many times its resume data was saved, for [`Manager::prepare_to_close`].
    resume_saved: u64,
    /// Its metadata was fetched once it was known, whether or not it could be kept.
    metadata_fetched: bool,
    /// The info hashes and upload last saved.
    identity: (Option<String>, Option<String>, u64),
}

impl TorrentJob {
    pub(super) fn new(options: TorrentOptions, origin: Option<TorrentOrigin>) -> Self {
        TorrentJob {
            options,
            origin,
            trackers: Vec::new(),
            torrent: None,
            incoming: None,
            resume_saved: 0,
            metadata_fetched: false,
            identity: (None, None, 0),
        }
    }
}

/// The record's name for a magnet link before its metadata is known: its display name.
fn magnet_name(magnet: &MagnetInfo) -> Option<String> {
    magnet.display_name.clone()
}

impl Manager {
    /// The torrent engine to run torrents with: the torrent helper's supervisor in the app. Until
    /// it is given, torrents wait, and the snapshot says why.
    pub fn with_torrents(mut self, mut torrents: Box<dyn TorrentEngine>) -> Self {
        self.torrent_events = torrents.subscribe();
        self.torrents = torrents;
        self
    }

    /// Adds a torrent at the end of the queue and returns its app id, as [`Manager::add_with`]
    /// does for a direct download. Refused with nothing stored: `InvalidUrl` for a magnet link or
    /// tracker that isn't one, `InvalidPath` for a `.torrent` path that isn't absolute, and
    /// `InvalidInput` for a policy, a limit or a file choice outside its rules.
    pub fn add_torrent(
        &mut self,
        request: TorrentRequest,
        options: AddOptions,
        now_ms: u64,
    ) -> Result<String, ErrorCode> {
        request.seeding.check()?;
        let limits_ok = request.upload_limit <= MAX_RATE_LIMIT
            && request.download_limit <= MAX_RATE_LIMIT
            && request.trackers.len() <= crate::torrent::MAX_TRACKERS;
        let files_ok = match (&request.origin, &request.files) {
            (_, None) => true,
            (TorrentOrigin::Magnet { .. }, Some(_)) => false,
            (TorrentOrigin::File { .. }, Some(files)) => !files.is_empty() && !request.choose_files,
        };
        if !limits_ok || !files_ok {
            return Err(ErrorCode::InvalidInput);
        }
        for tracker in &request.trackers {
            check_tracker(tracker).map_err(|e| e.code())?;
        }
        let (kind, stored_origin, name) = match &request.origin {
            TorrentOrigin::Magnet { uri } => {
                let magnet = parse_magnet(uri).map_err(|e| e.code())?;
                let stored = MagnetInfo {
                    trackers: Vec::new(),
                    ..magnet.clone()
                }
                .canonical_uri();
                (
                    DownloadKind::Magnet,
                    TorrentOrigin::Magnet { uri: stored },
                    magnet_name(&magnet),
                )
            }
            TorrentOrigin::File { path } => {
                if !path.is_absolute() {
                    return Err(ErrorCode::InvalidPath);
                }
                (DownloadKind::Torrent, request.origin.clone(), None)
            }
        };
        let source_url = match &stored_origin {
            TorrentOrigin::Magnet { uri } => uri.clone(),
            TorrentOrigin::File { path } => path.to_string_lossy().into_owned(),
        };
        let stored = TorrentOptions {
            origin: stored_origin,
            choose_files: request.choose_files,
            files: request.files.clone(),
            seeding: request.seeding,
            upload_limit: request.upload_limit,
            download_limit: request.download_limit,
        };
        let paused =
            options.start_paused || (!self.settings.start_immediately && !options.scheduled);
        self.next_id += 1;
        let id = format!("d-{:04}", self.next_id);
        let mut job = TorrentJob::new(stored.clone(), Some(request.origin));
        job.trackers = request.trackers;
        let mut download = Download {
            record: DownloadRecord {
                id: id.clone(),
                kind,
                source_url: source_url.clone(),
                file_name: name.clone(),
                directory: request.directory.to_string_lossy().into_owned(),
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
            job: Job::Torrent(Box::new(job)),
            running: false,
            paused,
            scheduled: options.scheduled,
            saved_at_ms: None,
            needs_login: false,
        };
        download.record.status = download.waiting_status();
        let new = NewDownload {
            id: id.clone(),
            engine: Engine::Libtorrent,
            kind,
            status: download.record.status,
            source_url,
            directory: download.record.directory.clone(),
            file_name: name,
            category_id: None,
            options: StoredOptions::default(),
            has_sensitive_options: false,
            created_at_ms: now_ms,
        };
        self.database
            .insert_torrent(&new, &stored)
            .map_err(|e| e.code)?;
        let starts_now = download.may_run(self.window_open) && self.free_slots() > 0;
        self.downloads.insert(id.clone(), download);
        self.order.push(id.clone());
        if starts_now {
            match self.start(&id) {
                // No helper now: it waits, and the next fill tries again.
                Ok(()) | Err(ErrorCode::EngineUnavailable) => {}
                Err(code) => {
                    self.downloads.remove(&id);
                    self.order.retain(|o| *o != id);
                    self.database.delete_download(&id).map_err(|e| e.code)?;
                    return Err(code);
                }
            }
        }
        self.flush();
        Ok(id)
    }

    /// Downloads exactly `files` of the torrent, by index: after Choosing files, or later to add
    /// or drop files. The choice is stored. `InvalidState` before the helper has its metadata.
    pub fn choose_files(&mut self, id: &str, files: Vec<u32>) -> Result<(), ErrorCode> {
        let job = self.torrent_job(id)?;
        let torrent = job.torrent.ok_or(ErrorCode::InvalidState)?;
        self.torrents.set_files(torrent, files.clone())?;
        let job = self.torrent_job_mut(id)?;
        job.options.files = Some(files);
        job.options.choose_files = false;
        self.store_torrent_options(id);
        Ok(())
    }

    /// Sets when the torrent stops seeding; `SeedPolicy::Never` stops it now (V1's "stop
    /// seeding"). `InvalidInput` for a policy outside its limits.
    pub fn set_seeding(&mut self, id: &str, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        seeding.check()?;
        if let Some(torrent) = self.torrent_job(id)?.torrent {
            self.torrents.set_seeding(torrent, seeding)?;
        }
        self.torrent_job_mut(id)?.options.seeding = seeding;
        self.store_torrent_options(id);
        Ok(())
    }

    /// Sets a torrent's download limit, in bytes per second; 0 for none.
    pub(super) fn set_torrent_download_limit(
        &mut self,
        id: &str,
        download: u64,
    ) -> Result<(), ErrorCode> {
        if download > MAX_RATE_LIMIT {
            return Err(ErrorCode::InvalidInput);
        }
        let job = self.torrent_job(id)?;
        let upload = job.options.upload_limit;
        if let Some(torrent) = job.torrent {
            self.torrents.set_limits(torrent, upload, download)?;
        }
        self.torrent_job_mut(id)?.options.download_limit = download;
        self.store_torrent_options(id);
        Ok(())
    }

    /// The torrent helper's health, for the app to show.
    pub fn torrent_helper_health(&self) -> crate::domain::EngineHealth {
        self.torrents.health()
    }

    /// Starts a new torrent helper in place of the running one; its supervisor adds the torrents
    /// back under their ids.
    pub fn restart_torrent_helper(&mut self) -> Result<(), ErrorCode> {
        self.torrents.restart()
    }

    /// Asks the helper for every torrent's resume data and applies its events for up to
    /// `timeout`, until each torrent it named has saved a fresh copy: what the app does before it
    /// closes, so torrents come back where they were.
    pub fn prepare_to_close(&mut self, timeout: Duration, now_ms: u64) {
        let saved_before = |m: &Manager, t: TorrentId| {
            m.by_torrent
                .get(&t)
                .and_then(|id| m.downloads.get(id))
                .and_then(|d| match &d.job {
                    Job::Torrent(job) => Some(job.resume_saved),
                    Job::Direct(_) => None,
                })
        };
        let Ok(asked) = self.torrents.save_resume() else {
            // No helper: there is nothing newer to save than what is stored.
            return;
        };
        let before: Vec<(TorrentId, Option<u64>)> =
            asked.iter().map(|t| (*t, saved_before(self, *t))).collect();
        let deadline = Instant::now() + timeout;
        loop {
            self.apply_torrent_events(now_ms);
            let all = before
                .iter()
                .all(|(t, count)| saved_before(self, *t) != *count || count.is_none());
            if all || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.flush();
    }

    pub(super) fn torrent_job(&self, id: &str) -> Result<&TorrentJob, ErrorCode> {
        match &self.downloads.get(id).ok_or(ErrorCode::NotFound)?.job {
            Job::Torrent(job) => Ok(job),
            Job::Direct(_) => Err(ErrorCode::Unsupported),
        }
    }

    fn torrent_job_mut(&mut self, id: &str) -> Result<&mut TorrentJob, ErrorCode> {
        match &mut self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?.job {
            Job::Torrent(job) => Ok(job),
            Job::Direct(_) => Err(ErrorCode::Unsupported),
        }
    }

    fn store_torrent_options(&mut self, id: &str) {
        let Ok(job) = self.torrent_job(id) else {
            return;
        };
        if let Err(e) = self.database.update_torrent_options(id, &job.options) {
            self.storage_error = Some(e);
        }
    }

    /// Gives the helper a torrent: from its stored resume data when there is some, otherwise from
    /// where it came from; or resumes it when the helper has it.
    pub(super) fn start_torrent(&mut self, id: &str) -> Result<(), ErrorCode> {
        let directory = PathBuf::from(&self.downloads[id].record.directory);
        let job = self.torrent_job(id)?;
        if let Some(torrent) = job.torrent {
            return self.torrents.resume(torrent);
        }
        let stored = self.database.torrent(id).map_err(|e| e.code)?;
        let resume = stored.and_then(|t| t.resume_data);
        let job = self.torrent_job(id)?;
        let options = job.options.clone();
        let origin = job.origin.clone().unwrap_or(options.origin.clone());
        let trackers = job.trackers.clone();
        let source = match resume {
            Some(resume) => {
                let mut offset = 0;
                while offset < resume.len() {
                    self.torrents
                        .upload(Chunk::of(&resume, offset as u64, CHUNK_BYTES))?;
                    offset += CHUNK_BYTES;
                }
                TorrentSource::Resume {}
            }
            None => match origin {
                TorrentOrigin::File { path } => TorrentSource::File { path },
                TorrentOrigin::Magnet { uri } => TorrentSource::Magnet { uri },
            },
        };
        let from_resume = source == TorrentSource::Resume {};
        let spec = TorrentSpec {
            files: match &source {
                TorrentSource::File { .. } => options.files.clone(),
                _ => None,
            },
            choose_files: options.choose_files && !from_resume,
            source,
            directory,
            trackers,
            paused: false,
            seeding: options.seeding,
            upload_limit: options.upload_limit,
            download_limit: options.download_limit,
        };
        let torrent = self.torrents.add(spec)?;
        self.torrent_job_mut(id)?.torrent = Some(torrent);
        self.by_torrent.insert(torrent, id.to_owned());
        Ok(())
    }

    /// Has the helper forget the torrent. Its files are kept.
    pub(super) fn forget_torrent(&mut self, id: &str) -> Result<(), ErrorCode> {
        let Some(torrent) = self.torrent_job(id)?.torrent else {
            return Ok(());
        };
        match self.torrents.remove(torrent) {
            Ok(()) | Err(ErrorCode::NotFound) => {}
            Err(code) => return Err(code),
        }
        self.by_torrent.remove(&torrent);
        self.torrent_job_mut(id)?.torrent = None;
        Ok(())
    }

    /// Deletes a removed torrent's own files ([`crate::torrent_files`]): those of the files ever
    /// chosen, libtorrent's part file, and the folders left empty. Without stored metadata there
    /// is nothing it can have written.
    pub(super) fn delete_torrent_files(
        &self,
        id: &str,
        download: &Download,
    ) -> Vec<(PathBuf, Kept)> {
        let Job::Torrent(job) = &download.job else {
            return Vec::new();
        };
        let Some(meta) = self
            .database
            .torrent(id)
            .ok()
            .flatten()
            .and_then(|t| t.metadata)
            .and_then(|m| parse_metadata(&m).ok())
        else {
            return Vec::new();
        };
        let chosen: Vec<u32> = match &job.options.files {
            Some(files) => files.clone(),
            None => meta.user_files().map(|f| f.index).collect(),
        };
        let root = PathBuf::from(&download.record.directory);
        delete_owned(
            &owned_paths(&root, &meta, &chosen),
            super::finished::DELETE_WAIT,
        )
        .kept
    }

    /// Applies every pending event of the helper.
    pub(super) fn apply_torrent_events(&mut self, now_ms: u64) -> usize {
        let mut applied = 0;
        while let Ok(event) = self.torrent_events.try_recv() {
            if self.apply_torrent(event, now_ms) {
                applied += 1;
            }
        }
        applied
    }

    fn apply_torrent(&mut self, event: TorrentEvent, now_ms: u64) -> bool {
        match event {
            TorrentEvent::Changed { snapshot } => {
                let Some(id) = self.by_torrent.get(&snapshot.id).cloned() else {
                    return false;
                };
                self.torrent_changed(&id, &snapshot, now_ms);
                true
            }
            TorrentEvent::Removed { id: torrent } => {
                let Some(id) = self.by_torrent.remove(&torrent) else {
                    return false;
                };
                if let Ok(job) = self.torrent_job_mut(&id) {
                    job.torrent = None;
                }
                if let Some(d) = self.downloads.get_mut(&id) {
                    d.running = false;
                }
                true
            }
            TorrentEvent::ResumeData { id: torrent, chunk } => {
                let Some(id) = self.by_torrent.get(&torrent).cloned() else {
                    return false;
                };
                self.resume_chunk(&id, &chunk, now_ms);
                true
            }
        }
    }

    fn torrent_changed(&mut self, id: &str, snapshot: &TorrentSnapshot, now_ms: u64) {
        // Lost by a helper that stopped: its supervisor adds it back under the same id.
        if snapshot.status == DownloadStatus::Failed
            && snapshot.error.as_ref().is_some_and(lost_by_helper)
        {
            let d = self.downloads.get_mut(id).expect("known");
            d.record.status = d.waiting_status();
            d.record.error = snapshot.error.clone();
            self.mark(id);
            return;
        }
        let d = self.downloads.get_mut(id).expect("known");
        let before = d.record.clone();
        // The queue pauses torrents it holds back; they show as waiting.
        let status = if snapshot.status == DownloadStatus::Paused && !d.running {
            d.waiting_status()
        } else {
            snapshot.status
        };
        let record = &mut d.record;
        record.status = status;
        record.progress = snapshot.progress.clone();
        record.error = snapshot.error.clone();
        if snapshot.name.is_some() {
            record.file_name = snapshot.name.clone();
        }
        let complete = matches!(status, DownloadStatus::Completed | DownloadStatus::Seeding);
        if complete && record.completed_at_ms.is_none() {
            record.completed_at_ms = Some(now_ms);
        }
        if record.status.is_terminal() {
            d.running = false;
            self.order.retain(|o| o != id);
        }
        let d = &self.downloads[id];
        let record = &d.record;
        let changed = record.status != before.status
            || record.error != before.error
            || record.file_name != before.file_name
            || record.completed_at_ms != before.completed_at_ms;
        let due = d
            .saved_at_ms
            .is_none_or(|at| now_ms.saturating_sub(at) >= super::persist::PROGRESS_SAVE_MS);
        if changed || (record.progress != before.progress && due) {
            self.downloads.get_mut(id).expect("known").saved_at_ms = Some(now_ms);
            self.mark(id);
        }
        self.torrent_identity(id, snapshot);
        self.torrent_metadata(id, snapshot, now_ms);
    }

    /// Saves the torrent's info hashes when they change, and its upload with them.
    fn torrent_identity(&mut self, id: &str, snapshot: &TorrentSnapshot) {
        let identity = (
            snapshot.info_hash_v1.clone(),
            snapshot.info_hash_v2.clone(),
            snapshot.uploaded_bytes,
        );
        let Ok(job) = self.torrent_job(id) else {
            return;
        };
        let hashes_changed = (job.identity.0.as_ref(), job.identity.1.as_ref())
            != (identity.0.as_ref(), identity.1.as_ref());
        if !hashes_changed && job.identity.2 == identity.2 {
            return;
        }
        let saved = self.database.update_torrent_identity(
            id,
            identity.0.as_deref(),
            identity.1.as_deref(),
            identity.2,
        );
        match saved {
            Ok(()) => {
                if let Ok(job) = self.torrent_job_mut(id) {
                    job.identity = identity;
                }
            }
            Err(e) => self.storage_error = Some(e),
        }
    }

    /// Once the torrent's metadata is known, fetches it from the helper and keeps it if it is the
    /// torrent's: checked with [`parse_metadata`], and with the info hash the helper reports.
    fn torrent_metadata(&mut self, id: &str, snapshot: &TorrentSnapshot, now_ms: u64) {
        let known = snapshot.info_hash_v1.is_some() || snapshot.info_hash_v2.is_some();
        let fetching = snapshot.status == DownloadStatus::FetchingMetadata;
        let Ok(job) = self.torrent_job(id) else {
            return;
        };
        if job.metadata_fetched || !known || fetching {
            return;
        }
        if let Ok(job) = self.torrent_job_mut(id) {
            job.metadata_fetched = true;
        }
        let Some(metadata) = self.fetch_metadata(snapshot.id) else {
            return;
        };
        let theirs = |meta: &TorrentMeta| {
            meta.info_hash == snapshot.info_hash_v1 && meta.info_hash_v2 == snapshot.info_hash_v2
        };
        if parse_metadata(&metadata).is_ok_and(|meta| theirs(&meta)) {
            if let Err(e) = self.database.save_torrent_metadata(id, &metadata, now_ms) {
                self.storage_error = Some(e);
            }
        }
    }

    /// The torrent's metadata from the helper, in chunks; `None` when it can't be had whole.
    fn fetch_metadata(&self, torrent: TorrentId) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        loop {
            let chunk = self.torrents.metadata(torrent, bytes.len() as u64).ok()?;
            if chunk.total > MAX_TORRENT_BYTES as u64 || chunk.offset != bytes.len() as u64 {
                return None;
            }
            let part = chunk.bytes().ok()?;
            if part.is_empty() && (bytes.len() as u64) < chunk.total {
                return None;
            }
            bytes.extend(part);
            if bytes.len() as u64 >= chunk.total {
                return (bytes.len() as u64 == chunk.total).then_some(bytes);
            }
        }
    }

    /// Puts the helper's resume data together; once whole, saves it without its trackers. Data
    /// that is out of order, too large or not libtorrent's isn't kept, and the copy saved before
    /// stays: the helper sends it again within seconds.
    fn resume_chunk(&mut self, id: &str, chunk: &Chunk, now_ms: u64) {
        let Ok(job) = self.torrent_job_mut(id) else {
            return;
        };
        if chunk.offset == 0 {
            job.incoming =
                (chunk.total <= MAX_RESUME_BYTES as u64).then(|| (chunk.total, Vec::new()));
        }
        let Some((total, bytes)) = job.incoming.as_mut() else {
            return;
        };
        let in_order = chunk.total == *total && chunk.offset == bytes.len() as u64;
        match chunk.bytes() {
            Ok(part) if in_order && bytes.len() + part.len() <= *total as usize => {
                bytes.extend(part)
            }
            _ => {
                job.incoming = None;
                return;
            }
        }
        if (bytes.len() as u64) < *total {
            return;
        }
        let whole = job.incoming.take().map(|(_, b)| b).unwrap_or_default();
        let Ok((without, _trackers)) = strip_trackers(&whole) else {
            return;
        };
        match self.database.save_torrent_resume(id, &without, now_ms) {
            Ok(()) => {
                if let Ok(job) = self.torrent_job_mut(id) {
                    job.resume_saved += 1;
                }
            }
            Err(e) => self.storage_error = Some(e),
        }
    }
}
