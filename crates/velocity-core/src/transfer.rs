//! The `DirectTransfer` contract between the manager and a direct-download engine (#36,
//! docs/ENGINE_CONTRACT.md).
//!
//! Direct downloads only: HTTP, FTP, SFTP and Metalink. Torrent operations (trackers, peers, file
//! priorities, seeding, magnet metadata) are never part of this contract (research report section
//! 4.2). Every value that crosses it is owned and serialisable, so the same contract works with an
//! engine in the app's process and with a client for an engine in a child process (#38, #39).

#[cfg(test)]
pub(crate) mod fake;
mod missing;

pub use missing::MissingEngine;

use crate::domain::{
    Capabilities, DownloadStatus, EngineHealth, ErrorCode, Progress, TransferError, Verification,
};
use crate::naming::check_file_name;
use crate::request::{
    check_credentials, check_headers, check_referrer, check_user_agent, split_login,
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
/// Version 7 (#57) added the performance profile's limits: [`DirectTransfer::set_engine_limits`].
/// Version 8 (#40) replaced `destination` with [`TransferSpec::directory`] and
/// [`TransferSpec::file_name`], the credential handle with [`Credentials`], added
/// [`TransferSpec::referrer`], [`TransferSpec::user_agent`] and [`TransferSnapshot::file_name`];
/// engines now send headers and logins.
/// Version 9 (#51) added [`TransferSpec::require_encryption`] and
/// [`crate::domain::ProtocolCapabilities::encryption`]; engines now download `ftp` and `ftps`.
/// Version 10 (#53) added [`TransferSpec::ssh_key`], [`crate::domain::TransferError::host_key`] and
/// [`DirectTransfer::trust_host_key`]; engines now download `sftp`.
/// Version 11 (#54) added [`TransferSnapshot::content_type`], and engines may report
/// [`crate::domain::Capabilities::metalink`].
/// Version 12 (#59) added [`TransferSnapshot::retry_at_ms`].
/// Version 13 (#28) added [`crate::domain::Capabilities::libcurl`], for diagnostics.
/// Version 14 (#30) added [`DirectTransfer::set_engine_settings`].
pub const TRANSFER_CONTRACT_VERSION: u32 = 14;

/// An engine's id for one transfer. Assigned by the engine, never reused, and never shown as the
/// app's download id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(pub u64);

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

/// A login for a download's server (#40): sent only to the server of the download's first URL,
/// never after a redirect leaves it. It crosses the contract in memory only, through the engine's
/// pipe when the engine is a child process (PO decision on #40, ADR-0012), never a file, a log or
/// a process argument; `Debug` never prints either part.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credentials {
    pub user_name: String,
    pub password: String,
}

/// A private key file for an SFTP login, and its passphrase if the key is encrypted (#53). The
/// passphrase crosses the contract like a password; `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshKey {
    /// An absolute path. The engine reads the key when it connects.
    pub path: PathBuf,
    pub passphrase: Option<String>,
}

impl fmt::Debug for SshKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshKey")
            .field("path", &self.path)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user_name", &"<redacted>")
            .field("password", &"<redacted>")
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

    /// A limit in KiB/s, as V1's commands and the UI give it: 0 for none.
    pub fn from_kib(kib: u64) -> Result<Limits, ErrorCode> {
        let limits = Limits {
            max_download_bps: match kib {
                0 => None,
                kib => Some(kib.checked_mul(1024).ok_or(ErrorCode::InvalidInput)?),
            },
        };
        limits.check()?;
        Ok(limits)
    }
}

/// How far an engine may go for every transfer it runs (#57): set by the performance profile
/// ([`crate::performance::PerformanceSettings::engine_limits`]). Within them the engine still
/// adds connections one at a time, only while each helps (#47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineLimits {
    /// The most connections to one server, for one download and for all of them together.
    pub max_connections_per_host: u32,
    /// The smallest piece a download is split into.
    pub min_segment_bytes: u64,
}

impl EngineLimits {
    /// The most connections to one server any profile allows (V1's Maximum).
    pub const MAX_CONNECTIONS_PER_HOST: u32 = 16;
    /// The smallest and largest pieces a profile may set: 1 MiB to 1 GiB.
    pub const MIN_SEGMENT_BYTES: u64 = 1 << 20;
    pub const MAX_SEGMENT_BYTES: u64 = 1 << 30;
    /// Today's engine values, and the Balanced profile's: 8 per server, 1 MiB pieces.
    pub const DEFAULT: EngineLimits = EngineLimits {
        max_connections_per_host: 8,
        min_segment_bytes: 1 << 20,
    };

    /// `InvalidInput` unless 1 to [`Self::MAX_CONNECTIONS_PER_HOST`] connections per server and a
    /// smallest piece from [`Self::MIN_SEGMENT_BYTES`] to [`Self::MAX_SEGMENT_BYTES`].
    pub fn check(&self) -> Result<(), ErrorCode> {
        let hosts = 1..=Self::MAX_CONNECTIONS_PER_HOST;
        let segments = Self::MIN_SEGMENT_BYTES..=Self::MAX_SEGMENT_BYTES;
        if hosts.contains(&self.max_connections_per_host)
            && segments.contains(&self.min_segment_bytes)
        {
            Ok(())
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }
}

impl Default for EngineLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How the engine makes room for a file before writing it (#30; V1's `file_allocation`,
/// ADR-0005).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preallocation {
    /// Nothing: the file grows as bytes arrive. On Windows, a write far into it first zero-fills
    /// the gap (ENGINE_INTERNALS.md, "Disk writes").
    None,
    /// The part file is sparse: a gap takes no space and is never zero-filled. The engine's
    /// behaviour since #42's measurement.
    #[default]
    Sparse,
    /// The whole size is reserved as soon as it is known, so the disk can't run out halfway.
    Full,
}

/// How the engine retries, times out, shares connections and writes (#30; V1's performance and
/// advanced settings, mapped by ADR-0005). It applies to downloads that start, or retry, after
/// it is set; the connection budget applies at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineSettings {
    /// Failures in a row, with no new byte in between, before a download fails (V1's
    /// `max_tries`).
    pub failures_in_a_row: u32,
    /// The longest wait before retrying a failed request (V1's `retry_wait_secs`).
    pub max_retry_wait_secs: u32,
    /// No byte for this long: the download shows Stalled (V1's `lowest_speed_limit_kib`, on time
    /// rather than speed).
    pub stalled_after_secs: u32,
    /// No byte on a request for this long: it is reset and retried (V1's `timeout_secs`).
    pub reset_after_secs: u32,
    /// The longest a connection may take to open (V1's `connect_timeout_secs`).
    pub connect_timeout_secs: u32,
    /// How many times a failed download is started again (V1's `app_retries`).
    pub download_retries: u32,
    /// The most connections all downloads use together (V1's `connection_budget`).
    pub connection_budget: u32,
    /// How room is made for a file (V1's `file_allocation`).
    pub preallocation: Preallocation,
    /// MiB of received data each connection may have waiting to be written (V1's
    /// `disk_cache_mib`, as the writer's buffer).
    pub write_buffer_mib: u32,
}

impl EngineSettings {
    /// The engine's values before #30 made them settings (PO decisions on #47, #48 and #49).
    pub const DEFAULT: EngineSettings = EngineSettings {
        failures_in_a_row: 5,
        max_retry_wait_secs: 60,
        stalled_after_secs: 15,
        reset_after_secs: 30,
        connect_timeout_secs: 30,
        download_retries: 2,
        connection_budget: 48,
        preallocation: Preallocation::Sparse,
        write_buffer_mib: 2,
    };

    /// `InvalidInput` outside the bounds (V1's where it had them): 1 to 100 failures, a wait of
    /// 1 to 600 s, stalled after 5 to 600 s, reset after 5 to 3600 s and never before stalled, a
    /// connect timeout of 1 to 300 s, 0 to 10 retries, a budget of 1 to 256 connections, and 1 to
    /// 64 MiB per connection.
    pub fn check(&self) -> Result<(), ErrorCode> {
        let ok = (1..=100).contains(&self.failures_in_a_row)
            && (1..=600).contains(&self.max_retry_wait_secs)
            && (5..=600).contains(&self.stalled_after_secs)
            && (5..=3600).contains(&self.reset_after_secs)
            && self.reset_after_secs >= self.stalled_after_secs
            && (1..=300).contains(&self.connect_timeout_secs)
            && self.download_retries <= 10
            && (1..=256).contains(&self.connection_budget)
            && (1..=64).contains(&self.write_buffer_mib);
        if ok {
            Ok(())
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Everything an engine needs to start one direct transfer. The manager builds it from a
/// validated download record; the engine still treats it as untrusted input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferSpec {
    /// The source and any mirrors, primary first, with credentials removed. Never empty.
    pub urls: Vec<String>,
    /// The folder the file is saved in. The engine writes to a part file there and renames it only
    /// after verification (#43, #46).
    pub directory: PathBuf,
    /// The file's name in [`TransferSpec::directory`]; `None` to take the server's (#40): its
    /// `Content-Disposition`, else the final URL's last segment, sanitised, and made free
    /// (`a (1).zip`) when taken (PO decision on #40). The engine reports it in
    /// [`TransferSnapshot::file_name`]. A given name that is taken is refused with `FileExists`,
    /// as before; #31 adds the conflict settings. A resume always gives the name.
    pub file_name: Option<String>,
    /// A login for the server of the first URL only (#40).
    pub credentials: Option<Credentials>,
    /// For an `ftp://` link: insist on TLS (`AUTH TLS`) for the login and the file, and fail if
    /// the server can't, never falling back to plain FTP (PO decision on #51). `https://` and
    /// `ftps://` are always encrypted; an `http://` link that requires it is refused.
    pub require_encryption: bool,
    /// For an `sftp://` link: log in with this private key, as the user in `credentials` (#53).
    /// Its passphrase travels like a password: in memory and through the engine's pipe only.
    pub ssh_key: Option<SshKey>,
    /// Custom request headers, checked by [`crate::request::check_headers`]. Dropped, with the
    /// login and the referrer, once a redirect leaves the first URL's origin.
    pub headers: Vec<Header>,
    /// Sent as `Referer`.
    pub referrer: Option<String>,
    /// Sent as `User-Agent`; `None` for [`crate::request::default_user_agent`].
    pub user_agent: Option<String>,
    pub checksum: Option<Checksum>,
    /// Trusted piece hashes (#46); `None` until a Metalink file supplies them (#54).
    pub pieces: Option<PieceHashes>,
    pub limits: Limits,
    /// Requested connections; the engine may apply fewer (see [`DirectTransfer::set_connections`]).
    pub max_connections: u32,
    /// How to treat what an earlier attempt left on disk (#43). `None` for a new download.
    pub resume: Option<Resume>,
}

impl TransferSpec {
    /// Checks what a user or the browser may set on the requests (#40): no login in a URL, a safe
    /// file name, and valid headers, referrer, user agent and login. A resume must give the name.
    pub fn check_request(&self) -> Result<(), ErrorCode> {
        if self.urls.iter().any(|url| split_login(url).1.is_some()) {
            return Err(ErrorCode::InvalidUrl);
        }
        let plain_http = |url: &String| {
            url.split_once("://")
                .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http"))
        };
        if self.require_encryption && self.urls.iter().any(plain_http) {
            return Err(ErrorCode::InvalidInput);
        }
        match &self.file_name {
            Some(name) => check_file_name(name)?,
            None if self.resume.is_some() => return Err(ErrorCode::InvalidInput),
            None => {}
        }
        check_headers(&self.headers)?;
        if let Some(referrer) = &self.referrer {
            check_referrer(referrer)?;
        }
        if let Some(user_agent) = &self.user_agent {
            check_user_agent(user_agent)?;
        }
        if let Some(credentials) = &self.credentials {
            check_credentials(credentials)?;
        }
        if let Some(key) = &self.ssh_key {
            // A key logs in to SFTP as a user, from a file the engine can find wherever it runs.
            let sftp = |url: &String| {
                url.split_once("://")
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("sftp"))
            };
            let passphrase_ok = key.passphrase.as_deref().is_none_or(|p| {
                check_credentials(&Credentials {
                    user_name: "key".to_owned(),
                    password: p.to_owned(),
                })
                .is_ok()
            });
            if !self.urls.iter().all(sftp)
                || self.credentials.is_none()
                || !key.path.is_absolute()
                || !passphrase_ok
            {
                return Err(ErrorCode::InvalidInput);
            }
        }
        Ok(())
    }
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
    /// The name the file is saved under, once the engine knows it (#40): the spec's, or the
    /// server's, either made free when taken.
    pub file_name: Option<String>,
    /// The file's media type as the server gave it (`Content-Type` without its parameters, in
    /// lowercase), once it answered with the file (#54): how a link to a Metalink file is known.
    /// `None` for FTP and SFTP, and when the server gave none or an invalid one.
    pub content_type: Option<String>,
    /// While Retrying, when the next attempt starts, in milliseconds since the Unix epoch (#59);
    /// `None` in every other status.
    pub retry_at_ms: Option<u64>,
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

/// What the engine's supervisor fails a transfer with when its engine stopped unexpectedly (#39).
pub const ENGINE_STOPPED: &str = "the transfer engine stopped unexpectedly";
/// ... when its engine stopped responding and was killed (#48).
pub const ENGINE_HUNG: &str = "the transfer engine stopped responding and was restarted";
/// ... when the engine was restarted by hand ("Restart transfer engine").
pub const ENGINE_RESTARTED: &str = "the transfer engine was restarted";

/// Whether `error` is the supervisor's report of a transfer its engine lost, rather than a
/// failure of the transfer itself: the manager then starts it again on its own once an engine runs
/// (PO decision on #58).
pub fn lost_by_engine(error: &TransferError) -> bool {
    [ENGINE_STOPPED, ENGINE_HUNG, ENGINE_RESTARTED].contains(&error.message.as_str())
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
    /// Sets the limits every transfer runs within (#57): the most connections to one server, and
    /// the smallest piece. A lower cap applies to running transfers at once; a new smallest piece
    /// applies to transfers that start after it. `InvalidInput` for limits out of bounds.
    fn set_engine_limits(&mut self, limits: EngineLimits) -> Result<(), ErrorCode>;
    /// Sets how the engine retries, times out, shares connections and writes (#30). It applies
    /// to downloads that start or retry after it, and the connection budget at once.
    /// `InvalidInput` for settings out of bounds.
    fn set_engine_settings(&mut self, settings: EngineSettings) -> Result<(), ErrorCode>;
    /// Requests a connection count and returns the count the engine applied, which is at least 1
    /// and at most `capabilities().max_connections_per_host`.
    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode>;
    /// Trusts the SFTP host key the user confirmed (#53): the key the engine last saw from `host`
    /// and `port`, whose SHA-256 fingerprint is `fingerprint`, as a transfer's
    /// [`crate::domain::TransferError::host_key`] showed it. `NotFound` when the engine saw no
    /// such key there; `InvalidState` when the host has another key pinned, since a changed key
    /// is never trusted. The caller then resumes or adds again the transfer that waited for it.
    fn trust_host_key(&mut self, host: &str, port: u16, fingerprint: &str)
        -> Result<(), ErrorCode>;
    /// The current state of every transfer the engine knows.
    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode>;
    /// A new receiver for this engine's events. Each receiver gets every event from the moment
    /// it subscribed.
    fn subscribe(&mut self) -> Receiver<TransferEvent>;
    /// Whether the engine can take commands now. An engine in the app's process always can; a
    /// supervised one reports its child (#39).
    fn health(&self) -> EngineHealth {
        EngineHealth::Running
    }
    /// Starts a new engine in place of the running one ("Restart transfer engine"): transfers it
    /// was running are reported lost (#39, #58). `Unsupported` for an engine in the app's process.
    fn restart(&mut self) -> Result<(), ErrorCode> {
        Err(ErrorCode::Unsupported)
    }
    /// The engine's last stderr lines, redacted, for diagnostics (#28). Empty for an engine in the
    /// app's process, which logs through the app.
    fn recent_log(&self) -> Vec<String> {
        Vec::new()
    }
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
    SetEngineLimits {
        limits: EngineLimits,
    },
    SetEngineSettings {
        settings: EngineSettings,
    },
    TrustHostKey {
        host: String,
        port: u16,
        fingerprint: String,
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
        TransferCommand::SetEngineLimits { limits } => {
            engine.set_engine_limits(limits)?;
            TransferReply::Done {}
        }
        TransferCommand::SetEngineSettings { settings } => {
            engine.set_engine_settings(settings)?;
            TransferReply::Done {}
        }
        TransferCommand::TrustHostKey {
            host,
            port,
            fingerprint,
        } => {
            engine.trust_host_key(&host, port, &fingerprint)?;
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
            directory: PathBuf::from("downloads"),
            file_name: Some("a.zip".to_owned()),
            credentials: Some(Credentials {
                user_name: "alice".to_owned(),
                password: "hunter2".to_owned(),
            }),
            headers: vec![Header {
                name: "Cookie".to_owned(),
                value: "session=secret-value".to_owned(),
            }],
            referrer: Some("https://example.com/page".to_owned()),
            user_agent: None,
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
            require_encryption: false,
            ssh_key: None,
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

    /// #30: V1's performance and advanced settings, with their bounds.
    #[test]
    fn engine_settings_are_bounded() {
        assert_eq!(EngineSettings::DEFAULT.check(), Ok(()));
        let out = |change: fn(&mut EngineSettings)| {
            let mut s = EngineSettings::DEFAULT;
            change(&mut s);
            s.check()
        };
        for change in [
            (|s: &mut EngineSettings| s.failures_in_a_row = 0) as fn(&mut EngineSettings),
            |s| s.max_retry_wait_secs = 601,
            |s| s.stalled_after_secs = 4,
            |s| s.reset_after_secs = 10,
            |s| s.connect_timeout_secs = 0,
            |s| s.download_retries = 11,
            |s| s.connection_budget = 257,
            |s| s.write_buffer_mib = 0,
        ] {
            assert_eq!(out(change), Err(ErrorCode::InvalidInput));
        }
        let json = serde_json::to_value(EngineSettings::DEFAULT).unwrap();
        assert_eq!(json["preallocation"], "sparse");
        assert_eq!(json["writeBufferMib"], 2);
    }

    #[test]
    fn engine_limits_are_bounded() {
        assert_eq!(EngineLimits::DEFAULT.check(), Ok(()));
        let with = |host: u32, segment: u64| EngineLimits {
            max_connections_per_host: host,
            min_segment_bytes: segment,
        };
        assert_eq!(with(16, 1 << 30).check(), Ok(()));
        for bad in [
            with(0, 1 << 20),
            with(17, 1 << 20),
            with(8, (1 << 20) - 1),
            with(8, (1 << 30) + 1),
        ] {
            assert_eq!(bad.check(), Err(ErrorCode::InvalidInput), "{bad:?}");
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
            file_name: Some("a.zip".to_owned()),
            content_type: None,
            retry_at_ms: None,
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
            TransferCommand::SetEngineLimits {
                limits: EngineLimits::DEFAULT,
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
        let engine_limits = TransferCommand::SetEngineLimits {
            limits: EngineLimits::DEFAULT,
        };
        for pointer in ["", "/limits"] {
            refuses_an_extra_field(&engine_limits, pointer);
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
    fn a_login_crosses_the_contract_in_memory_with_its_spec() {
        let json = serde_json::to_value(spec()).unwrap();
        assert_eq!(json["credentials"]["userName"], "alice");
        assert_eq!(json["directory"], "downloads");
        assert_eq!(json["fileName"], "a.zip");
        assert_eq!(json["referrer"], "https://example.com/page");
        assert!(json["userAgent"].is_null());
        let back: TransferSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back, spec());
    }

    #[test]
    fn debug_output_never_shows_header_values_or_logins() {
        let shown = format!("{:?}", spec());
        for secret in ["secret-value", "alice", "hunter2"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        assert!(shown.contains("Cookie"));
    }

    #[test]
    fn a_spec_with_valid_request_options_passes_its_check() {
        assert_eq!(spec().check_request(), Ok(()));
        let unnamed = TransferSpec {
            file_name: None,
            resume: None,
            ..spec()
        };
        assert_eq!(unnamed.check_request(), Ok(()));
    }

    #[test]
    fn a_spec_with_unsafe_request_options_is_refused() {
        let with = |change: fn(&mut TransferSpec)| {
            let mut s = spec();
            change(&mut s);
            s.check_request()
        };
        let invalid = Err(ErrorCode::InvalidInput);
        assert_eq!(
            with(|s| s.headers[0].value = "a\r\nX: 1".to_owned()),
            invalid
        );
        assert_eq!(with(|s| s.user_agent = Some("a\nb".to_owned())), invalid);
        assert_eq!(
            with(|s| s.credentials.as_mut().unwrap().user_name = "a:b".to_owned()),
            invalid
        );
        assert_eq!(
            with(|s| s.referrer = Some("file:///etc".to_owned())),
            Err(ErrorCode::InvalidUrl)
        );
        // A login stays out of URLs: it travels as `credentials`.
        assert_eq!(
            with(|s| s.urls = vec!["https://u:p@example.com/a.zip".to_owned()]),
            Err(ErrorCode::InvalidUrl)
        );
        assert_eq!(
            with(|s| s.file_name = Some("../a.zip".to_owned())),
            Err(ErrorCode::InvalidFilename)
        );
        // A resume continues a file that has its name already.
        assert_eq!(with(|s| s.file_name = None), invalid);
    }

    #[test]
    fn an_ssh_key_needs_an_sftp_link_a_user_and_an_absolute_path() {
        let key_path = std::env::temp_dir().join("id_ed25519");
        let with_key = |url: &str, path: PathBuf, passphrase: Option<&str>, user: bool| {
            TransferSpec {
                urls: vec![url.to_owned()],
                ssh_key: Some(SshKey {
                    path,
                    passphrase: passphrase.map(str::to_owned),
                }),
                credentials: user.then(|| Credentials {
                    user_name: "alice".to_owned(),
                    password: String::new(),
                }),
                ..spec()
            }
            .check_request()
        };
        let sftp = "sftp://files.example.org/a.zip";
        assert_eq!(with_key(sftp, key_path.clone(), None, true), Ok(()));
        assert_eq!(
            with_key(sftp, key_path.clone(), Some("phrase"), true),
            Ok(())
        );
        let invalid = Err(ErrorCode::InvalidInput);
        // A key logs in to SFTP only, as a user, from an absolute path.
        assert_eq!(
            with_key("https://example.com/a", key_path.clone(), None, true),
            invalid
        );
        assert_eq!(with_key(sftp, key_path.clone(), None, false), invalid);
        assert_eq!(
            with_key(sftp, PathBuf::from("id_ed25519"), None, true),
            invalid
        );
        assert_eq!(with_key(sftp, key_path, Some("a\nb"), true), invalid);
    }

    #[test]
    fn debug_output_never_shows_a_key_passphrase() {
        let key = SshKey {
            path: PathBuf::from("/home/alice/.ssh/id_ed25519"),
            passphrase: Some("SECRET-PHRASE".to_owned()),
        };
        let shown = format!("{key:?}");
        assert!(!shown.contains("SECRET-PHRASE"), "{shown}");
        assert!(shown.contains("id_ed25519"));
    }

    #[test]
    fn encryption_is_required_only_where_a_link_can_have_it() {
        let requiring = |url: &str| TransferSpec {
            urls: vec![url.to_owned()],
            require_encryption: true,
            ..spec()
        };
        for url in [
            "ftp://ftp.example.org/a.zip",
            "ftps://ftp.example.org/a.zip",
            "https://example.com/a.zip",
        ] {
            assert_eq!(requiring(url).check_request(), Ok(()), "{url}");
        }
        // A plain http:// link can't be encrypted: refused rather than sent in clear (#51).
        for url in ["http://example.com/a.zip", "HTTP://example.com/a.zip"] {
            assert_eq!(
                requiring(url).check_request(),
                Err(ErrorCode::InvalidInput),
                "{url}"
            );
        }
    }

    #[test]
    fn limits_and_connections_reach_the_engine_within_its_capabilities() {
        let mut engine = FakeTransfer::new();
        let control = engine.control();
        let id = engine.add(spec()).unwrap();
        assert_eq!(control.spec(id).credentials, spec().credentials);

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
    fn a_speed_limit_is_in_kib_per_second_and_0_is_none() {
        assert_eq!(Limits::from_kib(0), Ok(Limits::default()));
        assert_eq!(
            Limits::from_kib(64),
            Ok(Limits {
                max_download_bps: Some(64 * 1024)
            })
        );
        assert_eq!(Limits::from_kib(u64::MAX), Err(ErrorCode::InvalidInput));
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
        let maximum = EngineLimits {
            max_connections_per_host: 16,
            ..EngineLimits::DEFAULT
        };
        assert_eq!(
            dispatch(
                &mut engine,
                TransferCommand::SetEngineLimits { limits: maximum }
            ),
            Ok(TransferReply::Done {})
        );
        assert_eq!(engine.control().engine_limits(), maximum);
        match dispatch(&mut engine, TransferCommand::Snapshot {}).unwrap() {
            TransferReply::Snapshot { transfers } => {
                assert_eq!(transfers.len(), 1);
                assert_eq!(transfers[0].status, DownloadStatus::Paused);
            }
            other => panic!("{other:?}"),
        }
    }
}
