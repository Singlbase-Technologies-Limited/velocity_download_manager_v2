//! The `DirectTransfer` contract between the manager and a direct-download engine (#36,
//! docs/ENGINE_CONTRACT.md).
//!
//! Direct downloads only: HTTP, FTP, SFTP and Metalink. Torrent operations (trackers, peers, file
//! priorities, seeding, magnet metadata) are never part of this contract (research report section
//! 4.2). Every value that crosses it is owned and serialisable, so the same contract works with an
//! engine in the app's process and with a client for an engine in a child process (#38, #39).

#[cfg(test)]
pub(crate) mod fake;

use crate::domain::{
    Capabilities, DownloadStatus, ErrorCode, Progress, TransferError, Verification,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// Version of this contract. Raise it whenever a command, reply, event or type here changes
/// incompatibly; an engine in a child process reports the version it speaks (#39).
/// Version 2 (#43) added checkpoints: [`TransferSpec::resume`] and [`TransferEvent::Checkpoint`].
/// Version 3 (#49) added the global speed limit: [`DirectTransfer::set_global_limits`].
/// Version 4 (#47) added [`crate::domain::Progress::connection_detail`] to snapshots.
/// Version 5 (#45) added [`Checkpoint::content_encoding`].
/// Version 6 (#46) added [`TransferSnapshot::verification`], [`TransferSpec::pieces`], and SHA-224
/// and SHA-384 to [`HashAlgorithm`]; engines now check a [`TransferSpec::checksum`].
pub const TRANSFER_CONTRACT_VERSION: u32 = 6;

/// An engine's id for one transfer. Assigned by the engine, never reused, and never shown as the
/// app's download id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(pub u64);

/// Refers to credentials the app keeps in memory (#70). Only the handle crosses the contract; the
/// user name and password never do, so they can't reach a checkpoint, a log or a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialHandle(pub u64);

/// A custom request header. Its value may be a secret (a cookie, a token), so `Debug` never
/// prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Header")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Hash algorithms a trusted checksum can use (Metalink and user-entered hashes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    /// How many hexadecimal characters a digest of this algorithm has.
    pub const fn hex_len(self) -> usize {
        match self {
            HashAlgorithm::Md5 => 32,
            HashAlgorithm::Sha1 => 40,
            HashAlgorithm::Sha224 => 56,
            HashAlgorithm::Sha256 => 64,
            HashAlgorithm::Sha384 => 96,
            HashAlgorithm::Sha512 => 128,
        }
    }
}

/// A whole-file hash to verify before the download is marked Completed (#46).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checksum {
    pub algorithm: HashAlgorithm,
    /// Lowercase hexadecimal digest.
    pub hex: String,
}

impl Checksum {
    /// The checksum with its digest in lowercase, or `InvalidInput` when the digest isn't
    /// hexadecimal or doesn't have its algorithm's length.
    pub fn normalized(&self) -> Result<Checksum, ErrorCode> {
        if self.hex.len() != self.algorithm.hex_len()
            || !self.hex.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ErrorCode::InvalidInput);
        }
        Ok(Checksum {
            algorithm: self.algorithm,
            hex: self.hex.to_ascii_lowercase(),
        })
    }
}

/// Trusted hashes of a file's pieces, such as a Metalink file's `<pieces>` (#46, #54): piece `i`
/// is bytes `[i × length, (i + 1) × length)`, the last one ending with the file. Each piece is
/// checked once it is covered, and a bad one is fetched again on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PieceHashes {
    pub algorithm: HashAlgorithm,
    /// Bytes per piece; the last piece may be shorter.
    pub length: u64,
    /// One lowercase hexadecimal digest per piece, in file order.
    pub hex: Vec<String>,
}

impl PieceHashes {
    /// The fewest bytes a piece may have: a smaller one would make the list larger than the file
    /// it describes. Metalink pieces are typically 256 KiB or more.
    pub const MIN_LENGTH: u64 = 1024;
    /// The most hexadecimal characters a list's digests may have in all: 2 MiB, so a spec that
    /// carries them fits in one message to the engine's child process (4 MiB, ENGINE_PROTOCOL.md)
    /// with room for the rest. 32,768 SHA-256 digests: 32 GiB of file in 1 MiB pieces.
    pub const MAX_HEX_BYTES: usize = 2 << 20;

    /// The list with its digests in lowercase, or `InvalidInput` when a digest isn't hexadecimal
    /// or doesn't have its algorithm's length, the pieces are smaller than [`Self::MIN_LENGTH`],
    /// or there are none, or more digests than [`Self::MAX_HEX_BYTES`] holds.
    pub fn normalized(&self) -> Result<PieceHashes, ErrorCode> {
        if self.length < Self::MIN_LENGTH
            || self.hex.is_empty()
            || self.hex.len() > Self::MAX_HEX_BYTES / self.algorithm.hex_len()
        {
            return Err(ErrorCode::InvalidInput);
        }
        let hex = self
            .hex
            .iter()
            .map(|hex| {
                Checksum {
                    algorithm: self.algorithm,
                    hex: hex.clone(),
                }
                .normalized()
                .map(|c| c.hex)
            })
            .collect::<Result<_, _>>()?;
        Ok(PieceHashes {
            algorithm: self.algorithm,
            length: self.length,
            hex,
        })
    }
}

/// Speed limits, for one transfer or for all of them. `None` means no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub max_download_bps: Option<u64>,
}

impl Limits {
    /// The lowest limit an engine accepts: 1 KiB/s, V1's smallest unit (#49).
    pub const MIN_BPS: u64 = 1024;

    /// `InvalidInput` for a limit below [`Limits::MIN_BPS`]; no limit is always valid.
    pub fn check(&self) -> Result<(), ErrorCode> {
        match self.max_download_bps {
            Some(bps) if bps < Self::MIN_BPS => Err(ErrorCode::InvalidInput),
            _ => Ok(()),
        }
    }
}

/// Everything an engine needs to start one direct transfer. The manager builds it from a
/// validated download record; the engine still treats it as untrusted input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferSpec {
    /// The source and any mirrors, primary first, with credentials removed. Never empty.
    pub urls: Vec<String>,
    /// The final file path. The engine writes to a part file beside it and renames it only after
    /// verification (#43, #46).
    pub destination: PathBuf,
    pub credentials: Option<CredentialHandle>,
    pub headers: Vec<Header>,
    pub checksum: Option<Checksum>,
    /// Trusted piece hashes (#46); `None` until a Metalink file supplies them (#54).
    pub pieces: Option<PieceHashes>,
    pub limits: Limits,
    /// Requested connections; the engine may apply fewer (see [`DirectTransfer::set_connections`]).
    pub max_connections: u32,
    /// How to treat what an earlier attempt left on disk (#43). `None` for a new download.
    pub resume: Option<Resume>,
}

/// The format of [`Checkpoint`]. A checkpoint in another format isn't used.
pub const CHECKPOINT_FORMAT: u32 = 1;

/// How far a direct download got durably: what an engine needs to resume it (#43). It holds no
/// credential and no URL query, so no token can reach the database through it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checkpoint {
    /// [`CHECKPOINT_FORMAT`] when it was written.
    pub format: u32,
    /// The source as [`Checkpoint::source_of`] gives it: scheme, host and path only.
    pub source: String,
    /// The file's total size. A resumed answer with another total means another file.
    pub total: u64,
    /// What the first answer said about the file (#45): its ETag, strong or weak, and its
    /// `Last-Modified` only when that was the strong validator. Every later answer must agree.
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// The `Content-Encoding` the server applied, which V2 never asks for; `None` for none. Ranges
    /// count the encoded bytes, so a resume needs the same one (#45). Absent in checkpoints saved
    /// before it existed, which had none.
    pub content_encoding: Option<String>,
    /// The byte ranges synced to disk, as half-open `[start, end)` pairs, sorted and disjoint.
    /// Only these are trusted after a crash.
    pub durable: Vec<(u64, u64)>,
}

impl Checkpoint {
    /// `url` without its login, query and fragment: `https://user:pass@host/a.zip?token=x` is
    /// `https://host/a.zip`. Enough to tell a resumed source from another; never a secret.
    pub fn source_of(url: &str) -> String {
        let (without, _) = crate::database::without_login(url);
        match without.find(['?', '#']) {
            Some(cut) => without[..cut].to_owned(),
            None => without,
        }
    }
}

/// How a transfer starts relative to what an earlier attempt left on disk (#43).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "from",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Resume {
    /// Continue from this checkpoint. The part file is the download's own.
    Checkpoint { checkpoint: Checkpoint },
    /// The part file is the download's own but has no usable checkpoint: discard it and start
    /// over.
    DiscardPartFile {},
}

/// The state of one transfer as its engine reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferSnapshot {
    pub id: TransferId,
    pub status: DownloadStatus,
    /// `completed_bytes` counts accepted bytes only (research report section 5.1).
    pub progress: Progress,
    /// Bytes received but not accepted: revoked leases, overruns, rejected responses.
    pub wasted_bytes: u64,
    pub error: Option<TransferError>,
    /// What was proven about the file's bytes once it completed; `None` until then (#46).
    pub verification: Option<Verification>,
}

/// Something that happened to a transfer, pushed by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum TransferEvent {
    /// The transfer's status, progress or error changed.
    Changed { snapshot: TransferSnapshot },
    /// The engine forgot the transfer after [`DirectTransfer::remove`].
    Removed { id: TransferId },
    /// The transfer's part file was synced and these bytes are durable. The app saves it, and
    /// hands it back in [`TransferSpec::resume`] to continue after a crash (#43).
    Checkpoint {
        id: TransferId,
        checkpoint: Checkpoint,
    },
}

/// A direct-download engine, in the app's process or behind a client for a child process.
///
/// Commands fail with an [`ErrorCode`]: `NotFound` for an unknown id, `InvalidState` when the
/// transfer's status doesn't allow the command, `EngineUnavailable` when the engine can't be
/// reached. Transfer failures are not command failures: they arrive as events with a
/// [`TransferError`].
pub trait DirectTransfer: Send {
    /// What this engine really supports.
    fn capabilities(&self) -> Capabilities;
    /// Starts a transfer, which begins as Queued.
    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode>;
    /// Pauses an active transfer and keeps its progress.
    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode>;
    /// Continues a paused transfer from its durable progress.
    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode>;
    /// Stops an unfinished transfer and discards its part file; it ends as Removed. The
    /// destination file is never touched.
    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode>;
    /// Forgets a finished transfer (Completed, Failed or Removed) and its checkpoint. Files are
    /// kept.
    fn remove(&mut self, id: TransferId) -> Result<(), ErrorCode>;
    /// Limits one transfer. It holds alongside the global limit: the lower of the two applies.
    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode>;
    /// Limits all transfers together; the allowance is shared among the active ones (#49).
    fn set_global_limits(&mut self, limits: Limits) -> Result<(), ErrorCode>;
    /// Requests a connection count and returns the count the engine applied, which is at least 1
    /// and at most `capabilities().max_connections_per_host`.
    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode>;
    /// The current state of every transfer the engine knows.
    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode>;
    /// A new receiver for this engine's events. Each receiver gets every event from the moment
    /// it subscribed.
    fn subscribe(&mut self) -> Receiver<TransferEvent>;
}

/// One command, in the form that crosses a process boundary (#39).
///
/// Every wire form refuses unknown fields. Variants without data are written `Snapshot {}`, not
/// `Snapshot`, because serde checks the fields of a struct variant but ignores extra fields on a
/// unit variant of a tagged enum. The JSON is the same: `{"command":"snapshot"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum TransferCommand {
    Capabilities {},
    /// Boxed, because a spec with a checkpoint is much larger than any other command.
    Add {
        spec: Box<TransferSpec>,
    },
    Pause {
        id: TransferId,
    },
    Resume {
        id: TransferId,
    },
    Cancel {
        id: TransferId,
    },
    Remove {
        id: TransferId,
    },
    SetLimits {
        id: TransferId,
        limits: Limits,
    },
    SetConnections {
        id: TransferId,
        connections: u32,
    },
    SetGlobalLimits {
        limits: Limits,
    },
    Snapshot {},
}

/// The successful reply to a [`TransferCommand`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
pub enum TransferReply {
    Capabilities { capabilities: Capabilities },
    Added { id: TransferId },
    Done {},
    Connections { connections: u32 },
    Snapshot { transfers: Vec<TransferSnapshot> },
}

/// Runs one command against an engine. A child-process engine runs this on each message it
/// receives; the exhaustive match keeps the wire commands and the trait in step.
pub fn dispatch(
    engine: &mut dyn DirectTransfer,
    command: TransferCommand,
) -> Result<TransferReply, ErrorCode> {
    Ok(match command {
        TransferCommand::Capabilities {} => TransferReply::Capabilities {
            capabilities: engine.capabilities(),
        },
        TransferCommand::Add { spec } => TransferReply::Added {
            id: engine.add(*spec)?,
        },
        TransferCommand::Pause { id } => {
            engine.pause(id)?;
            TransferReply::Done {}
        }
        TransferCommand::Resume { id } => {
            engine.resume(id)?;
            TransferReply::Done {}
        }
        TransferCommand::Cancel { id } => {
            engine.cancel(id)?;
            TransferReply::Done {}
        }
        TransferCommand::Remove { id } => {
            engine.remove(id)?;
            TransferReply::Done {}
        }
        TransferCommand::SetLimits { id, limits } => {
            engine.set_limits(id, limits)?;
            TransferReply::Done {}
        }
        TransferCommand::SetConnections { id, connections } => TransferReply::Connections {
            connections: engine.set_connections(id, connections)?,
        },
        TransferCommand::SetGlobalLimits { limits } => {
            engine.set_global_limits(limits)?;
            TransferReply::Done {}
        }
        TransferCommand::Snapshot {} => TransferReply::Snapshot {
            transfers: engine.snapshot()?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::fake::FakeTransfer;
    use super::*;
    use crate::domain::FailureClass;
    use serde::de::DeserializeOwned;

    pub(crate) fn spec() -> TransferSpec {
        TransferSpec {
            urls: vec!["https://example.com/a.zip".to_owned()],
            destination: PathBuf::from("downloads/a.zip"),
            credentials: Some(CredentialHandle(7)),
            headers: vec![Header {
                name: "Cookie".to_owned(),
                value: "session=secret-value".to_owned(),
            }],
            checksum: Some(Checksum {
                algorithm: HashAlgorithm::Sha256,
                hex: "ab".repeat(32),
            }),
            pieces: Some(PieceHashes {
                algorithm: HashAlgorithm::Sha1,
                length: 256 << 10,
                hex: vec!["cd".repeat(20), "ef".repeat(20)],
            }),
            limits: Limits {
                max_download_bps: Some(1_000_000),
            },
            max_connections: 4,
            resume: Some(Resume::Checkpoint {
                checkpoint: checkpoint(),
            }),
        }
    }

    pub(crate) fn checkpoint() -> Checkpoint {
        Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: "https://example.com/a.zip".to_owned(),
            total: 1000,
            etag: Some("\"v1\"".to_owned()),
            last_modified: None,
            content_encoding: None,
            durable: vec![(0, 400), (600, 800)],
        }
    }

    #[test]
    fn a_checksum_needs_its_algorithms_length_in_hex_digits() {
        use HashAlgorithm::*;
        for (algorithm, len) in [
            (Md5, 32),
            (Sha1, 40),
            (Sha224, 56),
            (Sha256, 64),
            (Sha384, 96),
            (Sha512, 128),
        ] {
            assert_eq!(algorithm.hex_len(), len, "{algorithm:?}");
            let checksum = |hex: String| Checksum { algorithm, hex };
            assert_eq!(
                checksum("Ab".repeat(len / 2)).normalized(),
                Ok(checksum("ab".repeat(len / 2))),
                "{algorithm:?}: uppercase is accepted and lowercased"
            );
            for bad in [
                "a".repeat(len - 1),
                "a".repeat(len + 1),
                format!("{}g", "a".repeat(len - 1)),
                format!(" {}", "a".repeat(len - 1)),
                String::new(),
            ] {
                assert_eq!(
                    checksum(bad.clone()).normalized(),
                    Err(ErrorCode::InvalidInput),
                    "{algorithm:?}: {bad:?}"
                );
            }
        }
    }

    #[test]
    fn piece_hashes_need_valid_digests_a_sensible_length_and_a_bounded_count() {
        let pieces = |length: u64, hex: Vec<String>| PieceHashes {
            algorithm: HashAlgorithm::Sha256,
            length,
            hex,
        };
        assert_eq!(
            pieces(1024, vec!["AB".repeat(32), "cd".repeat(32)]).normalized(),
            Ok(pieces(1024, vec!["ab".repeat(32), "cd".repeat(32)])),
            "uppercase is accepted and lowercased"
        );
        for (why, bad) in [
            (
                "a short digest",
                pieces(1024, vec!["ab".repeat(32), "ab".repeat(31)]),
            ),
            (
                "not hex",
                pieces(1024, vec![format!("{}zz", "ab".repeat(31))]),
            ),
            ("pieces too small", pieces(1023, vec!["ab".repeat(32)])),
            ("no pieces", pieces(1024, Vec::new())),
            (
                "too many pieces",
                pieces(
                    1024,
                    vec!["ab".repeat(32); PieceHashes::MAX_HEX_BYTES / 64 + 1],
                ),
            ),
        ] {
            assert_eq!(bad.normalized(), Err(ErrorCode::InvalidInput), "{why}");
        }
    }

    #[test]
    fn a_checkpoint_saved_before_content_encoding_reads_as_none() {
        let saved = r#"{"format":1,"source":"https://example.com/a.zip","total":1000,"etag":"\"v1\"","lastModified":null,"durable":[[0,400],[600,800]]}"#;
        let read: Checkpoint = serde_json::from_str(saved).expect("an older checkpoint reads");
        assert_eq!(read, checkpoint());
        let gzip = Checkpoint {
            content_encoding: Some("gzip".to_owned()),
            ..checkpoint()
        };
        round_trip(&gzip);
    }

    fn snapshot() -> TransferSnapshot {
        TransferSnapshot {
            id: TransferId(1),
            status: DownloadStatus::Failed,
            progress: Progress {
                completed_bytes: 10,
                total_bytes: Some(20),
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: None,
            },
            wasted_bytes: 3,
            error: Some(TransferError::new(
                FailureClass::Permanent,
                Some(404),
                "Not found",
            )),
            verification: None,
        }
    }

    /// Owned, sendable and serialisable: what a value needs to cross a process boundary.
    fn assert_wire<T: Serialize + DeserializeOwned + Send + 'static>() {}

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + fmt::Debug>(value: &T) {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "{json}");
    }

    #[test]
    fn every_command_reply_and_event_can_cross_a_process_boundary() {
        assert_wire::<TransferCommand>();
        assert_wire::<TransferReply>();
        assert_wire::<TransferEvent>();
        assert_wire::<TransferSpec>();
        assert_wire::<TransferSnapshot>();

        for command in [
            TransferCommand::Capabilities {},
            TransferCommand::Add {
                spec: Box::new(spec()),
            },
            TransferCommand::Pause { id: TransferId(1) },
            TransferCommand::Resume { id: TransferId(1) },
            TransferCommand::Cancel { id: TransferId(1) },
            TransferCommand::Remove { id: TransferId(1) },
            TransferCommand::SetLimits {
                id: TransferId(1),
                limits: Limits::default(),
            },
            TransferCommand::SetConnections {
                id: TransferId(1),
                connections: 8,
            },
            TransferCommand::SetGlobalLimits {
                limits: Limits {
                    max_download_bps: Some(4 << 20),
                },
            },
            TransferCommand::Snapshot {},
        ] {
            round_trip(&command);
        }
        for reply in [
            TransferReply::Capabilities {
                capabilities: Capabilities::sample(),
            },
            TransferReply::Added { id: TransferId(9) },
            TransferReply::Done {},
            TransferReply::Connections { connections: 2 },
            TransferReply::Snapshot {
                transfers: vec![snapshot()],
            },
        ] {
            round_trip(&reply);
        }
        round_trip(&TransferEvent::Changed {
            snapshot: snapshot(),
        });
        round_trip(&TransferEvent::Removed { id: TransferId(1) });
    }

    /// Adds `"extra": 1` to the object at `pointer` and checks that `T` refuses the result.
    fn refuses_an_extra_field<T: Serialize + DeserializeOwned>(value: &T, pointer: &str) {
        let mut json = serde_json::to_value(value).unwrap();
        json.pointer_mut(pointer)
            .and_then(serde_json::Value::as_object_mut)
            .unwrap_or_else(|| panic!("no object at {pointer:?}"))
            .insert("extra".to_owned(), 1.into());
        assert!(
            serde_json::from_value::<T>(json.clone()).is_err(),
            "accepted {json}"
        );
    }

    #[test]
    fn the_wire_forms_refuse_unknown_fields() {
        let add = TransferCommand::Add {
            spec: Box::new(spec()),
        };
        for pointer in [
            "",
            "/spec",
            "/spec/headers/0",
            "/spec/checksum",
            "/spec/pieces",
            "/spec/limits",
        ] {
            refuses_an_extra_field(&add, pointer);
        }
        refuses_an_extra_field(&TransferCommand::Snapshot {}, "");
        let global = TransferCommand::SetGlobalLimits {
            limits: Limits::default(),
        };
        for pointer in ["", "/limits"] {
            refuses_an_extra_field(&global, pointer);
        }
        refuses_an_extra_field(&TransferCommand::Pause { id: TransferId(1) }, "");
        refuses_an_extra_field(&TransferReply::Done {}, "");
        let capabilities = TransferReply::Capabilities {
            capabilities: Capabilities::sample(),
        };
        refuses_an_extra_field(&capabilities, "/capabilities");
        let changed = TransferEvent::Changed {
            snapshot: snapshot(),
        };
        for pointer in ["", "/snapshot", "/snapshot/progress", "/snapshot/error"] {
            refuses_an_extra_field(&changed, pointer);
        }
        refuses_an_extra_field(&TransferEvent::Removed { id: TransferId(1) }, "");
        let checkpoint = TransferEvent::Checkpoint {
            id: TransferId(1),
            checkpoint: checkpoint(),
        };
        for pointer in ["", "/checkpoint"] {
            refuses_an_extra_field(&checkpoint, pointer);
        }
        for pointer in ["/spec/resume", "/spec/resume/checkpoint"] {
            refuses_an_extra_field(&add, pointer);
        }
        let discard = TransferCommand::Add {
            spec: Box::new(TransferSpec {
                resume: Some(Resume::DiscardPartFile {}),
                ..spec()
            }),
        };
        refuses_an_extra_field(&discard, "/spec/resume");
    }

    #[test]
    fn checkpoints_and_resumes_cross_a_process_boundary() {
        let event = TransferEvent::Checkpoint {
            id: TransferId(3),
            checkpoint: checkpoint(),
        };
        round_trip(&event);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""durable":[[0,400],[600,800]]"#), "{json}");
        round_trip(&Resume::DiscardPartFile {});
        assert_eq!(
            serde_json::to_string(&Resume::DiscardPartFile {}).unwrap(),
            r#"{"from":"discardPartFile"}"#
        );
    }

    #[test]
    fn a_checkpoint_source_keeps_no_login_query_or_fragment() {
        for (url, source) in [
            (
                "https://alice:pw@example.com:8443/a/b.zip?token=SECRET#part",
                "https://example.com:8443/a/b.zip",
            ),
            ("https://example.com/a.zip", "https://example.com/a.zip"),
            (
                "ftp://anon@ftp.example.org/pub/x",
                "ftp://ftp.example.org/pub/x",
            ),
            ("https://example.com?q=1", "https://example.com"),
        ] {
            assert_eq!(Checkpoint::source_of(url), source, "{url}");
        }
    }

    #[test]
    fn the_contract_is_object_safe_and_sendable() {
        fn assert_send<T: Send>() {}
        assert_send::<Box<dyn DirectTransfer>>();
        let _engine: Box<dyn DirectTransfer> = Box::new(FakeTransfer::new());
    }

    #[test]
    fn wire_names_are_camel_case_and_engine_neutral() {
        let json = serde_json::to_string(&TransferCommand::SetConnections {
            id: TransferId(3),
            connections: 2,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"command":"setConnections","id":3,"connections":2}"#
        );
        let spec_json = serde_json::to_string(&spec()).unwrap().to_lowercase();
        assert!(!spec_json.contains("aria2") && !spec_json.contains("gid"));
    }

    #[test]
    fn credentials_cross_only_as_a_handle() {
        let json = serde_json::to_value(spec()).unwrap();
        assert_eq!(json["credentials"], 7);
    }

    #[test]
    fn debug_output_never_shows_header_values() {
        let shown = format!("{:?}", spec());
        assert!(!shown.contains("secret-value"), "{shown}");
        assert!(shown.contains("Cookie"));
    }

    #[test]
    fn limits_and_connections_reach_the_engine_within_its_capabilities() {
        let mut engine = FakeTransfer::new();
        let control = engine.control();
        let id = engine.add(spec()).unwrap();
        assert_eq!(control.spec(id).credentials, Some(CredentialHandle(7)));

        let limits = Limits {
            max_download_bps: None,
        };
        engine.set_limits(id, limits).unwrap();
        assert_eq!(control.limits(id), limits);

        assert_eq!(engine.set_connections(id, 0), Ok(1));
        assert_eq!(control.spec(id).max_connections, 1);
        assert_eq!(
            engine.set_connections(TransferId(99), 2),
            Err(ErrorCode::NotFound)
        );
    }

    #[test]
    fn a_limit_below_one_kib_per_second_is_refused() {
        let limit = |bps| Limits {
            max_download_bps: bps,
        };
        assert_eq!(limit(None).check(), Ok(()));
        assert_eq!(limit(Some(Limits::MIN_BPS)).check(), Ok(()));
        assert_eq!(limit(Some(u64::MAX)).check(), Ok(()));
        for bps in [0, 1, Limits::MIN_BPS - 1] {
            assert_eq!(
                limit(Some(bps)).check(),
                Err(ErrorCode::InvalidInput),
                "{bps}"
            );
        }
        let mut engine = FakeTransfer::new();
        let id = engine.add(spec()).unwrap();
        assert_eq!(
            engine.set_limits(id, limit(Some(0))),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(
            engine.set_global_limits(limit(Some(0))),
            Err(ErrorCode::InvalidInput)
        );
    }

    #[test]
    fn every_subscriber_gets_every_later_event() {
        let mut engine = FakeTransfer::new();
        let (a, b) = (engine.subscribe(), engine.subscribe());
        let id = engine.add(spec()).unwrap();
        engine.cancel(id).unwrap();
        engine.remove(id).unwrap();
        for rx in [a, b] {
            let events: Vec<_> = rx.try_iter().collect();
            assert_eq!(events.len(), 3, "{events:?}");
            assert_eq!(events[2], TransferEvent::Removed { id });
        }
    }

    #[test]
    fn remove_is_only_for_finished_transfers() {
        let mut engine = FakeTransfer::new();
        let id = engine.add(spec()).unwrap();
        assert_eq!(engine.remove(id), Err(ErrorCode::InvalidState));
        engine.cancel(id).unwrap();
        assert_eq!(engine.remove(id), Ok(()));
        assert_eq!(engine.snapshot(), Ok(Vec::new()));
    }

    #[test]
    fn dispatch_runs_each_command_against_the_engine() {
        let mut engine = FakeTransfer::new();
        let id = match dispatch(
            &mut engine,
            TransferCommand::Add {
                spec: Box::new(spec()),
            },
        )
        .unwrap()
        {
            TransferReply::Added { id } => id,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            dispatch(&mut engine, TransferCommand::Pause { id }),
            Ok(TransferReply::Done {})
        );
        assert_eq!(
            dispatch(
                &mut engine,
                TransferCommand::SetConnections {
                    id,
                    connections: 1000
                }
            ),
            Ok(TransferReply::Connections {
                connections: Capabilities::sample().max_connections_per_host
            })
        );
        assert_eq!(
            dispatch(&mut engine, TransferCommand::Resume { id: TransferId(99) }),
            Err(ErrorCode::NotFound)
        );
        let global = Limits {
            max_download_bps: Some(4 << 20),
        };
        assert_eq!(
            dispatch(
                &mut engine,
                TransferCommand::SetGlobalLimits { limits: global }
            ),
            Ok(TransferReply::Done {})
        );
        assert_eq!(engine.control().global_limits(), global);
        match dispatch(&mut engine, TransferCommand::Snapshot {}).unwrap() {
            TransferReply::Snapshot { transfers } => {
                assert_eq!(transfers.len(), 1);
                assert_eq!(transfers[0].status, DownloadStatus::Paused);
            }
            other => panic!("{other:?}"),
        }
    }
}
