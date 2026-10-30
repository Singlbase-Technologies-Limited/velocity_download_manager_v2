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
//! - **Resume data:** libtorrent's resume data for each torrent is sent out in chunks when it
//!   changed (at most every [`RESUME_EVERY`]), when the torrent completes, and when asked. Added
//!   back, it is checked with the core's `check_resume_data` first, and libtorrent gets the
//!   metadata V2 checked, not the copy inside it.
//! - **Trackers and peers:** tracker URLs leave the helper redacted, and what trackers and peers
//!   say of themselves is cleaned before it is shown.
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
use std::time::{Duration, Instant};

use crate::portmap::PortMapTracker;
use velocity_core::domain::{DownloadStatus, ErrorCode, FailureClass, Progress, TransferError};
use velocity_core::proxy::{Proxy, ProxyKind};
use velocity_core::seeding::{seed_queue_plan, SeedPolicy};
use velocity_core::torrent::{
    check_resume_data, check_tracker, file_priorities, parse_magnet, parse_metadata, parse_torrent,
    redact_tracker, TorrentFile, MAX_RESUME_BYTES, MAX_TORRENT_BYTES,
};
use velocity_core::torrent_engine::{
    Chunk, NetworkStatus, PeerStatus, TorrentEngine, TorrentEvent, TorrentId, TorrentSettings,
    TorrentSnapshot, TorrentSource, TorrentSpec, TrackerState, TrackerStatus, CHUNK_BYTES,
    MAX_RATE_LIMIT,
};
use velocity_lt::{
    state, AddParams, FileEntry, PeerEntry, Session, SessionConfig, TorrentStatus, TrackerEntry,
    UniquePtr,
};

/// How often the poller reads libtorrent's state. A starting value, not a measured one.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How often a torrent's resume data is sent at most while it changes. A starting value, not a
/// measured one: a crash loses at most this much of the record of what was downloaded.
pub const RESUME_EVERY: Duration = Duration::from_secs(10);

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

/// The port to listen on for the settings' `listen_port`: that port, or for 0 a free one picked
/// now. One port for every interface: libtorrent given port 0 would pick one per interface, and
/// the port reported to the app and to peers would then be only one of them.
fn listening_port(asked: u16) -> u16 {
    if asked != 0 {
        return asked;
    }
    // The system's free port, let go at once. Should another program take it meanwhile,
    // libtorrent says so as a listen error.
    std::net::TcpListener::bind(("0.0.0.0", 0))
        .and_then(|l| l.local_addr())
        .map_or(0, |a| a.port())
}

/// The session's settings: listening on `port` on every interface, router port mapping only when
/// the user turned it on, and the global limits. With a proxy (#192, PO decision), nothing goes
/// around it: peers, trackers and their names go through it, local peer discovery and port
/// mapping are off, and the DHT is on only through SOCKS5, which carries it. An HTTPS proxy,
/// which libtorrent can't use, turns all of them off and sets no proxy: the app starts no torrent
/// meanwhile.
fn session_config(settings: &TorrentSettings, port: u16, proxy: Option<&Proxy>) -> SessionConfig {
    let kind = proxy.map(|p| p.kind);
    let direct = kind.is_none();
    let (proxy_type, host, proxy_port, user, password) = match proxy {
        Some(p) if matches!(p.kind, ProxyKind::Http | ProxyKind::Socks5) => (
            if p.kind == ProxyKind::Http { 1 } else { 2 },
            p.host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned(),
            p.port,
            p.login
                .as_ref()
                .map(|l| l.user_name.clone())
                .unwrap_or_default(),
            p.login
                .as_ref()
                .map(|l| l.password.expose().to_owned())
                .unwrap_or_default(),
        ),
        _ => (0, String::new(), 0, String::new(), String::new()),
    };
    SessionConfig {
        listen_interfaces: format!("0.0.0.0:{port},[::]:{port}"),
        enable_dht: settings.dht && (direct || kind == Some(ProxyKind::Socks5)),
        enable_lsd: settings.local_peer_discovery && direct,
        enable_upnp: settings.port_mapping && direct,
        enable_natpmp: settings.port_mapping && direct,
        dht_bootstrap_nodes: settings.dht_bootstrap_nodes.join(","),
        user_agent: user_agent(),
        peer_fingerprint: peer_fingerprint(env!("CARGO_PKG_VERSION")),
        upload_rate_limit: rate(settings.upload_limit),
        download_rate_limit: rate(settings.download_limit),
        connections_limit: 0,
        proxy_type,
        proxy_host: host,
        proxy_port,
        proxy_username: user,
        proxy_password: password,
    }
}

/// A torrent's trackers, then the settings' extra ones it hasn't.
fn with_extra(own: &[String], extra: &[String]) -> Vec<String> {
    let mut all = own.to_vec();
    for tracker in extra {
        if !all.contains(tracker) {
            all.push(tracker.clone());
        }
    }
    all
}

/// A limit in bytes per second as libtorrent takes it; 0 for none.
fn rate(limit: u64) -> i32 {
    i32::try_from(limit).unwrap_or(i32::MAX)
}

/// Untrusted text from a tracker or a peer: without control characters, at most `max`
/// characters.
fn clean_text(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// One tracker as the user sees it: its URL redacted, and its message cleaned of control
/// characters and of the URL itself.
fn tracker_status(t: &TrackerEntry) -> TrackerStatus {
    let url = redact_tracker(&t.url);
    let state = if t.working {
        TrackerState::Working
    } else if t.updating {
        TrackerState::Updating
    } else if t.fails > 0 {
        TrackerState::Error
    } else {
        TrackerState::NotContacted
    };
    let message = clean_text(&t.message.replace(t.url.as_str(), &url), 300);
    let count = |n: i32| u32::try_from(n).ok();
    TrackerStatus {
        url,
        tier: t.tier,
        state,
        message: (!message.is_empty()).then_some(message),
        fails: u32::from(t.fails),
        next_announce_secs: u64::try_from(t.next_announce_secs).ok(),
        seeds: count(t.scrape_complete),
        peers: count(t.scrape_incomplete),
    }
}

/// One peer as the user sees it; what it says of itself is untrusted.
fn peer_status(p: &PeerEntry) -> PeerStatus {
    let progress = if p.progress.is_finite() {
        p.progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    PeerStatus {
        ip: p.ip.clone(),
        port: p.port,
        download_speed_bps: u64::try_from(p.download_rate).unwrap_or(0),
        upload_speed_bps: u64::try_from(p.upload_rate).unwrap_or(0),
        seed: p.seed,
        progress_permille: (progress * 1000.0).round() as u16,
        client: clean_text(&p.client, 64),
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
    /// When its resume data was last asked for.
    resume_asked: Option<Instant>,
    /// Paused by the user, or added paused: it doesn't start seeding by itself on completing.
    user_paused: bool,
}

struct State {
    next_id: u64,
    settings: TorrentSettings,
    torrents: BTreeMap<TorrentId, Torrent>,
    subscribers: Vec<Sender<TorrentEvent>>,
    /// Resume data uploaded for the next [`TorrentSource::Resume`], and its announced size.
    upload: Vec<u8>,
    upload_total: u64,
    /// The port asked of libtorrent: the settings' port, or the one picked for 0.
    port: u16,
    /// Why libtorrent couldn't listen, since the port last changed: a later success on another
    /// interface doesn't hide it, since peers on the failed one can't connect.
    listen_error: Option<String>,
    portmap: PortMapTracker,
    /// The user's proxy (#192), applied with every change of the settings.
    proxy: Option<Proxy>,
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
    resume_asked: Option<Instant>,
    user_paused: bool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Notes whether the user paused `id`.
    fn set_user_paused(&self, id: TorrentId, paused: bool) {
        if let Some(t) = self.state().torrents.get_mut(&id) {
            t.user_paused = paused;
        }
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
        self.note_network(&alerts);
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
                resume_asked: t.resume_asked,
                user_paused: t.user_paused,
            })
            .collect();
        self.send_resume_data(&alerts);
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

    /// Notes what libtorrent says about listening and port mapping.
    fn note_network(&self, alerts: &[velocity_lt::AlertEntry]) {
        let mut state = self.state();
        for alert in alerts {
            let port = u16::try_from(alert.port).unwrap_or(0);
            match alert.kind.as_str() {
                "listen_failed" => {
                    state.listen_error =
                        Some(format!("port {port}: {}", clean_text(&alert.message, 300)));
                }
                "portmap" => state.portmap.on_mapped(&alert.transport, port),
                "portmap_error" => state
                    .portmap
                    .on_error(&alert.transport, &clean_text(&alert.message, 300)),
                _ => {}
            }
        }
    }

    /// Sends the resume data libtorrent saved, in chunks, for the torrents still here.
    fn send_resume_data(&self, alerts: &[velocity_lt::AlertEntry]) {
        let mut state = self.state();
        for alert in alerts {
            let saved = match alert.kind.as_str() {
                "save_resume_data" => true,
                "save_resume_data_failed" => false,
                _ => continue,
            };
            let Some(id) = state
                .torrents
                .iter()
                .find(|(_, t)| t.lt == Some(alert.id))
                .map(|(id, _)| *id)
            else {
                // Removed, or added again meanwhile.
                continue;
            };
            if !saved {
                eprintln!(
                    "the resume data of torrent {} couldn't be saved: {}",
                    id.0, alert.message
                );
                continue;
            }
            let mut offset = 0;
            while offset < alert.data.len() {
                let chunk = Chunk::of(&alert.data, offset as u64, CHUNK_BYTES);
                offset += CHUNK_BYTES;
                state.emit(TorrentEvent::ResumeData { id, chunk });
            }
        }
    }

    /// Asks libtorrent for `lt`'s resume data, and notes when.
    fn ask_resume(&self, id: TorrentId, lt: u64) -> bool {
        if self.session.request_resume_data(lt).is_err() {
            return false;
        }
        if let Some(t) = self.state().torrents.get_mut(&id) {
            t.resume_asked = Some(Instant::now());
        }
        true
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
        // Just completed and stopped: it joins the seeding queue if its policy lets it seed, or
        // stays paused if the user paused it.
        if finish == Finish::Done
            && seen.finish != Finish::Done
            && seen.seeding != (SeedPolicy::Never {})
        {
            finish = if seen.user_paused {
                Finish::SeedPaused
            } else {
                Finish::SeedQueued { since: self.tick() }
            };
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
            return;
        }
        // Without metadata there is nothing to add it back from: a magnet is added again by its
        // link.
        let due = seen
            .resume_asked
            .is_none_or(|asked| asked.elapsed() >= RESUME_EVERY);
        let completed = finish.complete() && !seen.finish.complete();
        if status.has_metadata && ((status.need_save_resume && due) || completed) {
            self.ask_resume(seen.id, lt);
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
        let mut portmap = PortMapTracker::default();
        portmap.reset(settings.port_mapping, Instant::now());
        let port = listening_port(settings.listen_port);
        let session = velocity_lt::new_session(&session_config(&settings, port, None))
            .map_err(|e| format!("starting libtorrent: {}", e.what()))?;
        let shared = Arc::new(Shared {
            session,
            state: Mutex::new(State {
                next_id: first_id.max(1),
                settings,
                torrents: BTreeMap::new(),
                subscribers: Vec::new(),
                upload: Vec::new(),
                upload_total: 0,
                port,
                listen_error: None,
                portmap,
                proxy: None,
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

    /// Adds a torrent under the next id, or under `given` when an earlier engine gave it that id.
    fn add_as(
        &mut self,
        spec: TorrentSpec,
        given: Option<TorrentId>,
    ) -> Result<TorrentId, ErrorCode> {
        if let Some(id) = given {
            let state = self.shared.state();
            if id.0 == 0 || id.0 >= state.next_id || state.torrents.contains_key(&id) {
                return Err(ErrorCode::InvalidInput);
            }
        }
        let save_path = check_directory(&spec.directory)?;
        for tracker in &spec.trackers {
            check_tracker(tracker).map_err(|e| e.code())?;
        }
        if spec.choose_files && spec.files.is_some() {
            return Err(ErrorCode::InvalidInput);
        }
        spec.seeding.check()?;
        if spec.upload_limit > MAX_RATE_LIMIT || spec.download_limit > MAX_RATE_LIMIT {
            return Err(ErrorCode::InvalidInput);
        }
        let settings = self.shared.state().settings.clone();
        let mut base = AddParams {
            save_path,
            paused: spec.paused,
            trackers: with_extra(&spec.trackers, &settings.extra_trackers),
            disable_pex: !settings.peer_exchange,
            upload_limit: rate(spec.upload_limit),
            download_limit: rate(spec.download_limit),
            max_connections: i32::try_from(settings.max_peers_per_torrent).unwrap_or(i32::MAX),
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
            TorrentSource::Resume {} => {
                let bytes = {
                    let mut state = self.shared.state();
                    let complete =
                        !state.upload.is_empty() && state.upload.len() as u64 == state.upload_total;
                    let bytes = std::mem::take(&mut state.upload);
                    state.upload_total = 0;
                    if !complete {
                        return Err(ErrorCode::InvalidInput);
                    }
                    bytes
                };
                // The resume data holds the choice; there is none to wait for.
                if spec.choose_files {
                    return Err(ErrorCode::InvalidInput);
                }
                let checked = check_resume_data(&bytes).map_err(|e| e.code())?;
                let priorities = match &spec.files {
                    Some(wanted) => {
                        file_priorities(&checked.meta.files, wanted).map_err(|e| e.code())?
                    }
                    // The resume data's own choice.
                    None => Vec::new(),
                };
                let first = AddParams {
                    torrent: checked.metadata.clone(),
                    resume_data: bytes,
                    file_priorities: priorities,
                    ..base.clone()
                };
                // Its phase is known once libtorrent has read the choice.
                (
                    first,
                    Phase::Wanted,
                    Some(checked.meta.files),
                    false,
                    Some(checked.metadata),
                )
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
        let (phase, partial) = if let TorrentSource::Resume {} = spec.source {
            // From the files libtorrent wants now: none chosen means Choosing files.
            let entries = self.shared.session.files(lt).unwrap_or_default();
            let all = entries.iter().filter(|f| !f.pad).count();
            let wanted = entries.iter().filter(|f| !f.pad && f.priority > 0).count();
            if wanted == 0 && all > 0 {
                (Phase::Choosing, false)
            } else {
                (Phase::Wanted, wanted < all)
            }
        } else {
            (phase, partial)
        };
        let mut state = self.shared.state();
        let id = match given {
            // Given meanwhile to another torrent: this one goes.
            Some(id) if state.torrents.contains_key(&id) => {
                drop(state);
                self.shared.session.remove(lt);
                return Err(ErrorCode::InvalidInput);
            }
            Some(id) => id,
            None => {
                state.next_id += 1;
                TorrentId(state.next_id - 1)
            }
        };
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
                resume_asked: None,
                user_paused: spec.paused,
            },
        );
        Ok(id)
    }
}

impl TorrentEngine for LtEngine {
    fn version(&self) -> Result<String, ErrorCode> {
        Ok(velocity_lt::version())
    }

    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        settings.check()?;
        let (moved, port) = {
            let state = self.shared.state();
            let moved = state.settings.listen_port != settings.listen_port;
            // Another setting changed: a random port picked before stays.
            let port = if moved {
                listening_port(settings.listen_port)
            } else {
                state.port
            };
            (moved, port)
        };
        self.shared
            .session
            .apply_config(&session_config(
                &settings,
                port,
                self.shared.state().proxy.as_ref(),
            ))
            .map_err(|_| ErrorCode::InvalidInput)?;
        let mut state = self.shared.state();
        state.port = port;
        let old = &state.settings;
        let mapping = moved || old.port_mapping != settings.port_mapping;
        if moved {
            state.listen_error = None;
        }
        if mapping {
            state.portmap.reset(settings.port_mapping, Instant::now());
        }
        state.settings = settings;
        Ok(())
    }

    fn set_proxy(&mut self, proxy: Option<Proxy>) -> Result<(), ErrorCode> {
        if let Some(proxy) = &proxy {
            proxy.check()?;
        }
        let (settings, port) = {
            let state = self.shared.state();
            (state.settings.clone(), state.port)
        };
        self.shared
            .session
            .apply_config(&session_config(&settings, port, proxy.as_ref()))
            .map_err(|_| ErrorCode::InvalidInput)?;
        self.shared.state().proxy = proxy;
        Ok(())
    }

    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        self.add_as(spec, None)
    }

    fn restore(&mut self, id: TorrentId, spec: TorrentSpec) -> Result<(), ErrorCode> {
        self.add_as(spec, Some(id)).map(|_| ())
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        match self.shared.lt_of(id)? {
            (_, Finish::Done) => Err(ErrorCode::InvalidState),
            (lt, finish) => {
                self.shared
                    .session
                    .pause(lt)
                    .map_err(|_| ErrorCode::NotFound)?;
                self.shared.set_user_paused(id, true);
                if finish.may_seed() {
                    self.shared.set_finish(id, lt, Finish::SeedPaused);
                }
                Ok(())
            }
        }
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let resumed = match self.shared.lt_of(id)? {
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
        };
        if resumed.is_ok() {
            self.shared.set_user_paused(id, false);
        }
        resumed
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

    fn metadata(&self, id: TorrentId, offset: u64) -> Result<Chunk, ErrorCode> {
        let (lt, _) = self.shared.lt_of(id)?;
        let bytes = self
            .shared
            .session
            .metadata(lt)
            .map_err(|_| ErrorCode::InvalidState)?;
        Ok(Chunk::of(&bytes, offset, CHUNK_BYTES))
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

    fn upload(&mut self, chunk: Chunk) -> Result<(), ErrorCode> {
        let bytes = chunk.bytes().map_err(|_| ErrorCode::InvalidInput)?;
        let mut state = self.shared.state();
        if chunk.offset == 0 {
            state.upload.clear();
            state.upload_total = chunk.total;
        }
        let fits = chunk.total <= MAX_RESUME_BYTES as u64
            && chunk.total == state.upload_total
            && chunk.offset == state.upload.len() as u64
            && (state.upload.len() + bytes.len()) as u64 <= chunk.total;
        if !fits {
            state.upload = Vec::new();
            state.upload_total = 0;
            return Err(ErrorCode::InvalidInput);
        }
        state.upload.extend(bytes);
        Ok(())
    }

    fn save_resume(&mut self) -> Result<Vec<TorrentId>, ErrorCode> {
        let torrents: Vec<(TorrentId, u64)> = self
            .shared
            .state()
            .torrents
            .iter()
            .filter_map(|(id, t)| t.lt.map(|lt| (*id, lt)))
            .collect();
        Ok(torrents
            .into_iter()
            .filter(|(_, lt)| {
                self.shared
                    .session
                    .status(*lt)
                    .is_ok_and(|s| s.has_metadata)
            })
            .filter(|(id, lt)| self.shared.ask_resume(*id, *lt))
            .map(|(id, _)| id)
            .collect())
    }

    fn set_limits(&mut self, id: TorrentId, upload: u64, download: u64) -> Result<(), ErrorCode> {
        if upload > MAX_RATE_LIMIT || download > MAX_RATE_LIMIT {
            return Err(ErrorCode::InvalidInput);
        }
        let (lt, _) = self.shared.lt_of(id)?;
        self.shared
            .session
            .set_limits(lt, rate(upload), rate(download))
            .map_err(|_| ErrorCode::NotFound)?;
        // Kept for when the torrent is added again.
        if let Some(t) = self.shared.state().torrents.get_mut(&id) {
            t.base.upload_limit = rate(upload);
            t.base.download_limit = rate(download);
        }
        Ok(())
    }

    fn trackers(&self, id: TorrentId) -> Result<Vec<TrackerStatus>, ErrorCode> {
        let (lt, _) = self.shared.lt_of(id)?;
        let entries = self
            .shared
            .session
            .trackers(lt)
            .map_err(|_| ErrorCode::NotFound)?;
        Ok(entries.iter().map(tracker_status).collect())
    }

    fn reannounce(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let (lt, _) = self.shared.lt_of(id)?;
        self.shared
            .session
            .force_reannounce(lt)
            .map_err(|_| ErrorCode::NotFound)
    }

    fn peers(&self, id: TorrentId) -> Result<Vec<PeerStatus>, ErrorCode> {
        let (lt, _) = self.shared.lt_of(id)?;
        let entries = self
            .shared
            .session
            .peers(lt)
            .map_err(|_| ErrorCode::NotFound)?;
        Ok(entries.iter().map(peer_status).collect())
    }

    fn network(&self) -> Result<NetworkStatus, ErrorCode> {
        let listen_port = self.shared.session.listen_port();
        let state = self.shared.state();
        // libtorrent moves on to the next free port when the one asked is taken; that stays,
        // and is said (PO decision on #62).
        let moved = (listen_port != 0 && listen_port != state.port).then(|| {
            format!(
                "port {} couldn't be opened; listening on {listen_port}",
                state.port
            )
        });
        Ok(NetworkStatus {
            listen_port,
            listen_error: state.listen_error.clone().or(moved),
            port_mapping: state.portmap.state(Instant::now()),
        })
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

    /// V1's `session_settings_follow_the_user_settings`, with V2's port.
    #[test]
    fn the_session_follows_the_settings() {
        let c = session_config(&TorrentSettings::default(), 6890, None);
        assert_eq!(c.listen_interfaces, "0.0.0.0:6890,[::]:6890");
        assert!(!c.enable_upnp && !c.enable_natpmp, "port mapping is opt-in");
        assert_eq!((c.upload_rate_limit, c.download_rate_limit), (0, 0));
        assert_eq!(c.dht_bootstrap_nodes, "", "libtorrent's own default nodes");
        let c = session_config(
            &TorrentSettings {
                dht_bootstrap_nodes: vec!["a.test:1".into(), "[2001:db8::1]:2".into()],
                ..TorrentSettings::default()
            },
            6890,
            None,
        );
        assert_eq!(c.dht_bootstrap_nodes, "a.test:1,[2001:db8::1]:2");
        let c = session_config(
            &TorrentSettings {
                listen_port: 0,
                port_mapping: true,
                dht: false,
                upload_limit: 262_144,
                download_limit: 1024,
                ..TorrentSettings::default()
            },
            41_000,
            None,
        );
        // A random port is picked first, and is the same on every interface.
        assert_eq!(c.listen_interfaces, "0.0.0.0:41000,[::]:41000");
        assert!(c.enable_upnp && c.enable_natpmp && !c.enable_dht);
        assert_eq!(
            (c.upload_rate_limit, c.download_rate_limit),
            (262_144, 1024)
        );
    }

    #[test]
    fn a_random_port_is_one_free_port_picked_up_front() {
        assert_eq!(listening_port(6890), 6890);
        let picked = listening_port(0);
        assert_ne!(picked, 0);
        // Free when picked: something can listen on it on every interface.
        assert!(std::net::TcpListener::bind(("0.0.0.0", picked)).is_ok());
    }

    /// V1's `tracker_errors_are_shown…` shows a tracker redacted, with its message; untrusted
    /// text is cleaned.
    #[test]
    fn a_tracker_is_shown_redacted_with_its_state_and_a_clean_message() {
        let url = "http://t.test:2710/a1b2c3secret/announce";
        let entry = |working, updating, fails, message: &str| TrackerEntry {
            url: url.into(),
            tier: 1,
            working,
            updating,
            fails,
            message: message.into(),
            next_announce_secs: 30,
            scrape_complete: 4,
            scrape_incomplete: -1,
        };
        let failing = tracker_status(&entry(false, false, 2, &format!("{url} offline\n\u{7}")));
        assert_eq!(failing.url, "http://t.test:2710/…");
        assert_eq!(failing.state, TrackerState::Error);
        assert_eq!(failing.fails, 2);
        let message = failing.message.unwrap();
        assert!(!message.contains("secret"), "{message}");
        assert!(!message.chars().any(char::is_control), "{message:?}");
        assert!(message.contains("offline"), "{message}");
        assert_eq!((failing.seeds, failing.peers), (Some(4), None));
        assert_eq!(failing.next_announce_secs, Some(30));
        assert_eq!(
            tracker_status(&entry(true, false, 0, "")).state,
            TrackerState::Working
        );
        assert_eq!(tracker_status(&entry(true, false, 0, "")).message, None);
        assert_eq!(
            tracker_status(&entry(false, true, 0, "")).state,
            TrackerState::Updating
        );
        assert_eq!(
            tracker_status(&entry(false, false, 0, "")).state,
            TrackerState::NotContacted
        );
        let unscheduled = TrackerEntry {
            next_announce_secs: -1,
            ..entry(true, false, 0, "")
        };
        assert_eq!(tracker_status(&unscheduled).next_announce_secs, None);
        // A long message is cut.
        let long = tracker_status(&entry(false, false, 1, &"x".repeat(5000)));
        assert!(long.message.unwrap().chars().count() <= 300);
    }

    #[test]
    fn a_peer_is_shown_with_what_it_says_of_itself_cleaned() {
        let p = peer_status(&PeerEntry {
            ip: "127.0.0.1".into(),
            port: 6890,
            download_rate: 10,
            upload_rate: -1,
            seed: true,
            am_choking: false,
            peer_choking: false,
            progress: 0.5004,
            client: format!("evil\u{1b}[31m{}", "z".repeat(200)),
        });
        assert_eq!((p.ip.as_str(), p.port), ("127.0.0.1", 6890));
        assert_eq!((p.download_speed_bps, p.upload_speed_bps), (10, 0));
        assert!(p.seed);
        assert_eq!(p.progress_permille, 500);
        assert!(!p.client.chars().any(char::is_control), "{:?}", p.client);
        assert!(p.client.chars().count() <= 64);
        // Progress outside 0 to 1 is untrusted too.
        let odd = peer_status(&PeerEntry {
            progress: f32::NAN,
            ..Default::default()
        });
        assert_eq!(odd.progress_permille, 0);
        let over = peer_status(&PeerEntry {
            progress: 7.0,
            ..Default::default()
        });
        assert_eq!(over.progress_permille, 1000);
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
