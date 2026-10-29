//! Download records in the database.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{from_sql_int, to_sql_int, Database, DatabaseError};
use crate::domain::{
    DownloadKind, DownloadRecord, DownloadStatus, ErrorCode, TransferError, Verification,
};
use crate::transfer::{Checkpoint, Checksum, PieceHashes, Resume, TransferSpec, CHECKPOINT_FORMAT};

/// The engine a record is routed to. A record never changes engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// V2's direct-download engine.
    Velocity,
    Libtorrent,
}

impl Engine {
    fn as_stored(self) -> &'static str {
        match self {
            Engine::Velocity => "velocity",
            Engine::Libtorrent => "libtorrent",
        }
    }

    fn from_stored(raw: String) -> Stored<Engine> {
        match raw.as_str() {
            "velocity" => Stored::Known(Engine::Velocity),
            "libtorrent" => Stored::Known(Engine::Libtorrent),
            _ => Stored::Unknown(raw),
        }
    }
}

/// A value read back from the database: one this build knows, or the raw text of one it doesn't
/// (persistence rule, report section 6). An unknown value is shown, never acted on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored<T> {
    Known(T),
    Unknown(String),
}

impl<T> Stored<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Stored::Known(v) => Some(v),
            Stored::Unknown(_) => None,
        }
    }
}

/// The wire name of a domain value (`DownloadKind::Http` is `"http"`), as stored.
fn wire_name(value: &impl Serialize) -> Result<String, DatabaseError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        other => Err(DatabaseError::new(
            ErrorCode::Internal,
            format!("not a stored name: {other:?}"),
        )),
    }
}

/// A domain value from its stored name, or the name itself when this build doesn't know it.
fn from_wire_name<T: DeserializeOwned>(raw: String) -> Stored<T> {
    match serde_json::from_value(serde_json::Value::String(raw.clone())) {
        Ok(v) => Stored::Known(v),
        Err(_) => Stored::Unknown(raw),
    }
}

/// The options a download needs to be retried, or restarted after the app restarts (#58).
/// Nothing secret has a field here: credentials, header values and SSH keys stay in memory (#70).
/// The fields added with #58 default when absent, so records saved before them still read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredOptions {
    pub max_connections: u32,
    pub max_download_bps: Option<u64>,
    /// Every source, primary first, without logins; empty in records saved before #58, whose one
    /// source is the record's.
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub require_encryption: bool,
    #[serde(default)]
    pub user_agent: Option<String>,
    /// The referrer without its login, query and fragment, which can carry a session.
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub pieces: Option<PieceHashes>,
}

impl StoredOptions {
    /// The options of a direct download's spec, without its secrets.
    pub fn of(spec: &TransferSpec) -> Self {
        StoredOptions {
            max_connections: spec.max_connections,
            max_download_bps: spec.limits.max_download_bps,
            urls: spec.urls.iter().map(|url| without_login(url).0).collect(),
            require_encryption: spec.require_encryption,
            user_agent: spec.user_agent.clone(),
            referrer: spec.referrer.as_deref().map(Checkpoint::source_of),
            checksum: spec.checksum.clone(),
            pieces: spec.pieces.clone(),
        }
    }
}

/// A download to add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDownload {
    pub id: String,
    pub engine: Engine,
    pub kind: DownloadKind,
    pub status: DownloadStatus,
    /// Without a login; [`Database::insert_download`] removes one anyway.
    pub source_url: String,
    pub directory: String,
    pub file_name: Option<String>,
    pub category_id: Option<String>,
    pub options: StoredOptions,
    /// The download had credentials, custom headers or a login in its URL, all kept out of here.
    pub has_sensitive_options: bool,
    pub created_at_ms: u64,
}

impl NewDownload {
    /// The record for a direct download about to be handed to the engine: the first URL without
    /// its login, the folder and the name (none yet when the server will name it), and the options
    /// without secrets.
    pub fn from_transfer(
        id: &str,
        kind: DownloadKind,
        spec: &TransferSpec,
        created_at_ms: u64,
    ) -> Self {
        let (source_url, had_login) = without_login(spec.urls.first().map_or("", String::as_str));
        let directory = spec.directory.to_string_lossy().into_owned();
        let file_name = spec.file_name.clone();
        NewDownload {
            id: id.to_owned(),
            engine: Engine::Velocity,
            kind,
            status: DownloadStatus::Queued,
            source_url,
            directory,
            file_name,
            category_id: None,
            options: StoredOptions::of(spec),
            has_sensitive_options: had_login
                || spec.credentials.is_some()
                || spec.ssh_key.is_some()
                || !spec.headers.is_empty(),
            created_at_ms,
        }
    }
}

/// A download read back from the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDownload {
    pub id: String,
    pub engine: Stored<Engine>,
    pub kind: Stored<DownloadKind>,
    pub status: Stored<DownloadStatus>,
    pub source_url: String,
    pub directory: String,
    pub file_name: Option<String>,
    pub category_id: Option<String>,
    pub total_bytes: Option<u64>,
    pub completed_bytes: u64,
    pub options: Stored<StoredOptions>,
    pub has_sensitive_options: bool,
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub in_list: bool,
    pub in_history: bool,
    /// The last failure; shown, and never acted on when this build can't read it.
    pub error: Option<Stored<TransferError>>,
    /// What was proven about its bytes once it completed (#46); schema 4.
    pub verification: Option<Stored<Verification>>,
    /// A torrent's v1 and v2 info hashes, once known, and the bytes it uploaded.
    pub info_hash: Option<String>,
    pub info_hash_v2: Option<String>,
    pub uploaded_bytes: u64,
    /// When it started, and how long and how much it ran (#59); schema 7.
    pub activity: Activity,
}

/// When a download started and how long and how much it ran (#59): what its "Started" and
/// "Avg. speed" come from; schema 7.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Activity {
    /// The first time the engine reported it running, in milliseconds since the Unix epoch.
    pub started_at_ms: Option<u64>,
    /// How long it ran, in milliseconds.
    pub active_ms: u64,
    /// The bytes it received while it ran.
    pub transferred_bytes: u64,
}

impl StoredDownload {
    /// The engine to hand this record to. A record with any value this build doesn't know is
    /// never routed: it is `EngineUnavailable`, shown and kept, never acted on. A direct download
    /// needs options this build reads; a torrent's are in its own table (#58).
    pub fn route(&self) -> Result<Engine, ErrorCode> {
        match (self.engine.known(), self.kind.known(), self.status.known()) {
            (Some(Engine::Velocity), Some(_), Some(_)) if self.options.known().is_some() => {
                Ok(Engine::Velocity)
            }
            (Some(Engine::Libtorrent), Some(_), Some(_)) => Ok(Engine::Libtorrent),
            _ => Err(ErrorCode::EngineUnavailable),
        }
    }
}

impl StoredDownload {
    /// How to restart this unfinished download after a restart or a crash: from `checkpoint` if it
    /// is usable (known, in this build's format, and of this record's source); otherwise by
    /// discarding the part file, which this record's download created, and starting over (PO
    /// decision on #43). `None` for a finished download. A record this build can't route is never
    /// resumed.
    pub fn resume(
        &self,
        checkpoint: Option<&Stored<Checkpoint>>,
    ) -> Result<Option<Resume>, ErrorCode> {
        let engine = self.route()?;
        let finished = self.status.known().is_some_and(|s| s.is_terminal());
        // Torrents resume through libtorrent's own data (#62), not through these checkpoints.
        if finished || engine != Engine::Velocity {
            return Ok(None);
        }
        Ok(Some(resume_from(&self.source_url, checkpoint)))
    }
}

/// How a direct download from `source_url` restarts: from `checkpoint` if it is usable (known, in
/// this build's format, and of this source); otherwise by discarding the part file, which the
/// download created, and starting over (PO decision on #43).
pub fn resume_from(source_url: &str, checkpoint: Option<&Stored<Checkpoint>>) -> Resume {
    match checkpoint {
        Some(Stored::Known(c))
            if c.format == CHECKPOINT_FORMAT && c.source == Checkpoint::source_of(source_url) =>
        {
            Resume::Checkpoint {
                checkpoint: c.clone(),
            }
        }
        _ => Resume::DiscardPartFile {},
    }
}

/// `url` without a login (`scheme://user:password@host/…` becomes `scheme://host/…`), and whether
/// it had one ([`crate::request::split_login`]).
pub fn without_login(url: &str) -> (String, bool) {
    let (url, login) = crate::request::split_login(url);
    (url, login.is_some())
}

/// The columns a new record is written with: every schema's, so a database still at an older
/// schema takes it too (as its upgrade tests do).
const INSERT_COLUMNS: &str = "id, engine, kind, status, source_url, directory, file_name,     category_id, total_bytes, completed_bytes, options_json, has_sensitive_options, created_at_ms,     completed_at_ms, in_list, in_history";

/// The columns a record is read back with.
const COLUMNS: &str = "id, engine, kind, status, source_url, directory, file_name, category_id,     total_bytes, completed_bytes, options_json, has_sensitive_options, created_at_ms,     completed_at_ms, in_list, in_history, error_json, verification, info_hash, info_hash_v2,     uploaded_bytes, started_at_ms, active_ms, transferred_bytes";

impl Database {
    /// Adds a download. Its options are checked, and its URL loses any login.
    pub fn insert_download(&self, new: &NewDownload) -> Result<(), DatabaseError> {
        insert_row(&self.conn(), new)
    }

    /// Saves what changed in a download as the engine reported it: its status, name, byte counts,
    /// error, verification, completion time and `activity` (#59). `NotFound` for a download that
    /// isn't stored.
    pub fn update_download(
        &self,
        record: &DownloadRecord,
        activity: &Activity,
    ) -> Result<(), DatabaseError> {
        let error = record
            .error
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        let verification = record.verification.as_ref().map(wire_name).transpose()?;
        let changed = self.conn().execute(
            "UPDATE downloads SET status = ?2, file_name = ?3, total_bytes = ?4,              completed_bytes = ?5, error_json = ?6, verification = ?7, completed_at_ms = ?8,              category_id = ?9, started_at_ms = ?10, active_ms = ?11, transferred_bytes = ?12              WHERE id = ?1",
            rusqlite::params![
                record.id,
                wire_name(&record.status)?,
                record.file_name,
                record.progress.total_bytes.map(to_sql_int).transpose()?,
                to_sql_int(record.progress.completed_bytes)?,
                error,
                verification,
                record.completed_at_ms.map(to_sql_int).transpose()?,
                record.category_id,
                activity.started_at_ms.map(to_sql_int).transpose()?,
                to_sql_int(activity.active_ms)?,
                to_sql_int(activity.transferred_bytes)?,
            ],
        )?;
        if changed != 1 {
            return Err(DatabaseError::new(
                ErrorCode::NotFound,
                format!("no download {} to update", record.id),
            ));
        }
        Ok(())
    }

    /// Writes a status this build doesn't know, as a later V2 could have: for tests of what an
    /// unreadable record does.
    #[cfg(test)]
    pub(crate) fn set_raw_status_for_test(&self, id: &str, raw: &str) {
        self.conn()
            .execute("UPDATE downloads SET status = ?2 WHERE id = ?1", [id, raw])
            .unwrap();
    }

    /// Saves a download's options, after the user changed its limit or connections.
    /// `InvalidInput` for no connections; `NotFound` for a download that isn't stored.
    pub fn update_options(&self, id: &str, options: &StoredOptions) -> Result<(), DatabaseError> {
        if options.max_connections == 0 {
            return Err(DatabaseError::new(
                ErrorCode::InvalidInput,
                "a download needs at least one connection",
            ));
        }
        let json = serde_json::to_string(options)
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        let changed = self.conn().execute(
            "UPDATE downloads SET options_json = ?2 WHERE id = ?1",
            rusqlite::params![id, json],
        )?;
        if changed != 1 {
            return Err(DatabaseError::new(
                ErrorCode::NotFound,
                format!("no download {id} to update"),
            ));
        }
        Ok(())
    }

    /// Takes a download out of the list; it stays in the history (V1's `in_list`), and out of the
    /// queue. `NotFound` for a download that isn't stored.
    pub fn hide_download(&self, id: &str) -> Result<(), DatabaseError> {
        let changed = self.conn().execute(
            "UPDATE downloads SET in_list = 0, queue_position = NULL WHERE id = ?1",
            [id],
        )?;
        if changed != 1 {
            return Err(DatabaseError::new(
                ErrorCode::NotFound,
                format!("no download {id} to hide"),
            ));
        }
        Ok(())
    }

    /// Forgets a download, with its checkpoint. Nothing happens for an unknown id.
    pub fn delete_download(&self, id: &str) -> Result<(), DatabaseError> {
        self.conn()
            .execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Every download, oldest first.
    pub fn downloads(&self) -> Result<Vec<StoredDownload>, DatabaseError> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM downloads ORDER BY created_at_ms, id"
        ))?;
        let rows = statement.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                engine: r.get(1)?,
                kind: r.get(2)?,
                status: r.get(3)?,
                source_url: r.get(4)?,
                directory: r.get(5)?,
                file_name: r.get(6)?,
                category_id: r.get(7)?,
                total_bytes: r.get(8)?,
                completed_bytes: r.get(9)?,
                options: r.get(10)?,
                has_sensitive_options: r.get(11)?,
                created_at_ms: r.get(12)?,
                completed_at_ms: r.get(13)?,
                in_list: r.get(14)?,
                in_history: r.get(15)?,
                error: r.get(16)?,
                verification: r.get(17)?,
                info_hash: r.get(18)?,
                info_hash_v2: r.get(19)?,
                uploaded_bytes: r.get(20)?,
                started_at_ms: r.get(21)?,
                active_ms: r.get(22)?,
                transferred_bytes: r.get(23)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_stored()?);
        }
        Ok(out)
    }
}

/// Writes a new `downloads` row. A direct download's options are checked and stored; a torrent's
/// are its own ([`super::TorrentOptions`]), so its row has none.
pub(super) fn insert_row(
    conn: &rusqlite::Connection,
    new: &NewDownload,
) -> Result<(), DatabaseError> {
    let invalid = |what: &str| DatabaseError::new(ErrorCode::InvalidInput, what.to_owned());
    if new.id.is_empty() {
        return Err(invalid("a download needs an id"));
    }
    let (source_url, had_login) = without_login(&new.source_url);
    if source_url.is_empty() {
        return Err(invalid("a download needs a source"));
    }
    let options = match new.engine {
        Engine::Velocity => {
            if new.options.max_connections == 0 {
                return Err(invalid("a download needs at least one connection"));
            }
            serde_json::to_string(&new.options)
                .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?
        }
        Engine::Libtorrent => "{}".to_owned(),
    };
    conn.execute(
        &format!(
            "INSERT INTO downloads ({INSERT_COLUMNS}) VALUES \
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 0, ?9, ?10, ?11, NULL, 1, 1)"
        ),
        rusqlite::params![
            new.id,
            new.engine.as_stored(),
            wire_name(&new.kind)?,
            wire_name(&new.status)?,
            source_url,
            new.directory,
            new.file_name,
            new.category_id,
            options,
            new.has_sensitive_options || had_login,
            to_sql_int(new.created_at_ms)?,
        ],
    )?;
    Ok(())
}

/// One `downloads` row as SQLite gives it, before its values are checked.
struct Row {
    id: String,
    engine: String,
    kind: String,
    status: String,
    source_url: String,
    directory: String,
    file_name: Option<String>,
    category_id: Option<String>,
    total_bytes: Option<i64>,
    completed_bytes: i64,
    options: String,
    has_sensitive_options: bool,
    created_at_ms: i64,
    completed_at_ms: Option<i64>,
    in_list: bool,
    in_history: bool,
    error: Option<String>,
    verification: Option<String>,
    info_hash: Option<String>,
    info_hash_v2: Option<String>,
    uploaded_bytes: i64,
    started_at_ms: Option<i64>,
    active_ms: i64,
    transferred_bytes: i64,
}

impl Row {
    fn into_stored(self) -> Result<StoredDownload, DatabaseError> {
        Ok(StoredDownload {
            id: self.id,
            engine: Engine::from_stored(self.engine),
            kind: from_wire_name(self.kind),
            status: from_wire_name(self.status),
            source_url: self.source_url,
            directory: self.directory,
            file_name: self.file_name,
            category_id: self.category_id,
            total_bytes: self.total_bytes.map(from_sql_int).transpose()?,
            completed_bytes: from_sql_int(self.completed_bytes)?,
            options: match serde_json::from_str(&self.options) {
                Ok(o) => Stored::Known(o),
                Err(_) => Stored::Unknown(self.options),
            },
            has_sensitive_options: self.has_sensitive_options,
            created_at_ms: from_sql_int(self.created_at_ms)?,
            completed_at_ms: self.completed_at_ms.map(from_sql_int).transpose()?,
            in_list: self.in_list,
            in_history: self.in_history,
            error: self.error.map(|raw| match serde_json::from_str(&raw) {
                Ok(e) => Stored::Known(e),
                Err(_) => Stored::Unknown(raw),
            }),
            verification: self.verification.map(from_wire_name),
            info_hash: self.info_hash,
            info_hash_v2: self.info_hash_v2,
            uploaded_bytes: from_sql_int(self.uploaded_bytes)?,
            activity: Activity {
                started_at_ms: self.started_at_ms.map(from_sql_int).transpose()?,
                active_ms: from_sql_int(self.active_ms)?,
                transferred_bytes: from_sql_int(self.transferred_bytes)?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_login_removes_only_the_login() {
        for (url, expected, had) in [
            ("https://a:b@example.com/f", "https://example.com/f", true),
            (
                "ftp://anon@ftp.example.org/f",
                "ftp://ftp.example.org/f",
                true,
            ),
            // A password may contain an @; the login ends at the last @ before the path.
            ("http://u:p@ss@host:8080/x", "http://host:8080/x", true),
            (
                "https://example.com/path@not-a-login?q=a@b",
                "https://example.com/path@not-a-login?q=a@b",
                false,
            ),
            ("https://example.com", "https://example.com", false),
            ("not a url", "not a url", false),
        ] {
            assert_eq!(without_login(url), (expected.to_owned(), had), "{url}");
        }
    }
}
