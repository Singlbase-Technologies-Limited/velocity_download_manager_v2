//! The history's commands (#32): what [`crate::history`] describes, on the manager's database.

use super::{Manager, MAX_BATCH};
use crate::domain::ErrorCode;
use crate::history::{HistoryPage, HistoryQuery, StorageSettings};

impl Manager {
    /// A page of the history, newest first. `InvalidInput` for a search longer than
    /// [`HistoryQuery::MAX_SEARCH_BYTES`].
    pub fn history(&self, query: &HistoryQuery) -> Result<HistoryPage, ErrorCode> {
        if query
            .search
            .as_ref()
            .is_some_and(|s| s.len() > HistoryQuery::MAX_SEARCH_BYTES)
        {
            return Err(ErrorCode::InvalidInput);
        }
        self.database.history(query).map_err(|e| e.code)
    }

    /// Clears the history; returns how many entries left it. Files and the list are untouched.
    pub fn clear_history(&mut self) -> Result<usize, ErrorCode> {
        self.database.clear_history().map_err(|e| e.code)
    }

    /// Takes these finished downloads out of the history. `InvalidInput` for more than
    /// [`MAX_BATCH`] ids.
    pub fn delete_history(&mut self, ids: &[String]) -> Result<(), ErrorCode> {
        if ids.len() > MAX_BATCH {
            return Err(ErrorCode::InvalidInput);
        }
        self.database
            .delete_history_entries(ids)
            .map_err(|e| e.code)
    }

    /// Saves the retention, as checked, and applies it at once (V1's).
    pub fn set_storage_settings(
        &mut self,
        settings: &StorageSettings,
        now_ms: u64,
    ) -> Result<StorageSettings, ErrorCode> {
        let saved = self
            .database
            .save_section(settings, now_ms)
            .map_err(|e| e.code)?;
        self.apply_retention(now_ms)?;
        Ok(saved)
    }

    /// Forgets the entries the saved retention no longer keeps; returns how many. The app calls
    /// it when it starts (V1's). A failure is also the snapshot's storage error, so it is shown.
    pub fn apply_retention(&mut self, now_ms: u64) -> Result<usize, ErrorCode> {
        let settings = self.settings_section::<StorageSettings>()?;
        self.database
            .prune_history(
                settings.history_retention_days,
                settings.max_history_entries,
                now_ms,
            )
            .map_err(|e| {
                let code = e.code;
                self.storage_error = Some(e);
                code
            })
    }
}
