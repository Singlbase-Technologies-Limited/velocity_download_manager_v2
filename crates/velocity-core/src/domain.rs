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
    /// Worth retrying soon: a reset connection, a timeout, a 5xx other than 503.
    Transient,
    /// The server asked to slow down: 429, 503, `Retry-After`.
    Throttled,
    /// Retrying won't help without a change: 404, 401, a changed object, a full disk.
    Permanent,
}

/// A transfer failure as the engine reports it, independent of the engine behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferError {
    pub class: FailureClass,
    pub http_status: Option<u16>,
    pub message: String,
    pub retryable: bool,
}

impl TransferError {
    /// Builds an error; only transient and throttled failures are retryable.
    pub fn new(class: FailureClass, http_status: Option<u16>, message: &str) -> Self {
        Self {
            class,
            http_status,
            message: message.to_owned(),
            retryable: class != FailureClass::Permanent,
        }
    }
}

/// Progress of one download. Sizes in bytes, speeds in bytes per second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Progress {
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
    /// The rest of the file is too small to split between more connections.
    FileSize,
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
    /// Milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

/// What an engine really supports, reported rather than assumed (research report section 4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub protocols: Vec<DownloadKind>,
    /// Connections can change during a transfer without restarting it.
    pub live_connection_changes: bool,
    /// Resume is checked against a strong validator (ETag or Last-Modified).
    pub validator_resume: bool,
    pub metalink: bool,
    pub max_connections_per_host: u32,
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
    Unsupported,
    Internal,
}

impl ErrorCode {
    /// Every code. V1's codes without the four aria2 JSON-RPC ones (`RPC_*`).
    pub const ALL: [Self; 22] = [
        Self::InvalidInput,
        Self::InvalidUrl,
        Self::UnsupportedScheme,
        Self::InvalidPath,
        Self::InvalidFilename,
        Self::DestinationMissing,
        Self::PermissionDenied,
        Self::FileExists,
        Self::NotFound,
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
            category_id: Some("archives".to_owned()),
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
            created_at_ms: 1_792_600_000_000,
            completed_at_ms: None,
        }
    }
}

impl Capabilities {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        Self {
            protocols: vec![DownloadKind::Http, DownloadKind::Ftp],
            live_connection_changes: true,
            validator_resume: true,
            metalink: false,
            max_connections_per_host: 8,
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
            r#"{"class":"throttled","httpStatus":503,"message":"Service unavailable","retryable":true}"#
        );
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
        let limits: Vec<_> = [
            ConnectionLimit::PerHostCap,
            ConnectionLimit::SharedBudget,
            ConnectionLimit::ServerThrottling,
            ConnectionLimit::FileSize,
        ]
        .iter()
        .map(|l| serde_json::to_value(l).unwrap())
        .collect();
        assert_eq!(
            limits,
            ["perHostCap", "sharedBudget", "serverThrottling", "fileSize"]
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
        fn contract_capabilities() {
            check(
                "capabilities.json",
                serde_json::to_value(Capabilities::sample()).unwrap(),
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
