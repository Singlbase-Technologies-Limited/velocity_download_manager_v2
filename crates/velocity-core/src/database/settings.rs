//! The settings document in the database. Its fields are defined and validated by #30; here it
//! is one JSON object.

use rusqlite::OptionalExtension;

use super::{to_sql_int, Database, DatabaseError};
use crate::domain::ErrorCode;

impl Database {
    /// The saved settings, or `None` before they were first saved.
    pub fn settings(&self) -> Result<Option<serde_json::Value>, DatabaseError> {
        let text: Option<String> = self
            .conn()
            .query_row("SELECT document FROM settings WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        text.map(|t| {
            serde_json::from_str(&t)
                .map_err(|e| DatabaseError::new(ErrorCode::Database, format!("the settings: {e}")))
        })
        .transpose()
    }

    /// Saves the settings document, which must be a JSON object.
    pub fn save_settings(
        &self,
        document: &serde_json::Value,
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        if !document.is_object() {
            return Err(DatabaseError::new(
                ErrorCode::InvalidInput,
                "the settings must be a JSON object",
            ));
        }
        self.conn().execute(
            "INSERT INTO settings (id, document, updated_at_ms) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET document = excluded.document,
                                            updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![document.to_string(), to_sql_int(now_ms)?],
        )?;
        Ok(())
    }
}
