//! Engine-neutral domain types shared with the UI (#25, docs/CONTRACTS.md).
//!
//! These types cross the UI contract, so they serialise in camelCase (error codes in
//! SCREAMING_SNAKE_CASE) and are mirrored by the Zod schemas in `src/contract.ts`. Contract
//! fixtures generated from them keep both sides in step. No aria2 shapes or option names appear
//! here: the V2 engine reports through these types (ADR-0001, ADR-0002).

use serde::{Deserialize, Serialize};

/// Where a download is in its life. The fourteen V1 values; new states need an ADR (#25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
    Queued,
    Connecting,
    Downloading,
    Paused,
    Verifying,
    Retrying,
    /// Transferring, but no bytes have arrived for a while (direct) or no peers (torrent).
    Stalled,
    Completed,
    Failed,
    Removed,
    /// Magnet or `.torrent` link: fetching the torrent's metadata.
    FetchingMetadata,
    /// Metadata is available and the user is choosing files.
    ChoosingFiles,
    /// Finished and verified; the torrent is uploading to other peers.
    Seeding,
    /// Reported by an engine the app doesn't recognise; shown, never acted on.
    Unknown,
}

impl DownloadStatus {
    /// Every status, in the order the UI lists them.
    pub const ALL: [Self; 14] = [
        Self::Queued,
        Self::Connecting,
        Self::Downloading,
        Self::Paused,
        Self::Verifying,
        Self::Retrying,
        Self::Stalled,
        Self::Completed,
        Self::Failed,
        Self::Removed,
        Self::FetchingMetadata,
        Self::ChoosingFiles,
        Self::Seeding,
        Self::Unknown,
    ];

    /// A terminal download doesn't change state on its own any more.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Removed)
    }
}

/// What kind of source a download comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadKind {
    Http,
    Ftp,
    Sftp,
    Metalink,
    Torrent,
    Magnet,
}

/// How a failure should be treated (V1's failure classes, without aria2 exit codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureClass {
    /// Worth retrying soon: a reset connection, a timeout, a 5xx other than 503, a file that
    /// changed on the server again after its download started over (#45).
    Transient,
    /// The server asked to slow down: 429, 503, `Retry-After`.
    Throttled,
    /// Retrying won't help without a change: 404, 401, an expired link, a full disk.
    Permanent,
}

/// A transfer failure as the engine reports it, independent of the engine behind it. The host key
/// and the confirmation are boxed, so the error stays small where it is passed by value (#67:
/// clippy's `result_large_err` in the engine).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferError {
    pub class: FailureClass,
    pub http_status: Option<u16>,
    pub message: String,
    pub retryable: bool,
    /// An SFTP server's host key that wasn't trusted (#53): the download waits for the user to
    /// confirm an unknown key's fingerprint, and a changed key is always refused. `None` otherwise.
    pub host_key: Option<Box<HostKeyCheck>>,
    /// What the user must confirm before the download can go on (#67): it waits, and goes on
    /// once allowed ([`crate::manager::Manager::allow_download`]). `None` otherwise; absent in
    /// errors stored before it existed.
    #[serde(default)]
    pub confirm: Option<Box<Confirm>>,
}

/// What a download stopped at, for the user to confirm (#67).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Confirm {
    /// The server redirected from `https` to `http`, to this host.
    InsecureRedirect { host: String },
    /// A link from the browser reached this computer or a private network (ADR-0006): the host
    /// the link names and the address it connected to.
    LocalAddress { host: String, address: String },
}

impl Confirm {
    /// What allowing it is called.
    pub fn kind(&self) -> ConfirmKind {
        match self {
            Confirm::InsecureRedirect { .. } => ConfirmKind::InsecureRedirect,
            Confirm::LocalAddress { .. } => ConfirmKind::LocalAddress,
        }
    }
}

/// What the user allows for one download (#67), answering a [`Confirm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfirmKind {
    InsecureRedirect,
    LocalAddress,
}

/// An SFTP server's host key, as the user must see it before trusting it (#53, research report
/// section 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostKeyCheck {
    pub host: String,
    pub port: u16,
    /// The key the server presented, as OpenSSH prints its SHA-256 fingerprint: `SHA256:…`.
    pub fingerprint: String,
    /// When the key changed: the fingerprint of the one pinned before. A changed key is never
    /// trusted; `None` for a key that is only unknown.
    pub pinned: Option<String>,
}

impl TransferError {
    /// Builds an error; only transient and throttled failures are retryable.
    pub fn new(class: FailureClass, http_status: Option<u16>, message: &str) -> Self {
        Self {
            class,
            http_status,
            message: message.to_owned(),
            retryable: class != FailureClass::Permanent,
            host_key: None,
            confirm: None,
        }
    }

    /// The same error, about an SFTP server's untrusted host key (#53).
    pub fn with_host_key(mut self, check: HostKeyCheck) -> Self {
        self.host_key = Some(Box::new(check));
        self
    }

    /// The same error, waiting for the user to confirm `confirm` (#67).
    pub fn with_confirm(mut self, confirm: Confirm) -> Self {
        self.confirm = Some(Box::new(confirm));
        self
    }
}

/// Progress of one download. Sizes in bytes, speeds in bytes per second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Progress {
    /// The bytes of the file the download holds. A resumed download counts what it resumed from
    /// as well as what arrived since (#257).
    pub completed_bytes: u64,
    /// `None` while the size is unknown (a chunked response, a magnet before metadata).
    pub total_bytes: Option<u64>,
    pub download_speed_bps: u64,
    pub upload_speed_bps: u64,
    /// Connections in use now.
    pub connections: u32,
    /// How a direct download's connection count came about (#47). `None` for torrents, and while
    /// no engine reports it.
    pub connection_detail: Option<ConnectionDetail>,
}

/// A direct download's connections: what was asked for, what the engine allows, and what holds
/// it back (#47, V1 #44's semantics). The connections in use now are [`Progress::connections`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionDetail {
    /// What the user or the download's settings asked for.
    pub requested: u32,
    /// What the engine allows now: at least 1, and never more than `requested`.
    pub allowed: u32,
    /// Why `allowed` is below `requested`; `None` when it isn't.
    pub limited_by: Option<ConnectionLimit>,
}

/// What holds a direct download's connections below what it asked for (#47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionLimit {
    /// The most connections the engine opens to one host.
    PerHostCap,
    /// The connections shared by all downloads; the others have their fair share.
    SharedBudget,
    /// The server answered `429` or `503`, so the download uses fewer connections.
    ServerThrottling,
    /// Adaptive connections: it is still finding how many help, by trying fewer (#221; #47
    /// added them one at a time, hence the name).
    RampingUp,
    /// Adaptive connections: more connections stopped making it faster.
    NoGain,
    /// The rest of the file is too small to split between more connections.
    FileSize,
}

impl ConnectionLimit {
    /// Every value, in contract order (the fixture shared with `src/contract.ts`).
    pub const ALL: [ConnectionLimit; 6] = [
        ConnectionLimit::PerHostCap,
        ConnectionLimit::SharedBudget,
        ConnectionLimit::ServerThrottling,
        ConnectionLimit::RampingUp,
        ConnectionLimit::NoGain,
        ConnectionLimit::FileSize,
    ];
}

/// What was proven about a completed download's bytes (#46, research report section 5.3,
/// ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verification {
    /// Its whole-file hash matched a checksum the user or a trusted source gave.
    Verified,
    /// Every byte arrived and the length matched; no hash was checked.
    SizeVerified,
    /// It resumed from bytes on disk without a strong validator, so a file that changed on the
    /// server at the same length could have mixed two versions (ADR-0010).
    NotVerified,
}

impl Verification {
    /// Every value, in contract order (the fixture shared with `src/contract.ts`).
    pub const ALL: [Verification; 3] = [
        Verification::Verified,
        Verification::SizeVerified,
        Verification::NotVerified,
    ];
}

/// One download as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecord {
    /// Stable app id; never an engine's own id.
    pub id: String,
    pub kind: DownloadKind,
    /// The source as the user gave it, with credentials removed (#70).
    pub source_url: String,
    pub file_name: Option<String>,
    pub directory: String,
    pub category_id: Option<String>,
    pub status: DownloadStatus,
    pub progress: Progress,
    pub error: Option<TransferError>,
    /// What was proven about its bytes, once it completed; `None` until then (#46).
    pub verification: Option<Verification>,
    /// Milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    /// While Retrying, when the engine tries again, in milliseconds since the Unix epoch (#59);
    /// `None` in every other status. Never invented: only an engine reports it.
    pub retry_at_ms: Option<u64>,
    /// The first time the engine reported it running, in milliseconds since the Unix epoch (#59).
    pub started_at_ms: Option<u64>,
    /// The bytes per second it received while it ran, once it ran for a second (#59).
    pub average_speed_bps: Option<u64>,
    /// The download's own speed limit, in bytes per second; `None` for none (#61). The global
    /// limit holds alongside it.
    pub speed_limit_bps: Option<u64>,
    /// The connections a direct download asks for (#61); `None` for a torrent, whose peers
    /// libtorrent chooses.
    pub requested_connections: Option<u32>,
    /// What the torrent helper last reported about a torrent's peers and seeding (#61); `None`
    /// for a direct download, and until the helper reports.
    pub torrent: Option<TorrentActivity>,
}

/// A torrent's peers and seeding, as the torrent helper reports them (#61).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentActivity {
    /// Peers connected now, and how many of them have the whole torrent.
    pub peers: u32,
    pub seeds: u32,
    /// Bytes uploaded and seconds spent seeding over the torrent's life.
    pub uploaded_bytes: u64,
    pub seeding_secs: u64,
    /// Completed and allowed to seed, but waiting for a seeding slot (#63).
    pub seed_queued: bool,
}

/// What the UI is sent about the downloads (#58, V1's `get_snapshot`), whenever something in it
/// changed, at most every 250 ms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadsSnapshot {
    /// The downloads in the list: the unfinished ones in queue order, then the finished ones,
    /// newest first.
    pub downloads: Vec<DownloadRecord>,
    /// Stored downloads this build can't act on: shown, never acted on (report section 6).
    pub unavailable: Vec<UnavailableDownload>,
    /// Downloads that came back after a restart without the login they had: resuming or retrying
    /// one asks for it first (PO decision on #58).
    pub needs_login: Vec<String>,
    pub engine: EngineHealth,
    /// The torrent helper's health (#58).
    pub torrent_engine: EngineHealth,
    /// The last failure to save a download, if any.
    pub storage_error: Option<ErrorCode>,
}

/// What adding a download would do, before it is added (#31, V1's destination check): the
/// folder and category chosen, the file's name when it is known, and whether that name is
/// taken. Free disk space isn't reported yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DestinationCheck {
    pub directory: String,
    pub directory_exists: bool,
    /// The folder came from the category or the settings, not from the user.
    pub automatic_directory: bool,
    pub category_id: Option<String>,
    /// `None` when the server will name the file.
    pub file_name: Option<String>,
    pub file_exists: bool,
    /// A download, V2's or V1's, that hasn't finished has the name.
    pub partial_download_exists: bool,
    /// When the name is taken, the first free `name (n).ext`.
    pub suggested_name: Option<String>,
}

/// What a command over several downloads did (#58, V1's batch result): the ids it was done for,
/// in order, those it failed for with why, and, when removing with files, the files kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchResult {
    pub done: Vec<String>,
    pub failed: Vec<BatchFailure>,
    pub kept: Vec<KeptFile>,
}

/// One id a command over several downloads failed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchFailure {
    pub id: String,
    pub code: ErrorCode,
}

/// A file a removal with files left in place, and why (PO decision on #58).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeptFile {
    pub path: String,
    pub reason: KeptReason,
    /// For [`KeptReason::Failed`]: what deleting it failed with.
    pub code: Option<ErrorCode>,
}

/// Why a file wasn't deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeptReason {
    /// A link, a reparse point or not a plain file: never followed.
    NotPlain,
    /// Larger than the download: not its own.
    TooLarge,
    /// Reached through a folder that is a link: never followed.
    BehindLink,
    /// A folder with something else in it.
    NotEmpty,
    /// Deleting it failed.
    Failed,
}

impl KeptReason {
    /// Every value, in contract order (the fixture shared with `src/contract.ts`).
    pub const ALL: [KeptReason; 5] = [
        KeptReason::NotPlain,
        KeptReason::TooLarge,
        KeptReason::BehindLink,
        KeptReason::NotEmpty,
        KeptReason::Failed,
    ];
}

/// A stored download this build can't act on (an engine, kind, status or options it doesn't know),
/// with what can be shown of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnavailableDownload {
    pub id: String,
    pub source_url: String,
    pub file_name: Option<String>,
    pub directory: String,
    pub created_at_ms: u64,
}

/// Whether a supervised engine, or the torrent helper, can take commands (#39, #58), as the app
/// shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum EngineHealth {
    Running,
    /// It stopped and is being started again. `attempt` counts the crashes within the restart
    /// window; it is 0 after a restart by hand.
    Restarting {
        attempt: u32,
    },
    /// It isn't running and won't be restarted on its own; "Restart transfer engine" tries again.
    Unavailable {
        code: ErrorCode,
    },
}

/// What an engine really supports, reported rather than assumed (research report section 4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    /// The protocols it downloads, each with what it does for it (#57). One that isn't listed
    /// isn't supported.
    pub protocols: Vec<ProtocolCapabilities>,
    /// Connections can change during a transfer without restarting it.
    pub live_connection_changes: bool,
    pub metalink: bool,
    /// The most connections to one server now: the performance profile's cap (#57).
    pub max_connections_per_host: u32,
    /// The libcurl it runs on, for diagnostics (#28); `None` for an engine without one.
    pub libcurl: Option<LibraryVersion>,
}

/// A native library an engine runs on, as it reports itself (#28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryVersion {
    /// For example `8.23.0`.
    pub version: String,
    /// Its TLS backend, for example `Schannel` or `OpenSSL/3.5.9`.
    pub tls: String,
}

/// What an engine does for one protocol (#57), reported rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolCapabilities {
    pub kind: DownloadKind,
    /// Parallel segments over several connections.
    pub segmented: bool,
    /// A download resumes after a pause, a restart or a crash.
    pub resume: bool,
    /// Resume is checked against a strong validator (ETag or Last-Modified, #45).
    pub validator_resume: bool,
    /// HTTP/2, with several requests as streams of one connection (#47).
    pub http2: bool,
    /// The connection can be encrypted, with the server's certificate verified: `https://` for
    /// HTTP, FTPS for FTP (#51). A plain `ftp://` link is encrypted only when the download requires
    /// it; the add form warns before it starts one that isn't (PO decision on #51).
    pub encryption: bool,
}

impl Capabilities {
    /// What the engine does for `kind`; `None` when it doesn't download it.
    pub fn protocol(&self, kind: DownloadKind) -> Option<&ProtocolCapabilities> {
        self.protocols.iter().find(|p| p.kind == kind)
    }
}

/// Stable error codes for command failures. The code is the contract; the message is for people.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidInput,
    InvalidUrl,
    UnsupportedScheme,
    InvalidPath,
    InvalidFilename,
    DestinationMissing,
    PermissionDenied,
    FileExists,
    NotFound,
    NameTaken,
    InvalidState,
    EngineMissing,
    EngineIncompatible,
    EngineStartFailed,
    EngineUnavailable,
    Database,
    DatabaseMigration,
    Io,
    InsufficientDiskSpace,
    ProtocolDisabled,
    ConfirmationRequired,
    /// The system keychain is missing or refused (#70): nothing was saved in it.
    KeychainUnavailable,
    /// The proxy the user set can't carry this kind of download (#192): torrents through an
    /// HTTPS proxy.
    ProxyUnsupported,
    Unsupported,
    Internal,
}

impl ErrorCode {
    /// Every code. V1's codes without the four aria2 JSON-RPC ones (`RPC_*`).
    pub const ALL: [Self; 25] = [
        Self::InvalidInput,
        Self::InvalidUrl,
        Self::UnsupportedScheme,
        Self::InvalidPath,
        Self::InvalidFilename,
        Self::DestinationMissing,
        Self::PermissionDenied,
        Self::FileExists,
        Self::NotFound,
        Self::NameTaken,
        Self::InvalidState,
        Self::EngineMissing,
        Self::EngineIncompatible,
        Self::EngineStartFailed,
        Self::EngineUnavailable,
        Self::Database,
        Self::DatabaseMigration,
        Self::Io,
        Self::InsufficientDiskSpace,
        Self::ProtocolDisabled,
        Self::ConfirmationRequired,
        Self::KeychainUnavailable,
        Self::ProxyUnsupported,
        Self::Unsupported,
        Self::Internal,
    ];

    /// The sentence shown to the user for this code.
    pub fn user_message(self) -> &'static str {
        match self {
            Self::InvalidInput => "Some of the values entered aren't valid.",
            Self::InvalidUrl => "The link isn't a valid download address.",
            Self::UnsupportedScheme => "This kind of link can't be downloaded.",
            Self::InvalidPath => "The folder path isn't valid.",
            Self::InvalidFilename => "The file name isn't allowed on this system.",
            Self::DestinationMissing => "The download folder doesn't exist.",
            Self::PermissionDenied => "Velocity isn't allowed to write to that location.",
            Self::FileExists => "A file with that name already exists.",
            Self::NotFound => "That download or file no longer exists.",
            Self::NameTaken => "That name is already taken.",
            Self::InvalidState => "That action isn't possible in the download's current state.",
            Self::EngineMissing => "The transfer engine is missing from this installation.",
            Self::EngineIncompatible => {
                "The transfer engine doesn't match this version of Velocity."
            }
            Self::EngineStartFailed => "The transfer engine couldn't be started.",
            Self::EngineUnavailable => "The transfer engine isn't responding.",
            Self::Database => "Velocity couldn't read or save its data.",
            Self::DatabaseMigration => "Velocity couldn't upgrade its data to this version.",
            Self::Io => "A file couldn't be read or written.",
            Self::InsufficientDiskSpace => "There isn't enough free disk space.",
            Self::ProtocolDisabled => "This kind of download is turned off in Settings.",
            Self::ConfirmationRequired => "This action needs your confirmation first.",
            Self::KeychainUnavailable => {
                "The system keychain isn't available, so the login wasn't saved."
            }
            Self::ProxyUnsupported => {
                "Torrents can't go through an HTTPS proxy. Choose an HTTP or SOCKS5 proxy in Settings, Proxy."
            }
            Self::Unsupported => "This isn't supported on this system yet.",
            Self::Internal => "Something went wrong inside Velocity.",
        }
    }
}

impl DownloadRecord {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        Self {
            id: "d-0001".to_owned(),
            kind: DownloadKind::Http,
            source_url: "https://example.com/files/archive.zip".to_owned(),
            file_name: Some("archive.zip".to_owned()),
            directory: "/home/user/Downloads".to_owned(),
            // A built-in category (#31), so the sample can be saved.
            category_id: Some("compressed".to_owned()),
            status: DownloadStatus::Retrying,
            progress: Progress {
                completed_bytes: 1_048_576,
                total_bytes: Some(4_194_304),
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: Some(ConnectionDetail {
                    requested: 8,
                    allowed: 4,
                    limited_by: Some(ConnectionLimit::ServerThrottling),
                }),
            },
            error: Some(TransferError::new(
                FailureClass::Throttled,
                Some(503),
                "Service unavailable",
            )),
            verification: None,
            created_at_ms: 1_792_600_000_000,
            completed_at_ms: None,
            retry_at_ms: Some(1_792_600_030_000),
            started_at_ms: Some(1_792_600_001_000),
            average_speed_bps: Some(262_144),
            speed_limit_bps: Some(524_288),
            requested_connections: Some(8),
            torrent: None,
        }
    }
}

impl DownloadsSnapshot {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        // A seeding torrent beside the direct sample, so the fixture has its peers and seeding.
        let torrent = DownloadRecord {
            id: "d-0003".to_owned(),
            kind: DownloadKind::Magnet,
            source_url: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned(),
            file_name: Some("album".to_owned()),
            category_id: None,
            status: DownloadStatus::Seeding,
            progress: Progress {
                completed_bytes: 4_194_304,
                total_bytes: Some(4_194_304),
                download_speed_bps: 0,
                upload_speed_bps: 65_536,
                connections: 7,
                connection_detail: None,
            },
            error: None,
            verification: None,
            retry_at_ms: None,
            completed_at_ms: Some(1_792_600_060_000),
            requested_connections: None,
            speed_limit_bps: None,
            torrent: Some(TorrentActivity {
                peers: 7,
                seeds: 2,
                uploaded_bytes: 8_388_608,
                seeding_secs: 600,
                seed_queued: false,
            }),
            ..DownloadRecord::sample()
        };
        Self {
            downloads: vec![DownloadRecord::sample(), torrent],
            unavailable: vec![UnavailableDownload {
                id: "d-0002".to_owned(),
                source_url: "https://example.com/files/later.iso".to_owned(),
                file_name: None,
                directory: "/home/user/Downloads".to_owned(),
                created_at_ms: 1_792_600_000_000,
            }],
            needs_login: vec!["d-0001".to_owned()],
            engine: EngineHealth::Restarting { attempt: 1 },
            torrent_engine: EngineHealth::Running,
            storage_error: Some(ErrorCode::Database),
        }
    }
}

impl DestinationCheck {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        Self {
            directory: "/home/user/Downloads/Documents".to_owned(),
            directory_exists: true,
            automatic_directory: true,
            category_id: Some("documents".to_owned()),
            file_name: Some("report.pdf".to_owned()),
            file_exists: true,
            partial_download_exists: false,
            suggested_name: Some("report (1).pdf".to_owned()),
        }
    }
}

impl BatchResult {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        Self {
            done: vec!["d-0001".to_owned()],
            failed: vec![BatchFailure {
                id: "d-0002".to_owned(),
                code: ErrorCode::InvalidState,
            }],
            kept: vec![
                KeptFile {
                    path: "/home/user/Downloads/archive.zip".to_owned(),
                    reason: KeptReason::TooLarge,
                    code: None,
                },
                KeptFile {
                    path: "/home/user/Downloads/archive.zip.vdmpart".to_owned(),
                    reason: KeptReason::Failed,
                    code: Some(ErrorCode::PermissionDenied),
                },
            ],
        }
    }
}

impl Capabilities {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        Self {
            protocols: vec![
                ProtocolCapabilities {
                    kind: DownloadKind::Http,
                    segmented: true,
                    resume: true,
                    validator_resume: true,
                    http2: true,
                    encryption: true,
                },
                ProtocolCapabilities {
                    kind: DownloadKind::Ftp,
                    segmented: false,
                    resume: true,
                    validator_resume: true,
                    http2: false,
                    encryption: true,
                },
            ],
            live_connection_changes: true,
            metalink: false,
            max_connections_per_host: 8,
            libcurl: Some(LibraryVersion {
                version: "8.23.0".to_owned(),
                tls: "OpenSSL/3.5.9".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_the_fourteen_v1_values_in_camel_case() {
        let names: Vec<String> = DownloadStatus::ALL
            .iter()
            .map(|s| {
                serde_json::to_value(s)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "queued",
                "connecting",
                "downloading",
                "paused",
                "verifying",
                "retrying",
                "stalled",
                "completed",
                "failed",
                "removed",
                "fetchingMetadata",
                "choosingFiles",
                "seeding",
                "unknown",
            ]
        );
    }

    #[test]
    fn only_completed_failed_and_removed_are_terminal() {
        let terminal: Vec<_> = DownloadStatus::ALL
            .iter()
            .filter(|s| s.is_terminal())
            .copied()
            .collect();
        assert_eq!(
            terminal,
            [
                DownloadStatus::Completed,
                DownloadStatus::Failed,
                DownloadStatus::Removed
            ]
        );
    }

    #[test]
    fn a_permanent_failure_is_never_retryable() {
        let e = TransferError::new(FailureClass::Permanent, Some(404), "Not found");
        assert!(!e.retryable);
        assert!(TransferError::new(FailureClass::Transient, None, "Reset").retryable);
        assert!(
            TransferError::new(FailureClass::Throttled, Some(429), "Too many requests").retryable
        );
    }

    #[test]
    fn transfer_errors_serialise_without_aria2_terms() {
        let json = serde_json::to_string(&TransferError::new(
            FailureClass::Throttled,
            Some(503),
            "Service unavailable",
        ))
        .unwrap();
        assert_eq!(
            json,
            r#"{"class":"throttled","httpStatus":503,"message":"Service unavailable","retryable":true,"hostKey":null,"confirm":null}"#
        );
    }

    #[test]
    fn what_a_download_waits_to_confirm_serialises_with_its_kind() {
        let e = TransferError::new(FailureClass::Permanent, None, "local").with_confirm(
            Confirm::LocalAddress {
                host: "router.example".to_owned(),
                address: "192.168.1.1".to_owned(),
            },
        );
        assert_eq!(
            serde_json::to_value(&e).unwrap()["confirm"],
            serde_json::json!({"kind":"localAddress","host":"router.example","address":"192.168.1.1"})
        );
        let insecure = Confirm::InsecureRedirect {
            host: "mirror.example".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&insecure).unwrap(),
            r#"{"kind":"insecureRedirect","host":"mirror.example"}"#
        );
        assert_eq!(insecure.kind(), ConfirmKind::InsecureRedirect);
        assert_eq!(
            serde_json::to_string(&ConfirmKind::LocalAddress).unwrap(),
            r#""localAddress""#
        );
        // An error stored before #67 has no `confirm`, and still reads.
        let old: TransferError = serde_json::from_str(
            r#"{"class":"permanent","httpStatus":null,"message":"m","retryable":false,"hostKey":null}"#,
        )
        .unwrap();
        assert_eq!(old.confirm, None);
    }

    #[test]
    fn connection_detail_serialises_in_camel_case() {
        let detail = ConnectionDetail {
            requested: 8,
            allowed: 4,
            limited_by: Some(ConnectionLimit::SharedBudget),
        };
        assert_eq!(
            serde_json::to_string(&detail).unwrap(),
            r#"{"requested":8,"allowed":4,"limitedBy":"sharedBudget"}"#
        );
        let limits: Vec<_> = ConnectionLimit::ALL
            .iter()
            .map(|l| serde_json::to_value(l).unwrap())
            .collect();
        assert_eq!(
            limits,
            [
                "perHostCap",
                "sharedBudget",
                "serverThrottling",
                "rampingUp",
                "noGain",
                "fileSize"
            ]
        );
    }

    #[test]
    fn every_error_code_has_a_distinct_user_facing_message() {
        let mut seen = std::collections::HashSet::new();
        for code in ErrorCode::ALL {
            let m = code.user_message();
            assert!(m.len() > 10, "{code:?} has no real message");
            assert!(m.ends_with('.'), "{code:?}: messages are full sentences");
            assert!(
                !m.contains("aria2") && !m.contains("RPC"),
                "{code:?} mentions aria2"
            );
            assert!(seen.insert(m), "{code:?} repeats another code's message");
        }
    }

    #[test]
    fn error_codes_are_screaming_snake_case_without_rpc_codes() {
        let names: Vec<String> = ErrorCode::ALL
            .iter()
            .map(|c| {
                serde_json::to_value(c)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert!(names.contains(&"INVALID_URL".to_owned()));
        assert!(names.contains(&"ENGINE_UNAVAILABLE".to_owned()));
        assert!(names.iter().all(|n| !n.starts_with("RPC_")));
        assert!(names
            .iter()
            .all(|n| n.chars().all(|c| c.is_ascii_uppercase() || c == '_')));
    }

    /// Contract fixtures shared with the TypeScript tests (`src/contract.test.ts`).
    /// Regenerate with `VELOCITY_UPDATE_FIXTURES=1 cargo test -p velocity-core contract`.
    mod contract {
        use super::*;

        fn check(name: &str, value: serde_json::Value) {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/contract")
                .join(name);
            let pretty = serde_json::to_string_pretty(&value).unwrap() + "\n";
            if std::env::var_os("VELOCITY_UPDATE_FIXTURES").is_some() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &pretty).unwrap();
                return;
            }
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("missing fixture {}", path.display()))
                .replace("\r\n", "\n");
            assert_eq!(
                on_disk, pretty,
                "fixture {name} is out of date; regenerate it with VELOCITY_UPDATE_FIXTURES=1"
            );
        }

        #[test]
        fn contract_statuses() {
            check(
                "statuses.json",
                serde_json::to_value(DownloadStatus::ALL).unwrap(),
            );
        }

        #[test]
        fn contract_connection_limits() {
            check(
                "connection-limits.json",
                serde_json::to_value(ConnectionLimit::ALL).unwrap(),
            );
        }

        #[test]
        fn contract_verifications() {
            check(
                "verifications.json",
                serde_json::to_value(Verification::ALL).unwrap(),
            );
        }

        #[test]
        fn contract_error_codes() {
            let codes: Vec<_> = ErrorCode::ALL
                .iter()
                .map(|c| serde_json::json!({ "code": c, "message": c.user_message() }))
                .collect();
            check("error-codes.json", serde_json::Value::Array(codes));
        }

        #[test]
        fn contract_download_record() {
            check(
                "download-record.json",
                serde_json::to_value(DownloadRecord::sample()).unwrap(),
            );
        }

        #[test]
        fn contract_capabilities_name_what_each_protocol_does() {
            let c = Capabilities::sample();
            assert!(c
                .protocol(DownloadKind::Http)
                .is_some_and(|p| p.segmented && p.http2));
            assert!(c
                .protocol(DownloadKind::Ftp)
                .is_some_and(|p| !p.segmented && p.resume && p.encryption));
            assert_eq!(
                c.protocol(DownloadKind::Sftp),
                None,
                "not listed: not supported"
            );
        }

        #[test]
        fn contract_capabilities() {
            check(
                "capabilities.json",
                serde_json::to_value(Capabilities::sample()).unwrap(),
            );
        }

        #[test]
        fn contract_downloads_snapshot() {
            check(
                "downloads-snapshot.json",
                serde_json::to_value(DownloadsSnapshot::sample()).unwrap(),
            );
        }

        #[test]
        fn contract_destination_check() {
            check(
                "destination-check.json",
                serde_json::to_value(DestinationCheck::sample()).unwrap(),
            );
        }

        /// The Add dialog's request and its results (#60).
        /// Every setting the Settings screen edits, at its defaults (#30): read by Zod.
        #[test]
        fn contract_settings() {
            check(
                "settings.json",
                serde_json::to_value(crate::settings::Settings::sample()).unwrap(),
            );
        }

        /// What the diagnostics show (#28).
        #[test]
        fn contract_diagnostics() {
            check(
                "diagnostics.json",
                serde_json::to_value(crate::diagnostics::DiagnosticReport::sample()).unwrap(),
            );
        }

        /// The history's page and query, and the retention's defaults (#32).
        #[test]
        fn contract_history() {
            use crate::history::{HistoryPage, HistoryQuery, HistoryStatus, StorageSettings};
            check(
                "history.json",
                serde_json::json!({
                    "page": HistoryPage::sample(),
                    "query": HistoryQuery {
                        search: Some("album".to_owned()),
                        status: Some(HistoryStatus::Cancelled),
                        limit: Some(200),
                        offset: Some(0),
                    },
                    "storage": StorageSettings::default(),
                }),
            );
        }

        /// A torrent's details, a `.torrent` file's preview, its trackers and its peers (#63).
        #[test]
        fn contract_torrent() {
            use crate::manager::{
                sample_swarm, Duplicate, TorrentContents, TorrentDetails, TorrentPreview,
            };
            let (trackers, peers) = sample_swarm();
            let preview = TorrentPreview {
                contents: TorrentContents::sample(),
                duplicate: Some(Duplicate {
                    existing_id: "d-0003".to_owned(),
                    name: Some("album".to_owned()),
                    status: DownloadStatus::Seeding,
                    in_list: true,
                    can_add_anyway: false,
                }),
            };
            check(
                "torrent.json",
                serde_json::json!({
                    "details": TorrentDetails::sample(),
                    "preview": preview,
                    "trackers": trackers,
                    "peers": peers,
                }),
            );
        }

        #[test]
        fn contract_add() {
            use crate::manager::{AddRequest, AddResult};
            let request = AddRequest::sample();
            check("add-request.json", serde_json::to_value(&request).unwrap());
            let back: AddRequest =
                serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
            assert_eq!(back, request);
            check(
                "add-results.json",
                serde_json::to_value(AddResult::samples()).unwrap(),
            );
        }

        /// The browser bridge's types (#65): the Settings section's integration, and the link
        /// the `browser-request` event carries.
        #[test]
        fn contract_browser() {
            use crate::browser::bridge::BrowserPrompt;
            use crate::browser::registration::BrowserIntegration;
            check(
                "browser-integration.json",
                serde_json::to_value(BrowserIntegration::sample()).unwrap(),
            );
            check(
                "browser-request.json",
                serde_json::to_value(BrowserPrompt::sample()).unwrap(),
            );
        }

        /// The names `src/naming.ts` must give, from the Rust rules (#31).
        #[test]
        fn contract_naming_cases() {
            use crate::naming::name_for_download;
            const KNOWN: [&str; 3] = ["zip", "pdf", "txt"];
            let long = "x".repeat(300);
            let long_url = format!("https://e.com/{long}.iso");
            let long_typed = "y".repeat(238);
            let urls = [
                "https://e.com/a/My%20File.iso?x=1#part",
                "https://e.com/a/b%2Fc.zip",
                "https://e.com/%E2%9C%93-r%C3%A9sum%C3%A9.pdf",
                "https://e.com/bad%FF%FEbytes.bin",
                "https://e.com/half%2.bin",
                "https://e.com/ctl%0A%09name.txt",
                "https://e.com/CON.txt",
                "https://e.com/con%20.tar.gz",
                "https://e.com/LPT%C2%B9.zip",
                "https://e.com/a.zip.VDMPART",
                "https://e.com/archive.tar.gz?token=abc",
                "ftp://e.com/pub/file.ISO",
                "https://e.com/trailing.%20.%20.zip",
                "https://e.com/%20%20.zip",
                "https://e.com/dir/",
                "https://e.com/files/12345",
                "https://e.com/download.php?id=3",
                "https://e.com/Get.ASPX?f=1",
                "https://e.com/.hidden",
                "https://e.com/has%20space.not%20ext",
                "https://e.com/",
                "https://e.com",
                "https://e.com?q=/a.zip",
                "not a url",
                long_url.as_str(),
            ];
            let typed = [
                ("Annual report", "https://e.com/report.pdf"),
                ("release-1.2", "https://e.com/r.zip"),
                ("Annual.PDF", "https://e.com/report.pdf"),
                ("notes.txt", "https://e.com/report.pdf"),
                ("backup.tar", "https://e.com/b.tar.gz"),
                ("  spaced  ", "https://e.com/r.zip"),
                ("   ", "https://e.com/r.zip"),
                ("as typed", "https://e.com/files/12345"),
                ("a/b", "https://e.com/r.zip"),
                ("CON", "https://e.com/r.zip"),
                ("x.vdmpart", "https://e.com/r.bin"),
                (".hidden", "https://e.com/r.zip"),
                (long_typed.as_str(), "https://e.com/r.zip"),
                (long_typed.as_str(), "https://e.com/files/12345"),
            ];
            let case = |url: &str, typed: Option<&str>| {
                let result = name_for_download(url, typed, |e| KNOWN.contains(&e));
                serde_json::json!({
                    "url": url,
                    "typed": typed,
                    "name": result.as_ref().ok().cloned().flatten(),
                    "error": result.err(),
                })
            };
            let cases: Vec<_> = urls
                .iter()
                .map(|url| case(url, None))
                .chain(typed.iter().map(|(t, url)| case(url, Some(t))))
                .collect();
            check(
                "naming-cases.json",
                serde_json::json!({ "known": KNOWN, "cases": cases }),
            );
        }

        #[test]
        fn contract_on_conflict() {
            check(
                "on-conflict.json",
                serde_json::to_value(crate::destination::OnConflict::ALL).unwrap(),
            );
        }

        #[test]
        fn contract_category() {
            check(
                "category.json",
                serde_json::to_value(crate::category::Category::sample()).unwrap(),
            );
        }

        #[test]
        fn contract_category_icons() {
            check(
                "category-icons.json",
                serde_json::to_value(crate::category::ICONS).unwrap(),
            );
        }

        #[test]
        fn contract_engine_health() {
            let all = [
                EngineHealth::Running,
                EngineHealth::Restarting { attempt: 2 },
                EngineHealth::Unavailable {
                    code: ErrorCode::EngineMissing,
                },
            ];
            check("engine-health.json", serde_json::to_value(all).unwrap());
        }

        #[test]
        fn contract_batch_result() {
            check(
                "batch-result.json",
                serde_json::to_value(BatchResult::sample()).unwrap(),
            );
        }

        #[test]
        fn contract_kept_reasons() {
            check(
                "kept-reasons.json",
                serde_json::to_value(KeptReason::ALL).unwrap(),
            );
        }

        #[test]
        fn contract_shell_settings() {
            check(
                "shell-settings.json",
                serde_json::to_value(crate::app_settings::ShellSettings::sample()).unwrap(),
            );
        }

        #[test]
        fn contract_themes_and_densities() {
            use crate::app_settings::{Density, Theme};
            check("themes.json", serde_json::to_value(Theme::ALL).unwrap());
            check(
                "densities.json",
                serde_json::to_value(Density::ALL).unwrap(),
            );
        }

        #[test]
        fn contract_download_login() {
            use crate::manager::SensitiveOptions;
            use crate::transfer::{Credentials, Header, SshKey};
            let login = SensitiveOptions {
                credentials: Some(Credentials {
                    user_name: "user".to_owned(),
                    password: "not-a-real-password".into(),
                }),
                ssh_key: Some(SshKey {
                    path: std::path::PathBuf::from("/home/user/.ssh/id_ed25519"),
                    passphrase: None,
                }),
                headers: vec![Header {
                    name: "X-Example".to_owned(),
                    value: "example".into(),
                }],
            };
            check("download-login.json", serde_json::to_value(login).unwrap());
        }

        #[test]
        fn contract_columns() {
            use crate::app_settings::{ColumnId, COLUMN_MAX_WIDTH, COLUMN_MIN_WIDTH};
            check(
                "columns.json",
                serde_json::json!({
                    "all": ColumnId::ALL,
                    "default": ColumnId::DEFAULT,
                    "minWidth": COLUMN_MIN_WIDTH,
                    "maxWidth": COLUMN_MAX_WIDTH,
                }),
            );
        }

        #[test]
        fn contract_app_info() {
            check(
                "app-info.json",
                serde_json::to_value(crate::app_info()).unwrap(),
            );
        }
    }
}
