//! The torrent engine contract (#62): what the manager asks of libtorrent and what it hears back,
//! in V2's own types. libtorrent runs in the torrent helper, a supervised child process
//! (ADR-0014, `crates/velocity-torrent`); only `crates/velocity-lt` calls it. Direct downloads
//! never pass through this contract, and torrents never pass through `DirectTransfer`.
//!

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};

use crate::domain::{DownloadStatus, ErrorCode, Progress, TransferError};
use crate::seeding::{SeedPolicy, MAX_ACTIVE_SEEDS};

/// Raised whenever a command, reply, event or shared type here changes incompatibly.
/// Version 1 (#62): settings for finding peers; add a `.torrent` file or a magnet link, pause,
/// resume, remove, snapshot.
/// Version 2 (#62's file choice): [`TorrentSpec::choose_files`] and [`TorrentSpec::files`], the
/// [`TorrentCommand::SetFiles`] and [`TorrentCommand::Metadata`] commands, and the Choosing files
/// status.
/// Version 3 (#62's lifecycle part): seeding ([`TorrentSpec::seeding`],
/// [`TorrentSettings::max_active_seeds`], the seeding fields of [`TorrentSnapshot`] and
/// [`TorrentCommand::SetSeeding`]); resume data ([`TorrentSource::Resume`],
/// [`TorrentEvent::ResumeData`], [`TorrentCommand::Upload`] and [`TorrentCommand::SaveResume`]);
/// [`TorrentCommand::Restore`]; `MetadataChunk` renamed [`Chunk`], with the same JSON.
/// Version 4 (#62's tracker and port-mapping part): the listening port, port mapping, limits,
/// extra trackers and peers per torrent in [`TorrentSettings`]; per-torrent limits in
/// [`TorrentSpec`] and [`TorrentCommand::SetLimits`]; [`TorrentCommand::Trackers`],
/// [`TorrentCommand::Reannounce`], [`TorrentCommand::Peers`] and [`TorrentCommand::Network`].
pub const TORRENT_CONTRACT_VERSION: u32 = 4;

/// The port the torrent helper listens on unless the user sets another (PO decision on #62):
/// not V1's 6881 or 6882.
pub const DEFAULT_LISTEN_PORT: u16 = 6890;
/// The lowest fixed listening port; 0 asks for a random one.
pub const MIN_LISTEN_PORT: u16 = 1024;
/// Peers per torrent, V1's default and limits.
pub const DEFAULT_MAX_PEERS: u32 = 55;
pub const MAX_PEERS: u32 = 512;
/// The highest speed limit, in bytes per second: libtorrent takes an `int`.
pub const MAX_RATE_LIMIT: u64 = i32::MAX as u64;
/// The most DHT bootstrap nodes the settings may name.
pub const MAX_DHT_NODES: usize = 16;
/// How long the helper looks for a router before it shows "no router found" (V1's).
pub const PORT_MAP_SEARCH_SECS: u64 = 30;

/// The most bytes one [`Chunk`] carries. As hex, twice that fits in one message of the
/// child-process protocol (4 MiB); a torrent's metadata may be up to 16 MiB, and its resume data
/// up to [`crate::torrent::MAX_RESUME_BYTES`].
pub const CHUNK_BYTES: usize = 1024 * 1024;

// A chunk as hex, with its envelope, fits in one message (`velocity_child`'s 4 MiB).
const _: () = assert!(CHUNK_BYTES * 2 + 1024 < 4 * 1024 * 1024);

/// The torrent engine's id for one torrent. Assigned by the engine, never reused, and never shown
/// as the app's download id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TorrentId(pub u64);

/// Where a torrent's metadata comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TorrentSource {
    /// A `.torrent` file. The engine reads it itself, so a large one never has to fit in a
    /// message, and checks it with [`crate::torrent::parse_torrent`] before libtorrent sees it.
    File { path: PathBuf },
    /// A magnet link, checked with [`crate::torrent::parse_magnet`]; libtorrent gets its
    /// canonical form only.
    Magnet { uri: String },
    /// libtorrent resume data, sent before with [`TorrentCommand::Upload`] and checked with
    /// [`crate::torrent::check_resume_data`]: the torrent comes back with its metadata, its
    /// pieces, its file choice and its seeding totals, without checking its files again. The
    /// spec's folder, trackers, pause and policy apply; `files` changes the choice.
    Resume {},
}

/// One torrent to add.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentSpec {
    pub source: TorrentSource,
    /// The folder the torrent's files go in: an existing absolute folder.
    pub directory: PathBuf,
    /// Trackers to use beside the torrent's own, each checked with
    /// [`crate::torrent::check_tracker`].
    pub trackers: Vec<String>,
    /// Added paused: nothing is downloaded until it is resumed.
    pub paused: bool,
    /// Download nothing until [`TorrentEngine::set_files`] chooses the files: a magnet fetches
    /// its metadata, then waits in Choosing files; a `.torrent` file waits there at once.
    pub choose_files: bool,
    /// For a `.torrent` file, the files to download, by [`crate::torrent::TorrentFile::index`];
    /// `None` downloads every file. Not with a magnet link, nor with `choose_files`.
    pub files: Option<Vec<u32>>,
    /// When it stops seeding once complete; checked with [`SeedPolicy::check`].
    pub seeding: SeedPolicy,
    /// Bytes per second, 0 for no limit, at most [`MAX_RATE_LIMIT`].
    pub upload_limit: u64,
    pub download_limit: u64,
}

/// The state of one torrent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentSnapshot {
    pub id: TorrentId,
    pub status: DownloadStatus,
    /// The torrent's name, once its metadata is known.
    pub name: Option<String>,
    /// Lowercase hexadecimal info hashes; `None` when the torrent has no such version, or before
    /// a magnet's metadata says whether it has.
    pub info_hash_v1: Option<String>,
    pub info_hash_v2: Option<String>,
    /// Bytes of the files wanted, and of those received and checked. `total_bytes` is `None`
    /// before the metadata is known.
    pub progress: Progress,
    /// Peers connected now, and how many of them have the whole torrent.
    pub peers: u32,
    pub seeds: u32,
    /// Bytes uploaded and seconds spent seeding over the torrent's life, a restart included: what
    /// its [`SeedPolicy`] is measured by.
    pub uploaded_bytes: u64,
    pub seeding_secs: u64,
    /// Completed and allowed to seed, but waiting for a seeding slot
    /// ([`TorrentSettings::max_active_seeds`]).
    pub seed_queued: bool,
    /// Why the torrent stopped, when it failed.
    pub error: Option<TransferError>,
}

/// The torrent helper's settings. The defaults are V1's (PO decisions on #62): DHT and peer
/// exchange on, local peer discovery off, port mapping off, no limits, 55 peers per torrent;
/// the listening port is V2's own. Private torrents never use DHT or peer exchange; libtorrent
/// enforces that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentSettings {
    pub dht: bool,
    /// Applies to torrents added after it is set.
    pub peer_exchange: bool,
    pub local_peer_discovery: bool,
    /// At most this many torrents seed at once, the rest wait in the seeding queue; 0 means no
    /// limit (V1's default), and more than [`MAX_ACTIVE_SEEDS`] is refused.
    pub max_active_seeds: u32,
    /// The TCP and UDP port the helper listens on: [`DEFAULT_LISTEN_PORT`], 0 for a random one,
    /// or one from [`MIN_LISTEN_PORT`].
    pub listen_port: u16,
    /// Ask the router to forward the listening port, by UPnP and NAT-PMP. Off unless the user
    /// turns it on.
    pub port_mapping: bool,
    /// The most bytes per second all torrents upload and download together; 0 for no limit, at
    /// most [`MAX_RATE_LIMIT`]. Separate from the direct downloads' limit (PO decision on #62).
    pub upload_limit: u64,
    pub download_limit: u64,
    /// Trackers added to every torrent added from now on, each checked with
    /// [`crate::torrent::check_tracker`]; at most [`crate::torrent::MAX_TRACKERS`].
    pub extra_trackers: Vec<String>,
    /// Peers each torrent connects to at most, 1 to [`MAX_PEERS`]; applies to torrents added
    /// from now on.
    pub max_peers_per_torrent: u32,
    /// Where DHT starts from, as `host:port` (V1's `dht_entry_point`); empty for libtorrent's own
    /// default nodes. At most [`MAX_DHT_NODES`].
    pub dht_bootstrap_nodes: Vec<String>,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        TorrentSettings {
            dht: true,
            peer_exchange: true,
            local_peer_discovery: false,
            max_active_seeds: 0,
            listen_port: DEFAULT_LISTEN_PORT,
            port_mapping: false,
            upload_limit: 0,
            download_limit: 0,
            extra_trackers: Vec::new(),
            max_peers_per_torrent: DEFAULT_MAX_PEERS,
            dht_bootstrap_nodes: Vec::new(),
        }
    }
}

impl TorrentSettings {
    /// `InvalidInput` for a value outside its limits, `InvalidUrl` for an extra tracker that
    /// isn't one.
    pub fn check(&self) -> Result<(), ErrorCode> {
        let in_range = self.max_active_seeds <= MAX_ACTIVE_SEEDS
            && (self.listen_port == 0 || self.listen_port >= MIN_LISTEN_PORT)
            && (1..=MAX_PEERS).contains(&self.max_peers_per_torrent)
            && self.upload_limit <= MAX_RATE_LIMIT
            && self.download_limit <= MAX_RATE_LIMIT
            && self.extra_trackers.len() <= crate::torrent::MAX_TRACKERS
            && self.dht_bootstrap_nodes.len() <= MAX_DHT_NODES
            && self.dht_bootstrap_nodes.iter().all(|n| is_host_port(n));
        if !in_range {
            return Err(ErrorCode::InvalidInput);
        }
        for tracker in &self.extra_trackers {
            crate::torrent::check_tracker(tracker).map_err(|e| e.code())?;
        }
        Ok(())
    }
}

/// `host:port`: a host name or IPv4 address of letters, digits, dots and hyphens, or an IPv6
/// address in brackets, and a port from 1.
fn is_host_port(node: &str) -> bool {
    let Some((host, port)) = node.rsplit_once(':') else {
        return false;
    };
    let port_ok = !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|p| p > 0);
    let host_ok = match host.strip_prefix('[') {
        Some(v6) => v6.strip_suffix(']').is_some_and(|a| {
            !a.is_empty()
                && a.bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        }),
        None => {
            !host.is_empty()
                && host.len() <= 253
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        }
    };
    port_ok && host_ok
}

/// Where a tracker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackerState {
    /// Its last announce succeeded.
    Working,
    /// Announcing now.
    Updating,
    /// Its last announce failed.
    Error,
    /// Not announced to yet: a lower tier, or the torrent just started.
    NotContacted,
}

/// One tracker of a torrent, as the user sees it (V1's details panel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackerStatus {
    /// Redacted with [`crate::torrent::redact_tracker`]: never a passkey or a login.
    pub url: String,
    pub tier: u8,
    pub state: TrackerState,
    /// The tracker's last message or error: untrusted text, without control characters, and
    /// without the tracker's own URL.
    pub message: Option<String>,
    pub fails: u32,
    pub next_announce_secs: Option<u64>,
    /// From the tracker's scrape, when it answered one.
    pub seeds: Option<u32>,
    pub peers: Option<u32>,
}

/// One connected peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerStatus {
    pub ip: String,
    pub port: u16,
    pub download_speed_bps: u64,
    pub upload_speed_bps: u64,
    /// It has the whole torrent.
    pub seed: bool,
    /// How much of the torrent it has, in thousandths.
    pub progress_permille: u16,
    /// The client it says it is: untrusted text, without control characters, at most 64
    /// characters.
    pub client: String,
}

/// Router port mapping, as the user sees it (V1's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PortMapState {
    /// The user hasn't turned it on.
    Off {},
    Searching {},
    /// A router forwards `external_port` to the helper; `via` is `upnp` or `natpmp`.
    Mapped {
        external_port: u16,
        via: String,
    },
    /// Every protocol that answered refused or failed.
    Failed {
        message: String,
    },
    /// Nothing answered within [`PORT_MAP_SEARCH_SECS`].
    NoRouter {},
}

/// How the helper reaches peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkStatus {
    /// The TCP port the helper listens on; 0 before it listens.
    pub listen_port: u16,
    /// Why the helper couldn't listen on the port asked for, if it couldn't.
    pub listen_error: Option<String>,
    pub port_mapping: PortMapState,
}

/// Part of a byte string too large for one message: a torrent's metadata (`d4:info…e`, which the
/// app checks with [`crate::torrent::parse_metadata`] before keeping it) or its resume data.
/// `total` bytes in all, of which this chunk holds the bytes from `offset` as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Chunk {
    pub total: u64,
    pub offset: u64,
    pub hex: String,
}

impl Chunk {
    /// At most `max` bytes of `metadata` from `offset`; empty past the end.
    pub fn of(metadata: &[u8], offset: u64, max: usize) -> Self {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(metadata.len());
        let end = start.saturating_add(max).min(metadata.len());
        Chunk {
            total: metadata.len() as u64,
            offset,
            hex: metadata[start..end]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        }
    }

    /// The chunk's bytes. Hex that isn't lowercase pairs of digits is refused: it came from
    /// another process.
    pub fn bytes(&self) -> Result<Vec<u8>, ErrorCode> {
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let hex = self.hex.as_bytes();
        if !hex.len().is_multiple_of(2) {
            return Err(ErrorCode::EngineIncompatible);
        }
        hex.chunks(2)
            .map(|pair| match (digit(pair[0]), digit(pair[1])) {
                (Some(h), Some(l)) => Ok(h * 16 + l),
                _ => Err(ErrorCode::EngineIncompatible),
            })
            .collect()
    }
}

/// What the torrent engine reports on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum TorrentEvent {
    /// Boxed, because a snapshot is much larger than an id; the JSON is the same.
    Changed {
        snapshot: Box<TorrentSnapshot>,
    },
    Removed {
        id: TorrentId,
    },
    /// libtorrent's resume data for the torrent, in chunks of at most [`CHUNK_BYTES`] sent in
    /// order: sent when it changed (at most every few seconds), when the torrent completes, and
    /// for [`TorrentCommand::SaveResume`]. It may hold tracker addresses with passkeys: it is
    /// never logged.
    ResumeData {
        id: TorrentId,
        chunk: Chunk,
    },
}

/// Commands fail with an [`ErrorCode`]: `NotFound` for an unknown id, `InvalidInput` for metadata,
/// a link or a folder that can't be used, `FileExists` for a torrent the engine already has, and
/// `EngineUnavailable` when the engine can't be reached. Torrent failures are not command
/// failures: they arrive as events with a [`TransferError`].
pub trait TorrentEngine: Send {
    /// The libtorrent version the engine runs, for diagnostics.
    fn version(&self) -> Result<String, ErrorCode>;
    /// Sets how the engine finds peers. An engine starts with [`TorrentSettings::default`].
    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode>;
    /// Starts a torrent: Fetching metadata for a magnet link, otherwise Queued (or Paused).
    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode>;
    /// Adds a torrent as [`TorrentEngine::add`] does, under `id`, which an earlier engine gave
    /// it: the torrent helper's supervisor gives a new helper a crashed one's torrents this way.
    /// `InvalidInput` for an id this engine has, or could still give.
    fn restore(&mut self, id: TorrentId, spec: TorrentSpec) -> Result<(), ErrorCode>;
    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    /// Forgets a torrent. Its files are always kept: deleting them is the manager's decision.
    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    /// Downloads exactly `files`, by [`crate::torrent::TorrentFile::index`], and none of the
    /// others: at once, after Choosing files, and later to add or drop files. Files already on
    /// disk are never deleted. `InvalidInput` for an empty list, a duplicate, a padding file or an
    /// index the torrent hasn't; `InvalidState` before the metadata is known.
    fn set_files(&mut self, id: TorrentId, files: Vec<u32>) -> Result<(), ErrorCode>;
    /// Part of the torrent's metadata, from `offset`. `InvalidState` before it is known.
    fn metadata(&self, id: TorrentId, offset: u64) -> Result<Chunk, ErrorCode>;
    /// Sets when the torrent stops seeding. A completed torrent whose new policy isn't met yet
    /// seeds again, through the seeding queue; one seeding whose new policy is met stops, so
    /// [`SeedPolicy::Never`] stops seeding. `InvalidInput` for a policy outside its limits.
    fn set_seeding(&mut self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode>;
    /// Part of the resume data the next [`TorrentSource::Resume`] adds from, in order from offset
    /// 0, which starts it anew. `InvalidInput` for a chunk out of order, or more than
    /// [`crate::torrent::MAX_RESUME_BYTES`] in all.
    fn upload(&mut self, chunk: Chunk) -> Result<(), ErrorCode>;
    /// Asks for every torrent's resume data now; it follows as [`TorrentEvent::ResumeData`].
    /// Returns the torrents asked for.
    fn save_resume(&mut self) -> Result<Vec<TorrentId>, ErrorCode>;
    /// Sets the torrent's upload and download limits, in bytes per second; 0 for none.
    /// `InvalidInput` above [`MAX_RATE_LIMIT`].
    fn set_limits(&mut self, id: TorrentId, upload: u64, download: u64) -> Result<(), ErrorCode>;
    /// The torrent's trackers, in tier order.
    fn trackers(&self, id: TorrentId) -> Result<Vec<TrackerStatus>, ErrorCode>;
    /// Announces the torrent to its trackers now, without waiting for their interval.
    fn reannounce(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    /// The torrent's connected peers.
    fn peers(&self, id: TorrentId) -> Result<Vec<PeerStatus>, ErrorCode>;
    /// The listening port and port mapping.
    fn network(&self) -> Result<NetworkStatus, ErrorCode>;
    /// The current state of every torrent the engine has.
    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode>;
    /// A new receiver for this engine's events, from the moment it subscribed.
    fn subscribe(&mut self) -> Receiver<TorrentEvent>;
}

/// One command, in the form that crosses a process boundary. As for the transfer contract,
/// variants without data are written `Snapshot {}` so their unknown fields are refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum TorrentCommand {
    Version {},
    SetSettings {
        settings: TorrentSettings,
    },
    Add {
        spec: TorrentSpec,
    },
    Restore {
        id: TorrentId,
        spec: TorrentSpec,
    },
    Pause {
        id: TorrentId,
    },
    Resume {
        id: TorrentId,
    },
    Remove {
        id: TorrentId,
    },
    SetFiles {
        id: TorrentId,
        files: Vec<u32>,
    },
    Metadata {
        id: TorrentId,
        offset: u64,
    },
    SetSeeding {
        id: TorrentId,
        seeding: SeedPolicy,
    },
    Upload {
        chunk: Chunk,
    },
    SaveResume {},
    SetLimits {
        id: TorrentId,
        upload: u64,
        download: u64,
    },
    Trackers {
        id: TorrentId,
    },
    Reannounce {
        id: TorrentId,
    },
    Peers {
        id: TorrentId,
    },
    Network {},
    Snapshot {},
}

/// The successful reply to a [`TorrentCommand`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
pub enum TorrentReply {
    Version { libtorrent: String },
    Added { id: TorrentId },
    Done {},
    Metadata { chunk: Chunk },
    Saved { ids: Vec<TorrentId> },
    Trackers { trackers: Vec<TrackerStatus> },
    Peers { peers: Vec<PeerStatus> },
    Network { network: NetworkStatus },
    Snapshot { torrents: Vec<TorrentSnapshot> },
}

/// Runs one command against an engine; the exhaustive match keeps the wire commands and the trait
/// in step.
pub fn dispatch(
    engine: &mut dyn TorrentEngine,
    command: TorrentCommand,
) -> Result<TorrentReply, ErrorCode> {
    Ok(match command {
        TorrentCommand::Version {} => TorrentReply::Version {
            libtorrent: engine.version()?,
        },
        TorrentCommand::SetSettings { settings } => {
            engine.set_settings(settings)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Add { spec } => TorrentReply::Added {
            id: engine.add(spec)?,
        },
        TorrentCommand::Restore { id, spec } => {
            engine.restore(id, spec)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Pause { id } => {
            engine.pause(id)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Resume { id } => {
            engine.resume(id)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Remove { id } => {
            engine.remove(id)?;
            TorrentReply::Done {}
        }
        TorrentCommand::SetFiles { id, files } => {
            engine.set_files(id, files)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Metadata { id, offset } => TorrentReply::Metadata {
            chunk: engine.metadata(id, offset)?,
        },
        TorrentCommand::SetSeeding { id, seeding } => {
            engine.set_seeding(id, seeding)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Upload { chunk } => {
            engine.upload(chunk)?;
            TorrentReply::Done {}
        }
        TorrentCommand::SaveResume {} => TorrentReply::Saved {
            ids: engine.save_resume()?,
        },
        TorrentCommand::SetLimits {
            id,
            upload,
            download,
        } => {
            engine.set_limits(id, upload, download)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Trackers { id } => TorrentReply::Trackers {
            trackers: engine.trackers(id)?,
        },
        TorrentCommand::Reannounce { id } => {
            engine.reannounce(id)?;
            TorrentReply::Done {}
        }
        TorrentCommand::Peers { id } => TorrentReply::Peers {
            peers: engine.peers(id)?,
        },
        TorrentCommand::Network {} => TorrentReply::Network {
            network: engine.network()?,
        },
        TorrentCommand::Snapshot {} => TorrentReply::Snapshot {
            torrents: engine.snapshot()?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_specs_use_camel_case_names() {
        let add = TorrentCommand::Add {
            spec: TorrentSpec {
                source: TorrentSource::Magnet {
                    uri: "magnet:?xt=urn:btih:557f8c61bcecd14914e421389d4f46c5d7c336db".into(),
                },
                directory: PathBuf::from("d"),
                trackers: vec!["udp://t.example:1".into()],
                paused: true,
                choose_files: true,
                files: None,
                seeding: SeedPolicy::Ratio { ratio: 1.5 },
                upload_limit: 1024,
                download_limit: 0,
            },
        };
        // Contract 2 (#62's file choice) added `chooseFiles` and `files`; contract 3 (#62's
        // lifecycle part) `seeding`; contract 4 (trackers and port mapping) the limits.
        assert_eq!(
            serde_json::to_string(&add).unwrap(),
            r#"{"command":"add","spec":{"source":{"kind":"magnet","uri":"magnet:?xt=urn:btih:557f8c61bcecd14914e421389d4f46c5d7c336db"},"directory":"d","trackers":["udp://t.example:1"],"paused":true,"chooseFiles":true,"files":null,"seeding":{"mode":"ratio","ratio":1.5},"uploadLimit":1024,"downloadLimit":0}}"#
        );
        let seeding = TorrentCommand::SetSeeding {
            id: TorrentId(2),
            seeding: SeedPolicy::Never {},
        };
        assert_eq!(
            serde_json::to_string(&seeding).unwrap(),
            r#"{"command":"setSeeding","id":2,"seeding":{"mode":"never"}}"#
        );
        assert_eq!(
            serde_json::to_string(&TorrentSource::Resume {}).unwrap(),
            r#"{"kind":"resume"}"#
        );
        let upload = TorrentCommand::Upload {
            chunk: Chunk::of(b"de", 0, CHUNK_BYTES),
        };
        assert_eq!(
            serde_json::to_string(&upload).unwrap(),
            r#"{"command":"upload","chunk":{"total":2,"offset":0,"hex":"6465"}}"#
        );
        assert_eq!(
            serde_json::to_string(&TorrentCommand::SaveResume {}).unwrap(),
            r#"{"command":"saveResume"}"#
        );
        assert_eq!(
            serde_json::to_string(&TorrentReply::Saved {
                ids: vec![TorrentId(1)]
            })
            .unwrap(),
            r#"{"reply":"saved","ids":[1]}"#
        );
        assert_eq!(
            serde_json::to_string(&TorrentEvent::ResumeData {
                id: TorrentId(1),
                chunk: Chunk::of(b"de", 0, CHUNK_BYTES),
            })
            .unwrap(),
            r#"{"type":"resumeData","id":1,"chunk":{"total":2,"offset":0,"hex":"6465"}}"#
        );
        assert_eq!(
            serde_json::to_string(&TorrentSettings::default()).unwrap(),
            r#"{"dht":true,"peerExchange":true,"localPeerDiscovery":false,"maxActiveSeeds":0,"listenPort":6890,"portMapping":false,"uploadLimit":0,"downloadLimit":0,"extraTrackers":[],"maxPeersPerTorrent":55,"dhtBootstrapNodes":[]}"#
        );
        assert_eq!(
            serde_json::to_string(&TorrentCommand::SetLimits {
                id: TorrentId(1),
                upload: 2,
                download: 3
            })
            .unwrap(),
            r#"{"command":"setLimits","id":1,"upload":2,"download":3}"#
        );
        for (command, json) in [
            (
                TorrentCommand::Trackers { id: TorrentId(1) },
                r#"{"command":"trackers","id":1}"#,
            ),
            (
                TorrentCommand::Reannounce { id: TorrentId(1) },
                r#"{"command":"reannounce","id":1}"#,
            ),
            (
                TorrentCommand::Peers { id: TorrentId(1) },
                r#"{"command":"peers","id":1}"#,
            ),
            (TorrentCommand::Network {}, r#"{"command":"network"}"#),
        ] {
            assert_eq!(serde_json::to_string(&command).unwrap(), json);
        }
        let set = TorrentCommand::SetFiles {
            id: TorrentId(2),
            files: vec![1, 3],
        };
        assert_eq!(
            serde_json::to_string(&set).unwrap(),
            r#"{"command":"setFiles","id":2,"files":[1,3]}"#
        );
        let ask = TorrentCommand::Metadata {
            id: TorrentId(2),
            offset: 0,
        };
        assert_eq!(
            serde_json::to_string(&ask).unwrap(),
            r#"{"command":"metadata","id":2,"offset":0}"#
        );
        let file = TorrentSource::File {
            path: "a.torrent".into(),
        };
        assert_eq!(
            serde_json::to_string(&file).unwrap(),
            r#"{"kind":"file","path":"a.torrent"}"#
        );
    }

    #[test]
    fn unknown_fields_are_refused_everywhere() {
        for json in [
            r#"{"command":"snapshot","extra":1}"#,
            r#"{"command":"pause","id":1,"extra":1}"#,
            r#"{"command":"add","spec":{"source":{"kind":"file","path":"a","extra":1},"directory":"d","trackers":[],"paused":false,"chooseFiles":false,"files":null,"seeding":{"mode":"never"},"uploadLimit":0,"downloadLimit":0}}"#,
            r#"{"command":"add","spec":{"source":{"kind":"file","path":"a"},"directory":"d","trackers":[],"paused":false,"chooseFiles":false,"files":null,"seeding":{"mode":"never"},"uploadLimit":0,"downloadLimit":0,"extra":1}}"#,
            r#"{"command":"setFiles","id":1,"files":[1],"extra":1}"#,
            r#"{"command":"metadata","id":1,"offset":0,"extra":1}"#,
            r#"{"command":"setSeeding","id":1,"seeding":{"mode":"never"},"extra":1}"#,
            r#"{"command":"setSeeding","id":1,"seeding":{"mode":"never","extra":1}}"#,
            r#"{"command":"saveResume","extra":1}"#,
            r#"{"command":"network","extra":1}"#,
            r#"{"command":"setLimits","id":1,"upload":0,"download":0,"extra":1}"#,
            r#"{"command":"setSettings","settings":{"dht":true,"peerExchange":true,"localPeerDiscovery":false,"maxActiveSeeds":0,"listenPort":6890,"portMapping":false,"uploadLimit":0,"downloadLimit":0,"extraTrackers":[],"maxPeersPerTorrent":55,"dhtBootstrapNodes":[],"extra":1}}"#,
            r#"{"command":"restore","id":1,"spec":{"source":{"kind":"resume"},"directory":"d","trackers":[],"paused":false,"chooseFiles":false,"files":null,"seeding":{"mode":"never"},"uploadLimit":0,"downloadLimit":0},"extra":1}"#,
            r#"{"command":"upload","chunk":{"total":2,"offset":0,"hex":"6465","extra":1}}"#,
            r#"{"command":"add","spec":{"source":{"kind":"resume","extra":1},"directory":"d","trackers":[],"paused":false,"chooseFiles":false,"files":null,"seeding":{"mode":"never"},"uploadLimit":0,"downloadLimit":0}}"#,
        ] {
            assert!(
                serde_json::from_str::<TorrentCommand>(json).is_err(),
                "{json}"
            );
        }
        assert!(
            serde_json::from_str::<TorrentEvent>(r#"{"type":"removed","id":1,"extra":1}"#).is_err()
        );
        assert!(serde_json::from_str::<TorrentReply>(r#"{"reply":"done","extra":1}"#).is_err());
        assert!(serde_json::from_str::<TorrentReply>(
            r#"{"reply":"metadata","chunk":{"total":2,"offset":0,"hex":"6465","extra":1}}"#
        )
        .is_err());
        // Without the extra field, the same command is accepted: the refusal above is for it.
        assert!(serde_json::from_str::<TorrentCommand>(
            r#"{"command":"add","spec":{"source":{"kind":"file","path":"a"},"directory":"d","trackers":[],"paused":false,"chooseFiles":false,"files":null,"seeding":{"mode":"never"},"uploadLimit":0,"downloadLimit":0}}"#
        )
        .is_ok());
    }

    /// V1's `serialises_for_the_frontend` (`portmap.rs`), and the other new forms.
    #[test]
    fn network_trackers_and_peers_have_v1s_shapes() {
        assert_eq!(
            serde_json::to_value(PortMapState::Mapped {
                external_port: 1,
                via: "upnp".into()
            })
            .unwrap(),
            serde_json::json!({"state": "mapped", "externalPort": 1, "via": "upnp"})
        );
        assert_eq!(
            serde_json::to_value(PortMapState::NoRouter {}).unwrap(),
            serde_json::json!({"state": "noRouter"})
        );
        let network = NetworkStatus {
            listen_port: 6890,
            listen_error: None,
            port_mapping: PortMapState::Off {},
        };
        assert_eq!(
            serde_json::to_value(&network).unwrap(),
            serde_json::json!({"listenPort": 6890, "listenError": null, "portMapping": {"state": "off"}})
        );
        let tracker = TrackerStatus {
            url: "http://t.test/…".into(),
            tier: 0,
            state: TrackerState::NotContacted,
            message: None,
            fails: 0,
            next_announce_secs: Some(5),
            seeds: None,
            peers: None,
        };
        let json = serde_json::to_string(&tracker).unwrap();
        assert!(json.contains(r#""state":"notContacted""#), "{json}");
        assert!(json.contains(r#""nextAnnounceSecs":5"#), "{json}");
        let peer = PeerStatus {
            ip: "127.0.0.1".into(),
            port: 1,
            download_speed_bps: 0,
            upload_speed_bps: 0,
            seed: true,
            progress_permille: 1000,
            client: "libtorrent".into(),
        };
        assert_eq!(
            serde_json::from_str::<PeerStatus>(&serde_json::to_string(&peer).unwrap()).unwrap(),
            peer
        );
    }

    /// The listening port, peers per torrent, limits and extra trackers keep to their limits.
    #[test]
    fn settings_keep_to_their_limits() {
        let ok = TorrentSettings::default();
        assert_eq!(ok.check(), Ok(()));
        let with = |change: &dyn Fn(&mut TorrentSettings)| {
            let mut s = TorrentSettings::default();
            change(&mut s);
            s.check()
        };
        assert_eq!(with(&|s| s.listen_port = 0), Ok(()), "random");
        assert_eq!(with(&|s| s.listen_port = 1024), Ok(()));
        assert_eq!(
            with(&|s| s.listen_port = 1023),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(
            with(&|s| s.max_peers_per_torrent = 0),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(with(&|s| s.max_peers_per_torrent = 512), Ok(()));
        assert_eq!(
            with(&|s| s.max_peers_per_torrent = 513),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(with(&|s| s.upload_limit = MAX_RATE_LIMIT), Ok(()));
        assert_eq!(
            with(&|s| s.upload_limit = MAX_RATE_LIMIT + 1),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(
            with(&|s| s.download_limit = MAX_RATE_LIMIT + 1),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(
            with(&|s| s.extra_trackers = vec!["udp://t.test:1".into()]),
            Ok(())
        );
        assert_eq!(
            with(&|s| s.extra_trackers = vec!["ftp://t.test/a".into()]),
            Err(ErrorCode::InvalidUrl)
        );
        assert_eq!(
            with(&|s| s.extra_trackers = vec!["udp://t.test:1".into(); 65]),
            Err(ErrorCode::InvalidInput)
        );
        // V1's default entry point, and the other forms.
        for good in [
            "router.bittorrent.com:6881",
            "10.0.0.1:1",
            "[2001:db8::1]:6881",
        ] {
            assert_eq!(
                with(&|s| s.dht_bootstrap_nodes = vec![good.into()]),
                Ok(()),
                "{good}"
            );
        }
        for bad in [
            "router.bittorrent.com",
            "host:0",
            "host:65536",
            ":6881",
            "a b:1",
            "a,b:1",
            "[::1:1",
            "host:1x",
        ] {
            assert_eq!(
                with(&|s| s.dht_bootstrap_nodes = vec![bad.into()]),
                Err(ErrorCode::InvalidInput),
                "{bad}"
            );
        }
        assert_eq!(
            with(&|s| s.dht_bootstrap_nodes = vec!["h:1".into(); 17]),
            Err(ErrorCode::InvalidInput)
        );
    }

    #[test]
    fn settings_allow_at_most_v1s_number_of_active_seeds() {
        let with = |max_active_seeds| TorrentSettings {
            max_active_seeds,
            ..TorrentSettings::default()
        };
        assert_eq!(with(0).check(), Ok(()));
        assert_eq!(with(MAX_ACTIVE_SEEDS).check(), Ok(()));
        assert_eq!(
            with(MAX_ACTIVE_SEEDS + 1).check(),
            Err(ErrorCode::InvalidInput)
        );
    }

    #[test]
    fn a_metadata_chunk_carries_its_bytes_as_hex() {
        let chunk = Chunk::of(b"d4:infoe", 2, 4);
        assert_eq!(
            chunk,
            Chunk {
                total: 8,
                offset: 2,
                hex: "3a696e66".into()
            }
        );
        assert_eq!(chunk.bytes(), Ok(b":inf".to_vec()));
        // Past the end: an empty chunk, never a panic.
        assert_eq!(Chunk::of(b"de", 5, 4).hex, "");
        // Hex from the other side is untrusted.
        for bad in ["3", "zz", "3A6"] {
            let chunk = Chunk {
                total: 1,
                offset: 0,
                hex: bad.into(),
            };
            assert_eq!(chunk.bytes(), Err(ErrorCode::EngineIncompatible), "{bad}");
        }
    }

    #[test]
    fn a_snapshot_round_trips() {
        let snapshot = TorrentSnapshot {
            id: TorrentId(3),
            status: DownloadStatus::Downloading,
            name: Some("album".into()),
            info_hash_v1: Some("a".repeat(40)),
            info_hash_v2: None,
            progress: Progress {
                completed_bytes: 10,
                total_bytes: Some(20),
                download_speed_bps: 5,
                upload_speed_bps: 0,
                connections: 1,
                connection_detail: None,
            },
            peers: 1,
            seeds: 1,
            uploaded_bytes: 30,
            seeding_secs: 4,
            seed_queued: false,
            error: None,
        };
        let event = TorrentEvent::Changed {
            snapshot: Box::new(snapshot),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""infoHashV1":"#), "{json}");
        assert!(json.contains(r#""seedQueued":false"#), "{json}");
        assert_eq!(serde_json::from_str::<TorrentEvent>(&json).unwrap(), event);
    }
}
