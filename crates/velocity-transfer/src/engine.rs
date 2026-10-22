//! The prototype engine behind the `DirectTransfer` contract (#36, #37), in the app's process.
//! Each transfer runs [`download_with`] on its own worker thread, sharing one libcurl loop; the
//! engine turns what happens into contract events. `stdio.rs` serves the same engine from a child
//! process.
//!
//! What the prototype can't do is refused, not faked: pause and resume need checkpoints (#43),
//! limits need #49, and live connection changes need #47, so those commands return `Unsupported`
//! and `capabilities()` says so.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Instant;

use velocity_core::domain::{
    Capabilities, DownloadKind, DownloadStatus, ErrorCode, FailureClass, Progress, TransferError,
};
use velocity_core::transfer::{
    DirectTransfer, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions};

use crate::download::{download_with, Cadence, Control, DownloadError, DownloadOptions};
use crate::scheduler::Sizes;

/// The most connections one transfer may use. A starting value for #47 to tune, not a requirement.
pub const MAX_CONNECTIONS: u32 = 8;
/// The probe's size: the research report's hypothesis S = 4 MiB (section 5.1), to be measured.
pub const PROBE_BYTES: u64 = 4 * 1024 * 1024;
/// A progress event is sent at most once per this many new bytes (plus every status change).
const PROGRESS_STEP: u64 = 256 * 1024;

pub struct InProcessEngine {
    shared: Arc<Shared>,
}

struct Shared {
    curl: CurlLoop,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    transfers: BTreeMap<TransferId, Entry>,
    subscribers: Vec<Sender<TransferEvent>>,
}

struct Entry {
    snapshot: TransferSnapshot,
    control: Arc<Control>,
}

impl State {
    fn emit(&mut self, event: TransferEvent) {
        // A subscriber that went away is dropped, not an error.
        self.subscribers.retain(|s| s.send(event.clone()).is_ok());
    }

    fn changed(&mut self, id: TransferId) {
        if let Some(entry) = self.transfers.get(&id) {
            let event = TransferEvent::Changed {
                snapshot: entry.snapshot.clone(),
            };
            self.emit(event);
        }
    }
}

fn finished(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Completed | DownloadStatus::Failed | DownloadStatus::Removed
    )
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // The state stays consistent between whole updates, so a worker's panic doesn't corrupt it.
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl InProcessEngine {
    /// Starts libcurl (refusing one that fails the capability check, #35) and the engine's loop.
    pub fn start() -> Result<Self, String> {
        Self::start_from(TransferId(1))
    }

    /// Starts the engine with `first` as the first transfer id it assigns. A child process gets
    /// it from the app, so a restarted engine never reuses an earlier engine's ids (#39).
    pub fn start_from(first: TransferId) -> Result<Self, String> {
        let libcurl = Libcurl::start().map_err(|e| e.to_string())?;
        let curl = CurlLoop::start(&libcurl, LoopOptions::default()).map_err(|e| e.to_string())?;
        Ok(InProcessEngine {
            shared: Arc::new(Shared {
                curl,
                state: Mutex::new(State {
                    next_id: first.0.saturating_sub(1),
                    ..State::default()
                }),
            }),
        })
    }

    /// Cancels every unfinished transfer (a child process does this when its parent goes away).
    pub fn cancel_all(&self) {
        for entry in lock(&self.shared.state).transfers.values() {
            entry.control.cancel();
        }
    }

    /// Whether every transfer has finished.
    pub fn idle(&self) -> bool {
        lock(&self.shared.state)
            .transfers
            .values()
            .all(|e| finished(e.snapshot.status))
    }

    fn known(&self, id: TransferId) -> Result<MutexGuard<'_, State>, ErrorCode> {
        let state = lock(&self.shared.state);
        if state.transfers.contains_key(&id) {
            Ok(state)
        } else {
            Err(ErrorCode::NotFound)
        }
    }
}

/// Checks a URL as untrusted input: `http` or `https`, with a host.
fn check_url(url: &str) -> Result<(), ErrorCode> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(ErrorCode::InvalidUrl);
    };
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return Err(ErrorCode::InvalidUrl);
    }
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(ErrorCode::UnsupportedScheme);
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ErrorCode::InvalidUrl);
    }
    Ok(())
}

/// Turns the downloader's progress into rate-limited `Downloading` snapshots with a measured speed.
fn progress_reporter(
    shared: &Arc<Shared>,
    id: TransferId,
    connections: u32,
) -> impl Fn(u64, Option<u64>) + Send + Sync + 'static {
    let shared = Arc::clone(shared);
    // (bytes at the last event, when it was sent)
    let last = Mutex::new((0u64, Instant::now()));
    move |written, total| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        let (bytes_then, at) = *last;
        if bytes_then != 0 && written.saturating_sub(bytes_then) < PROGRESS_STEP {
            return;
        }
        let elapsed = at.elapsed().as_secs_f64();
        // A measured rate over the bytes since the last event, never an estimate.
        let speed = if elapsed > 0.0 {
            (written.saturating_sub(bytes_then) as f64 / elapsed) as u64
        } else {
            0
        };
        *last = (written, Instant::now());
        update(&shared, id, |s| {
            s.status = DownloadStatus::Downloading;
            s.progress.completed_bytes = written;
            s.progress.total_bytes = total;
            s.progress.download_speed_bps = speed;
            s.progress.connections = connections;
        });
    }
}

impl DirectTransfer for InProcessEngine {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            protocols: vec![DownloadKind::Http],
            live_connection_changes: false,
            validator_resume: false,
            metalink: false,
            max_connections_per_host: MAX_CONNECTIONS,
        }
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        let url = spec.urls.first().ok_or(ErrorCode::InvalidInput)?.clone();
        check_url(&url)?;
        if spec.destination.as_os_str().is_empty() {
            return Err(ErrorCode::InvalidPath);
        }
        // What the prototype can't honour is refused, never silently dropped: credentials and
        // headers (#70), checksums (#46) and limits (#49).
        if spec.credentials.is_some()
            || !spec.headers.is_empty()
            || spec.checksum.is_some()
            || spec.limits != Limits::default()
        {
            return Err(ErrorCode::Unsupported);
        }
        let connections = spec.max_connections.clamp(1, MAX_CONNECTIONS);
        let mut state = lock(&self.shared.state);
        state.next_id += 1;
        let id = TransferId(state.next_id);
        let control = Arc::new(Control::with_progress(progress_reporter(
            &self.shared,
            id,
            connections,
        )));
        let snapshot = TransferSnapshot {
            id,
            status: DownloadStatus::Queued,
            progress: Progress {
                completed_bytes: 0,
                total_bytes: None,
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
            },
            wasted_bytes: 0,
            error: None,
        };
        state.transfers.insert(
            id,
            Entry {
                snapshot,
                control: Arc::clone(&control),
            },
        );
        state.changed(id);
        drop(state);
        let shared = Arc::clone(&self.shared);
        let destination = spec.destination.clone();
        let started = thread::Builder::new()
            .name(format!("velocity-transfer-{}", id.0))
            .spawn(move || run(&shared, id, &url, destination, connections, &control));
        if started.is_err() {
            update(&self.shared, id, |s| {
                s.status = DownloadStatus::Failed;
                s.error = Some(TransferError::new(
                    FailureClass::Transient,
                    None,
                    "the engine couldn't start a worker thread",
                ));
            });
        }
        Ok(id)
    }

    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        drop(self.known(id)?);
        Err(ErrorCode::Unsupported)
    }

    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        drop(self.known(id)?);
        Err(ErrorCode::Unsupported)
    }

    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let state = self.known(id)?;
        let entry = &state.transfers[&id];
        if finished(entry.snapshot.status) {
            return Err(ErrorCode::InvalidState);
        }
        // The worker sees it, removes the part file, and reports Removed.
        entry.control.cancel();
        Ok(())
    }

    fn remove(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut state = self.known(id)?;
        if !finished(state.transfers[&id].snapshot.status) {
            return Err(ErrorCode::InvalidState);
        }
        state.transfers.remove(&id);
        state.emit(TransferEvent::Removed { id });
        Ok(())
    }

    fn set_limits(&mut self, id: TransferId, _limits: Limits) -> Result<(), ErrorCode> {
        drop(self.known(id)?);
        Err(ErrorCode::Unsupported)
    }

    fn set_connections(&mut self, id: TransferId, _connections: u32) -> Result<u32, ErrorCode> {
        drop(self.known(id)?);
        Err(ErrorCode::Unsupported)
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        Ok(lock(&self.shared.state)
            .transfers
            .values()
            .map(|e| e.snapshot.clone())
            .collect())
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        lock(&self.shared.state).subscribers.push(tx);
        rx
    }
}

/// A transfer's worker: runs the download and reports how it ended.
fn run(
    shared: &Shared,
    id: TransferId,
    url: &str,
    destination: PathBuf,
    connections: u32,
    control: &Control,
) {
    update(shared, id, |s| s.status = DownloadStatus::Connecting);
    let options = DownloadOptions {
        connections,
        probe_bytes: PROBE_BYTES,
        leases: Sizes::DEFAULT,
        checkpoints: Cadence::DEFAULT,
    };
    let result = download_with(&shared.curl, url, &destination, options, control);
    update(shared, id, |s| {
        s.progress.download_speed_bps = 0;
        s.progress.connections = 0;
        match result {
            Ok(report) => {
                s.status = DownloadStatus::Completed;
                s.progress.completed_bytes = report.total;
                s.progress.total_bytes = Some(report.total);
                s.wasted_bytes = report.wasted_bytes;
            }
            Err(DownloadError::Cancelled) => s.status = DownloadStatus::Removed,
            Err(DownloadError::Paused) => s.status = DownloadStatus::Paused,
            Err(DownloadError::Transfer(e) | DownloadError::Changed(e)) => {
                s.status = DownloadStatus::Failed;
                s.error = Some(e);
            }
            Err(DownloadError::FileExists) => {
                s.status = DownloadStatus::Failed;
                s.error = Some(TransferError::new(
                    FailureClass::Permanent,
                    None,
                    "the destination file already exists; it is never replaced",
                ));
            }
            Err(DownloadError::Io(message)) => {
                s.status = DownloadStatus::Failed;
                s.error = Some(TransferError::new(FailureClass::Permanent, None, &message));
            }
        }
    });
}

/// Applies `change` to a transfer's snapshot and announces it, unless the transfer was removed.
fn update(shared: &Shared, id: TransferId, change: impl FnOnce(&mut TransferSnapshot)) {
    let mut state = lock(&shared.state);
    if let Some(entry) = state.transfers.get_mut(&id) {
        change(&mut entry.snapshot);
        state.changed(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_urls_as_untrusted_input() {
        assert_eq!(check_url("https://example.com/a.zip"), Ok(()));
        assert_eq!(check_url("HTTP://example.com"), Ok(()));
        assert_eq!(
            check_url("ftp://example.com/a"),
            Err(ErrorCode::UnsupportedScheme)
        );
        assert_eq!(
            check_url("file:///etc/passwd"),
            Err(ErrorCode::UnsupportedScheme)
        );
        assert_eq!(check_url("not a url"), Err(ErrorCode::InvalidUrl));
        assert_eq!(check_url("https:///path-only"), Err(ErrorCode::InvalidUrl));
        assert_eq!(
            check_url("https://exa mple.com/"),
            Err(ErrorCode::InvalidUrl)
        );
        assert_eq!(check_url("ht tp://example.com"), Err(ErrorCode::InvalidUrl));
    }
}
