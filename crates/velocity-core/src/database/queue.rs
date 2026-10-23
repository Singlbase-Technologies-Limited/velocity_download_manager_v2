//! The download queue in the database (#50): each waiting download's place, and whether it is
//! scheduled. Paused is the record's status. The manager saves it; loading it into the manager when
//! the app starts is #58 (PO decision on #50).

use super::{to_sql_int, Database, DatabaseError};
use crate::domain::ErrorCode;

/// One unfinished download's place in the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEntry {
    pub id: String,
    /// It runs only while the schedule's window is open.
    pub scheduled: bool,
}

impl Database {
    /// Saves the queue: `entries` in order, first to start first, in one transaction. It
    /// replaces the last one: a download that isn't in `entries` leaves the queue. `NotFound`, with
    /// nothing changed, if an entry names a download that doesn't exist.
    pub fn save_queue(&self, entries: &[QueueEntry]) -> Result<(), DatabaseError> {
        let mut conn = self.conn();
        // Dropped without a commit on an error: the last queue stays.
        let tx = conn.transaction()?;
        tx.execute("UPDATE downloads SET queue_position = NULL", [])?;
        for (position, entry) in entries.iter().enumerate() {
            let changed = tx.execute(
                "UPDATE downloads SET queue_position = ?1, scheduled = ?2 WHERE id = ?3",
                rusqlite::params![to_sql_int(position as u64)?, entry.scheduled, entry.id],
            )?;
            if changed != 1 {
                return Err(DatabaseError::new(
                    ErrorCode::NotFound,
                    format!("no download {} to queue", entry.id),
                ));
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The saved queue, first to start first.
    pub fn queue(&self) -> Result<Vec<QueueEntry>, DatabaseError> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, scheduled FROM downloads WHERE queue_position IS NOT NULL              ORDER BY queue_position, id",
        )?;
        let rows = statement.query_map([], |r| {
            Ok(QueueEntry {
                id: r.get(0)?,
                scheduled: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
