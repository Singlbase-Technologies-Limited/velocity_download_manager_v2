//! The torrent engine in process (#62): one libtorrent session behind the [`TorrentEngine`]
//! contract. The torrent helper runs it as a child process (`crate::stdio`); tests run it in
//! process too. Only `velocity-lt` calls libtorrent; this module decides what reaches it and what
//! its state means in V2's types.
//!
//! - **Untrusted input:** a `.torrent` file is read within [`MAX_TORRENT_BYTES`] and checked with
//!   the core's parser, and a magnet link is parsed and passed on in its canonical form, before
//!   libtorrent sees either. Extra trackers are checked too.
//! - **Folders:** a torrent's folder must be an existing absolute folder, not a link.
//! - **Magnets:** a magnet's metadata comes from peers, so every magnet is added with nothing
//!   wanted. Its metadata is checked with the core's parser when it arrives; only then are its
//!   files wanted, or it waits for the user to choose them. Metadata that fails the check fails the
//!   torrent before any byte of it is written (libtorrent itself would rename unsafe names and go
//!   on).
//! - **File choice:** a torrent downloads exactly the files chosen, by their index in the torrent;
//!   padding files never. Choosing again adds or drops files; nothing on disk is deleted.
//! - **Completion:** a torrent whose wanted pieces have all passed their hash check has its files
//!   flushed first, then stops, and only then reports Completed.
//! - **Seeding:** a completed torrent seeds only under a [`SeedPolicy`] other than `Never` (the
//!   default, ADR-0005), until its first limit is reached, and only while the seeding queue gives
//!   it a slot ([`TorrentSettings::max_active_seeds`]). The limits count libtorrent's own seeding
//!   time and upload totals, which its resume data keeps across restarts.
//! - **Files are never deleted** here: removing a torrent keeps its data.
//!
//! libtorrent has no push status, so a poller reads every torrent's state each [`POLL_INTERVAL`]
//! and sends a `changed` event when a snapshot differs from the last one sent.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use velocity_core::domain::{DownloadStatus, ErrorCode, FailureClass, Progress, TransferError};
use velocity_core::seeding::{seed_queue_plan, SeedPolicy};
use velocity_core::torrent::{
    check_tracker, file_priorities, parse_magnet, parse_metadata, parse_torrent, TorrentFile,
    MAX_TORRENT_BYTES,
};
use velocity_core::torrent_engine::{
    MetadataChunk, TorrentEngine, TorrentEvent, TorrentId, TorrentSettings, TorrentSnapshot,
    TorrentSource, TorrentSpec, METADATA_CHUNK_BYTES,
};
use velocity_lt::{state, AddParams, FileEntry, Session, SessionConfig, TorrentStatus, UniquePtr};

/// How often the poller reads libtorrent's state. A starting value, not a measured one.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The Azureus-style peer-id prefix: `-VD0100-` for version 0.1.0 (PO decision on #62; V1 used
/// `-VL`, which V2 doesn't reuse).
pub fn peer_fingerprint(version: &str) -> String {
    let digit = |s: Option<&str>| {
        s.and_then(|s| s.parse::<u32>().ok())
            .and_then(|n| char::from_digit(n, 10))
            .unwrap_or('0')
    };
    let mut it = version.split('.');
    let (a, b, c) = (digit(it.next()), digit(it.next()), digit(it.next()));
    format!("-VD{a}{b}{c}0-")
}

/// The user agent trackers see: `Velocity V2/0.1.0 libtorrent/2.0.15.0`.
pub fn user_agent() -> String {
    format!(
        "Velocity V2/{} libtorrent/{}",
        env!("CARGO_PKG_VERSION"),
        velocity_lt::version()
    )
}

/// The session's settings. Listening on a random port on every interface until #62's
/// port-mapping part adds a fixed port; router port mapping stays off (opt-in).
fn session_config(settings: TorrentSettings) -> SessionConfig {
    SessionConfig {
        listen_interfaces: "0.0.0.0:0,[::]:0".into(),
        enable_dht: settings.dht,
        enable_lsd: settings.local_peer_discovery,
        enable_upnp: false,
        enable_natpmp: false,
        dht_bootstrap_nodes: String::new(),
        user_agent: user_agent(),
        peer_fingerprint: peer_fingerprint(env!("CARGO_PKG_VERSION")),
        upload_rate_limit: 0,
        download_rate_limit: 0,
        connections_limit: 0,
    }
}

/// Where a torrent is on its way to Completed, and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finish {
    /// Still downloading, or paused.
    No,
    /// Every wanted piece passed its check; its files are being flushed.
    Flushing,
    /// Flushed and stopped: Completed.
    Done,
    /// Complete and uploading under its policy, since `since` (the engine's clock).
    Seeding { since: u64 },
    /// Complete and allowed to seed, stopped until the seeding queue gives it a slot. In the queue
    /// since `since`.
    SeedQueued { since: u64 },
    /// Complete and allowed to seed, paused by the user.
    SeedPaused,
}

impl Finish {
    /// Flushed: the download is complete, whether it seeds or not.
    fn complete(self) -> bool {
        !matches!(self, Finish::No | Finish::Flushing)
    }

    /// Complete, with seeding still allowed by its policy.
    fn may_seed(self) -> bool {
        matches!(
            self,
            Finish::Seeding { .. } | Finish::SeedQueued { .. } | Finish::SeedPaused
        )
    }
}

/// What V2 knows of a torrent's files, which decides what it may download.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// A magnet whose metadata hasn't arrived, or hasn't been checked yet: nothing is wanted.
    Unchecked,
    /// The files are known; nothing is wanted until [`TorrentEngine::set_files`] chooses.
    Choosing,
    /// The files are known and chosen.
    Wanted,
    /// The metadata failed V2's check, with why: nothing was downloaded.
    Refused(String),
}

/// The priorities that want every file but the padding.
fn every_file(files: &[TorrentFile]) -> Vec<u32> {
    files.iter().filter(|f| !f.pad).map(|f| f.index).collect()
}

/// What the poller does next for one torrent on its way to Completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Wait,
    /// Write out what libtorrent still has queued for the torrent's files.
    Flush,
    /// Stop the torrent; the seeding queue starts it again if its policy lets it seed.
    Stop,
}

/// The next step towards Completed, and where the torrent is once it is done. `finished`: every
/// wanted piece passed its hash check, without an error. `flushed`: libtorrent said, in this poll,
/// that the torrent's files are flushed. A torrent is completed only after a flush that started
/// once it had finished.
fn next_step(finish: Finish, finished: bool, flushed: bool) -> (Finish, Step) {
    match finish {
        Finish::No if finished => (Finish::Flushing, Step::Flush),
        Finish::Flushing if flushed => (Finish::Done, Step::Stop),
        other => (other, Step::Wait),
    }
}

/// The V2 state of one torrent from libtorrent's.
fn snapshot_of(id: TorrentId, s: &TorrentStatus, finish: Finish, phase: &Phase) -> TorrentSnapshot {
    let refused = match phase {
        Phase::Refused(why) => Some(why.as_str()),
        _ => None,
    };
    let failure = refused.or((!s.error.is_empty()).then_some(s.error.as_str()));
    let status = if failure.is_some() {
        DownloadStatus::Failed
    } else if let Finish::Seeding { .. } = finish {
        DownloadStatus::Seeding
    } else if finish == Finish::SeedPaused {
        DownloadStatus::Paused
    } else if finish.complete() {
        DownloadStatus::Completed
    } else if finish == Finish::Flushing {
        DownloadStatus::Verifying
    } else if s.paused {
        DownloadStatus::Paused
    } else if *phase == Phase::Unchecked {
        DownloadStatus::FetchingMetadata
    } else if *phase == Phase::Choosing {
        DownloadStatus::ChoosingFiles
    } else {
        match s.state {
            state::CHECKING_FILES | state::CHECKING_RESUME_DATA => DownloadStatus::Verifying,
            state::DOWNLOADING_METADATA => DownloadStatus::FetchingMetadata,
            state::DOWNLOADING => DownloadStatus::Downloading,
            // Finished but not yet flushed: the poller starts the flush.
            state::FINISHED | state::SEEDING => DownloadStatus::Verifying,
            _ => DownloadStatus::Queued,
        }
    };
    let bytes = |n: i64| u64::try_from(n).unwrap_or(0);
    let count = |n: i32| u32::try_from(n).unwrap_or(0);
    let text = |t: &str| (!t.is_empty()).then(|| t.to_owned());
    // Stopped after completing, it moves nothing, though libtorrent's averaged rates take a while
    // to fall.
    let stopped = finish.complete() && !matches!(finish, Finish::Seeding { .. });
    let speed = |rate: i64| if stopped { 0 } else { bytes(rate) };
    TorrentSnapshot {
        id,
        status,
        name: text(&s.name),
        info_hash_v1: text(&s.info_hash_v1),
        info_hash_v2: text(&s.info_hash_v2),
        progress: Progress {
            completed_bytes: bytes(s.total_wanted_done),
            // Unknown until the files are known and chosen.
            total_bytes: (*phase == Phase::Wanted || finish.complete())
                .then(|| bytes(s.total_wanted)),
            download_speed_bps: speed(s.download_rate),
            upload_speed_bps: speed(s.upload_rate),
            connections: count(s.num_peers),
            connection_detail: None,
        },
        peers: count(s.num_peers),
        seeds: count(s.num_seeds),
        uploaded_bytes: bytes(s.all_time_upload),
        seeding_secs: bytes(s.seeding_secs),
        seed_queued: failure.is_none() && matches!(finish, Finish::SeedQueued { .. }),
        error: failure.map(|why| TransferError::new(FailureClass::Permanent, None, why)),
    }
}

/// Reads a `.torrent` file of at most [`MAX_TORRENT_BYTES`].
fn read_torrent_file(path: &Path) -> Result<Vec<u8>, ErrorCode> {
    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => ErrorCode::Io,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_TORRENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Io)?;
    if bytes.len() > MAX_TORRENT_BYTES {
        return Err(ErrorCode::InvalidInput);
    }
    Ok(bytes)
}

/// The folder a torrent's files go in: an existing absolute folder, not a link, as text.
fn check_directory(directory: &Path) -> Result<String, ErrorCode> {
    if !directory.is_absolute() {
        return Err(ErrorCode::InvalidPath);
    }
    let meta = std::fs::symlink_metadata(directory).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ErrorCode::DestinationMissing,
        std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => ErrorCode::Io,
    })?;
    if !meta.is_dir() {
        return Err(ErrorCode::InvalidPath);
    }
    directory
        .to_str()
        .map(str::to_owned)
        .ok_or(ErrorCode::InvalidPath)
}

/// The bytes of the wanted files (priority above 0, padding never), and of those received: a
/// partial choice's progress. libtorrent's own counts are whole pieces, and a v1 piece spans files.
fn wanted_bytes(entries: &[FileEntry]) -> (u64, u64) {
    entries
        .iter()
        .filter(|f| f.priority > 0 && !f.pad)
        .fold((0, 0), |(done, total), f| {
            let size = u64::try_from(f.size).unwrap_or(0);
            let got = u64::try_from(f.done).unwrap_or(0).min(size);
            (done + got, total + size)
        })
}

struct Torrent {
    /// libtorrent's id for it, through the bridge; `None` while it is added again (`readd`).
    lt: Option<u64>,
    finish: Finish,
    phase: Phase,
    /// The torrent's files, once checked; for a magnet, from its metadata.
    files: Option<Vec<TorrentFile>>,
    /// Some files are wanted and others not: progress is counted per file.
    partial: bool,
    /// For a magnet: wait for the user's choice once the metadata is checked.
    choose_after_metadata: bool,
    /// When it stops seeding once complete.
    seeding: SeedPolicy,
    /// How it is added: its folder, trackers, pause and peer exchange, without metadata.
    base: AddParams,
    /// The metadata V2 checked (a `.torrent` file, or a magnet's `d4:info…e`), to add the torrent
    /// again with new files: after Choosing files, and when a completed torrent gets more files.
    metadata: Option<Vec<u8>>,
    /// The parameters to add it again with, while libtorrent still removes it. A torrent that
    /// wanted nothing has dropped its peers; added again, it finds them as any new one does.
    readd: Option<AddParams>,
    /// The last snapshot sent.
    last: Option<TorrentSnapshot>,
}

struct State {
    next_id: u64,
    settings: TorrentSettings,
    torrents: BTreeMap<TorrentId, Torrent>,
    subscribers: Vec<Sender<TorrentEvent>>,
}

impl State {
    fn emit(&mut self, event: TorrentEvent) {
        self.subscribers.retain(|s| s.send(event.clone()).is_ok());
    }
}

struct Shared {
    session: UniquePtr<Session>,
    state: Mutex<State>,
    stop: AtomicBool,
    /// Orders when torrents started seeding or joined the seeding queue.
    clock: AtomicU64,
}

/// What one poll reads of a torrent before talking to libtorrent.
struct Seen {
    id: TorrentId,
    lt: Option<u64>,
    finish: Finish,
    phase: Phase,
    partial: bool,
    readd: Option<AddParams>,
    choose_after_metadata: bool,
    seeding: SeedPolicy,
    base: AddParams,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Sets `id`'s place after completion, if it is still `lt`.
    fn set_finish(&self, id: TorrentId, lt: u64, finish: Finish) {
        let mut state = self.state();
        if let Some(t) = state.torrents.get_mut(&id) {
            if t.lt == Some(lt) {
                t.finish = finish;
            }
        }
    }

    /// libtorrent's id for `id`. `InvalidState` for the moment it is added again.
    fn lt_of(&self, id: TorrentId) -> Result<(u64, Finish), ErrorCode> {
        let state = self.state();
        let torrent = state.torrents.get(&id).ok_or(ErrorCode::NotFound)?;
        Ok((torrent.lt.ok_or(ErrorCode::InvalidState)?, torrent.finish))
    }

    /// One poll: adds again the torrents that wait for it, checks magnets' metadata, reads every
    /// torrent's state, moves finished torrents towards Completed, and sends a `changed` event for
    /// each snapshot that changed. libtorrent is called with no lock held.
    fn poll(&self) {
        let alerts = self.session.pop_alerts();
        let torrents: Vec<Seen> = self
            .state()
            .torrents
            .iter()
            .map(|(id, t)| Seen {
                id: *id,
                lt: t.lt,
                finish: t.finish,
                phase: t.phase.clone(),
                partial: t.partial,
                readd: t.readd.clone(),
                choose_after_metadata: t.choose_after_metadata,
                seeding: t.seeding,
                base: t.base.clone(),
            })
            .collect();
        for seen in torrents {
            if let Some(params) = &seen.readd {
                self.add_again(seen.id, params);
                continue;
            }
            let Some(lt) = seen.lt else { continue };
            self.poll_one(&seen, lt, &alerts);
        }
        self.run_seed_queue();
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    /// Starts queued torrents in free seeding slots, oldest in the queue first, and sends the
    /// newest seeders back to the queue when there are more than the limit.
    fn run_seed_queue(&self) {
        let (plan, lts) = {
            let state = self.state();
            let mut seeding = Vec::new();
            let mut queued = Vec::new();
            let mut lts = BTreeMap::new();
            for (id, t) in &state.torrents {
                let Some(lt) = t.lt else { continue };
                match t.finish {
                    Finish::Seeding { since } => seeding.push((*id, since)),
                    Finish::SeedQueued { since } => queued.push((*id, since)),
                    _ => continue,
                }
                lts.insert(*id, lt);
            }
            let plan = seed_queue_plan(state.settings.max_active_seeds, &seeding, &queued);
            (plan, lts)
        };
        for (ids, start) in [(plan.start, true), (plan.demote, false)] {
            for id in ids {
                let lt = lts[&id];
                let called = if start {
                    self.session.resume(lt)
                } else {
                    self.session.pause(lt)
                };
                // Refused: the next poll plans again.
                if called.is_err() {
                    continue;
                }
                let since = self.tick();
                let mut state = self.state();
                let Some(t) = state.torrents.get_mut(&id) else {
                    continue;
                };
                match (t.finish, start) {
                    (Finish::SeedQueued { .. }, true) if t.lt == Some(lt) => {
                        t.finish = Finish::Seeding { since }
                    }
                    (Finish::Seeding { .. }, false) if t.lt == Some(lt) => {
                        t.finish = Finish::SeedQueued { since }
                    }
                    // Changed meanwhile, by the user: theirs stands.
                    _ => {}
                }
            }
        }
    }

    /// Adds a torrent again once libtorrent has let go of it.
    fn add_again(&self, id: TorrentId, params: &AddParams) {
        let added = self.session.add(params);
        let mut state = self.state();
        let Some(torrent) = state.torrents.get_mut(&id) else {
            // Removed meanwhile: so is the new one.
            drop(state);
            if let Ok(lt) = added {
                self.session.remove(lt);
            }
            return;
        };
        match added {
            Ok(lt) => {
                torrent.lt = Some(lt);
                torrent.readd = None;
            }
            // The old one is still going away: the next poll tries again.
            Err(e) if e.what() == velocity_lt::DUPLICATE_TORRENT => {}
            Err(e) => {
                torrent.readd = None;
                torrent.phase =
                    Phase::Refused(format!("the torrent couldn't be added again: {}", e.what()));
            }
        }
    }

    fn poll_one(&self, seen: &Seen, lt: u64, alerts: &[velocity_lt::AlertEntry]) {
        let mut finish = seen.finish;
        let mut phase = seen.phase.clone();
        let flushed = alerts
            .iter()
            .any(|a| a.kind == "cache_flushed" && a.id == lt);
        let Ok(status) = self.session.status(lt) else {
            return;
        };
        let mut checked = None;
        let mut readd = None;
        if phase == Phase::Unchecked && status.has_metadata {
            match self.check_metadata(lt) {
                Ok((files, bytes)) => {
                    if seen.choose_after_metadata {
                        phase = Phase::Choosing;
                    } else {
                        let priorities =
                            file_priorities(&files, &every_file(&files)).unwrap_or_default();
                        readd = Some(AddParams {
                            torrent: bytes.clone(),
                            file_priorities: priorities,
                            ..seen.base.clone()
                        });
                        phase = Phase::Wanted;
                    }
                    checked = Some((files, bytes));
                }
                Err(why) => {
                    // Nothing was wanted, so nothing was written; stop it talking to peers.
                    let _ = self.session.pause(lt);
                    phase = Phase::Refused(why);
                }
            }
        }
        // With nothing wanted, libtorrent calls a torrent finished: only a chosen one is, and only
        // by a status read after it was chosen, so not in the poll that chose its files.
        let finished = seen.phase == Phase::Wanted
            && phase == Phase::Wanted
            && status.is_finished
            && status.error.is_empty();
        let (next, step) = next_step(finish, finished, flushed);
        let done = match step {
            Step::Wait => true,
            Step::Flush => self.session.flush(lt).is_ok(),
            Step::Stop => self.session.pause(lt).is_ok(),
        };
        // A step libtorrent refused is tried again at the next poll.
        if done {
            finish = next;
        }
        let partial = if seen.partial && phase == Phase::Wanted {
            self.session
                .files(lt)
                .ok()
                .map(|entries| wanted_bytes(&entries))
        } else {
            None
        };
        // Just completed and stopped: it joins the seeding queue if its policy lets it seed.
        if finish == Finish::Done
            && seen.finish != Finish::Done
            && seen.seeding != (SeedPolicy::Never {})
        {
            finish = Finish::SeedQueued { since: self.tick() };
        }
        if finish.may_seed() {
            let wanted = partial.map_or(
                u64::try_from(status.total_wanted).unwrap_or(0),
                |(_, total)| total,
            );
            let seeded = Duration::from_secs(u64::try_from(status.seeding_secs).unwrap_or(0));
            let uploaded = u64::try_from(status.all_time_upload).unwrap_or(0);
            if seen.seeding.done(seeded, uploaded, wanted) {
                // A seeding torrent that libtorrent didn't stop is tried again at the next poll.
                let stopped =
                    !matches!(finish, Finish::Seeding { .. }) || self.session.pause(lt).is_ok();
                if stopped {
                    finish = Finish::Done;
                }
            }
        }
        let mut snapshot = snapshot_of(seen.id, &status, finish, &phase);
        if let Some((done, total)) = partial {
            snapshot.progress.completed_bytes = done;
            snapshot.progress.total_bytes = Some(total);
        }
        let mut state = self.state();
        let Some(torrent) = state.torrents.get_mut(&seen.id) else {
            // Removed meanwhile.
            return;
        };
        // A choice, a pause or a policy made meanwhile changed the torrent: the next poll starts
        // from it.
        if torrent.phase != seen.phase
            || torrent.lt != Some(lt)
            || torrent.finish != seen.finish
            || torrent.seeding != seen.seeding
        {
            return;
        }
        torrent.phase = phase;
        torrent.finish = finish;
        if let Some((files, bytes)) = checked {
            torrent.files = Some(files);
            torrent.metadata = Some(bytes);
        }
        if readd.is_some() {
            // Out of the session now; the next poll adds it again with its files wanted.
            torrent.lt = None;
            torrent.readd = readd;
        }
        let removed = torrent.lt.is_none();
        if torrent.last.as_ref() != Some(&snapshot) {
            torrent.last = Some(snapshot.clone());
            state.emit(TorrentEvent::Changed {
                snapshot: Box::new(snapshot),
            });
        }
        drop(state);
        if removed {
            self.session.remove(lt);
        }
    }

    /// Checks a magnet's metadata, now that libtorrent has it. Returns its files and its bytes,
    /// or why the metadata is refused.
    fn check_metadata(&self, lt: u64) -> Result<(Vec<TorrentFile>, Vec<u8>), String> {
        let bytes = self
            .session
            .metadata(lt)
            .map_err(|e| format!("the torrent's metadata couldn't be read: {}", e.what()))?;
        let meta = parse_metadata(&bytes).map_err(|e| e.to_string())?;
        let listed = self
            .session
            .files(lt)
            .map_err(|e| format!("the torrent's files couldn't be read: {}", e.what()))?;
        if listed.len() != meta.files.len() {
            return Err(
                "libtorrent lists the torrent's files differently from its metadata.".into(),
            );
        }
        Ok((meta.files, bytes))
    }
}

/// A torrent as `snapshot` reads it: its id, libtorrent's id, where it is, and its last snapshot.
type Listed = (
    TorrentId,
    Option<u64>,
    Finish,
    Phase,
    Option<TorrentSnapshot>,
);

/// The torrent engine, with its poller running.
pub struct LtEngine {
    shared: Arc<Shared>,
    poller: Option<JoinHandle<()>>,
}

impl LtEngine {
    /// Starts a session with `settings`; its torrent ids start at `first_id`.
    pub fn start(first_id: u64, settings: TorrentSettings) -> Result<Self, String> {
        let session = velocity_lt::new_session(&session_config(settings))
            .map_err(|e| format!("starting libtorrent: {}", e.what()))?;
        let shared = Arc::new(Shared {
            session,
            state: Mutex::new(State {
                next_id: first_id.max(1),
                settings,
                torrents: BTreeMap::new(),
                subscribers: Vec::new(),
            }),
            stop: AtomicBool::new(false),
            clock: AtomicU64::new(0),
        });
        let polled = Arc::clone(&shared);
        let poller = thread::Builder::new()
            .name("velocity-torrent-poller".into())
            .spawn(move || {
                while !polled.stop.load(Ordering::Relaxed) {
                    polled.poll();
                    thread::sleep(POLL_INTERVAL);
                }
            })
            .map_err(|e| format!("starting the poller: {e}"))?;
        Ok(LtEngine {
            shared,
            poller: Some(poller),
        })
    }

    /// The TCP port libtorrent listens on, 0 before it listens.
    pub fn listen_port(&self) -> u16 {
        self.shared.session.listen_port()
    }
}

impl TorrentEngine for LtEngine {
    fn version(&self) -> Result<String, ErrorCode> {
        Ok(velocity_lt::version())
    }

    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        settings.check()?;
        self.shared
            .session
            .apply_config(&session_config(settings))
            .map_err(|_| ErrorCode::InvalidInput)?;
        self.shared.state().settings = settings;
        Ok(())
    }

    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        let save_path = check_directory(&spec.directory)?;
        for tracker in &spec.trackers {
            check_tracker(tracker).map_err(|e| e.code())?;
        }
        if spec.choose_files && spec.files.is_some() {
            return Err(ErrorCode::InvalidInput);
        }
        spec.seeding.check()?;
        let settings = self.shared.state().settings;
        let mut base = AddParams {
            save_path,
            paused: spec.paused,
            trackers: spec.trackers.clone(),
            disable_pex: !settings.peer_exchange,
            ..Default::default()
        };
        let (first, phase, files, partial, metadata) = match &spec.source {
            TorrentSource::File { path } => {
                let bytes = read_torrent_file(path)?;
                let meta = parse_torrent(&bytes).map_err(|e| e.code())?;
                let all = every_file(&meta.files);
                let (priorities, phase, partial) = if spec.choose_files {
                    (vec![0; meta.files.len()], Phase::Choosing, false)
                } else {
                    let wanted = spec.files.clone().unwrap_or_else(|| all.clone());
                    let priorities = file_priorities(&meta.files, &wanted).map_err(|e| e.code())?;
                    (priorities, Phase::Wanted, wanted.len() < all.len())
                };
                let first = AddParams {
                    torrent: bytes.clone(),
                    file_priorities: priorities,
                    ..base.clone()
                };
                (first, phase, Some(meta.files), partial, Some(bytes))
            }
            TorrentSource::Magnet { uri } => {
                // Its files are chosen once its metadata is known and checked.
                if spec.files.is_some() {
                    return Err(ErrorCode::InvalidInput);
                }
                let magnet = parse_magnet(uri).map_err(|e| e.code())?;
                // Added again from its metadata later: keep the link's trackers for then.
                for tracker in &magnet.trackers {
                    if !base.trackers.contains(tracker) {
                        base.trackers.push(tracker.clone());
                    }
                }
                let first = AddParams {
                    magnet: magnet.canonical_uri(),
                    // It wants nothing until its metadata is checked.
                    dont_download: true,
                    ..base.clone()
                };
                (first, Phase::Unchecked, None, false, None)
            }
        };
        let lt = self.shared.session.add(&first).map_err(|e| {
            if e.what() == velocity_lt::DUPLICATE_TORRENT {
                ErrorCode::FileExists
            } else {
                ErrorCode::InvalidInput
            }
        })?;
        let mut state = self.shared.state();
        let id = TorrentId(state.next_id);
        state.next_id += 1;
        state.torrents.insert(
            id,
            Torrent {
                lt: Some(lt),
                finish: Finish::No,
                phase,
                files,
                partial,
                choose_after_metadata: spec.choose_files,
                seeding: spec.seeding,
                base,
                metadata,
                readd: None,
                last: None,
            },
        );
        Ok(id)
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        match self.shared.lt_of(id)? {
            (_, Finish::Done) => Err(ErrorCode::InvalidState),
            (lt, finish) => {
                self.shared
                    .session
                    .pause(lt)
                    .map_err(|_| ErrorCode::NotFound)?;
                if finish.may_seed() {
                    self.shared.set_finish(id, lt, Finish::SeedPaused);
                }
                Ok(())
            }
        }
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        match self.shared.lt_of(id)? {
            // A completed torrent seeds again through `set_seeding`.
            (_, Finish::Done) => Err(ErrorCode::InvalidState),
            // Back in the seeding queue, which starts it when a slot is free.
            (lt, Finish::SeedPaused) => {
                let since = self.shared.tick();
                self.shared.set_finish(id, lt, Finish::SeedQueued { since });
                Ok(())
            }
            (_, Finish::Seeding { .. } | Finish::SeedQueued { .. }) => Ok(()),
            (lt, _) => self
                .shared
                .session
                .resume(lt)
                .map_err(|_| ErrorCode::NotFound),
        }
    }

    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let lt = {
            let mut state = self.shared.state();
            let torrent = state.torrents.remove(&id).ok_or(ErrorCode::NotFound)?;
            state.emit(TorrentEvent::Removed { id });
            torrent.lt
        };
        // The bridge never deletes files.
        if let Some(lt) = lt {
            self.shared.session.remove(lt);
        }
        Ok(())
    }

    fn set_files(&mut self, id: TorrentId, wanted: Vec<u32>) -> Result<(), ErrorCode> {
        let (lt, files, phase, finish) = {
            let state = self.shared.state();
            let torrent = state.torrents.get(&id).ok_or(ErrorCode::NotFound)?;
            match (&torrent.phase, &torrent.files, torrent.lt) {
                (Phase::Choosing | Phase::Wanted, Some(files), Some(lt)) => {
                    (lt, files.clone(), torrent.phase.clone(), torrent.finish)
                }
                _ => return Err(ErrorCode::InvalidState),
            }
        };
        let priorities = file_priorities(&files, &wanted).map_err(|e| e.code())?;
        let partial = wanted.len() < every_file(&files).len();
        // A torrent that wanted nothing, or that completed and stopped, has dropped its peers: it
        // goes out of the session and comes back with its files, and finds peers as a new one
        // does. libtorrent checks the files already on disk then.
        if phase == Phase::Choosing || finish.complete() {
            {
                let mut state = self.shared.state();
                let torrent = state.torrents.get_mut(&id).ok_or(ErrorCode::NotFound)?;
                if torrent.phase != phase || torrent.lt != Some(lt) {
                    return Err(ErrorCode::InvalidState);
                }
                let metadata = torrent.metadata.clone().ok_or(ErrorCode::InvalidState)?;
                torrent.readd = Some(AddParams {
                    torrent: metadata,
                    file_priorities: priorities,
                    ..torrent.base.clone()
                });
                torrent.lt = None;
                torrent.phase = Phase::Wanted;
                torrent.finish = Finish::No;
                torrent.partial = partial;
            }
            self.shared.session.remove(lt);
            return Ok(());
        }
        self.shared
            .session
            .set_file_priorities(lt, &priorities)
            .map_err(|_| ErrorCode::NotFound)?;
        let mut state = self.shared.state();
        if let Some(torrent) = state.torrents.get_mut(&id) {
            torrent.partial = partial;
        }
        Ok(())
    }

    fn metadata(&self, id: TorrentId, offset: u64) -> Result<MetadataChunk, ErrorCode> {
        let (lt, _) = self.shared.lt_of(id)?;
        let bytes = self
            .shared
            .session
            .metadata(lt)
            .map_err(|_| ErrorCode::InvalidState)?;
        Ok(MetadataChunk::of(&bytes, offset, METADATA_CHUNK_BYTES))
    }

    fn set_seeding(&mut self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        seeding.check()?;
        let since = self.shared.tick();
        let mut state = self.shared.state();
        let torrent = state.torrents.get_mut(&id).ok_or(ErrorCode::NotFound)?;
        torrent.seeding = seeding;
        // A completed torrent may seed again: the poller checks the new policy before the queue
        // starts it. One seeding under a policy now met is stopped by the poller.
        if torrent.finish == Finish::Done && seeding != (SeedPolicy::Never {}) {
            torrent.finish = Finish::SeedQueued { since };
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode> {
        let torrents: Vec<Listed> = self
            .shared
            .state()
            .torrents
            .iter()
            .map(|(id, t)| (*id, t.lt, t.finish, t.phase.clone(), t.last.clone()))
            .collect();
        Ok(torrents
            .into_iter()
            .filter_map(|(id, lt, finish, phase, last)| match lt {
                Some(lt) => {
                    let status = self.shared.session.status(lt).ok()?;
                    Some(snapshot_of(id, &status, finish, &phase))
                }
                // Being added again: its last state stands.
                None => last,
            })
            .collect())
    }

    fn subscribe(&mut self) -> Receiver<TorrentEvent> {
        let (tx, rx) = channel();
        self.shared.state().subscribers.push(tx);
        rx
    }
}

impl Drop for LtEngine {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(poller) = self.poller.take() {
            let _ = poller.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: u8) -> TorrentStatus {
        TorrentStatus {
            state,
            has_metadata: true,
            total_wanted: 100,
            total_wanted_done: 40,
            download_rate: 7,
            num_peers: 2,
            num_seeds: 1,
            name: "album".into(),
            info_hash_v1: "a".repeat(40),
            ..Default::default()
        }
    }

    #[test]
    fn the_peer_id_prefix_is_v2s_own() {
        assert_eq!(peer_fingerprint("0.1.0"), "-VD0100-");
        assert_eq!(peer_fingerprint("1.2.3"), "-VD1230-");
        // A part with two digits doesn't fit Azureus style; it becomes 0, as in V1.
        assert_eq!(peer_fingerprint("10.0.1"), "-VD0010-");
        assert_eq!(peer_fingerprint("x"), "-VD0000-");
        assert!(user_agent().starts_with("Velocity V2/"));
    }

    #[test]
    fn libtorrent_states_map_to_v2_statuses() {
        let id = TorrentId(1);
        let cases = [
            (state::CHECKING_FILES, DownloadStatus::Verifying),
            (state::CHECKING_RESUME_DATA, DownloadStatus::Verifying),
            (
                state::DOWNLOADING_METADATA,
                DownloadStatus::FetchingMetadata,
            ),
            (state::DOWNLOADING, DownloadStatus::Downloading),
            (state::FINISHED, DownloadStatus::Verifying),
            (state::SEEDING, DownloadStatus::Verifying),
        ];
        for (state, want) in cases {
            assert_eq!(
                snapshot_of(id, &status(state), Finish::No, &Phase::Wanted).status,
                want,
                "{state}"
            );
        }
        let paused = TorrentStatus {
            paused: true,
            ..status(state::DOWNLOADING)
        };
        assert_eq!(
            snapshot_of(id, &paused, Finish::No, &Phase::Wanted).status,
            DownloadStatus::Paused
        );
        // Completed only once flushed and stopped, whatever libtorrent says about pausing.
        assert_eq!(
            snapshot_of(
                id,
                &status(state::SEEDING),
                Finish::Flushing,
                &Phase::Wanted
            )
            .status,
            DownloadStatus::Verifying
        );
        assert_eq!(
            snapshot_of(id, &paused, Finish::Done, &Phase::Wanted).status,
            DownloadStatus::Completed
        );
    }

    /// Flush before complete: a finished torrent is flushed first, stopped only once libtorrent
    /// says the flush is done, and never completed straight away.
    #[test]
    fn a_finished_torrent_is_flushed_before_it_completes() {
        assert_eq!(
            next_step(Finish::No, false, false),
            (Finish::No, Step::Wait)
        );
        assert_eq!(
            next_step(Finish::No, true, false),
            (Finish::Flushing, Step::Flush)
        );
        // The flush alert can come in the same poll that sees the torrent finished: it is for an
        // earlier flush, if any, so the torrent still flushes first.
        assert_eq!(
            next_step(Finish::No, true, true),
            (Finish::Flushing, Step::Flush)
        );
        assert_eq!(
            next_step(Finish::Flushing, true, false),
            (Finish::Flushing, Step::Wait)
        );
        assert_eq!(
            next_step(Finish::Flushing, true, true),
            (Finish::Done, Step::Stop)
        );
        assert_eq!(
            next_step(Finish::Done, true, true),
            (Finish::Done, Step::Wait)
        );
    }

    /// File choice and magnets (#62): what V2 knows of the files decides the status.
    #[test]
    fn the_files_phase_decides_metadata_choosing_and_refused_statuses() {
        let id = TorrentId(1);
        let downloading = status(state::DOWNLOADING);
        let s = snapshot_of(id, &downloading, Finish::No, &Phase::Unchecked);
        assert_eq!(
            s.status,
            DownloadStatus::FetchingMetadata,
            "until it is checked"
        );
        let s = snapshot_of(id, &downloading, Finish::No, &Phase::Choosing);
        assert_eq!(s.status, DownloadStatus::ChoosingFiles);
        assert_eq!(s.progress.total_bytes, None, "nothing chosen yet");
        let s = snapshot_of(
            id,
            &downloading,
            Finish::No,
            &Phase::Refused("unsafe".into()),
        );
        assert_eq!(s.status, DownloadStatus::Failed);
        assert_eq!(s.error.unwrap().message, "unsafe");
        // A pause shows as such in every phase but a refusal.
        let paused = TorrentStatus {
            paused: true,
            ..status(state::DOWNLOADING)
        };
        for phase in [Phase::Unchecked, Phase::Choosing, Phase::Wanted] {
            let s = snapshot_of(id, &paused, Finish::No, &phase);
            assert_eq!(s.status, DownloadStatus::Paused, "{phase:?}");
        }
    }

    /// A partial choice counts only the wanted files' bytes, never a whole v1 piece's.
    #[test]
    fn a_partial_choice_counts_only_the_wanted_files() {
        let f = |size: i64, done: i64, priority: u8, pad: bool| FileEntry {
            path: String::new(),
            size,
            done,
            priority,
            pad,
        };
        let entries = [
            f(100, 100, 4, false),
            f(50, 50, 0, false),
            f(10, 0, 4, true),
            f(70, 30, 4, false),
        ];
        assert_eq!(wanted_bytes(&entries), (130, 170));
        // libtorrent's counts are untrusted: more done than the file holds counts as the file.
        assert_eq!(wanted_bytes(&[f(10, 99, 4, false)]), (10, 10));
    }

    #[test]
    fn every_file_but_the_padding_is_wanted_by_default() {
        let f = |index: u32, pad: bool| TorrentFile {
            index,
            path: vec![format!("f{index}")],
            length: 1,
            pad,
        };
        assert_eq!(every_file(&[f(1, false), f(2, true), f(3, false)]), [1, 3]);
    }

    #[test]
    fn an_error_fails_the_torrent_with_its_message() {
        let failed = TorrentStatus {
            error: "No space left on device".into(),
            ..status(state::DOWNLOADING)
        };
        let s = snapshot_of(TorrentId(1), &failed, Finish::Done, &Phase::Wanted);
        assert_eq!(s.status, DownloadStatus::Failed);
        let error = s.error.unwrap();
        assert_eq!(error.message, "No space left on device");
        assert!(!error.retryable);
    }

    #[test]
    fn a_snapshot_carries_progress_names_and_hashes() {
        let s = snapshot_of(
            TorrentId(4),
            &status(state::DOWNLOADING),
            Finish::No,
            &Phase::Wanted,
        );
        assert_eq!(s.name.as_deref(), Some("album"));
        assert_eq!(s.info_hash_v1.as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(s.info_hash_v2, None, "no v2 hash is made up");
        assert_eq!(s.progress.completed_bytes, 40);
        assert_eq!(s.progress.total_bytes, Some(100));
        assert_eq!((s.peers, s.seeds, s.progress.connections), (2, 1, 2));
        let magnet = TorrentStatus {
            has_metadata: false,
            name: String::new(),
            ..status(state::DOWNLOADING_METADATA)
        };
        let s = snapshot_of(TorrentId(4), &magnet, Finish::No, &Phase::Unchecked);
        assert_eq!((s.name, s.progress.total_bytes), (None, None));
    }

    /// Seeding (#62): what each place after completion shows, with libtorrent's totals.
    #[test]
    fn a_completed_torrent_shows_its_seeding_and_no_stale_speeds() {
        let s = TorrentStatus {
            upload_rate: 50,
            download_rate: 3,
            all_time_upload: 300,
            seeding_secs: 9,
            ..status(state::SEEDING)
        };
        let at = |finish| snapshot_of(TorrentId(1), &s, finish, &Phase::Wanted);
        let seeding = at(Finish::Seeding { since: 0 });
        assert_eq!(seeding.status, DownloadStatus::Seeding);
        assert_eq!(seeding.progress.upload_speed_bps, 50);
        assert_eq!((seeding.uploaded_bytes, seeding.seeding_secs), (300, 9));
        assert!(!seeding.seed_queued);
        let queued = at(Finish::SeedQueued { since: 0 });
        assert_eq!(
            (queued.status, queued.seed_queued),
            (DownloadStatus::Completed, true)
        );
        assert_eq!(at(Finish::SeedPaused).status, DownloadStatus::Paused);
        assert_eq!(at(Finish::Done).status, DownloadStatus::Completed);
        // Stopped, libtorrent's averaged rates still fall for a while: a stopped torrent moves
        // nothing, so it shows no speed.
        for stopped in [
            Finish::Done,
            Finish::SeedQueued { since: 0 },
            Finish::SeedPaused,
        ] {
            let p = at(stopped).progress;
            assert_eq!(
                (p.upload_speed_bps, p.download_speed_bps),
                (0, 0),
                "{stopped:?}"
            );
            // Its totals stay.
            assert_eq!(at(stopped).uploaded_bytes, 300);
        }
    }

    #[test]
    fn a_folder_must_exist_be_absolute_and_not_a_link() {
        assert_eq!(
            check_directory(Path::new("relative")),
            Err(ErrorCode::InvalidPath)
        );
        let dir = std::env::temp_dir().join(format!("v2-torrent-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(check_directory(&dir), Err(ErrorCode::DestinationMissing));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(check_directory(&dir).is_ok());
        let file = dir.join("f");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(check_directory(&file), Err(ErrorCode::InvalidPath));
        // Creating a link needs extra rights on Windows, so this part runs on Unix.
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&dir, &link).unwrap();
            assert_eq!(check_directory(&link), Err(ErrorCode::InvalidPath));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_torrent_file_is_refused_unread() {
        let dir = std::env::temp_dir().join(format!("v2-torrent-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.torrent");
        std::fs::write(&big, vec![b'd'; MAX_TORRENT_BYTES + 1]).unwrap();
        assert_eq!(read_torrent_file(&big), Err(ErrorCode::InvalidInput));
        assert_eq!(
            read_torrent_file(&dir.join("none")),
            Err(ErrorCode::NotFound)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
