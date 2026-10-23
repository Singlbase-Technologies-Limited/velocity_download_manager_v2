//! The app database (#26, docs/DATA_AND_MIGRATIONS.md): download records, settings and
//! categories, in SQLite.
//!
//! - **Migrations:** numbered SQL files in `migrations/`, applied in one transaction and recorded
//!   in `PRAGMA user_version`. Released migrations are never edited.
//! - **Before upgrading** an existing database, a copy is written next to it (`VACUUM INTO`).
//! - **A newer database** (from a later V2) is refused without being modified.
//! - **A failed upgrade** keeps the original and the copy; user data is never deleted or recreated.
//! - **Unknown values** read back (an engine, kind or status) are [`Stored::Unknown`], and such a
//!   record is never routed to an engine.
//! - **No secrets:** nothing that holds credentials or header values can be stored.

mod checkpoints;
mod downloads;
mod settings;

pub use downloads::{without_login, Engine, NewDownload, Stored, StoredDownload, StoredOptions};

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use crate::domain::ErrorCode;

const MIGRATIONS: &[M<'static>] = &[
    M::up(include_str!("../../migrations/0001_initial.sql")),
    M::up(include_str!("../../migrations/0002_checkpoints.sql")),
];

/// The migrations this build applies.
pub fn migrations() -> Migrations<'static> {
    Migrations::from_slice(MIGRATIONS)
}

/// The schema version this build writes.
pub fn latest_schema_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// A database failure: the code for the UI, and the detail for diagnostics (#28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseError {
    pub code: ErrorCode,
    pub detail: String,
}

impl DatabaseError {
    fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        DatabaseError {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.code.user_message(), self.detail)
    }
}

impl std::error::Error for DatabaseError {}

impl From<rusqlite::Error> for DatabaseError {
    fn from(e: rusqlite::Error) -> Self {
        DatabaseError::new(ErrorCode::Database, e.to_string())
    }
}

/// The open app database.
pub struct Database {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl Database {
    /// Opens the database at `path`, creating it owner-only if it doesn't exist, and upgrades it
    /// to this build's schema.
    pub fn open(path: &Path) -> Result<Self, DatabaseError> {
        open_with(path, &migrations())
    }

    /// A database in memory, at the latest schema, for tests and tools.
    pub fn open_in_memory() -> Result<Self, DatabaseError> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrations()
            .to_latest(&mut conn)
            .map_err(|e| DatabaseError::new(ErrorCode::DatabaseMigration, e.to_string()))?;
        Ok(Database {
            conn: Mutex::new(conn),
            path: None,
        })
    }

    /// The file, or `None` in memory.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn schema_version(&self) -> Result<u32, DatabaseError> {
        Ok(user_version(&self.conn())?)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while the lock was held can't leave a half-written transaction behind: SQLite
        // rolls it back.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A count or time as SQLite stores it (a signed 64-bit integer).
fn to_sql_int(v: u64) -> Result<i64, DatabaseError> {
    i64::try_from(v).map_err(|_| {
        DatabaseError::new(
            ErrorCode::InvalidInput,
            format!("{v} is too large to store"),
        )
    })
}

/// A stored count or time, which is never negative.
fn from_sql_int(v: i64) -> Result<u64, DatabaseError> {
    u64::try_from(v).map_err(|_| {
        DatabaseError::new(
            ErrorCode::Database,
            format!("a negative count was stored: {v}"),
        )
    })
}

fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// The schema version in the file's header (bytes 60 to 63, big-endian), read without opening the
/// database, so that nothing can be written to it. An empty file is version 0.
fn header_version(path: &Path) -> Result<u32, DatabaseError> {
    let bytes = std::fs::read(path).map_err(|e| {
        DatabaseError::new(
            ErrorCode::Database,
            format!("reading {}: {e}", path.display()),
        )
    })?;
    if bytes.is_empty() {
        return Ok(0);
    }
    if bytes.len() < 100 || &bytes[..16] != b"SQLite format 3\0" {
        return Err(DatabaseError::new(
            ErrorCode::Database,
            format!("{} isn't a SQLite database", path.display()),
        ));
    }
    Ok(u32::from_be_bytes([
        bytes[60], bytes[61], bytes[62], bytes[63],
    ]))
}

fn newer(found: u32, known: usize) -> DatabaseError {
    DatabaseError::new(
        ErrorCode::DatabaseMigration,
        format!(
            "the database is at schema {found}, newer than this build's {known}; it was not modified"
        ),
    )
}

/// Creates the database file owner-only, so that SQLite's WAL and shared-memory files, which take
/// its mode, are owner-only too.
fn create_owner_only(path: &Path) -> Result<(), DatabaseError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path).map(drop).map_err(|e| {
        DatabaseError::new(
            ErrorCode::Database,
            format!("creating {}: {e}", path.display()),
        )
    })
}

fn owner_only(path: &Path) -> Result<(), DatabaseError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            DatabaseError::new(
                ErrorCode::Database,
                format!("protecting {}: {e}", path.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn configure(conn: &Connection) -> Result<(), DatabaseError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    // Defensive mode refuses SQL that could corrupt the file (writable_schema and the like); both
    // SQLite CVEs fixed in 3.53.2 needed it off.
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    // WAL: atomic, crash-safe commits.
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

/// [`Database::open`] with the given migrations (tests add one to prove the upgrade path).
fn open_with(path: &Path, migrations: &Migrations<'_>) -> Result<Database, DatabaseError> {
    let existed = match std::fs::symlink_metadata(path) {
        // Never follow a link to somewhere else (persistence rule).
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => {
            return Err(DatabaseError::new(
                ErrorCode::InvalidPath,
                format!("{} is a link or not a file", path.display()),
            ))
        }
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            return Err(DatabaseError::new(
                ErrorCode::Database,
                format!("{}: {e}", path.display()),
            ))
        }
    };
    let known = latest_of(migrations);
    if existed {
        let found = header_version(path)?;
        if found as usize > known {
            return Err(newer(found, known));
        }
    } else {
        create_owner_only(path)?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    // Again after opening: a WAL not yet merged into the file may hold a newer version.
    let current = user_version(&conn)?;
    if current as usize > known {
        return Err(newer(current, known));
    }
    if existed && current > 0 && (current as usize) < known {
        backup(&conn, path, current)?;
    }
    migrations.to_latest(&mut conn).map_err(|e| {
        DatabaseError::new(
            ErrorCode::DatabaseMigration,
            format!(
                "upgrading from schema {current} failed; the database and its backup were kept: {e}"
            ),
        )
    })?;
    Ok(Database {
        conn: Mutex::new(conn),
        path: Some(path.to_path_buf()),
    })
}

/// The schema version `migrations` lead to.
fn latest_of(migrations: &Migrations<'_>) -> usize {
    // An in-memory database at version 0 has every migration pending.
    Connection::open_in_memory()
        .ok()
        .and_then(|c| migrations.pending_migrations(&c).ok())
        .map_or(0, |n| n.max(0) as usize)
}

/// Copies the database next to itself before an upgrade: `<file>.bak-v<version>-<ms>`.
fn backup(conn: &Connection, path: &Path, version: u32) -> Result<(), DatabaseError> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".bak-v{version}-{ms}"));
    let target = PathBuf::from(name);
    conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
        .map_err(|e| {
            DatabaseError::new(
                ErrorCode::DatabaseMigration,
                format!("backing up before the upgrade failed; nothing was changed: {e}"),
            )
        })?;
    owner_only(&target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DownloadKind, DownloadStatus};
    use crate::transfer::{
        Checkpoint, CredentialHandle, Header, Limits, Resume, TransferSpec, CHECKPOINT_FORMAT,
    };

    /// A fresh directory for one test.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("v2-db-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }

        fn db(&self) -> PathBuf {
            self.0.join("velocity.sqlite3")
        }

        /// The file names in the directory, sorted.
        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// This build's migrations plus a test-only next one, to prove the upgrade path.
    fn with_next(sql: &'static str) -> Migrations<'static> {
        let mut all = MIGRATIONS.to_vec();
        all.push(M::up(sql));
        Migrations::new(all)
    }

    const NEXT: &str = "ALTER TABLE downloads ADD COLUMN note TEXT;";

    fn sample(id: &str) -> NewDownload {
        NewDownload {
            id: id.to_owned(),
            engine: Engine::Velocity,
            kind: DownloadKind::Http,
            status: DownloadStatus::Queued,
            source_url: "https://example.com/files/a.zip".to_owned(),
            directory: "/home/user/Downloads".to_owned(),
            file_name: Some("a.zip".to_owned()),
            category_id: None,
            options: StoredOptions {
                max_connections: 4,
                max_download_bps: None,
            },
            has_sensitive_options: false,
            created_at_ms: 1_792_600_000_000,
        }
    }

    #[test]
    fn the_migrations_are_valid() {
        assert_eq!(migrations().validate(), Ok(()));
        assert_eq!(latest_schema_version(), 2);
    }

    #[test]
    fn a_new_database_is_created_at_the_latest_schema() {
        let dir = Dir::new("new");
        let db = Database::open(&dir.db()).expect("opens");
        assert_eq!(db.schema_version(), Ok(latest_schema_version()));
        assert_eq!(db.path(), Some(dir.db().as_path()));
        drop(db);
        assert!(
            dir.names().iter().all(|n| !n.contains(".bak")),
            "no backup for a new database: {:?}",
            dir.names()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.db()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "owner-only");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_database_is_refused() {
        let dir = Dir::new("link");
        let target = dir.0.join("elsewhere.sqlite3");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, dir.db()).unwrap();
        let e = Database::open(&dir.db()).err().expect("refused");
        assert_eq!(e.code, ErrorCode::InvalidPath, "{e}");
    }

    #[test]
    fn an_upgrade_backs_up_first_and_keeps_the_data() {
        let dir = Dir::new("upgrade");
        {
            let db = open_with(&dir.db(), &Migrations::from_slice(MIGRATIONS)).unwrap();
            db.insert_download(&sample("d-0001")).unwrap();
        }
        let latest = latest_schema_version();
        let db = open_with(&dir.db(), &with_next(NEXT)).expect("upgrades");
        assert_eq!(db.schema_version(), Ok(latest + 1));
        assert_eq!(db.downloads().unwrap().len(), 1, "the record survived");
        // The backup is a complete database at the old version with the record in it.
        let backup = dir
            .names()
            .into_iter()
            .find(|n| n.starts_with(&format!("velocity.sqlite3.bak-v{latest}-")))
            .expect("a backup before the upgrade");
        let old = Connection::open(dir.0.join(&backup)).unwrap();
        let version: u32 = old
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let rows: u32 = old
            .query_row("SELECT count(*) FROM downloads", [], |r| r.get(0))
            .unwrap();
        assert_eq!((version, rows), (latest, 1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.0.join(&backup))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the backup is owner-only too");
        }
    }

    #[test]
    fn a_newer_database_is_refused_unmodified() {
        let dir = Dir::new("newer");
        drop(open_with(&dir.db(), &with_next(NEXT)).unwrap());
        let before = std::fs::read(dir.db()).unwrap();
        let names = dir.names();
        let e = Database::open(&dir.db()).err().expect("refused");
        assert_eq!(e.code, ErrorCode::DatabaseMigration, "{e}");
        assert!(
            std::fs::read(dir.db()).unwrap() == before,
            "the file is unchanged"
        );
        assert_eq!(dir.names(), names, "no file was added: no backup, no WAL");
    }

    #[test]
    fn a_failed_upgrade_keeps_the_data_and_the_backup() {
        let dir = Dir::new("failed");
        {
            let db = Database::open(&dir.db()).unwrap();
            db.insert_download(&sample("d-0001")).unwrap();
        }
        let broken = with_next("ALTER TABLE downloads ADD COLUMN note TEXT; THIS IS NOT SQL;");
        let e = open_with(&dir.db(), &broken)
            .err()
            .expect("the upgrade fails");
        assert_eq!(e.code, ErrorCode::DatabaseMigration, "{e}");
        // The original is still at its version with its record, and the backup is there.
        let latest = latest_schema_version();
        let db = Database::open(&dir.db()).expect("the original still opens");
        assert_eq!(db.schema_version(), Ok(latest));
        assert_eq!(db.downloads().unwrap().len(), 1);
        assert!(dir
            .names()
            .iter()
            .any(|n| n.contains(&format!(".bak-v{latest}-"))));
    }

    #[test]
    fn schema_1_is_upgraded_with_its_downloads_and_gains_checkpoints() {
        // Migration 0002 (#43), from the previous schema with a record in it.
        let dir = Dir::new("v1-to-v2");
        {
            let db = open_with(&dir.db(), &Migrations::from_slice(&MIGRATIONS[..1])).unwrap();
            assert_eq!(db.schema_version(), Ok(1));
            db.insert_download(&sample("d-0001")).unwrap();
        }
        let db = Database::open(&dir.db()).expect("upgrades");
        assert_eq!(db.schema_version(), Ok(2));
        assert_eq!(db.downloads().unwrap()[0].id, "d-0001");
        db.save_checkpoint("d-0001", &checkpoint(), 5).unwrap();
        assert!(dir.names().iter().any(|n| n.contains(".bak-v1-")));
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: "https://example.com/files/a.zip".to_owned(),
            total: 10_000,
            etag: None,
            last_modified: None,
            durable: vec![(0, 4000)],
        }
    }

    #[test]
    fn a_checkpoint_is_saved_replaced_and_removed() {
        let db = Database::open_in_memory().unwrap();
        db.insert_download(&sample("d-0001")).unwrap();
        assert_eq!(db.checkpoint("d-0001"), Ok(None));
        let mut cp = checkpoint();
        db.save_checkpoint("d-0001", &cp, 1).unwrap();
        assert_eq!(db.checkpoint("d-0001"), Ok(Some(Stored::Known(cp.clone()))));
        cp.durable = vec![(0, 9000)];
        db.save_checkpoint("d-0001", &cp, 2).unwrap();
        assert_eq!(db.checkpoint("d-0001"), Ok(Some(Stored::Known(cp))));
        db.remove_checkpoint("d-0001").unwrap();
        assert_eq!(db.checkpoint("d-0001"), Ok(None));
        // A checkpoint belongs to a download; there is none for an unknown one.
        let e = db
            .save_checkpoint("d-9999", &checkpoint(), 3)
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::NotFound, "{e}");
    }

    #[test]
    fn a_checkpoint_in_another_format_is_never_used() {
        let db = Database::open_in_memory().unwrap();
        db.insert_download(&sample("d-0001")).unwrap();
        for raw in [
            r#"{"format":99,"source":"x","total":1,"etag":null,"lastModified":null,"durable":[]}"#,
            r#"{"format":1,"source":"x","total":1,"etag":null,"lastModified":null,"durable":[],"spy":1}"#,
        ] {
            db.conn()
                .execute(
                    "INSERT OR REPLACE INTO checkpoints (download_id, document, updated_at_ms)                      VALUES ('d-0001', ?1, 1)",
                    [raw],
                )
                .unwrap();
            assert_eq!(
                db.checkpoint("d-0001"),
                Ok(Some(Stored::Unknown(raw.to_owned()))),
                "{raw}"
            );
        }
    }

    #[test]
    fn recovery_resumes_from_a_usable_checkpoint_and_otherwise_starts_over() {
        let db = Database::open_in_memory().unwrap();
        db.insert_download(&sample("d-0001")).unwrap();
        let record = db.downloads().unwrap().remove(0);
        let cp = checkpoint();
        // A usable checkpoint of this record's source.
        assert_eq!(
            record.resume(Some(&Stored::Known(cp.clone()))),
            Ok(Some(Resume::Checkpoint {
                checkpoint: cp.clone()
            }))
        );
        // No checkpoint, an unreadable one, another format, or another source: start over.
        let other_source = Checkpoint {
            source: "https://elsewhere.example/a.zip".to_owned(),
            ..cp.clone()
        };
        let other_format = Checkpoint {
            format: CHECKPOINT_FORMAT + 1,
            ..cp.clone()
        };
        for checkpoint in [
            None,
            Some(Stored::Unknown("{}".to_owned())),
            Some(Stored::Known(other_source)),
            Some(Stored::Known(other_format)),
        ] {
            assert_eq!(
                record.resume(checkpoint.as_ref()),
                Ok(Some(Resume::DiscardPartFile {})),
                "{checkpoint:?}"
            );
        }
        // A finished download has nothing to resume.
        let mut done = record.clone();
        done.status = Stored::Known(DownloadStatus::Completed);
        assert_eq!(done.resume(Some(&Stored::Known(cp.clone()))), Ok(None));
        // A record this build can't route is never resumed.
        let mut unknown = record;
        unknown.engine = Stored::Unknown("aria2".to_owned());
        assert_eq!(
            unknown.resume(Some(&Stored::Known(cp))),
            Err(ErrorCode::EngineUnavailable)
        );
    }

    #[test]
    fn a_download_round_trips() {
        let db = Database::open_in_memory().unwrap();
        let new = sample("d-0001");
        db.insert_download(&new).unwrap();
        let stored = db.downloads().unwrap();
        assert_eq!(stored.len(), 1);
        let s = &stored[0];
        assert_eq!(s.id, "d-0001");
        assert_eq!(s.engine, Stored::Known(Engine::Velocity));
        assert_eq!(s.kind, Stored::Known(DownloadKind::Http));
        assert_eq!(s.status, Stored::Known(DownloadStatus::Queued));
        assert_eq!(s.source_url, new.source_url);
        assert_eq!(s.options, Stored::Known(new.options.clone()));
        assert_eq!(s.route(), Ok(Engine::Velocity));
    }

    #[test]
    fn unknown_stored_values_are_unavailable_and_never_routed() {
        let db = Database::open_in_memory().unwrap();
        db.insert_download(&sample("d-0001")).unwrap();
        db.conn()
            .execute_batch(
                "UPDATE downloads SET engine = 'aria2', kind = 'gopher', status = 'teleporting';",
            )
            .unwrap();
        let s = &db.downloads().unwrap()[0];
        assert_eq!(s.engine, Stored::Unknown("aria2".to_owned()));
        assert_eq!(s.kind, Stored::Unknown("gopher".to_owned()));
        assert_eq!(s.status, Stored::Unknown("teleporting".to_owned()));
        assert_eq!(s.route(), Err(ErrorCode::EngineUnavailable));
        // A known engine with an unknown status isn't routed either.
        db.conn()
            .execute_batch("UPDATE downloads SET engine = 'velocity', kind = 'http';")
            .unwrap();
        assert_eq!(
            db.downloads().unwrap()[0].route(),
            Err(ErrorCode::EngineUnavailable)
        );
    }

    #[test]
    fn invalid_options_are_refused() {
        let db = Database::open_in_memory().unwrap();
        let mut new = sample("d-0001");
        new.options.max_connections = 0;
        assert_eq!(
            db.insert_download(&new).err().map(|e| e.code),
            Some(ErrorCode::InvalidInput),
            "options are checked"
        );
    }

    #[test]
    fn no_secret_reaches_the_database_files() {
        let dir = Dir::new("secrets");
        let spec = TransferSpec {
            urls: vec!["https://alice:SECRET-PASS-5678@example.com/files/a.zip?x=1".to_owned()],
            destination: PathBuf::from("/home/user/Downloads/a.zip"),
            credentials: Some(CredentialHandle(7)),
            headers: vec![Header {
                name: "Cookie".to_owned(),
                value: "session=SECRET-COOKIE-1234".to_owned(),
            }],
            checksum: None,
            limits: Limits::default(),
            max_connections: 4,
            resume: None,
        };
        let new =
            NewDownload::from_transfer("d-0001", DownloadKind::Http, &spec, 1_792_600_000_000);
        assert!(new.has_sensitive_options);
        assert_eq!(new.source_url, "https://example.com/files/a.zip?x=1");
        let db = Database::open(&dir.db()).unwrap();
        db.insert_download(&new).unwrap();
        // Search every file SQLite wrote, while the WAL is still there and after it is merged.
        let scan = |when: &str| {
            for name in dir.names() {
                let bytes = std::fs::read(dir.0.join(&name)).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                for secret in ["SECRET-PASS-5678", "SECRET-COOKIE-1234", "alice"] {
                    assert!(!text.contains(secret), "{secret} in {name} ({when})");
                }
            }
        };
        scan("with the WAL");
        drop(db);
        scan("after closing");
    }

    #[test]
    fn the_settings_are_one_json_object() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.settings(), Ok(None));
        let doc = serde_json::json!({ "maxActiveDownloads": 3 });
        db.save_settings(&doc, 1).unwrap();
        assert_eq!(db.settings(), Ok(Some(doc)));
        let e = db
            .save_settings(&serde_json::json!([1, 2]), 2)
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::InvalidInput);
    }
}
