//! Saving the manager's records (#58): what changed is written to the database once the command
//! or the events that changed it are done, so a restart brings back what the user last saw.
//!
//! A write that fails doesn't undo what happened in memory: the download really did pause or
//! complete. The failure is kept for the app to show ([`Manager::storage_error`]), and the next
//! change to the record writes it again.

use super::Manager;
use crate::database::DatabaseError;

impl Manager {
    /// Marks a record as changed since it was last saved.
    pub(super) fn mark(&mut self, id: &str) {
        self.unsaved.insert(id.to_owned());
    }

    /// Writes every changed record. A record that is no longer known (it was refused before it
    /// began) is skipped.
    pub(super) fn flush(&mut self) {
        for id in std::mem::take(&mut self.unsaved) {
            let Some(download) = self.downloads.get(&id) else {
                continue;
            };
            if let Err(e) = self.database.update_download(&download.record) {
                self.storage_error = Some(e);
            }
        }
    }

    /// The last failure to save a record, for the app to show; `None` while every save worked.
    pub fn storage_error(&self) -> Option<&DatabaseError> {
        self.storage_error.as_ref()
    }
}
