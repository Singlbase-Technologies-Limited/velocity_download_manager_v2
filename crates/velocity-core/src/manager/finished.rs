//! What the user does with finished downloads (#58): retry them (V1's `retry_downloads` and
//! `retry_failed`), remove them (`remove_downloads`) and clear the completed ones
//! (`clear_completed`).
//!
//! - **Retrying** is the user's, on top of the engine's own automatic retries (#48), never another
//!   automatic one. A failed download resumes from its checkpoint, which it kept; a removed one
//!   starts over, since cancelling discarded its part file. Either waits at the end of the queue.
//! - **Removing** takes a download out of the list; it stays in the history (#32) and doesn't come
//!   back after a restart. An unfinished one is cancelled first, which discards its part file.
//! - **With its files** (PO decision on #58), only the download's own go: its part file, and a
//!   completed download's file if it is a plain file inside its folder, not reached through a link,
//!   and no larger than the download. Anything else is kept and reported with the reason
//!   ([`crate::torrent_files::delete_owned`], the rules torrents use).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Download, Manager};
use crate::database::resume_from;
use crate::domain::{DownloadStatus, ErrorCode};
use crate::naming::{check_file_name, PART_FILE_SUFFIX};
use crate::torrent_files::{delete_owned, Kept, OwnedFile, OwnedPaths};

/// How long a file that can't be deleted yet is tried again: on Windows, a file is deleted only
/// once nothing has it open.
pub(super) const DELETE_WAIT: Duration = Duration::from_secs(2);

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
        self.forget_transfer(id, true)?;
        let checkpoint = self.database.checkpoint(id).map_err(|e| e.code)?;
        let d = self.downloads.get_mut(id).expect("known");
        let removed = d.record.status == DownloadStatus::Removed;
        let resume = resume_from(&d.record.source_url, checkpoint.as_ref());
        if let Some(job) = d.direct_mut() {
            job.resume = Some(resume);
        }
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
        let mut first_error = None;
        for id in self.ids_with(DownloadStatus::Failed) {
            if let Err(code) = self.retry(&id) {
                first_error.get_or_insert(code);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Removes a download from the list, cancelling it first if it is unfinished, and with
    /// `delete_files` its own files. Returns the files that were kept, with why. When the engine
    /// refuses to cancel or forget it, nothing changes and that is the error.
    pub fn remove(
        &mut self,
        id: &str,
        delete_files: bool,
    ) -> Result<Vec<(PathBuf, Kept)>, ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        let finished = d.record.status.is_terminal();
        let engine_had_it = d.transfer().is_some();
        self.forget_transfer(id, finished)?;
        let mut d = self.take(id);
        if !finished {
            d.record.status = DownloadStatus::Removed;
        }
        // A login the user asked to remember goes with the download (PO decision on #70).
        if d.direct().is_some_and(|job| job.login_saved) {
            if let Err(code) = self.keychain.delete(id) {
                tracing::warn!("the saved login of {id} couldn't be deleted: {code:?}");
            }
        }
        let saved = self
            .database
            .update_download(&d.record, &d.activity.totals)
            .and_then(|()| self.database.remove_checkpoint(id))
            .and_then(|()| self.database.hide_download(id));
        if let Err(e) = saved {
            self.storage_error = Some(e);
        }
        // The engine discards the part file of a transfer it cancels; the manager deletes that of
        // an unfinished download the engine never had, as cancelling it would have.
        let kept = if matches!(d.job, super::Job::Torrent(_)) {
            if delete_files {
                self.delete_torrent_files(id, &d)
            } else {
                Vec::new()
            }
        } else {
            let part = (delete_files && finished) || (!finished && !engine_had_it);
            let file = delete_files && d.record.status == DownloadStatus::Completed;
            delete_own_files(&d, part, file)
        };
        self.fill();
        Ok(kept)
    }

    /// Removes every completed download from the list, keeping its file; returns how many. The
    /// first engine refusal to forget one is returned, after the rest are cleared.
    pub fn clear_completed(&mut self) -> Result<usize, ErrorCode> {
        let mut first_error = None;
        let mut cleared = 0;
        for id in self.ids_with(DownloadStatus::Completed) {
            match self.remove(&id, false) {
                Ok(_) => cleared += 1,
                Err(code) => {
                    first_error.get_or_insert(code);
                }
            }
        }
        first_error.map_or(Ok(cleared), Err)
    }

    /// The downloads with `status`, in id order.
    fn ids_with(&self, status: DownloadStatus) -> Vec<String> {
        self.downloads
            .iter()
            .filter(|(_, d)| d.record.status == status)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Has the engine forget the download's transfer: `remove` for a finished one, `cancel` for
    /// one still running. One the engine no longer knows is forgotten already.
    fn forget_transfer(&mut self, id: &str, finished: bool) -> Result<(), ErrorCode> {
        if self
            .downloads
            .get(id)
            .is_some_and(|d| d.torrent().is_some())
        {
            return self.forget_torrent(id);
        }
        let Some(transfer) = self.downloads.get(id).and_then(|d| d.transfer()) else {
            return Ok(());
        };
        let result = if finished {
            self.engine.remove(transfer)
        } else {
            self.engine.cancel(transfer)
        };
        match result {
            Ok(()) | Err(ErrorCode::NotFound) => {}
            Err(code) => return Err(code),
        }
        self.by_transfer.remove(&transfer);
        if let Some(job) = self.downloads.get_mut(id).expect("known").direct_mut() {
            job.transfer = None;
        }
        Ok(())
    }

    /// Takes a download out of the manager.
    fn take(&mut self, id: &str) -> Download {
        self.order.retain(|o| o != id);
        self.unsaved.remove(id);
        self.downloads.remove(id).expect("known")
    }
}

/// Deletes the download's part file (`part`) and its finished file (`file`), as its own: a name
/// that isn't a plain file name in its folder deletes nothing.
fn delete_own_files(d: &Download, part: bool, file: bool) -> Vec<(PathBuf, Kept)> {
    let Some(name) = d.record.file_name.as_deref() else {
        return Vec::new();
    };
    if check_file_name(name).is_err() || !(part || file) {
        return Vec::new();
    }
    let root = Path::new(&d.record.directory).to_path_buf();
    let progress = &d.record.progress;
    let owned = OwnedPaths {
        files: if file {
            vec![OwnedFile {
                path: root.join(name),
                max_len: progress.total_bytes.unwrap_or(progress.completed_bytes),
            }]
        } else {
            Vec::new()
        },
        parts: if part {
            vec![root.join(format!("{name}{PART_FILE_SUFFIX}"))]
        } else {
            Vec::new()
        },
        folders: Vec::new(),
        root,
    };
    delete_owned(&owned, DELETE_WAIT).kept
}
