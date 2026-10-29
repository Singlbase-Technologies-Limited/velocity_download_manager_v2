//! The history (#32, V1's `get_history`, `clear_history` and `delete_history_entries`): every
//! finished download, whether or not it is still in the list, with its retention.
//!
//! - **Never files:** clearing or deleting history entries forgets records only; files on disk are
//!   never touched.
//! - **The list wins:** an entry still in the download list only leaves the history; its record
//!   stays for the list. One no longer in the list is forgotten.
//! - **Retention** (V1's `storage` settings): entries older than the retention, and the oldest
//!   beyond the most kept, are forgotten when the app starts and when the settings change. An entry
//!   still in the list never is.

use serde::{Deserialize, Serialize};

use crate::database::settings::Section;
use crate::domain::{DownloadKind, DownloadStatus, ErrorCode};

/// How long the history keeps entries (V1's `storage` settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct StorageSettings {
    /// Days an entry is kept after it finished; 0 keeps entries forever.
    pub history_retention_days: u32,
    /// The most entries kept; the oldest go first.
    pub max_history_entries: u32,
}

impl StorageSettings {
    pub const MAX_RETENTION_DAYS: u32 = 3650;
    pub const MIN_ENTRIES: u32 = 100;
    pub const MAX_ENTRIES: u32 = 1_000_000;
}

impl Default for StorageSettings {
    /// V1's.
    fn default() -> Self {
        Self {
            history_retention_days: 90,
            max_history_entries: 10_000,
        }
    }
}

impl Section for StorageSettings {
    const KEY: &'static str = "storage";

    /// `InvalidInput` for a retention above [`StorageSettings::MAX_RETENTION_DAYS`] or a most
    /// outside [`StorageSettings::MIN_ENTRIES`] to [`StorageSettings::MAX_ENTRIES`] (V1's bounds).
    fn checked(&self) -> Result<Self, ErrorCode> {
        let ok = self.history_retention_days <= Self::MAX_RETENTION_DAYS
            && (Self::MIN_ENTRIES..=Self::MAX_ENTRIES).contains(&self.max_history_entries);
        if ok {
            Ok(*self)
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }
}

/// How a finished download ended, as the history filters it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryStatus {
    /// Completed, seeding included.
    Completed,
    Failed,
    /// Cancelled (`removed`).
    Cancelled,
}

/// What the history view asks for: a search, a status, and a page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct HistoryQuery {
    /// Matches the file name or the link, or a torrent's info hash exactly.
    pub search: Option<String>,
    pub status: Option<HistoryStatus>,
    /// At most [`HistoryQuery::MAX_LIMIT`]; [`HistoryQuery::DEFAULT_LIMIT`] when `None`.
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl HistoryQuery {
    pub const DEFAULT_LIMIT: u32 = 200;
    pub const MAX_LIMIT: u32 = 1000;
    /// The longest search, in bytes.
    pub const MAX_SEARCH_BYTES: usize = 1024;
}

/// One finished download in the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryEntry {
    pub id: String,
    /// `None` for a kind a later V2 stored, which this build doesn't know.
    pub kind: Option<DownloadKind>,
    pub file_name: Option<String>,
    /// The link, with credentials removed (#70).
    pub source_url: String,
    pub directory: String,
    pub category_id: Option<String>,
    pub status: DownloadStatus,
    pub total_bytes: Option<u64>,
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    /// Why it failed, when it did.
    pub error_message: Option<String>,
    /// It is still in the download list.
    pub in_list: bool,
}

/// A page of the history, newest first, and how many entries match in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub total: u64,
}

impl HistoryPage {
    /// A fixed example for the contract fixtures and UI tests (`history.json`).
    pub fn sample() -> Self {
        Self {
            entries: vec![
                HistoryEntry {
                    id: "d-0007".to_owned(),
                    kind: Some(DownloadKind::Http),
                    file_name: Some("archive.zip".to_owned()),
                    source_url: "https://example.com/files/archive.zip".to_owned(),
                    directory: "/home/user/Downloads".to_owned(),
                    category_id: Some("compressed".to_owned()),
                    status: DownloadStatus::Completed,
                    total_bytes: Some(4_194_304),
                    created_at_ms: 1_792_600_000_000,
                    completed_at_ms: Some(1_792_600_060_000),
                    error_message: None,
                    in_list: false,
                },
                HistoryEntry {
                    id: "d-0006".to_owned(),
                    kind: None,
                    file_name: None,
                    source_url: "https://example.com/files/missing.iso".to_owned(),
                    directory: "/home/user/Downloads".to_owned(),
                    category_id: None,
                    status: DownloadStatus::Failed,
                    total_bytes: None,
                    created_at_ms: 1_792_500_000_000,
                    completed_at_ms: None,
                    error_message: Some(
                        "The file was not found on the server (HTTP 404).".to_owned(),
                    ),
                    in_list: true,
                },
            ],
            total: 2,
        }
    }
}
