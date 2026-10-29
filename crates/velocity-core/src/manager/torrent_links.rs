//! `.torrent` links (PO decision on #60): a link to a `.torrent` file is fetched first, through
//! the direct-download engine, into a folder the app owns, and then added as a torrent from
//! there, so it gets the same results as a `.torrent` file the user chose.
//!
//! - **Not a download:** a fetch takes no queue slot and isn't in the list. The app waits for it,
//!   with its own time limit, and cancels it when it gives up ([`Manager::cancel_torrent_link`]).
//! - **Bounded:** one larger than [`MAX_TORRENT_BYTES`] is cancelled, and what arrives must be a
//!   torrent.
//! - **Kept without trackers:** the file is kept, as the torrent's origin, with its trackers taken
//!   out ([`strip_torrent_trackers`]); they stay in memory, as every torrent's do (PO decision on #58).
//! - **The folder is the app's:** when the app starts, any plain file in it that no download
//!   comes from is deleted: a fetch it quit during, or a torrent fetched and never added. Nothing
//!   else there is touched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use super::adding::{direct_kind, AddOutcome, AddRequest};
use super::Manager;
use crate::database::TorrentOrigin;
use crate::domain::{DownloadKind, DownloadStatus, ErrorCode};
use crate::links::is_link;
use crate::request::split_login;
use crate::torrent::{parse_torrent, strip_torrent_trackers, MAX_TORRENT_BYTES};
use crate::transfer::{Limits, TransferId, TransferSnapshot, TransferSpec};

/// The start of a fetch's file name, until it is known to be a torrent.
const FETCH_PREFIX: &str = "fetch-";

/// What a fetch answers: the kept `.torrent`, or why there is none.
pub type FetchReply = Sender<Result<PathBuf, AddOutcome>>;

/// The fetches under way, and what was fetched.
#[derive(Default)]
pub(super) struct TorrentLinks {
    /// Where fetched files go; `None` until [`Manager::with_torrent_links`].
    folder: Option<PathBuf>,
    fetches: HashMap<TransferId, Fetch>,
    /// Fetches cancelled, until the engine has ended them and can forget them.
    cancelled: HashSet<TransferId>,
    /// Numbers the fetches' files.
    next: u64,
    /// The trackers of each fetched `.torrent`, by the path it was kept at.
    trackers: HashMap<PathBuf, Vec<String>>,
}

struct Fetch {
    /// The file the engine writes, in the folder.
    name: String,
    reply: FetchReply,
}

impl Manager {
    /// Fetches `.torrent` links into `folder`, the app's own, created if missing. Clears what an
    /// earlier run left there, as the module says. `InvalidPath` when `folder` is a link or not a
    /// folder.
    pub fn with_torrent_links(mut self, folder: PathBuf) -> Result<Self, ErrorCode> {
        match std::fs::symlink_metadata(&folder) {
            Ok(meta) if is_link(&meta) || !meta.is_dir() => return Err(ErrorCode::InvalidPath),
            Ok(_) => {}
            Err(_) => std::fs::create_dir_all(&folder).map_err(|_| ErrorCode::Io)?,
        }
        let origins: Vec<PathBuf> = self
            .database
            .downloads()
            .map_err(|e| e.code)?
            .into_iter()
            .map(|d| PathBuf::from(d.source_url))
            .collect();
        let entries = std::fs::read_dir(&folder).map_err(|_| ErrorCode::Io)?;
        for entry in entries.flatten() {
            let path = entry.path();
            let plain = std::fs::symlink_metadata(&path)
                .is_ok_and(|meta| !is_link(&meta) && meta.is_file());
            if plain && !origins.contains(&path) {
                // A file that stays is retried at the next start.
                let _ = std::fs::remove_file(&path);
            }
        }
        self.links.folder = Some(folder);
        Ok(self)
    }

    /// Starts fetching the `.torrent` at `link`, with the request's login, headers and referrer;
    /// `reply` gets the kept file, or why there is none. Returns the fetch, to cancel it.
    /// `UnsupportedScheme` for a link that isn't `http` or `https`, as V1's were, `InvalidUrl`
    /// for one with a login in it, and `Unsupported` before [`Manager::with_torrent_links`].
    pub fn fetch_torrent_link(
        &mut self,
        link: &str,
        request: &AddRequest,
        reply: FetchReply,
    ) -> Result<TransferId, ErrorCode> {
        if direct_kind(link.trim())? != DownloadKind::Http {
            return Err(ErrorCode::UnsupportedScheme);
        }
        if split_login(link).1.is_some() {
            return Err(ErrorCode::InvalidUrl);
        }
        let folder = self.links.folder.clone().ok_or(ErrorCode::Unsupported)?;
        self.links.next += 1;
        let name = format!("{FETCH_PREFIX}{}.torrent", self.links.next);
        let spec = TransferSpec {
            urls: vec![link.trim().to_owned()],
            directory: folder,
            file_name: Some(name.clone()),
            credentials: request.credentials.clone(),
            require_encryption: false,
            ssh_key: None,
            headers: request.headers.clone(),
            referrer: request.referrer.clone(),
            user_agent: None,
            checksum: None,
            pieces: None,
            limits: Limits::default(),
            max_connections: 1,
            resume: None,
        };
        spec.check_request()?;
        let transfer = self.engine.add(spec)?;
        self.links.fetches.insert(transfer, Fetch { name, reply });
        Ok(transfer)
    }

    /// Stops a fetch the app no longer waits for.
    pub fn cancel_torrent_link(&mut self, fetch: TransferId) {
        if let Some(state) = self.links.fetches.remove(&fetch) {
            self.end_fetch(fetch, &state.name);
        }
    }

    /// Cancels a fetch and deletes its files. The engine forgets it once it has ended.
    fn end_fetch(&mut self, fetch: TransferId, name: &str) {
        // An engine that can't cancel it has lost it already.
        if self.engine.cancel(fetch).is_ok() {
            self.links.cancelled.insert(fetch);
        }
        self.drop_fetch_file(name);
    }

    /// Whether the engine's `snapshot` is a fetch's, which it then handles.
    pub(super) fn apply_fetch(&mut self, snapshot: &TransferSnapshot) -> bool {
        if self.links.cancelled.contains(&snapshot.id) {
            if snapshot.status.is_terminal() {
                self.links.cancelled.remove(&snapshot.id);
                // Ended: the engine may forget it; one that can't has already.
                let _ = self.engine.remove(snapshot.id);
            }
            return true;
        }
        if !self.links.fetches.contains_key(&snapshot.id) {
            return false;
        }
        let too_large = snapshot
            .progress
            .total_bytes
            .unwrap_or(0)
            .max(snapshot.progress.completed_bytes)
            > MAX_TORRENT_BYTES as u64;
        let answer = if too_large {
            Err(AddOutcome::Failed {
                code: ErrorCode::InvalidInput,
            })
        } else {
            match (snapshot.status, &snapshot.error) {
                (DownloadStatus::Completed, _) => {
                    let name = self.links.fetches[&snapshot.id].name.clone();
                    self.keep_fetched(&name)
                        .map_err(|code| AddOutcome::Failed { code })
                }
                (DownloadStatus::Failed, Some(error)) => Err(AddOutcome::NotFetched {
                    error: error.clone(),
                }),
                (DownloadStatus::Failed | DownloadStatus::Removed, _) => Err(AddOutcome::Failed {
                    code: ErrorCode::Io,
                }),
                _ => return true,
            }
        };
        let fetch = self.links.fetches.remove(&snapshot.id).expect("a fetch");
        if snapshot.status.is_terminal() {
            // Ended: the engine may forget it; one that can't has already.
            let _ = self.engine.remove(snapshot.id);
            self.drop_fetch_file(&fetch.name);
        } else {
            self.end_fetch(snapshot.id, &fetch.name);
        }
        // The app may have stopped waiting; then the kept file goes at the next start.
        let _ = fetch.reply.send(answer);
        true
    }

    /// Keeps the fetched file `name` as `<info hash>.torrent`, without its trackers, and remembers
    /// them. `InvalidInput` when it isn't a torrent.
    fn keep_fetched(&mut self, name: &str) -> Result<PathBuf, ErrorCode> {
        let folder = self.links.folder.clone().ok_or(ErrorCode::Unsupported)?;
        let path = folder.join(name);
        let meta = std::fs::symlink_metadata(&path).map_err(|_| ErrorCode::Io)?;
        if is_link(&meta) || !meta.is_file() {
            return Err(ErrorCode::InvalidPath);
        }
        if meta.len() > MAX_TORRENT_BYTES as u64 {
            return Err(ErrorCode::InvalidInput);
        }
        let bytes = std::fs::read(&path).map_err(|_| ErrorCode::Io)?;
        let torrent = parse_torrent(&bytes).map_err(|e| e.code())?;
        let (stripped, trackers) = strip_torrent_trackers(&bytes).map_err(|e| e.code())?;
        let kept = folder.join(format!("{}.torrent", torrent.key()));
        // Our own file, from an earlier fetch of the same torrent: the same bytes.
        if std::fs::symlink_metadata(&kept).is_ok_and(|m| is_link(&m) || !m.is_file()) {
            return Err(ErrorCode::InvalidPath);
        }
        std::fs::write(&kept, stripped).map_err(|_| ErrorCode::Io)?;
        self.links.trackers.insert(kept.clone(), trackers);
        Ok(kept)
    }

    /// Deletes a fetch's own file and its part file, if they are plain files.
    fn drop_fetch_file(&self, name: &str) {
        let Some(folder) = &self.links.folder else {
            return;
        };
        let part = format!("{name}{}", crate::naming::PART_FILE_SUFFIX);
        for file in [name, part.as_str()] {
            let path: &Path = &folder.join(file);
            if std::fs::symlink_metadata(path).is_ok_and(|m| !is_link(&m) && m.is_file()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// The trackers taken out of a fetched `.torrent`, for the torrent added from it.
    pub(super) fn fetched_trackers(&mut self, origin: &TorrentOrigin) -> Vec<String> {
        match origin {
            TorrentOrigin::File { path } => self.links.trackers.remove(path).unwrap_or_default(),
            TorrentOrigin::Magnet { .. } => Vec::new(),
        }
    }
}
