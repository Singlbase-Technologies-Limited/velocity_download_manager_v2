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
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use velocity_core::domain::{
    Capabilities, ConnectionDetail, ConnectionLimit, DownloadKind, DownloadStatus, ErrorCode,
    FailureClass, LibraryVersion, Progress, ProtocolCapabilities, TransferError,
};
use velocity_core::paths::Paths;
use velocity_core::request::default_user_agent;
use velocity_core::transfer::{
    Checkpoint, Checksum, DirectTransfer, EngineLimits, Limits, PieceHashes, Resume, TransferEvent,
    TransferId, TransferSnapshot, TransferSpec,
};
use velocity_curl::{
    CurlLoop, Fetch, FetchRequest, GateFn, Libcurl, LoopOptions, RateLimit, RequestOptions,
    SshKeyFile, Transport,
};
use velocity_sftp::known_hosts::KnownHosts;
use velocity_sftp::{SftpOptions, SftpTransport, TrustError};

use crate::adaptive::{AdaptivePolicy, Tuning};
use crate::budget::{self, Demand, Share};
use crate::download::{
    download_from, part_path, wait_out, Cadence, ConnectionsNow, Control, DownloadError,
    DownloadOptions, Expected, Target,
};
use crate::files;
use crate::retry::RetryPolicy;
use crate::scheduler::Sizes;

/// The most connections to one host the engine starts with: the Balanced profile's
/// ([`EngineLimits::DEFAULT`]). [`DirectTransfer::set_engine_limits`] changes it, up to
/// [`EngineLimits::MAX_CONNECTIONS_PER_HOST`] (#57).
pub const MAX_CONNECTIONS: u32 = EngineLimits::DEFAULT.max_connections_per_host;
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

/// What the engine fetches through, by the link's scheme (#53): SFTP on russh, everything else
/// on libcurl's loop.
struct Transports {
    curl: CurlLoop,
    sftp: SftpTransport,
}

impl Transport for Transports {
    fn fetch(&self, request: FetchRequest, gate: GateFn) -> Fetch {
        if is_sftp(&request.url) {
            self.sftp.fetch(request, gate)
        } else {
            self.curl.fetch(request, gate)
        }
    }
}

struct Shared {
    transports: Transports,
    state: Mutex<State>,
    /// When downloads retry and wait (#48); the user's settings once #30 passes them.
    retry: RetryPolicy,
    /// The limit all transfers share (#49).
    global: RateLimit,
    /// The libcurl the engine runs on, as it reports itself, for diagnostics (#28).
    libcurl: LibraryVersion,
}

#[derive(Default)]
struct State {
    next_id: u64,
    transfers: BTreeMap<TransferId, Entry>,
    subscribers: Vec<Sender<TransferEvent>>,
    /// The performance profile's limits (#57): the per-host cap the budget shares, and the
    /// smallest piece a starting transfer is split into.
    limits: EngineLimits,
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
    /// The URL and its mirrors, `url` first (#55).
    urls: Vec<String>,
    directory: PathBuf,
    /// The file's path; `None` until the server's first answer names a file in `directory` (#40).
    destination: Option<PathBuf>,
    connections: u32,
    /// The hashes checked before the file is renamed into place (#46), in lowercase.
    expected: Expected,
    /// What every request carries (#40).
    request: RequestOptions,
}

/// What every request of `spec` carries (#40): its user agent, or Velocity's own; its referrer;
/// and its custom headers and login, which the libcurl layer sends only to the first URL's origin.
fn request_options(spec: &TransferSpec) -> RequestOptions {
    RequestOptions {
        user_agent: Some(spec.user_agent.clone().unwrap_or_else(default_user_agent)),
        referrer: spec.referrer.clone(),
        headers: spec
            .headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect(),
        login: spec
            .credentials
            .as_ref()
            .map(|c| (c.user_name.clone(), c.password.clone())),
        require_encryption: spec.require_encryption,
        ssh_key: spec.ssh_key.as_ref().map(|key| SshKeyFile {
            path: key.path.clone(),
            passphrase: key.passphrase.clone(),
        }),
    }
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
        let shares = budget::share(
            &demands,
            self.limits.max_connections_per_host,
            CONNECTION_BUDGET,
        );
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
    let port = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "ftp" => 21,
        "ftps" => 990,
        "sftp" => 22,
        _ => 80,
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

    /// Starts the engine with `known_hosts` as its file of trusted SFTP host keys, in place of
    /// the one in the app's data folder (#53).
    pub fn start_with_known_hosts(known_hosts: PathBuf) -> Result<Self, String> {
        Self::start_all(TransferId(1), RetryPolicy::DEFAULT, known_hosts)
    }

    fn start_with_policy(first: TransferId, retry: RetryPolicy) -> Result<Self, String> {
        // V2's own known_hosts file, beside its database (#53).
        let paths = Paths::resolve()
            .map_err(|code| format!("the app's data folder is unknown ({code:?})"))?;
        Self::start_all(first, retry, paths.known_hosts())
    }

    fn start_all(
        first: TransferId,
        retry: RetryPolicy,
        known_hosts: PathBuf,
    ) -> Result<Self, String> {
        let libcurl = Libcurl::start().map_err(|e| e.to_string())?;
        let linked = LibraryVersion {
            version: libcurl.linked().version.clone(),
            tls: libcurl.linked().tls.clone(),
        };
        let options = LoopOptions {
            // libcurl's own cap follows the profile's (#57): with many small downloads, each
            // keeping one connection, it is what bounds a host, not the budget.
            max_connections_per_host: MAX_CONNECTIONS as usize,
            ..LoopOptions::default()
        };
        let curl = CurlLoop::start(&libcurl, options).map_err(|e| e.to_string())?;
        let sftp = SftpTransport::start(KnownHosts::new(known_hosts), SftpOptions::default())
            .map_err(|e| format!("the SFTP transport couldn't start: {e}"))?;
        Ok(InProcessEngine {
            shared: Arc::new(Shared {
                transports: Transports { curl, sftp },
                state: Mutex::new(State {
                    next_id: first.0.saturating_sub(1),
                    ..State::default()
                }),
                retry,
                global: RateLimit::new(None),
                libcurl: linked,
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

/// Whether `url`, checked by [`check_url`], is FTP or FTPS (#51).
fn is_ftp(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("ftp") || scheme.eq_ignore_ascii_case("ftps")
    })
}

/// Whether `url`, checked by [`check_url`], is SFTP (#53).
fn is_sftp(url: &str) -> bool {
    url.split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("sftp"))
}

/// Whether a transfer of `url` uses one connection and one stream: FTP (PO decision on #51) and
/// SFTP (#53), until #77 measures whether more help.
fn one_stream(url: &str) -> bool {
    is_ftp(url) || is_sftp(url)
}

/// The connections a transfer of `url` uses when `asked` are asked for: one for FTP and SFTP,
/// otherwise up to the per-host cap.
fn connections_for(url: &str, asked: u32, cap: u32) -> u32 {
    if one_stream(url) {
        1
    } else {
        asked.clamp(1, cap)
    }
}

/// Checks a URL as untrusted input: `http`, `https`, `ftp`, `ftps` (#51) or `sftp` (#53), with
/// a host.
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
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps" | "sftp"
    ) {
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
            protocols: vec![
                ProtocolCapabilities {
                    kind: DownloadKind::Http,
                    // Ranges as leases over several connections (#42).
                    segmented: true,
                    // From a checkpoint (#43), with the strong validator in If-Range (#45).
                    resume: true,
                    validator_resume: true,
                    // libcurl is refused at start without HTTP/2 (velocity-curl's capability
                    // check).
                    http2: true,
                    // https://, verified with the platform's trust store (#69).
                    encryption: true,
                },
                ProtocolCapabilities {
                    kind: DownloadKind::Ftp,
                    // One connection until #77 measures more (PO decision on #51).
                    segmented: false,
                    // From a checkpoint, with REST, checked against MDTM when the server gives
                    // it, as a strong Last-Modified (#51, ADR-0010).
                    resume: true,
                    validator_resume: true,
                    http2: false,
                    // FTPS, explicit and implicit, verified as for HTTPS; libcurl is refused at
                    // start without ftps (velocity-curl's capability check).
                    encryption: true,
                },
                ProtocolCapabilities {
                    kind: DownloadKind::Sftp,
                    // One connection, as for FTP, until #77 measures more.
                    segmented: false,
                    // From a checkpoint, at an offset, checked against the file's time as a
                    // strong Last-Modified (#53, ADR-0010).
                    resume: true,
                    validator_resume: true,
                    http2: false,
                    // SSH is always encrypted, and the host key is checked against V2's own
                    // known_hosts file (#53).
                    encryption: true,
                },
            ],
            live_connection_changes: true,
            // #54: the manager reads Metalink documents; this engine downloads their files, with
            // their checksums and piece hashes (#46), and their mirrors in the spec's URLs.
            metalink: true,
            max_connections_per_host: lock(&self.shared.state).limits.max_connections_per_host,
            libcurl: Some(self.shared.libcurl.clone()),
        }
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        let url = spec.urls.first().ok_or(ErrorCode::InvalidInput)?.clone();
        // Every mirror is held to the same rules as the first URL (#55).
        for url in &spec.urls {
            check_url(url)?;
        }
        if spec.directory.as_os_str().is_empty() {
            return Err(ErrorCode::InvalidPath);
        }
        spec.check_request()?;
        // Without a name, the server's first answer names the file (#40).
        let destination = spec
            .file_name
            .as_ref()
            .map(|name| spec.directory.join(name));
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
        let request = request_options(&spec);
        let mut state = lock(&self.shared.state);
        let connections = connections_for(
            &url,
            spec.max_connections,
            state.limits.max_connections_per_host,
        );
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
            file_name: spec.file_name.clone(),
            content_type: None,
            retry_at_ms: None,
        };
        state.transfers.insert(
            id,
            Entry {
                snapshot,
                control: Arc::new(Control::default()),
                job: Job {
                    urls: spec.urls.clone(),
                    url,
                    directory: spec.directory.clone(),
                    destination,
                    connections,
                    expected,
                    request,
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
        // Paused: no worker holds the part file, so it is discarded here. A download into a folder
        // that was never named has none.
        let discarded = entry.job.destination.as_deref().map(discard_part);
        if let Some(Err(DownloadError::Io(message))) = discarded {
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

    fn set_engine_limits(&mut self, limits: EngineLimits) -> Result<(), ErrorCode> {
        limits.check()?;
        // libcurl first: if it refuses, nothing changed.
        self.shared
            .transports
            .curl
            .set_max_connections_per_host(limits.max_connections_per_host as usize)
            .map_err(|_| ErrorCode::Internal)?;
        let mut state = lock(&self.shared.state);
        state.limits = limits;
        // A lower cap holds running transfers back at their next step; the smallest piece
        // applies to transfers that start after it.
        state.rebalance();
        Ok(())
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        let mut state = self.known(id)?;
        let cap = state.limits.max_connections_per_host;
        let entry = state.transfers.get_mut(&id).expect("known");
        let applied = connections_for(&entry.job.url, connections, cap);
        // A running worker sees it at its next step; a later worker starts with it.
        entry.job.connections = applied;
        entry.control.set_connections(applied);
        state.rebalance();
        Ok(applied)
    }

    fn trust_host_key(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        self.shared
            .transports
            .sftp
            .trust_host_key(host, port, fingerprint)
            .map_err(|e| match e {
                TrustError::NotSeen => ErrorCode::NotFound,
                TrustError::Changed => ErrorCode::InvalidState,
                TrustError::Io(_) => ErrorCode::Io,
            })
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
        let (reporter, namer, typer) = (Arc::clone(shared), Arc::clone(shared), Arc::clone(shared));
        let control = Control::with_progress(progress_reporter(shared, id))
            .with_status(status_reporter(shared, id))
            .with_limits(vec![entry.limit.clone(), shared.global.clone()])
            .with_request(entry.job.request.clone())
            .with_checkpoints(move |checkpoint| {
                let mut state = lock(&reporter.state);
                if let Some(entry) = state.transfers.get_mut(&id) {
                    entry.last_checkpoint = Some(checkpoint.clone());
                    state.emit(TransferEvent::Checkpoint { id, checkpoint });
                }
            })
            .with_naming(move |name| {
                // From now on the transfer is this file: a retry, a pause or a cancel uses it.
                let mut state = lock(&namer.state);
                if let Some(entry) = state.transfers.get_mut(&id) {
                    entry.job.destination = Some(entry.job.directory.join(name));
                    entry.snapshot.file_name = Some(name.to_owned());
                    state.changed(id);
                }
            })
            .with_content_type(move |content_type| {
                let mut state = lock(&typer.state);
                if let Some(entry) = state.transfers.get_mut(&id) {
                    if entry.snapshot.content_type.as_deref() != Some(content_type) {
                        entry.snapshot.content_type = Some(content_type.to_owned());
                        state.changed(id);
                    }
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

/// The lease and split sizes for the profile's smallest piece (#57): a lease is never cut into
/// pieces smaller than it.
fn sizes_for(limits: EngineLimits) -> Sizes {
    Sizes {
        min_split: limits.min_segment_bytes,
        ..Sizes::DEFAULT
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
        leases: sizes_for(lock(&shared.state).limits),
        checkpoints: Cadence::DEFAULT,
        retry: shared.retry,
        // The asked-for count is a ceiling, not a must (PO decision on #47).
        adaptive: Some(AdaptivePolicy::DEFAULT),
    };
    let options = if one_stream(&job.url) {
        options.one_connection()
    } else {
        options
    };
    let mut resume = resume.cloned();
    let mut restarts = 0;
    let result = loop {
        // Named by now when an earlier attempt's answer named it (#40).
        let destination = destination_of(shared, id).or_else(|| job.destination.clone());
        let target = match &destination {
            Some(destination) => Target::File(destination),
            None => Target::Folder(&job.directory),
        };
        let result = download_from(
            &shared.transports,
            &job.urls,
            target,
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
            // The restart's own time, never the last request's wait, which is over.
            s.retry_at_ms = Some(epoch_ms(SystemTime::now() + wait));
            s.error = Some(waiting);
            s.progress.download_speed_bps = 0;
        });
        if let Err(stopped) = wait_out(control, wait) {
            if matches!(stopped, DownloadError::Cancelled) {
                // A failure that had a checkpoint kept the part file for this restart.
                let destination = destination_of(shared, id).or_else(|| job.destination.clone());
                if let Some(Err(e)) = destination.as_deref().map(discard_part) {
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

/// The file transfer `id` downloads to now: `None` until the server named a file in its folder.
fn destination_of(shared: &Shared, id: TransferId) -> Option<PathBuf> {
    lock(&shared.state)
        .transfers
        .get(&id)
        .and_then(|e| e.job.destination.clone())
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

/// Turns the downloader's status changes into snapshots: Retrying and Stalled carry no speed,
/// Retrying carries when it tries again (#59), and Downloading again clears the error of a wait
/// that is over.
fn status_reporter(
    shared: &Arc<Shared>,
    id: TransferId,
) -> impl Fn(DownloadStatus, Option<SystemTime>) + Send + Sync + 'static {
    let shared = Arc::clone(shared);
    move |status, retry_at| {
        update(&shared, id, |s| {
            s.status = status;
            s.retry_at_ms = retry_at.map(epoch_ms);
            if status == DownloadStatus::Downloading {
                s.error = None;
            } else {
                s.progress.download_speed_bps = 0;
            }
        })
    }
}

/// Milliseconds since the Unix epoch, the contract's time (0 for a clock set before it).
fn epoch_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Only Retrying has a retry time.
fn changed_status(snapshot: &mut TransferSnapshot, change: impl FnOnce(&mut TransferSnapshot)) {
    change(snapshot);
    if snapshot.status != DownloadStatus::Retrying {
        snapshot.retry_at_ms = None;
    }
}

/// Applies `change` to a transfer's snapshot and announces it, unless the transfer was removed.
fn update(shared: &Shared, id: TransferId, change: impl FnOnce(&mut TransferSnapshot)) {
    let mut state = lock(&shared.state);
    if let Some(entry) = state.transfers.get_mut(&id) {
        changed_status(&mut entry.snapshot, change);
        state.changed(id);
    }
}

/// [`update`] for the end of a worker: it is no longer running.
fn ended(shared: &Shared, id: TransferId, change: impl FnOnce(&mut TransferSnapshot)) {
    let mut state = lock(&shared.state);
    if let Some(entry) = state.transfers.get_mut(&id) {
        changed_status(&mut entry.snapshot, change);
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
    fn the_profiles_smallest_piece_is_the_split_size() {
        // #57: Balanced keeps today's sizes; Conservative splits nothing below 20 MiB.
        assert_eq!(sizes_for(EngineLimits::DEFAULT), Sizes::DEFAULT);
        let conservative = EngineLimits {
            max_connections_per_host: 2,
            min_segment_bytes: 20 << 20,
        };
        let sizes = sizes_for(conservative);
        assert_eq!(sizes.min_split, 20 << 20);
        assert_eq!(sizes.max_lease, Sizes::DEFAULT.max_lease);
    }

    #[test]
    fn checks_urls_as_untrusted_input() {
        assert_eq!(check_url("https://example.com/a.zip"), Ok(()));
        assert_eq!(check_url("HTTP://example.com"), Ok(()));
        // FTP and FTPS since #51, SFTP since #53; SCP is a non-goal.
        assert_eq!(check_url("ftp://example.com/a"), Ok(()));
        assert_eq!(check_url("FTPS://example.com/a"), Ok(()));
        assert_eq!(check_url("sftp://example.com/a"), Ok(()));
        assert_eq!(
            check_url("scp://example.com/a"),
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
        assert_eq!(host_of("ftp://ftp.example.org/f"), "ftp.example.org:21");
        assert_eq!(host_of("FTPS://ftp.example.org/f"), "ftp.example.org:990");
        assert_eq!(host_of("ftp://h:2121/f"), "h:2121");
        assert_eq!(
            host_of("sftp://files.example.org/f"),
            "files.example.org:22"
        );
    }

    #[test]
    fn ftp_uses_one_connection_whatever_is_asked() {
        assert_eq!(connections_for("ftp://h/f", 8, 6), 1);
        assert_eq!(connections_for("ftps://h/f", 0, 6), 1);
        assert_eq!(connections_for("sftp://h/f", 8, 6), 1);
        assert_eq!(connections_for("https://h/f", 8, 6), 6);
        assert_eq!(connections_for("https://h/f", 0, 6), 1);
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
