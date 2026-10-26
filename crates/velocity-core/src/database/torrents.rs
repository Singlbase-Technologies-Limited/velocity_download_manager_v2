//! Torrents in the database (#58): beside its `downloads` row, each torrent has a row in
//! `torrents` with how it is added again, its metadata and its resume data. Nothing secret is
//! stored: a magnet link without its trackers, and resume data without its trackers, whose
//! addresses can carry a private tracker's passkey (PO decision on #58).

use std::path::PathBuf;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::downloads::insert_row;
use super::{to_sql_int, Database, DatabaseError, NewDownload, Stored};
use crate::domain::ErrorCode;
use crate::seeding::SeedPolicy;

/// Where a torrent came from, to add it again when nothing better is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TorrentOrigin {
    /// The `.torrent` file the user chose.
    File { path: PathBuf },
    /// The magnet link, without its trackers.
    Magnet { uri: String },
}

/// How a torrent is added again: the parts of its spec that aren't secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentOptions {
    pub origin: TorrentOrigin,
    /// It waits in Choosing files until the user chooses.
    pub choose_files: bool,
    /// The files chosen, by index; `None` for every file.
    pub files: Option<Vec<u32>>,
    pub seeding: SeedPolicy,
    /// Bytes per second; 0 for no limit.
    pub upload_limit: u64,
    pub download_limit: u64,
}

/// A torrent read back.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredTorrent {
    /// [`Stored::Unknown`] when this build can't read them: the torrent is never added again.
    pub options: Stored<TorrentOptions>,
    /// The torrent's metadata (`d4:info…e`), checked before it was stored.
    pub metadata: Option<Vec<u8>>,
    /// libtorrent's resume data, without its trackers.
    pub resume_data: Option<Vec<u8>>,
}

fn json(options: &TorrentOptions) -> Result<String, DatabaseError> {
    serde_json::to_string(options)
        .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))
}

fn not_found(id: &str) -> DatabaseError {
    DatabaseError::new(ErrorCode::NotFound, format!("no torrent {id}"))
}

impl Database {
    /// Adds a torrent: its `downloads` row and its `torrents` row, in one transaction.
    pub fn insert_torrent(
        &self,
        new: &NewDownload,
        options: &TorrentOptions,
    ) -> Result<(), DatabaseError> {
        let options = json(options)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        insert_row(&tx, new)?;
        tx.execute(
            "INSERT INTO torrents (download_id, options_json, updated_at_ms) VALUES (?1, ?2, ?3)",
            rusqlite::params![new.id, options, to_sql_int(new.created_at_ms)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The torrent kept for the download `id`, if it is one.
    pub fn torrent(&self, id: &str) -> Result<Option<StoredTorrent>, DatabaseError> {
        type Row = (String, Option<Vec<u8>>, Option<Vec<u8>>);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT options_json, metadata, resume_data FROM torrents WHERE download_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(raw, metadata, resume_data)| StoredTorrent {
            options: match serde_json::from_str(&raw) {
                Ok(o) => Stored::Known(o),
                Err(_) => Stored::Unknown(raw),
            },
            metadata,
            resume_data,
        }))
    }

    /// Saves the torrent's metadata, checked by the caller.
    pub fn save_torrent_metadata(
        &self,
        id: &str,
        metadata: &[u8],
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        self.set_torrent("metadata", id, metadata, now_ms)
    }

    /// Saves the torrent's resume data, without its trackers, replacing the last.
    pub fn save_torrent_resume(
        &self,
        id: &str,
        resume_data: &[u8],
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        self.set_torrent("resume_data", id, resume_data, now_ms)
    }

    fn set_torrent(
        &self,
        column: &'static str,
        id: &str,
        bytes: &[u8],
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        let changed = self.conn().execute(
            &format!(
                "UPDATE torrents SET {column} = ?2, updated_at_ms = ?3 WHERE download_id = ?1"
            ),
            rusqlite::params![id, bytes, to_sql_int(now_ms)?],
        )?;
        if changed != 1 {
            return Err(not_found(id));
        }
        Ok(())
    }

    /// Saves how the torrent is added again, after the user changed its files, policy or limits.
    pub fn update_torrent_options(
        &self,
        id: &str,
        options: &TorrentOptions,
    ) -> Result<(), DatabaseError> {
        let changed = self.conn().execute(
            "UPDATE torrents SET options_json = ?2 WHERE download_id = ?1",
            rusqlite::params![id, json(options)?],
        )?;
        if changed != 1 {
            return Err(not_found(id));
        }
        Ok(())
    }

    /// Saves a torrent's info hashes, once its metadata is known, and the bytes it uploaded.
    pub fn update_torrent_identity(
        &self,
        id: &str,
        info_hash: Option<&str>,
        info_hash_v2: Option<&str>,
        uploaded_bytes: u64,
    ) -> Result<(), DatabaseError> {
        let changed = self.conn().execute(
            "UPDATE downloads SET info_hash = ?2, info_hash_v2 = ?3, uploaded_bytes = ?4 \
             WHERE id = ?1",
            rusqlite::params![id, info_hash, info_hash_v2, to_sql_int(uploaded_bytes)?],
        )?;
        if changed != 1 {
            return Err(not_found(id));
        }
        Ok(())
    }
}
