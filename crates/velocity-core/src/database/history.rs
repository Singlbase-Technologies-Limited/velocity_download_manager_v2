//! The history's queries (#32, V1's `history`, `clear_history`, `delete_history_entries` and
//! `prune_history`). A row still in the list only leaves the history; one out of it is deleted,
//! with its checkpoint and torrent row (`ON DELETE CASCADE`). Files are never touched.

use rusqlite::params;

use super::downloads::from_wire_name;
use super::{from_sql_int, to_sql_int, Database, DatabaseError, Stored};
use crate::domain::{DownloadKind, DownloadStatus, TransferError};
use crate::history::{HistoryEntry, HistoryPage, HistoryQuery, HistoryStatus};

/// The finished statuses the history holds; a seeding torrent has finished downloading.
const FINISHED: &str = "status IN ('completed', 'seeding', 'failed', 'removed')";
/// When an entry finished, for ordering and retention.
const FINISHED_AT: &str = "COALESCE(completed_at_ms, created_at_ms)";

impl Database {
    /// A page of the history, newest first.
    pub fn history(&self, query: &HistoryQuery) -> Result<HistoryPage, DatabaseError> {
        let limit = query
            .limit
            .unwrap_or(HistoryQuery::DEFAULT_LIMIT)
            .min(HistoryQuery::MAX_LIMIT);
        let offset = query.offset.unwrap_or(0);
        let raw = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // `%`, `_` and the escape itself are searched for as themselves.
        let like = raw.map(|s| {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
        let status = query.status.map(|s| match s {
            HistoryStatus::Completed => "completed",
            HistoryStatus::Failed => "failed",
            HistoryStatus::Cancelled => "removed",
        });
        let filter = format!(
            "in_history = 1 AND {FINISHED}
             AND (?1 IS NULL OR file_name LIKE ?1 ESCAPE '\\' OR source_url LIKE ?1 ESCAPE '\\'
                  OR info_hash = lower(?2) OR info_hash_v2 = lower(?2))
             AND (?3 IS NULL OR status = ?3 OR (?3 = 'completed' AND status = 'seeding'))"
        );
        let conn = self.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT count(*) FROM downloads WHERE {filter}"),
            params![like, raw, status],
            |r| r.get(0),
        )?;
        let mut statement = conn.prepare(&format!(
            "SELECT id, kind, file_name, source_url, directory, category_id, status, total_bytes,
                    created_at_ms, completed_at_ms, error_json, in_list
             FROM downloads WHERE {filter}
             ORDER BY {FINISHED_AT} DESC, id DESC LIMIT ?4 OFFSET ?5"
        ))?;
        let rows = statement.query_map(params![like, raw, status, limit, offset], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, Option<i64>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, bool>(11)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (
                id,
                kind,
                file_name,
                source_url,
                directory,
                category_id,
                status,
                total,
                created,
                completed,
                error,
                in_list,
            ) = row?;
            // Only the finished statuses are selected, so the status is always one this build knows.
            let status = match from_wire_name::<DownloadStatus>(status) {
                Stored::Known(status) => status,
                Stored::Unknown(_) => DownloadStatus::Unknown,
            };
            entries.push(HistoryEntry {
                id,
                // A kind a later V2 wrote is shown as unknown, never as another (persistence rules).
                kind: from_wire_name::<DownloadKind>(kind).known().copied(),
                file_name,
                source_url,
                directory,
                category_id,
                status,
                total_bytes: total.map(from_sql_int).transpose()?,
                created_at_ms: from_sql_int(created)?,
                completed_at_ms: completed.map(from_sql_int).transpose()?,
                error_message: error
                    .and_then(|e| serde_json::from_str::<TransferError>(&e).ok())
                    .map(|e| e.message),
                in_list,
            });
        }
        Ok(HistoryPage {
            entries,
            total: from_sql_int(total)?,
        })
    }

    /// Clears the history: entries out of the list are deleted, the others only leave the
    /// history. Returns how many entries left it.
    pub fn clear_history(&self) -> Result<usize, DatabaseError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let deleted = tx.execute(
            &format!("DELETE FROM downloads WHERE in_list = 0 AND in_history = 1 AND {FINISHED}"),
            [],
        )?;
        let hidden = tx.execute(
            &format!("UPDATE downloads SET in_history = 0 WHERE in_history = 1 AND {FINISHED}"),
            [],
        )?;
        tx.commit()?;
        Ok(deleted + hidden)
    }

    /// Takes these finished downloads out of the history, as [`Database::clear_history`] does.
    /// An unknown id, or one that hasn't finished, is left alone.
    pub fn delete_history_entries(&self, ids: &[String]) -> Result<(), DatabaseError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute(
                &format!("DELETE FROM downloads WHERE id = ?1 AND in_list = 0 AND {FINISHED}"),
                [id],
            )?;
            tx.execute(
                &format!("UPDATE downloads SET in_history = 0 WHERE id = ?1 AND {FINISHED}"),
                [id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Deletes the entries out of the list that finished more than `days` before `now_ms` (none
    /// for 0 days), then the oldest beyond `max_entries`. Returns how many.
    pub fn prune_history(
        &self,
        days: u32,
        max_entries: u32,
        now_ms: u64,
    ) -> Result<usize, DatabaseError> {
        const DAY_MS: u64 = 86_400_000;
        let prunable = format!("in_list = 0 AND in_history = 1 AND {FINISHED}");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut deleted = 0;
        if days > 0 {
            let cutoff = to_sql_int(now_ms.saturating_sub(u64::from(days) * DAY_MS))?;
            deleted += tx.execute(
                &format!("DELETE FROM downloads WHERE {prunable} AND {FINISHED_AT} < ?1"),
                [cutoff],
            )?;
        }
        deleted += tx.execute(
            &format!(
                "DELETE FROM downloads WHERE id IN (
                    SELECT id FROM downloads WHERE {prunable}
                    ORDER BY {FINISHED_AT} DESC, id DESC LIMIT -1 OFFSET ?1)"
            ),
            [max_entries],
        )?;
        tx.commit()?;
        Ok(deleted)
    }
}
