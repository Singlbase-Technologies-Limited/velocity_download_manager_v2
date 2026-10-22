//! Download records in the database.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{from_sql_int, to_sql_int, Database, DatabaseError};
use crate::domain::{DownloadKind, DownloadStatus, ErrorCode};
use crate::transfer::TransferSpec;

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

/// The options a download needs to be retried. Nothing secret has a field here: credentials and
/// header values stay in memory (#70).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredOptions {
    pub max_connections: u32,
    pub max_download_bps: Option<u64>,
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
    /// its login, the destination split into folder and name, and the options without secrets.
    pub fn from_transfer(
        id: &str,
        kind: DownloadKind,
        spec: &TransferSpec,
        created_at_ms: u64,
    ) -> Self {
        let (source_url, had_login) = without_login(spec.urls.first().map_or("", String::as_str));
        let directory = spec
            .destination
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_name = spec
            .destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        NewDownload {
            id: id.to_owned(),
            engine: Engine::Velocity,
            kind,
            status: DownloadStatus::Queued,
            source_url,
            directory,
            file_name,
            category_id: None,
            options: StoredOptions {
                max_connections: spec.max_connections,
                max_download_bps: spec.limits.max_download_bps,
            },
            has_sensitive_options: had_login
                || spec.credentials.is_some()
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
}

impl StoredDownload {
    /// The engine to hand this record to. A record with any value this build doesn't know is
    /// never routed: it is `EngineUnavailable`, shown and kept, never acted on.
    pub fn route(&self) -> Result<Engine, ErrorCode> {
        match (
            self.engine.known(),
            self.kind.known(),
            self.status.known(),
            self.options.known(),
        ) {
            (Some(engine), Some(_), Some(_), Some(_)) => Ok(*engine),
            _ => Err(ErrorCode::EngineUnavailable),
        }
    }
}

/// `url` without a login (`scheme://user:password@host/…` becomes `scheme://host/…`), and whether
/// it had one.
pub fn without_login(url: &str) -> (String, bool) {
    let Some(at) = url.find("://") else {
        return (url.to_owned(), false);
    };
    let (scheme, rest) = url.split_at(at + 3);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..end].rfind('@') {
        Some(login) => (format!("{scheme}{}", &rest[login + 1..]), true),
        None => (url.to_owned(), false),
    }
}

const COLUMNS: &str = "id, engine, kind, status, source_url, directory, file_name, category_id, \
    total_bytes, completed_bytes, options_json, has_sensitive_options, created_at_ms, \
    completed_at_ms, in_list, in_history";

impl Database {
    /// Adds a download. Its options are checked, and its URL loses any login.
    pub fn insert_download(&self, new: &NewDownload) -> Result<(), DatabaseError> {
        let invalid = |what: &str| DatabaseError::new(ErrorCode::InvalidInput, what.to_owned());
        if new.id.is_empty() {
            return Err(invalid("a download needs an id"));
        }
        let (source_url, had_login) = without_login(&new.source_url);
        if source_url.is_empty() {
            return Err(invalid("a download needs a source"));
        }
        if new.options.max_connections == 0 {
            return Err(invalid("a download needs at least one connection"));
        }
        let options = serde_json::to_string(&new.options)
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        self.conn().execute(
            &format!(
                "INSERT INTO downloads ({COLUMNS}) VALUES \
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
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_stored()?);
        }
        Ok(out)
    }
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
