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
//!   ([`Manager::resume_with`], PO decision on #58).
//! - **A record this build can't read** is listed by [`Manager::unavailable`], and never acted on
//!   (report section 6). Torrent records are too, until the manager runs torrents (#58, part 2).
//! - **Ids** continue after the highest one stored, so none is given twice.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::{Download, Manager};
use crate::database::{Database, DatabaseError, Engine, StoredDownload, StoredOptions};
use crate::domain::{
    DownloadRecord, DownloadStatus, ErrorCode, FailureClass, Progress, TransferError,
};
use crate::transfer::{Credentials, DirectTransfer, Header, Limits, SshKey, TransferSpec};

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
            let (Ok(Engine::Velocity), Some(kind), Some(&status), Some(options)) = (
                stored.route(),
                stored.kind.known(),
                stored.status.known(),
                stored.options.known(),
            ) else {
                m.unavailable.push(stored);
                continue;
            };
            let finished = status.is_terminal();
            let checkpoint = m.database.checkpoint(&stored.id)?;
            // A resume gives the name the file is saved as; one the server never named has
            // nothing on disk to resume.
            let resume = match (&stored.file_name, finished) {
                (Some(_), false) => {
                    stored
                        .resume(checkpoint.as_ref())
                        .map_err(|code| DatabaseError {
                            code,
                            detail: format!("download {} can't be resumed", stored.id),
                        })?
                }
                _ => None,
            };
            let needs_login = stored.has_sensitive_options && !finished;
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
                },
                request: request_of(&stored, options),
                resume,
                transfer: None,
                running: false,
                paused,
                scheduled,
                content_type: None,
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

    /// Records this build can't read, or whose engine the manager doesn't run yet: shown, never
    /// acted on.
    pub fn unavailable(&self) -> &[StoredDownload] {
        &self.unavailable
    }

    /// Whether the download came back after a restart without the login, headers or SSH key it
    /// had; it resumes only through [`Manager::resume_with`].
    pub fn needs_login(&self, id: &str) -> bool {
        self.downloads.get(id).is_some_and(|d| d.needs_login)
    }

    /// Resumes a download that came back without its login, with `options` given again.
    /// `InvalidState` for one that doesn't need them.
    pub fn resume_with(&mut self, id: &str, options: SensitiveOptions) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?;
        if !d.needs_login {
            return Err(ErrorCode::InvalidState);
        }
        d.request.credentials = options.credentials;
        d.request.ssh_key = options.ssh_key;
        d.request.headers = options.headers;
        d.needs_login = false;
        d.record.error = None;
        self.resume(id)
    }
}
