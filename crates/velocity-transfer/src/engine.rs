//! The engine behind the `DirectTransfer` contract (#36, #37, #43), in the app's process. Each
//! transfer runs [`download_resumable`] on its own worker thread, sharing one libcurl loop; the
//! engine turns what happens into contract events, checkpoints included. `stdio.rs` serves the
//! same engine from a child process.
//!
//! - **Pause** stops the worker and keeps the part file, after a last checkpoint; **resume** starts
//!   a new worker from that checkpoint (#43).
//! - **Retries** (#48): the downloader retries its own requests; a download that still fails in a
//!   way worth retrying is started again by its worker, up to the policy's limit, and the snapshot
//!   shows Retrying while it waits and Stalled while no bytes come.
//! - **Speed limits** (#49): each transfer has its own limit, and all of them share the global
//!   one; both can change while transfers run.
//! - **Connections** (#47): `set_connections` changes the count a transfer uses while it runs,
//!   without restarting it; a paused transfer uses the new count when it resumes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Instant;

use velocity_core::domain::{
    Capabilities, ConnectionDetail, ConnectionLimit, DownloadKind, DownloadStatus, ErrorCode,
    FailureClass, Progress, TransferError,
};
use velocity_core::transfer::{
    Checkpoint, Checksum, DirectTransfer, Limits, PieceHashes, Resume, TransferEvent, TransferId,
    TransferSnapshot, TransferSpec,
};
use velocity_curl::{CurlLoop, Libcurl, LoopOptions, RateLimit};

use crate::adaptive::{AdaptivePolicy, Tuning};
use crate::budget::{self, Demand, Share};
use crate::download::{
    download_resumable, part_path, wait_out, Cadence, ConnectionsNow, Control, DownloadError,
    DownloadOptions, Expected,
};
use crate::files;
use crate::retry::RetryPolicy;
use crate::scheduler::Sizes;

/// The most connections to one host, which its transfers share, and so the most one transfer may
/// use. Report section 5.5's starting value (8, at most 16; PO decision on #47), not a
/// requirement; #30 makes it a setting.
pub const MAX_CONNECTIONS: u32 = 8;
/// The connections all transfers share, max-min fairly (V1's 48; PO decision on #47). A starting
/// value; #30 makes it a setting.
pub const CONNECTION_BUDGET: u32 = 48;
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
    /// When downloads retry and wait (#48); the user's settings once #30 passes them.
    retry: RetryPolicy,
    /// The limit all transfers share (#49).
    global: RateLimit,
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
    job: Job,
    /// This transfer's own speed limit; it outlives its workers (#49).
    limit: RateLimit,
    /// The last checkpoint the worker sent, to resume from after a pause.
    last_checkpoint: Option<Checkpoint>,
    /// A worker thread is running the download.
    running: bool,
    /// Its share of the connections while it runs (#47).
    share: Share,
}

/// What a transfer downloads, kept to start a worker again after a pause.
#[derive(Clone)]
struct Job {
    url: String,
    destination: PathBuf,
    connections: u32,
    /// The hashes checked before the file is renamed into place (#46), in lowercase.
    expected: Expected,
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

    /// Shares the connections between the running transfers again (#47), after one starts, ends
    /// or asks for a different count. Each worker sees its new allowance at its next step; a
    /// snapshot whose connection detail changed is announced.
    fn rebalance(&mut self) {
        let running: Vec<(TransferId, String, u32)> = self
            .transfers
            .iter()
            .filter(|(_, e)| e.running)
            .map(|(id, e)| (*id, host_of(&e.job.url), e.job.connections))
            .collect();
        let demands: Vec<Demand<'_>> = running
            .iter()
            .map(|(_, host, wants)| Demand {
                host,
                wants: *wants,
            })
            .collect();
        let shares = budget::share(&demands, MAX_CONNECTIONS, CONNECTION_BUDGET);
        let mut changed = Vec::new();
        for ((id, _, _), share) in running.iter().zip(shares) {
            let entry = self.transfers.get_mut(id).expect("running");
            entry.share = share;
            entry.control.allow(share.allowed);
            let detail = Some(detail(
                entry.job.connections,
                share,
                entry.control.connections_now(),
            ));
            if entry.snapshot.progress.connection_detail != detail {
                entry.snapshot.progress.connection_detail = detail;
                changed.push(*id);
            }
        }
        for id in changed {
            self.changed(id);
        }
    }
}

/// A running transfer's connection detail (#47): what it asked for, what it may use, and the
/// tightest of the engine's share, a throttling server, the adaptive ramp and the file's size.
fn detail(requested: u32, share: Share, now: ConnectionsNow) -> ConnectionDetail {
    let mut allowed = requested.min(share.allowed);
    let mut limited_by = share.limited_by;
    if let Some(t) = now.throttled_to.filter(|t| *t < allowed) {
        allowed = t;
        limited_by = Some(ConnectionLimit::ServerThrottling);
    }
    if let Some((t, why)) = now.tuned.filter(|(t, _)| *t < allowed) {
        allowed = t;
        limited_by = Some(match why {
            Tuning::RampingUp => ConnectionLimit::RampingUp,
            Tuning::NoGain => ConnectionLimit::NoGain,
        });
    }
    if now.file_bound && now.active < allowed {
        allowed = now.active.max(1);
        limited_by = Some(ConnectionLimit::FileSize);
    }
    ConnectionDetail {
        requested,
        allowed,
        limited_by: limited_by.filter(|_| allowed < requested),
    }
}

/// The host name and port a URL's connections go to, as libcurl's per-host cap counts them
/// (`CURLMOPT_MAX_HOST_CONNECTIONS`: "a host name + port number pair"). The URL was checked by
/// [`check_url`].
fn host_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .to_ascii_lowercase();
    // A port follows the last colon, unless that colon is inside an IPv6 literal.
    if host
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.contains(']'))
    {
        return host;
    }
    let port = if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        80
    };
    format!("{host}:{port}")
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
        Self::start_with_policy(first, RetryPolicy::DEFAULT)
    }

    /// Starts the engine with `retry` in place of [`RetryPolicy::DEFAULT`].
    pub fn start_with(retry: RetryPolicy) -> Result<Self, String> {
        Self::start_with_policy(TransferId(1), retry)
    }

    fn start_with_policy(first: TransferId, retry: RetryPolicy) -> Result<Self, String> {
        let libcurl = Libcurl::start().map_err(|e| e.to_string())?;
        let options = LoopOptions {
            max_connections_per_host: MAX_CONNECTIONS as usize,
            ..LoopOptions::default()
        };
        let curl = CurlLoop::start(&libcurl, options).map_err(|e| e.to_string())?;
        Ok(InProcessEngine {
            shared: Arc::new(Shared {
                curl,
                state: Mutex::new(State {
                    next_id: first.0.saturating_sub(1),
                    ..State::default()
                }),
                retry,
                global: RateLimit::new(None),
            }),
        })
    }

    /// Cancels every unfinished transfer, removing its part file.
    pub fn cancel_all(&self) {
        for entry in lock(&self.shared.state).transfers.values() {
            entry.control.cancel();
        }
    }

    /// Pauses every running transfer and keeps its part file (a child process does this when its
    /// parent goes away, so the app can resume them from their checkpoints, #43).
    pub fn pause_all(&self) {
        for entry in lock(&self.shared.state).transfers.values() {
            if entry.running {
                entry.control.pause();
            }
        }
    }

    /// Whether no worker is running.
    pub fn idle(&self) -> bool {
        lock(&self.shared.state)
            .transfers
            .values()
            .all(|e| !e.running)
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
        let mut state = lock(&shared.state);
        if let Some(entry) = state.transfers.get_mut(&id) {
            let now = entry.control.connections_now();
            let s = &mut entry.snapshot;
            s.status = DownloadStatus::Downloading;
            s.progress.completed_bytes = written;
            s.progress.total_bytes = total;
            s.progress.download_speed_bps = speed;
            s.progress.connections = now.active;
            s.progress.connection_detail = Some(detail(entry.job.connections, entry.share, now));
            state.changed(id);
        }
    }
}

impl DirectTransfer for InProcessEngine {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            protocols: vec![DownloadKind::Http],
            live_connection_changes: true,
            // With a strong validator, resume sends it in If-Range (#45).
            validator_resume: true,
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
        spec.limits.check()?;
        let expected = Expected {
            checksum: spec
                .checksum
                .as_ref()
                .map(Checksum::normalized)
                .transpose()?,
            pieces: spec
                .pieces
                .as_ref()
                .map(PieceHashes::normalized)
                .transpose()?,
        };
        // What the prototype can't honour is refused, never silently dropped: credentials and
        // headers (#70).
        if spec.credentials.is_some() || !spec.headers.is_empty() {
            return Err(ErrorCode::Unsupported);
        }
        let connections = spec.max_connections.clamp(1, MAX_CONNECTIONS);
        let mut state = lock(&self.shared.state);
        state.next_id += 1;
        let id = TransferId(state.next_id);
        let snapshot = TransferSnapshot {
            id,
            status: DownloadStatus::Queued,
            progress: Progress {
                completed_bytes: 0,
                total_bytes: None,
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: None,
            },
            wasted_bytes: 0,
            error: None,
            verification: None,
        };
        state.transfers.insert(
            id,
            Entry {
                snapshot,
                control: Arc::new(Control::default()),
                job: Job {
                    url,
                    destination: spec.destination.clone(),
                    connections,
                    expected,
                },
                limit: RateLimit::new(spec.limits.max_download_bps),
                last_checkpoint: None,
                running: false,
                share: Share {
                    allowed: connections,
                    limited_by: None,
                },
            },
        );
        state.changed(id);
        drop(state);
        start_worker(&self.shared, id, spec.resume);
        Ok(id)
    }

    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let state = self.known(id)?;
        let entry = &state.transfers[&id];
        if !entry.running || finished(entry.snapshot.status) {
            return Err(ErrorCode::InvalidState);
        }
        // The worker sees it, takes a last checkpoint, keeps the part file, and reports Paused.
        entry.control.pause();
        Ok(())
    }

    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut state = self.known(id)?;
        let entry = state.transfers.get_mut(&id).expect("known");
        if entry.running || entry.snapshot.status != DownloadStatus::Paused {
            return Err(ErrorCode::InvalidState);
        }
        // From the last checkpoint; without one, the part file is this transfer's own and starts
        // over (PO decision on #43).
        let resume = match entry.last_checkpoint.clone() {
            Some(checkpoint) => Resume::Checkpoint { checkpoint },
            None => Resume::DiscardPartFile {},
        };
        entry.snapshot.status = DownloadStatus::Queued;
        state.changed(id);
        drop(state);
        start_worker(&self.shared, id, Some(resume));
        Ok(())
    }

    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut state = self.known(id)?;
        let entry = state.transfers.get_mut(&id).expect("known");
        if finished(entry.snapshot.status) {
            return Err(ErrorCode::InvalidState);
        }
        if entry.running {
            // The worker sees it, removes the part file, and reports Removed.
            entry.control.cancel();
            return Ok(());
        }
        // Paused: no worker holds the part file, so it is discarded here.
        if let Err(DownloadError::Io(message)) = discard_part(&entry.job.destination) {
            entry.snapshot.status = DownloadStatus::Failed;
            entry.snapshot.error =
                Some(TransferError::new(FailureClass::Permanent, None, &message));
            state.changed(id);
            return Ok(());
        }
        entry.snapshot.status = DownloadStatus::Removed;
        entry.last_checkpoint = None;
        state.changed(id);
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

    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        let state = self.known(id)?;
        limits.check()?;
        // A running worker's requests see it at their next write.
        state.transfers[&id].limit.set(limits.max_download_bps);
        Ok(())
    }

    fn set_global_limits(&mut self, limits: Limits) -> Result<(), ErrorCode> {
        limits.check()?;
        self.shared.global.set(limits.max_download_bps);
        Ok(())
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        let mut state = self.known(id)?;
        let applied = connections.clamp(1, MAX_CONNECTIONS);
        let entry = state.transfers.get_mut(&id).expect("known");
        // A running worker sees it at its next step; a later worker starts with it.
        entry.job.connections = applied;
        entry.control.set_connections(applied);
        state.rebalance();
        Ok(applied)
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

/// Starts a worker for transfer `id`, with a new control that reports its progress and passes its
/// checkpoints on as events.
fn start_worker(shared: &Arc<Shared>, id: TransferId, resume: Option<Resume>) {
    let job = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.transfers.get_mut(&id) else {
            return;
        };
        let reporter = Arc::clone(shared);
        let control = Control::with_progress(progress_reporter(shared, id))
            .with_status(status_reporter(shared, id))
            .with_limits(vec![entry.limit.clone(), shared.global.clone()])
            .with_checkpoints(move |checkpoint| {
                let mut state = lock(&reporter.state);
                if let Some(entry) = state.transfers.get_mut(&id) {
                    entry.last_checkpoint = Some(checkpoint.clone());
                    state.emit(TransferEvent::Checkpoint { id, checkpoint });
                }
            });
        entry.control = Arc::new(control);
        entry.running = true;
        let job = (entry.job.clone(), Arc::clone(&entry.control));
        state.rebalance();
        job
    };
    let (job, control) = job;
    let worker = Arc::clone(shared);
    let started = thread::Builder::new()
        .name(format!("velocity-transfer-{}", id.0))
        .spawn(move || run(&worker, id, &job, resume.as_ref(), &control));
    if started.is_err() {
        ended(shared, id, |s| {
            s.status = DownloadStatus::Failed;
            s.error = Some(TransferError::new(
                FailureClass::Transient,
                None,
                "the engine couldn't start a worker thread",
            ));
        });
    }
}

/// A transfer's worker: runs the download and reports how it ended. A download that fails in a
/// way worth retrying is started again after a wait, at most [`RetryPolicy::download_retries`]
/// times (V1's `app_retries`, #48), from its last checkpoint when it has one.
fn run(shared: &Shared, id: TransferId, job: &Job, resume: Option<&Resume>, control: &Control) {
    update(shared, id, |s| s.status = DownloadStatus::Connecting);
    let options = DownloadOptions {
        connections: job.connections,
        probe_bytes: PROBE_BYTES,
        leases: Sizes::DEFAULT,
        checkpoints: Cadence::DEFAULT,
        retry: shared.retry,
        // The asked-for count is a ceiling, not a must (PO decision on #47).
        adaptive: Some(AdaptivePolicy::DEFAULT),
    };
    let mut resume = resume.cloned();
    let mut restarts = 0;
    let result = loop {
        let result = download_resumable(
            &shared.curl,
            &job.url,
            &job.destination,
            options,
            resume.as_ref(),
            &job.expected,
            control,
        );
        let Err(DownloadError::Transfer(e)) = &result else {
            break result;
        };
        restarts += 1;
        let Some(wait) = shared.retry.download_wait(e.class, restarts) else {
            break result;
        };
        let mut waiting = e.clone();
        waiting.message = format!(
            "{} Retrying automatically ({restarts} of {}).",
            e.message, shared.retry.download_retries
        );
        update(shared, id, |s| {
            s.status = DownloadStatus::Retrying;
            s.error = Some(waiting);
            s.progress.download_speed_bps = 0;
        });
        if let Err(stopped) = wait_out(control, wait) {
            if matches!(stopped, DownloadError::Cancelled) {
                // A failure that had a checkpoint kept the part file for this restart.
                if let Err(e) = discard_part(&job.destination) {
                    break Err(e);
                }
            }
            break Err(stopped);
        }
        // From the last checkpoint; without one, the part file is this transfer's own and the
        // download starts over (PO decision on #43).
        resume = Some(match last_checkpoint(shared, id) {
            Some(checkpoint) => Resume::Checkpoint { checkpoint },
            None => Resume::DiscardPartFile {},
        });
    };
    ended(shared, id, |s| {
        s.progress.download_speed_bps = 0;
        s.progress.connections = 0;
        match result {
            Ok(report) => {
                s.status = DownloadStatus::Completed;
                s.progress.completed_bytes = report.total;
                s.progress.total_bytes = Some(report.total);
                s.wasted_bytes = report.wasted_bytes;
                s.verification = Some(report.verification);
            }
            Err(DownloadError::Cancelled) => s.status = DownloadStatus::Removed,
            Err(DownloadError::Paused) => s.status = DownloadStatus::Paused,
            Err(
                DownloadError::Transfer(e)
                | DownloadError::Changed(e)
                | DownloadError::EncodingChanged(e)
                | DownloadError::IntegrityFailed(e),
            ) => {
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

/// The last checkpoint transfer `id` sent.
fn last_checkpoint(shared: &Shared, id: TransferId) -> Option<Checkpoint> {
    lock(&shared.state)
        .transfers
        .get(&id)
        .and_then(|e| e.last_checkpoint.clone())
}

/// Removes the part file of a download whose worker is stopping; there may be none.
fn discard_part(destination: &std::path::Path) -> Result<(), DownloadError> {
    let part = part_path(destination);
    match files::remove_part(&part) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DownloadError::Io(format!(
            "couldn't remove {}: {e}",
            part.display()
        ))),
    }
}

/// Turns the downloader's status changes into snapshots: Retrying and Stalled carry no speed, and
/// Downloading again clears the error of a wait that is over.
fn status_reporter(
    shared: &Arc<Shared>,
    id: TransferId,
) -> impl Fn(DownloadStatus) + Send + Sync + 'static {
    let shared = Arc::clone(shared);
    move |status| {
        update(&shared, id, |s| {
            s.status = status;
            if status == DownloadStatus::Downloading {
                s.error = None;
            } else {
                s.progress.download_speed_bps = 0;
            }
        })
    }
}

/// Applies `change` to a transfer's snapshot and announces it, unless the transfer was removed.
fn update(shared: &Shared, id: TransferId, change: impl FnOnce(&mut TransferSnapshot)) {
    let mut state = lock(&shared.state);
    if let Some(entry) = state.transfers.get_mut(&id) {
        change(&mut entry.snapshot);
        state.changed(id);
    }
}

/// [`update`] for the end of a worker: it is no longer running.
fn ended(shared: &Shared, id: TransferId, change: impl FnOnce(&mut TransferSnapshot)) {
    let mut state = lock(&shared.state);
    if let Some(entry) = state.transfers.get_mut(&id) {
        change(&mut entry.snapshot);
        entry.running = false;
        entry.snapshot.progress.connection_detail = None;
        if entry.snapshot.status != DownloadStatus::Paused {
            entry.last_checkpoint = None;
        }
        state.changed(id);
        // Its connections go to the others.
        state.rebalance();
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

    #[test]
    fn hosts_are_counted_by_name_and_port_as_libcurl_does() {
        assert_eq!(host_of("https://Example.com/a.zip"), "example.com:443");
        assert_eq!(host_of("http://example.com"), "example.com:80");
        assert_eq!(host_of("http://example.com:8080/a"), "example.com:8080");
        assert_eq!(host_of("https://user:pw@example.com/a"), "example.com:443");
        assert_eq!(host_of("http://[::1]/a"), "[::1]:80");
        assert_eq!(host_of("http://[::1]:8080/a"), "[::1]:8080");
    }

    fn now(active: u32, throttled_to: Option<u32>, file_bound: bool) -> ConnectionsNow {
        ConnectionsNow {
            active,
            throttled_to,
            file_bound,
            tuned: None,
        }
    }

    fn share(allowed: u32, limited_by: Option<ConnectionLimit>) -> Share {
        Share {
            allowed,
            limited_by,
        }
    }

    #[test]
    fn the_detail_names_the_tightest_limit() {
        let detail_of = |requested, s, n| {
            let d = detail(requested, s, n);
            (d.allowed, d.limited_by)
        };
        assert_eq!(detail_of(4, share(4, None), now(4, None, false)), (4, None));
        assert_eq!(
            detail_of(
                8,
                share(4, Some(ConnectionLimit::PerHostCap)),
                now(4, None, false)
            ),
            (4, Some(ConnectionLimit::PerHostCap))
        );
        // A throttling server below the share binds instead.
        assert_eq!(
            detail_of(
                8,
                share(4, Some(ConnectionLimit::SharedBudget)),
                now(2, Some(2), false)
            ),
            (2, Some(ConnectionLimit::ServerThrottling))
        );
        // A file too small to split binds below both.
        assert_eq!(
            detail_of(8, share(8, None), now(3, Some(4), true)),
            (3, Some(ConnectionLimit::FileSize))
        );
        // Throttled to more than was asked for: nothing holds it back.
        assert_eq!(
            detail_of(2, share(2, None), now(2, Some(4), false)),
            (2, None)
        );
    }
}
