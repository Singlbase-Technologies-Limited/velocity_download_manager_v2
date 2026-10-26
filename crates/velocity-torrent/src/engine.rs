//! The torrent engine in process (#62): one libtorrent session behind the [`TorrentEngine`]
//! contract. The torrent helper runs it as a child process (`crate::stdio`); tests run it in
//! process too. Only `velocity-lt` calls libtorrent; this module decides what reaches it and what
//! its state means in V2's types.
//!
//! - **Untrusted input:** a `.torrent` file is read within [`MAX_TORRENT_BYTES`] and checked with
//!   the core's parser, and a magnet link is parsed and passed on in its canonical form, before
//!   libtorrent sees either. Extra trackers are checked too.
//! - **Folders:** a torrent's folder must be an existing absolute folder, not a link.
//! - **Completion:** a torrent whose wanted pieces have all passed their hash check has its files
//!   flushed first, then stops uploading (seeding is off by default, ADR-0005), and only then
//!   reports Completed.
//! - **Files are never deleted** here: removing a torrent keeps its data.
//!
//! libtorrent has no push status, so a poller reads every torrent's state each [`POLL_INTERVAL`]
//! and sends a `changed` event when a snapshot differs from the last one sent.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use velocity_core::domain::{DownloadStatus, ErrorCode, FailureClass, Progress, TransferError};
use velocity_core::torrent::{check_tracker, parse_magnet, parse_torrent, MAX_TORRENT_BYTES};
use velocity_core::torrent_engine::{
    TorrentEngine, TorrentEvent, TorrentId, TorrentSettings, TorrentSnapshot, TorrentSource,
    TorrentSpec,
};
use velocity_lt::{state, AddParams, Session, SessionConfig, TorrentStatus, UniquePtr};

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

/// Where a torrent is on its way to Completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finish {
    /// Still downloading, or paused.
    No,
    /// Every wanted piece passed its check; its files are being flushed.
    Flushing,
    /// Flushed and stopped: Completed.
    Done,
}

/// What the poller does next for one torrent on its way to Completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Wait,
    /// Write out what libtorrent still has queued for the torrent's files.
    Flush,
    /// Stop the torrent: seeding is off by default (ADR-0005).
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
fn snapshot_of(id: TorrentId, s: &TorrentStatus, finish: Finish) -> TorrentSnapshot {
    let status = if !s.error.is_empty() {
        DownloadStatus::Failed
    } else if finish == Finish::Done {
        DownloadStatus::Completed
    } else if finish == Finish::Flushing {
        DownloadStatus::Verifying
    } else if s.paused {
        DownloadStatus::Paused
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
    TorrentSnapshot {
        id,
        status,
        name: text(&s.name),
        info_hash_v1: text(&s.info_hash_v1),
        info_hash_v2: text(&s.info_hash_v2),
        progress: Progress {
            completed_bytes: bytes(s.total_wanted_done),
            total_bytes: s.has_metadata.then(|| bytes(s.total_wanted)),
            download_speed_bps: bytes(s.download_rate),
            upload_speed_bps: bytes(s.upload_rate),
            connections: count(s.num_peers),
            connection_detail: None,
        },
        peers: count(s.num_peers),
        seeds: count(s.num_seeds),
        error: (!s.error.is_empty())
            .then(|| TransferError::new(FailureClass::Permanent, None, &s.error)),
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

struct Torrent {
    /// libtorrent's id for it, through the bridge.
    lt: u64,
    finish: Finish,
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
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lt_of(&self, id: TorrentId) -> Result<(u64, Finish), ErrorCode> {
        self.state()
            .torrents
            .get(&id)
            .map(|t| (t.lt, t.finish))
            .ok_or(ErrorCode::NotFound)
    }

    /// One poll: reads the alerts and every torrent's state, moves finished torrents towards
    /// Completed, and sends a `changed` event for each snapshot that changed. libtorrent is
    /// called with no lock held.
    fn poll(&self) {
        let alerts = self.session.pop_alerts();
        let torrents: Vec<(TorrentId, u64, Finish)> = self
            .state()
            .torrents
            .iter()
            .map(|(id, t)| (*id, t.lt, t.finish))
            .collect();
        for (id, lt, mut finish) in torrents {
            let flushed = alerts
                .iter()
                .any(|a| a.kind == "cache_flushed" && a.id == lt);
            let Ok(status) = self.session.status(lt) else {
                continue;
            };
            let finished = status.is_finished && status.error.is_empty();
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
            let snapshot = snapshot_of(id, &status, finish);
            let mut state = self.state();
            let Some(torrent) = state.torrents.get_mut(&id) else {
                // Removed meanwhile.
                continue;
            };
            torrent.finish = finish;
            if torrent.last.as_ref() != Some(&snapshot) {
                torrent.last = Some(snapshot.clone());
                state.emit(TorrentEvent::Changed {
                    snapshot: Box::new(snapshot),
                });
            }
        }
    }
}

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
        let (torrent, magnet) = match &spec.source {
            TorrentSource::File { path } => {
                let bytes = read_torrent_file(path)?;
                parse_torrent(&bytes).map_err(|e| e.code())?;
                (bytes, String::new())
            }
            TorrentSource::Magnet { uri } => {
                let magnet = parse_magnet(uri).map_err(|e| e.code())?;
                (Vec::new(), magnet.canonical_uri())
            }
        };
        let settings = self.shared.state().settings;
        let params = AddParams {
            torrent,
            magnet,
            save_path,
            paused: spec.paused,
            trackers: spec.trackers.clone(),
            disable_pex: !settings.peer_exchange,
            ..Default::default()
        };
        let lt = self.shared.session.add(&params).map_err(|e| {
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
                lt,
                finish: Finish::No,
                last: None,
            },
        );
        Ok(id)
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        match self.shared.lt_of(id)? {
            (_, Finish::Done) => Err(ErrorCode::InvalidState),
            (lt, _) => self
                .shared
                .session
                .pause(lt)
                .map_err(|_| ErrorCode::NotFound),
        }
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        match self.shared.lt_of(id)? {
            // Seeding a completed torrent comes with the seeding policies (#62's later part).
            (_, Finish::Done) => Err(ErrorCode::InvalidState),
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
        self.shared.session.remove(lt);
        Ok(())
    }

    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode> {
        let torrents: Vec<(TorrentId, u64, Finish)> = self
            .shared
            .state()
            .torrents
            .iter()
            .map(|(id, t)| (*id, t.lt, t.finish))
            .collect();
        Ok(torrents
            .into_iter()
            .filter_map(|(id, lt, finish)| {
                let status = self.shared.session.status(lt).ok()?;
                Some(snapshot_of(id, &status, finish))
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
                snapshot_of(id, &status(state), Finish::No).status,
                want,
                "{state}"
            );
        }
        let paused = TorrentStatus {
            paused: true,
            ..status(state::DOWNLOADING)
        };
        assert_eq!(
            snapshot_of(id, &paused, Finish::No).status,
            DownloadStatus::Paused
        );
        // Completed only once flushed and stopped, whatever libtorrent says about pausing.
        assert_eq!(
            snapshot_of(id, &status(state::SEEDING), Finish::Flushing).status,
            DownloadStatus::Verifying
        );
        assert_eq!(
            snapshot_of(id, &paused, Finish::Done).status,
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

    #[test]
    fn an_error_fails_the_torrent_with_its_message() {
        let failed = TorrentStatus {
            error: "No space left on device".into(),
            ..status(state::DOWNLOADING)
        };
        let s = snapshot_of(TorrentId(1), &failed, Finish::Done);
        assert_eq!(s.status, DownloadStatus::Failed);
        let error = s.error.unwrap();
        assert_eq!(error.message, "No space left on device");
        assert!(!error.retryable);
    }

    #[test]
    fn a_snapshot_carries_progress_names_and_hashes() {
        let s = snapshot_of(TorrentId(4), &status(state::DOWNLOADING), Finish::No);
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
        let s = snapshot_of(TorrentId(4), &magnet, Finish::No);
        assert_eq!((s.name, s.progress.total_bytes), (None, None));
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
