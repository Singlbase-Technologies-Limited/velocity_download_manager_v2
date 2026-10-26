//! What the user does with finished downloads (#58): retry them (V1's `retry_downloads` and
//! `retry_failed`).
//!
//! A retry is the user's, on top of the engine's own automatic retries (#48), never another
//! automatic one. A failed download resumes from its checkpoint, which it kept; a removed one
//! starts over, since cancelling discarded its part file. Either waits at the end of the queue.

use super::Manager;
use crate::database::resume_from;
use crate::domain::{DownloadStatus, ErrorCode};

impl Manager {
    /// Retries a failed or removed download: the engine forgets the old transfer, and the download
    /// waits as Queued at the end of the queue. `InvalidState` for any other status;
    /// `ConfirmationRequired` for one that came back after a restart without its login, until
    /// [`Manager::set_login`].
    pub fn retry(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        if !matches!(
            d.record.status,
            DownloadStatus::Failed | DownloadStatus::Removed
        ) {
            return Err(ErrorCode::InvalidState);
        }
        if d.needs_login {
            return Err(ErrorCode::ConfirmationRequired);
        }
        if let Some(transfer) = d.transfer {
            // An engine that restarted no longer knows it, which is what a retry needs anyway.
            match self.engine.remove(transfer) {
                Ok(()) | Err(ErrorCode::NotFound) => {}
                Err(code) => return Err(code),
            }
            self.by_transfer.remove(&transfer);
        }
        let checkpoint = self.database.checkpoint(id).map_err(|e| e.code)?;
        let d = self.downloads.get_mut(id).expect("known");
        let removed = d.record.status == DownloadStatus::Removed;
        d.resume = Some(resume_from(&d.record.source_url, checkpoint.as_ref()));
        d.transfer = None;
        d.paused = false;
        d.running = false;
        let record = &mut d.record;
        record.error = None;
        record.verification = None;
        record.completed_at_ms = None;
        if removed {
            record.progress.completed_bytes = 0;
        }
        record.progress.download_speed_bps = 0;
        record.progress.connections = 0;
        d.record.status = d.waiting_status();
        self.order.push(id.to_owned());
        self.mark(id);
        self.fill();
        Ok(())
    }

    /// Retries every failed download, in id order. Every one is tried; the first refusal is
    /// returned.
    pub fn retry_failed(&mut self) -> Result<(), ErrorCode> {
        let failed: Vec<String> = self
            .downloads
            .iter()
            .filter(|(_, d)| d.record.status == DownloadStatus::Failed)
            .map(|(id, _)| id.clone())
            .collect();
        let mut first_error = None;
        for id in failed {
            if let Err(code) = self.retry(&id) {
                first_error.get_or_insert(code);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
