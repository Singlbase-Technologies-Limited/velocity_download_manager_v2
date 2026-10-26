//! The torrent engine contract (#62): what the manager asks of libtorrent and what it hears back,
//! in V2's own types. libtorrent runs in the torrent helper, a supervised child process
//! (ADR-0014, `crates/velocity-torrent`); only `crates/velocity-lt` calls it. Direct downloads
//! never pass through this contract, and torrents never pass through `DirectTransfer`.
//!
//! The contract grows with #62's later parts: file selection and metadata (magnets), seeding
//! policies, trackers and port mapping.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};

use crate::domain::{DownloadStatus, ErrorCode, Progress, TransferError};

/// Raised whenever a command, reply, event or shared type here changes incompatibly.
/// Version 1 (#62): settings for finding peers; add a `.torrent` file or a magnet link, pause,
/// resume, remove, snapshot.
pub const TORRENT_CONTRACT_VERSION: u32 = 1;

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
}

/// One torrent to add.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Why the torrent stopped, when it failed.
    pub error: Option<TransferError>,
}

/// How the engine finds peers besides trackers. The defaults are V1's (PO decision on #62):
/// DHT and peer exchange on, local peer discovery off. Private torrents never use DHT or peer
/// exchange; libtorrent enforces that. Router port mapping is opt-in and comes with #62's tracker
/// and port-mapping part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentSettings {
    pub dht: bool,
    /// Applies to torrents added after it is set.
    pub peer_exchange: bool,
    pub local_peer_discovery: bool,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        TorrentSettings {
            dht: true,
            peer_exchange: true,
            local_peer_discovery: false,
        }
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
    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    /// Forgets a torrent. Its files are always kept: deleting them is the manager's decision.
    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode>;
    /// The current state of every torrent the engine has.
    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode>;
    /// A new receiver for this engine's events, from the moment it subscribed.
    fn subscribe(&mut self) -> Receiver<TorrentEvent>;
}

/// One command, in the form that crosses a process boundary. As for the transfer contract,
/// variants without data are written `Snapshot {}` so their unknown fields are refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum TorrentCommand {
    Version {},
    SetSettings { settings: TorrentSettings },
    Add { spec: TorrentSpec },
    Pause { id: TorrentId },
    Resume { id: TorrentId },
    Remove { id: TorrentId },
    Snapshot {},
}

/// The successful reply to a [`TorrentCommand`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
pub enum TorrentReply {
    Version { libtorrent: String },
    Added { id: TorrentId },
    Done {},
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
            },
        };
        assert_eq!(
            serde_json::to_string(&add).unwrap(),
            r#"{"command":"add","spec":{"source":{"kind":"magnet","uri":"magnet:?xt=urn:btih:557f8c61bcecd14914e421389d4f46c5d7c336db"},"directory":"d","trackers":["udp://t.example:1"],"paused":true}}"#
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
            r#"{"command":"add","spec":{"source":{"kind":"file","path":"a","extra":1},"directory":"d","trackers":[],"paused":false}}"#,
            r#"{"command":"add","spec":{"source":{"kind":"file","path":"a"},"directory":"d","trackers":[],"paused":false,"extra":1}}"#,
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
            error: None,
        };
        let event = TorrentEvent::Changed {
            snapshot: Box::new(snapshot),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""infoHashV1":"#), "{json}");
        assert_eq!(serde_json::from_str::<TorrentEvent>(&json).unwrap(), event);
    }
}
