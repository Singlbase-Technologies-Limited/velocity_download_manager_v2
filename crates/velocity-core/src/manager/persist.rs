//! Saving the manager's records (#58): what changed is written to the database once the command
//! or the events that changed it are done, so a restart brings back what the user last saw.
//!
//! A write that fails doesn't undo what happened in memory: the download really did pause or
//! complete. The failure is kept for the app to show ([`Manager::storage_error`]), and the next
//! change to the record writes it again.

use super::Manager;
use crate::database::DatabaseError;
use crate::domain::DownloadStatus;

/// How often a download's progress alone is saved. A status, error or name change is saved at
/// once; what a restart resumes from is the checkpoint, which the engine sends on its own (#43).
pub(super) const PROGRESS_SAVE_MS: u64 = 5_000;

impl Manager {
    /// Marks a record as changed since it was last saved.
    pub(super) fn mark(&mut self, id: &str) {
        self.unsaved.insert(id.to_owned());
    }

    /// Writes every changed record, and the queue when it changed. A download that completed or
    /// was removed loses its checkpoint, which nothing resumes from any more; a failed one keeps it
    /// for a retry. A record that is no longer known (it was refused before it began) is skipped.
    pub(super) fn flush(&mut self) {
        for id in std::mem::take(&mut self.unsaved) {
            let Some(download) = self.downloads.get(&id) else {
                continue;
            };
            let record = &download.record;
            let mut result = self
                .database
                .update_download(record, &download.activity.totals);
            if matches!(
                record.status,
                DownloadStatus::Completed | DownloadStatus::Removed
            ) {
                result = result.and_then(|()| self.database.remove_checkpoint(&id));
            }
            if let Err(e) = result {
                self.storage_error = Some(e);
            }
        }
        let queue = self.queue();
        if queue != self.saved_queue {
            match self.database.save_queue(&queue) {
                Ok(()) => self.saved_queue = queue,
                Err(e) => self.storage_error = Some(e),
            }
        }
    }

    /// The last failure to save a record, for the app to show; `None` while every save worked.
    pub fn storage_error(&self) -> Option<&DatabaseError> {
        self.storage_error.as_ref()
    }
}
