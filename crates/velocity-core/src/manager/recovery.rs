//! Bringing the downloads back when the app starts (#58): the records, the queue and the
//! checkpoints the database kept.
//!
//! - **Finished downloads** come back as they ended, with their progress, error and verification.
//! - **Unfinished ones** wait in their saved order, a paused one still paused, and resume from
//!   their checkpoint when it fits, or start over by discarding their part file
//!   ([`StoredDownload::resume`], PO decision on #43). Nothing starts in [`Manager::open`]: the app
//!   applies its queue settings first, and the next [`Manager::pump`] starts what may run.
//! - **Logins aren't kept** (#70): an unfinished download that had a login, custom headers or an
//!   SSH key comes back paused with a note, and resumes only once they are given again
//!   ([`Manager::set_login`], PO decision on #58). A finished one that had them is retried only
//!   once they are given again too.
//! - **A record this build can't read** is listed by [`Manager::unavailable`], and never acted on
//!   (report section 6).
//! - **Torrents** come back as the torrent module says ([`super::torrents`]).
//! - **Downloads removed from the list** stay in the history only (#32), and aren't loaded.
//! - **Ids** continue after the highest one stored, hidden ones included, so none is given twice.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::torrents::TorrentJob;
use super::{DirectJob, Download, Job, Manager};
use crate::database::{
    resume_from, Database, DatabaseError, Engine, Stored, StoredDownload, StoredOptions,
    StoredTorrent,
};
use crate::domain::{
    DownloadRecord, DownloadStatus, ErrorCode, FailureClass, Progress, TransferError,
};
use crate::transfer::{
    Credentials, DirectTransfer, Header, Limits, SshKey, TransferId, TransferSpec,
};

/// The note on a download that came back without the login it had.
const LOGIN_NOTE: &str = "Velocity doesn't keep logins, custom headers or SSH keys after it \
                          closes. Resume this download to enter its login again.";

/// What a download keeps in memory only (#70), given again by the user when a download that had
/// it comes back after a restart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SensitiveOptions {
    pub credentials: Option<Credentials>,
    pub ssh_key: Option<SshKey>,
    pub headers: Vec<Header>,
}

/// The number in an id the manager gave (`d-0012` is 12).
fn id_number(id: &str) -> Option<u64> {
    id.strip_prefix("d-")?.parse().ok()
}

/// What the engine is given to start a stored download again: its sources and options, without
/// the secrets that were never stored.
fn request_of(stored: &StoredDownload, options: &StoredOptions) -> TransferSpec {
    let urls = if options.urls.is_empty() {
        vec![stored.source_url.clone()]
    } else {
        options.urls.clone()
    };
    TransferSpec {
        urls,
        directory: PathBuf::from(&stored.directory),
        file_name: stored.file_name.clone(),
        credentials: None,
        require_encryption: options.require_encryption,
        ssh_key: None,
        headers: Vec::new(),
        referrer: options.referrer.clone(),
        user_agent: options.user_agent.clone(),
        checksum: options.checksum.clone(),
        pieces: options.pieces.clone(),
        limits: Limits {
            max_download_bps: options.max_download_bps,
        },
        max_connections: options.max_connections,
        resume: None,
    }
}

impl Manager {
    /// The manager over `database`, with the downloads it kept. Nothing starts until the next
    /// [`Manager::pump`] or settings change.
    pub fn open(
        engine: Box<dyn DirectTransfer>,
        database: Arc<Database>,
    ) -> Result<Self, DatabaseError> {
        let mut m = Manager::new(engine, database);
        let queue = m.database.queue()?;
        let places: HashMap<&str, usize> = queue
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.as_str(), i))
            .collect();
        let mut unfinished = Vec::new();
        for stored in m.database.downloads()? {
            if let Some(n) = id_number(&stored.id) {
                m.next_id = m.next_id.max(n);
            }
            // Removed from the list: history only (#32).
            if !stored.in_list {
                continue;
            }
            let (Ok(engine), Some(kind), Some(&status)) =
                (stored.route(), stored.kind.known(), stored.status.known())
            else {
                m.unavailable.push(stored);
                continue;
            };
            let finished = status.is_terminal();
            let job = match engine {
                Engine::Velocity => {
                    let Some(options) = stored.options.known() else {
                        m.unavailable.push(stored);
                        continue;
                    };
                    let resume = if finished {
                        None
                    } else {
                        let checkpoint = m.database.checkpoint(&stored.id)?;
                        Some(resume_from(&stored.source_url, checkpoint.as_ref()))
                    };
                    Job::Direct(Box::new(DirectJob::new(
                        request_of(&stored, options),
                        resume,
                    )))
                }
                // A torrent comes back from its resume data, or where it came from (#58).
                Engine::Libtorrent => match m.database.torrent(&stored.id)? {
                    Some(StoredTorrent {
                        options: Stored::Known(options),
                        ..
                    }) => Job::Torrent(Box::new(TorrentJob::new(options, None))),
                    _ => {
                        m.unavailable.push(stored);
                        continue;
                    }
                },
            };
            let needs_login = stored.has_sensitive_options;
            let paused = !finished && (status == DownloadStatus::Paused || needs_login);
            let scheduled = queue
                .iter()
                .find(|e| e.id == stored.id)
                .is_some_and(|e| e.scheduled);
            let error = match &stored.error {
                Some(crate::database::Stored::Known(e)) => Some(e.clone()),
                _ => None,
            };
            let mut download = Download {
                record: DownloadRecord {
                    id: stored.id.clone(),
                    kind: *kind,
                    source_url: stored.source_url.clone(),
                    file_name: stored.file_name.clone(),
                    directory: stored.directory.clone(),
                    category_id: stored.category_id.clone(),
                    status,
                    progress: Progress {
                        completed_bytes: stored.completed_bytes,
                        total_bytes: stored.total_bytes,
                        download_speed_bps: 0,
                        upload_speed_bps: 0,
                        connections: 0,
                        connection_detail: None,
                    },
                    error,
                    verification: stored
                        .verification
                        .as_ref()
                        .and_then(|v| v.known().copied()),
                    created_at_ms: stored.created_at_ms,
                    completed_at_ms: stored.completed_at_ms,
                    retry_at_ms: None,
                },
                job,
                running: false,
                paused,
                scheduled,
                saved_at_ms: None,
                needs_login,
            };
            if !finished {
                download.record.status = download.waiting_status();
                if needs_login {
                    download.record.error = Some(TransferError::new(
                        FailureClass::Permanent,
                        None,
                        LOGIN_NOTE,
                    ));
                }
                let place = places
                    .get(stored.id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                unfinished.push((place, stored.created_at_ms, stored.id.clone()));
                m.unsaved.insert(stored.id.clone());
            }
            m.downloads.insert(stored.id.clone(), download);
        }
        // The saved order; one missing from it (saved before schema 3) goes last, oldest first.
        unfinished.sort();
        m.order = unfinished.into_iter().map(|(_, _, id)| id).collect();
        m.saved_queue = queue;
        Ok(m)
    }

    /// A download whose engine was lost under it (#39): it waits again, with the reason shown, to
    /// resume from its checkpoint once an engine runs (PO decision on #58). A paused one stays
    /// paused.
    pub(super) fn lose(&mut self, id: &str, transfer: TransferId, error: TransferError) {
        self.by_transfer.remove(&transfer);
        // The supervisor keeps a lost transfer as failed until it is removed. Failing to remove it
        // only leaves that finished entry in its memory; the download no longer refers to it.
        let _ = self.engine.remove(transfer);
        let checkpoint = self.database.checkpoint(id).unwrap_or_else(|e| {
            self.storage_error = Some(e);
            None
        });
        let Some(d) = self.downloads.get_mut(id) else {
            return;
        };
        let resume = resume_from(&d.record.source_url, checkpoint.as_ref());
        if let Some(job) = d.direct_mut() {
            job.transfer = None;
            job.resume = Some(resume);
        }
        d.running = false;
        d.record.progress.download_speed_bps = 0;
        d.record.progress.connections = 0;
        d.record.error = Some(error);
        d.record.status = d.waiting_status();
        // The engine that would have tried again is gone (#59).
        d.record.retry_at_ms = None;
        self.mark(id);
    }

    /// Records this build can't read, or whose engine the manager doesn't run yet: shown, never
    /// acted on.
    pub fn unavailable(&self) -> &[StoredDownload] {
        &self.unavailable
    }

    /// Whether the download came back after a restart without the login, headers or SSH key it
    /// had; it resumes or is retried only after [`Manager::set_login`].
    pub fn needs_login(&self, id: &str) -> bool {
        self.downloads.get(id).is_some_and(|d| d.needs_login)
    }

    /// Gives a download that came back without its login, headers or SSH key those again; the
    /// caller then resumes or retries it. `InvalidState` for one that doesn't need them.
    pub fn set_login(&mut self, id: &str, options: SensitiveOptions) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?;
        if !d.needs_login {
            return Err(ErrorCode::InvalidState);
        }
        let request = &mut d.direct_mut().ok_or(ErrorCode::InvalidState)?.request;
        request.credentials = options.credentials;
        request.ssh_key = options.ssh_key;
        request.headers = options.headers;
        d.needs_login = false;
        if d.record
            .error
            .as_ref()
            .is_some_and(|e| e.message == LOGIN_NOTE)
        {
            d.record.error = None;
            self.unsaved.insert(id.to_owned());
        }
        Ok(())
    }
}
