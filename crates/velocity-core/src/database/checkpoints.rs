//! Checkpoints of direct downloads in the database (#43).

use rusqlite::OptionalExtension;

use super::{to_sql_int, Database, DatabaseError, Stored};
use crate::domain::ErrorCode;
use crate::transfer::{Checkpoint, CHECKPOINT_FORMAT};

impl Database {
    /// Saves the download's checkpoint, replacing the one before.
    pub fn save_checkpoint(
        &self,
        download_id: &str,
        checkpoint: &Checkpoint,
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        let document = serde_json::to_string(checkpoint)
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        let conn = self.conn();
        let known: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM downloads WHERE id = ?1",
                [download_id],
                |r| r.get(0),
            )
            .optional()?;
        if known.is_none() {
            return Err(DatabaseError::new(
                ErrorCode::NotFound,
                format!("no download {download_id} to checkpoint"),
            ));
        }
        conn.execute(
            "INSERT INTO checkpoints (download_id, document, updated_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (download_id) DO UPDATE SET document = excluded.document,
                                                     updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![download_id, document, to_sql_int(now_ms)?],
        )?;
        Ok(())
    }

    /// The download's checkpoint. One this build can't read, or in another format, is
    /// [`Stored::Unknown`] and is never used to resume.
    pub fn checkpoint(
        &self,
        download_id: &str,
    ) -> Result<Option<Stored<Checkpoint>>, DatabaseError> {
        let document: Option<String> = self
            .conn()
            .query_row(
                "SELECT document FROM checkpoints WHERE download_id = ?1",
                [download_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(
            document.map(|raw| match serde_json::from_str::<Checkpoint>(&raw) {
                Ok(c) if c.format == CHECKPOINT_FORMAT => Stored::Known(c),
                _ => Stored::Unknown(raw),
            }),
        )
    }

    /// Forgets the download's checkpoint (it finished, or was cancelled).
    pub fn remove_checkpoint(&self, download_id: &str) -> Result<(), DatabaseError> {
        self.conn().execute(
            "DELETE FROM checkpoints WHERE download_id = ?1",
            [download_id],
        )?;
        Ok(())
    }
}
