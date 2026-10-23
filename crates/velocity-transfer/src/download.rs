//! The downloader (#37, #41): one file over HTTP(S), with a probe that is also the first segment
//! and decides between ranges, sequential ranges and one stream (docs/ENGINE_INTERNALS.md), static
//! parallel segments when ranges work, retries with waits and stall detection (#48), and a finish
//! that never replaces an existing file. What it doesn't do yet is the rest of R5: validators
//! (#45), whole-file hashes (#46) and adaptive connections (#47).
//!
//! The byte rules (`.claude/rules/transfer-engine.md`, research report sections 5.1 and 5.2):
//! - a byte is accepted only from the segment that owns it, at its own offset, once;
//! - a response that isn't exactly what was asked for is aborted before any body byte is written;
//! - every received byte is either written or counted as wasted;
//! - the file is renamed into place only after its coverage is complete and synced to disk.
//!
//! The retry rules (#48, [`RetryPolicy`]): a failure worth retrying frees its connection for a
//! wait, then a new lease takes the rest; the download fails after too many failures in a row
//! with no new byte; a stall counts only time without bytes from the network, never time spent
//! waiting for the disk.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use velocity_core::domain::{DownloadStatus, FailureClass, TransferError};
use velocity_core::transfer::{Checkpoint, Resume, CHECKPOINT_FORMAT};
use velocity_curl::{
    CurlLoop, Fetch, FetchError, FetchEvent, FetchLimit, FetchRequest, Gate, RateLimit,
    ResponseHead,
};

use crate::adaptive::{AdaptivePolicy, Ramp, Tuning};
use crate::failure::{self, fetch_failure};
use crate::files;
use crate::probe::{
    check_continuation, check_segment, classify_probe, is_multipart, Continuation, ProbeOutcome,
    SegmentRejection,
};
use crate::retry::{self, RetryPolicy};
use crate::scheduler::{Assignment, Coverage, Lease, LeaseId, Scheduler, Sizes};
use crate::validator::Representation;

/// How often a waiting download checks whether it was cancelled.
const CANCEL_CHECK: Duration = Duration::from_millis(100);
/// How many fetch events each segment may have waiting for the writer. Each holds at most the
/// libcurl layer's per-fetch buffer (1 MiB by default), so this bounds the queue's memory.
const QUEUED_PER_SEGMENT: usize = 2;
/// Anomalous answers a download tolerates before ranges stop for it, and the rest comes as one
/// stream (report section 5.1, step 7: "two consecutive anomalies").
const MAX_ANOMALIES: u32 = 2;

/// Called with the bytes written so far and the total, once known.
type ProgressFn = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;
/// Called with each checkpoint, after the part file was synced.
type CheckpointFn = Box<dyn Fn(Checkpoint) + Send + Sync>;
/// Called when the download starts waiting to retry, stalls, or receives bytes again.
type StatusFn = Box<dyn Fn(DownloadStatus) + Send + Sync>;

/// Lets a caller follow, pause and stop a running download (the engine, #37, #43).
#[derive(Default)]
pub struct Control {
    cancelled: AtomicBool,
    paused: AtomicBool,
    progress: Option<ProgressFn>,
    checkpoints: Option<CheckpointFn>,
    status: Option<StatusFn>,
    /// The status last passed to `status`, so each change is reported once.
    shown: Mutex<Option<DownloadStatus>>,
    /// The speed limits every request of the download draws from (#49).
    limits: Vec<RateLimit>,
    /// The most bytes one of its requests held for the writer (#49); shared with the forwarders.
    peak_buffered: Arc<AtomicU64>,
    /// The connections asked for while the download runs (#47). 0 until
    /// [`Control::set_connections`] is called; [`DownloadOptions::connections`] applies until then.
    connections: AtomicU32,
    /// The most connections the engine's budget allows it (#47); 0 for no limit.
    allowed: AtomicU32,
    /// What the download runs now, for the engine to report (#47).
    active: AtomicU32,
    /// The most connections a throttling server left it; 0 when none did.
    throttled_to: AtomicU32,
    /// The rest of the file was too small to split for another connection.
    file_bound: AtomicBool,
    /// The adaptive ramp's count, when it is below what the download may use, and why.
    tuned: Mutex<Option<(u32, Tuning)>>,
    /// The requests go on connections of their own, not as HTTP/2 streams of one (#47).
    own_connections: AtomicBool,
}

/// The connections a download runs now, and what of its own holds it back (#47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionsNow {
    /// Requests open now, including ones being stopped.
    pub active: u32,
    /// The most connections a throttling server left it (#48).
    pub throttled_to: Option<u32>,
    /// The rest of the file is too small to split between more connections.
    pub file_bound: bool,
    /// The adaptive ramp's count, when it is below what the download may use, and why.
    pub tuned: Option<(u32, Tuning)>,
}

impl Control {
    /// `progress` is called with the bytes written so far and the total, once known.
    pub fn with_progress(progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static) -> Self {
        Control {
            progress: Some(Box::new(progress)),
            ..Control::default()
        }
    }

    /// `checkpoint` is called with each checkpoint, after the part file was synced (#43).
    pub fn with_checkpoints(
        mut self,
        checkpoint: impl Fn(Checkpoint) + Send + Sync + 'static,
    ) -> Self {
        self.checkpoints = Some(Box::new(checkpoint));
        self
    }

    /// `status` is called with [`DownloadStatus::Retrying`] when the download waits to retry,
    /// [`DownloadStatus::Stalled`] when no byte has come from the network for a while, and
    /// [`DownloadStatus::Downloading`] when bytes come again (#48).
    pub fn with_status(mut self, status: impl Fn(DownloadStatus) + Send + Sync + 'static) -> Self {
        self.status = Some(Box::new(status));
        self
    }

    /// `limits` are the speed limits every request of the download draws from, such as its own
    /// and the global one (#49).
    pub fn with_limits(mut self, limits: Vec<RateLimit>) -> Self {
        self.limits = limits;
        self
    }

    /// Stops the download and keeps its part file: it ends with [`DownloadError::Paused`], after a
    /// last checkpoint when it has ranges to resume from.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Stops the download; it ends with [`DownloadError::Cancelled`] and removes its part file.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Changes the connections the download uses, at least 1, while it runs and without
    /// restarting it (#47).
    pub fn set_connections(&self, connections: u32) {
        self.connections
            .store(connections.max(1), Ordering::Release);
    }

    /// The connections asked for: the last [`Control::set_connections`], or `initial`.
    fn connections(&self, initial: u32) -> u32 {
        match self.connections.load(Ordering::Acquire) {
            0 => initial.max(1),
            n => n,
        }
    }

    /// Caps the connections at what the engine's budget allows, at least 1, while the download
    /// runs (#47).
    pub fn allow(&self, connections: u32) {
        self.allowed.store(connections.max(1), Ordering::Release);
    }

    /// The engine's cap from [`Control::allow`]; no limit until it is called.
    fn allowed(&self) -> u32 {
        match self.allowed.load(Ordering::Acquire) {
            0 => u32::MAX,
            n => n,
        }
    }

    /// Records the requests open now, and whether the file is too small for more.
    fn running(&self, active: u32, file_bound: bool) {
        self.active.store(active, Ordering::Release);
        self.file_bound.store(file_bound, Ordering::Release);
    }

    /// Records the adaptive ramp's count when it is below what the download may use.
    fn tune(&self, tuned: Option<(u32, Tuning)>) {
        *self.tuned.lock().expect("the tuning lock") = tuned;
    }

    /// What the download runs now (#47).
    pub fn connections_now(&self) -> ConnectionsNow {
        ConnectionsNow {
            active: self.active.load(Ordering::Acquire),
            throttled_to: match self.throttled_to.load(Ordering::Acquire) {
                0 => None,
                n => Some(n),
            },
            file_bound: self.file_bound.load(Ordering::Acquire),
            tuned: *self.tuned.lock().expect("the tuning lock"),
        }
    }

    /// Reports progress. Bytes arriving also mean the download is downloading again.
    fn report(&self, written: u64, total: Option<u64>) {
        self.show(DownloadStatus::Downloading);
        if let Some(p) = &self.progress {
            p(written, total);
        }
    }

    /// Records how many bytes a finished request held for the writer at most.
    fn note_buffered(&self, fetch: &Fetch) {
        note_peak(&self.peak_buffered, fetch);
    }

    /// Reports `status`, if it isn't the one reported last.
    fn show(&self, status: DownloadStatus) {
        let Some(s) = &self.status else { return };
        let changed = {
            let mut shown = self.shown.lock().expect("the status lock");
            shown.replace(status) != Some(status)
        };
        // Called outside the lock, so a callback may take its time.
        if changed {
            s(status);
        }
    }

    fn checkpoint(&self, checkpoint: Checkpoint) {
        if let Some(c) = &self.checkpoints {
            c(checkpoint);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadOptions {
    /// Connections for a server that honours ranges: the probe, then up to this many segments at
    /// once (at least 1).
    pub connections: u32,
    /// The probe's size (the report's hypothesis S = 4 MiB; a starting value for #47 to tune).
    pub probe_bytes: u64,
    /// Lease and split sizes for the range scheduler (#42); [`Sizes::DEFAULT`] outside tests.
    pub leases: Sizes,
    /// How often a checkpoint is taken (#43); [`Cadence::DEFAULT`] outside tests.
    pub checkpoints: Cadence,
    /// When to retry, wait and give up (#48); [`RetryPolicy::DEFAULT`] outside tests.
    pub retry: RetryPolicy,
    /// Adaptive connections (#47): start with one and add while each helps, up to `connections`.
    /// `None` uses `connections` from the start, as the CLI does to compare fixed counts.
    pub adaptive: Option<AdaptivePolicy>,
}

/// How often the part file is synced and a checkpoint taken: after this many new bytes or this
/// much time, whichever comes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    pub bytes: u64,
    pub interval: Duration,
}

impl Cadence {
    /// The report's hypothesis (section 5.2): 16 MiB or 2 s, a starting value to tune by
    /// measurement (#43's risk: fsync costs on slow disks).
    pub const DEFAULT: Cadence = Cadence {
        bytes: 16 << 20,
        interval: Duration::from_secs(2),
    };
}

/// How the file was fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ranges worked: the probe plus parallel segments.
    Ranges,
    /// Ranges worked for the probe, then stopped: a later range came back as `200`, or the server
    /// sent two anomalous answers. The rest was fetched as one stream, writing only the missing
    /// bytes.
    RangesLost,
    /// The server honours ranges but gives no total (`bytes 0-b/*`): the rest was fetched as one
    /// open range after the probe, with no parallel segments.
    SequentialRanges,
    /// The server ignored the probe's range: the probe's response was the whole file.
    SingleStream,
    /// A `416` for an empty file.
    Empty,
}

/// What a finished download did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadReport {
    pub mode: Mode,
    pub total: u64,
    /// Fetches started, the probe included.
    pub fetches: u32,
    /// Bytes written to the part file, each exactly once.
    pub written_bytes: u64,
    /// Bytes received but not written (already covered, or past a segment's end).
    pub wasted_bytes: u64,
    /// Leases cut from a live one, whose old request then stopped at the new boundary.
    pub splits: u32,
    /// Checkpoints sent.
    pub checkpoints: u32,
    /// Bytes trusted from the checkpoint it resumed from, not fetched again.
    pub resumed_bytes: u64,
    /// The part file was marked sparse (Windows; see `files::create_part`).
    pub sparse: bool,
    /// The most body bytes one request held for the writer at a time, measured (#49). Bounded by
    /// the libcurl layer's buffer per fetch: a slow disk pauses the transfer instead.
    pub peak_buffered_bytes: u64,
    /// Over HTTP/2, more streams on one connection stopped helping, and connections of their own
    /// did better, so the requests went on those (#47).
    pub own_connections: bool,
}

#[derive(Debug)]
pub enum DownloadError {
    /// The server or the network failed the transfer.
    Transfer(TransferError),
    /// The destination already exists; it is never replaced.
    FileExists,
    /// A local file operation failed.
    Io(String),
    /// [`Control::cancel`] stopped it.
    Cancelled,
    /// [`Control::pause`] stopped it; its part file is kept.
    Paused,
    /// The file on the server isn't the one this download started (another total): its bytes
    /// are never mixed with the old ones (report section 5.3).
    Changed(TransferError),
}

impl From<TransferError> for DownloadError {
    fn from(e: TransferError) -> Self {
        DownloadError::Transfer(e)
    }
}

/// Downloads `url` to `destination`, through a part file beside it (`<destination>.vdmpart`).
pub fn download(
    lp: &CurlLoop,
    url: &str,
    destination: &Path,
    options: DownloadOptions,
) -> Result<DownloadReport, DownloadError> {
    download_with(lp, url, destination, options, &Control::default())
}

/// [`download_with`], continuing what an earlier attempt left on disk as `resume` says (#43).
///
/// - A usable checkpoint (this format, this source, ranges that fit its file) is trusted as far as
///   its durable ranges go, and only the rest is fetched. If the file changed on the server since
///   (another total), its old bytes are discarded and the download starts over.
/// - Without a usable checkpoint, the part file, which the caller says is the download's own, is
///   discarded and the download starts over (PO decision on #43).
/// - A paused download, or one that fails in a way worth retrying after a checkpoint was taken,
///   keeps its part file; any other failure removes it.
pub fn download_resumable(
    lp: &CurlLoop,
    url: &str,
    destination: &Path,
    options: DownloadOptions,
    resume: Option<&Resume>,
    control: &Control,
) -> Result<DownloadReport, DownloadError> {
    // symlink_metadata doesn't follow a link: a dangling link at the destination still counts.
    if fs::symlink_metadata(destination).is_ok() {
        return Err(DownloadError::FileExists);
    }
    let path = part_path(destination);
    let mut part = None;
    let mut from = None;
    if let Some(Resume::Checkpoint { checkpoint }) = resume {
        if let Some(durable) = trusted(checkpoint, url) {
            match files::open_part(&path) {
                Ok(file) => {
                    part = Some(PartFile::resumed(path.clone(), file, durable));
                    from = Some(checkpoint);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_error("open the part file", &path, &e)),
            }
        }
    }
    if part.is_none() && resume.is_some() {
        discard(&path)?;
    }
    let mut part = match part {
        Some(p) => p,
        None => PartFile::create(path)?,
    };

    let mut ck = Checkpointer::new(options.checkpoints, url);
    let mut resumed_bytes = 0;
    // One request until the ranges start: the probe, or the first lease of a resume.
    control.running(1, false);
    let mut outcome = match from {
        Some(checkpoint) => {
            resumed_bytes = part.coverage.len();
            ck.etag.clone_from(&checkpoint.etag);
            ck.last_modified.clone_from(&checkpoint.last_modified);
            ck.resumed = true;
            fetch_ranges(
                lp,
                url,
                checkpoint.total,
                options,
                &mut part,
                &mut ck,
                control,
            )
            .map(|(fetches, splits, mode)| Fetched {
                mode,
                total: checkpoint.total,
                fetches,
                splits,
            })
        }
        None => fetch_fresh(lp, url, options, &mut part, &mut ck, control),
    };
    if from.is_some() && matches!(outcome, Err(DownloadError::Changed(_))) {
        // The file changed on the server since the checkpoint: its old bytes are never mixed with
        // the new ones (report section 5.3).
        part.start_over()?;
        ck = Checkpointer::new(options.checkpoints, url);
        resumed_bytes = 0;
        outcome = fetch_fresh(lp, url, options, &mut part, &mut ck, control);
    }
    control.running(0, false);
    match outcome {
        Ok(fetched) => {
            let (written, wasted, sparse) = (part.written, part.wasted, part.sparse);
            part.finish(fetched.total, destination)?;
            Ok(DownloadReport {
                mode: fetched.mode,
                total: fetched.total,
                fetches: fetched.fetches,
                written_bytes: written,
                wasted_bytes: wasted,
                splits: fetched.splits,
                checkpoints: ck.sent,
                resumed_bytes,
                sparse,
                peak_buffered_bytes: control.peak_buffered.load(Ordering::Acquire),
                own_connections: control.own_connections.load(Ordering::Acquire),
            })
        }
        Err(e) => {
            let retryable = matches!(&e, DownloadError::Transfer(t) if t.retryable);
            if matches!(e, DownloadError::Paused) || (retryable && ck.sent > 0) {
                part.keep = true;
            }
            Err(e)
        }
    }
}

/// [`download`], followed and stoppable through `control`.
pub fn download_with(
    lp: &CurlLoop,
    url: &str,
    destination: &Path,
    options: DownloadOptions,
    control: &Control,
) -> Result<DownloadReport, DownloadError> {
    download_resumable(lp, url, destination, options, None, control)
}

/// How the file was fetched, before it is finished.
struct Fetched {
    mode: Mode,
    total: u64,
    fetches: u32,
    splits: u32,
}

/// A download from the start: the probe decides how the rest is fetched.
fn fetch_fresh(
    lp: &CurlLoop,
    url: &str,
    options: DownloadOptions,
    part: &mut PartFile,
    ck: &mut Checkpointer,
    control: &Control,
) -> Result<Fetched, DownloadError> {
    let probe_end = options.probe_bytes.max(1) - 1;
    let mut fetches = 1;
    let mut splits = 0;
    let retry = &options.retry;
    let request = FetchRequest::range(url.to_owned(), 0, Some(probe_end));
    let (probed, retries) = probe_retrying(lp, &request, probe_end, part, ck, control, retry)?;
    fetches += retries;
    let (outcome, cursor, validators) = match probed {
        Probed::Outcome(outcome, cursor, validators) => (outcome, cursor, validators),
        Probed::Paused(total, validators) => {
            return paused_in_probe(total, validators, part, ck, control)
        }
        // Report 5.1: a multipart answer to one range is retried once without Range.
        Probed::Multipart => {
            fetches += 1;
            let request = FetchRequest::get(url.to_owned());
            let (probed, retries) =
                probe_retrying(lp, &request, probe_end, part, ck, control, retry)?;
            fetches += retries;
            match probed {
                Probed::Outcome(outcome, cursor, validators) => (outcome, cursor, validators),
                Probed::Paused(total, validators) => {
                    return paused_in_probe(total, validators, part, ck, control)
                }
                Probed::Multipart => {
                    return Err(TransferError::new(
                        FailureClass::Permanent,
                        Some(206),
                        "the server answered multipart/byteranges even without a Range",
                    )
                    .into())
                }
            }
        }
    };
    (ck.etag, ck.last_modified) = validators;

    let (mode, total) = match outcome {
        ProbeOutcome::Empty => (Mode::Empty, 0),
        ProbeOutcome::SingleStream { length } => {
            if let Some(len) = length.filter(|len| *len != cursor) {
                return Err(TransferError::new(
                    FailureClass::Transient,
                    Some(200),
                    &format!("the stream ended after {cursor} of {len} bytes"),
                )
                .into());
            }
            (Mode::SingleStream, cursor)
        }
        ProbeOutcome::SequentialRanges { received_end } => {
            let (n, total, mode) =
                fetch_sequential(lp, url, received_end + 1, part, control, retry)?;
            fetches += n;
            (mode, total)
        }
        ProbeOutcome::RangesOk { total, .. } => {
            let (n, s, mode) = fetch_ranges(lp, url, total, options, part, ck, control)?;
            fetches += n;
            splits = s;
            (mode, total)
        }
    };
    Ok(Fetched {
        mode,
        total,
        fetches,
        splits,
    })
}

/// A pause while the probe ran. When the probe's answer gave the file's total, ranges work, so
/// its bytes are synced and checkpointed like any others; otherwise there is nothing to resume
/// from, and a resume starts over.
fn paused_in_probe(
    total: Option<u64>,
    validators: Validators,
    part: &mut PartFile,
    ck: &mut Checkpointer,
    control: &Control,
) -> Result<Fetched, DownloadError> {
    if let Some(total) = total {
        (ck.etag, ck.last_modified) = validators;
        ck.take_probe(part, total, control)?;
    }
    Err(DownloadError::Paused)
}

/// The durable ranges of `checkpoint`, if it can be trusted for `url`: this format, this source,
/// and ranges that are sorted, disjoint, non-empty and inside the file. A checkpoint comes back
/// from the app, so it is checked like any other input.
fn trusted(checkpoint: &Checkpoint, url: &str) -> Option<Coverage> {
    if checkpoint.format != CHECKPOINT_FORMAT
        || checkpoint.source != Checkpoint::source_of(url)
        || checkpoint.total == 0
    {
        return None;
    }
    let mut end = 0;
    for (i, &(a, b)) in checkpoint.durable.iter().enumerate() {
        if a >= b || b > checkpoint.total || (i > 0 && a < end) {
            return None;
        }
        end = b;
    }
    Some(Coverage::of(&checkpoint.durable))
}

/// Discards a part file the caller says is the download's own; there may be none. A link, or
/// anything that isn't a plain file, is refused and left alone.
fn discard(path: &Path) -> Result<(), DownloadError> {
    match files::remove_part(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("discard the part file", path, &e)),
    }
}

/// Takes checkpoints at the cadence (#43, report section 5.2): syncs the part file first, then
/// records the durable coverage, so a byte is in D only once it is on disk (invariant I5).
struct Checkpointer {
    cadence: Cadence,
    source: String,
    etag: Option<String>,
    last_modified: Option<String>,
    last_at: Instant,
    written_at_last: u64,
    sent: u32,
    /// The download resumed from a checkpoint: what it starts with is durable already.
    resumed: bool,
}

impl Checkpointer {
    fn new(cadence: Cadence, url: &str) -> Self {
        Checkpointer {
            cadence,
            source: Checkpoint::source_of(url),
            etag: None,
            last_modified: None,
            last_at: Instant::now(),
            written_at_last: 0,
            sent: 0,
            resumed: false,
        }
    }

    fn due(&self, part: &PartFile) -> bool {
        part.written.saturating_sub(self.written_at_last) >= self.cadence.bytes
            || self.last_at.elapsed() >= self.cadence.interval
    }

    /// Syncs the part file, marks what the scheduler covers durable, and sends the checkpoint.
    fn take(
        &mut self,
        part: &PartFile,
        sched: &mut Scheduler,
        control: &Control,
    ) -> Result<(), DownloadError> {
        part.sync()?;
        sched.synced();
        self.send(sched.total(), sched.durable(), part, control);
        Ok(())
    }

    /// During the probe, before a scheduler exists: everything written so far is the probe's own,
    /// at its place, so once synced it is all durable.
    fn take_probe(
        &mut self,
        part: &PartFile,
        total: u64,
        control: &Control,
    ) -> Result<(), DownloadError> {
        part.sync()?;
        self.send(total, &part.coverage, part, control);
        Ok(())
    }

    fn send(&mut self, total: u64, durable: &Coverage, part: &PartFile, control: &Control) {
        control.checkpoint(Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: self.source.clone(),
            total,
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
            durable: durable.intervals().to_vec(),
        });
        self.sent += 1;
        self.last_at = Instant::now();
        self.written_at_last = part.written;
    }
}

/// `<destination>.vdmpart`.
pub(crate) fn part_path(destination: &Path) -> PathBuf {
    let mut p = destination.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

/// An answer's validators: its ETag and Last-Modified, as sent.
type Validators = (Option<String>, Option<String>);

/// What the probe found.
enum Probed {
    /// Its outcome, how far its bytes reached, and its answer's validators.
    Outcome(ProbeOutcome, u64, Validators),
    /// A `multipart/byteranges` answer, aborted before any byte was written.
    Multipart,
    /// Paused while the probe ran; the file's total, if its answer gave one, and its validators.
    Paused(Option<u64>, Validators),
}

/// A failure worth retrying, and how long the server asked to wait, if it did.
struct Retryable {
    error: TransferError,
    retry_after: Option<Duration>,
}

/// `error` as a [`Retryable`] when it is worth retrying, or as the download's end otherwise.
fn retryable(
    error: TransferError,
    retry_after: &RetryAfter,
) -> Result<Result<Probed, Retryable>, DownloadError> {
    if error.retryable {
        Ok(Err(Retryable {
            error,
            retry_after: retry_after.get(),
        }))
    } else {
        Err(error.into())
    }
}

/// [`probe`], tried again after each failure worth retrying, until the policy gives up (#48).
/// Returns what it found and how many retries it made.
fn probe_retrying(
    lp: &CurlLoop,
    request: &FetchRequest,
    probe_end: u64,
    part: &mut PartFile,
    ck: &mut Checkpointer,
    control: &Control,
    policy: &RetryPolicy,
) -> Result<(Probed, u32), DownloadError> {
    let mut retrier = Retrier::new(*policy);
    let mut retries = 0;
    loop {
        match probe(lp, request.clone(), probe_end, part, ck, control, policy)? {
            Ok(probed) => return Ok((probed, retries)),
            Err(failed) => {
                let wait = retrier
                    .wait(&failed.error, failed.retry_after, part.written)
                    .ok_or(failed.error)?;
                wait_out(control, wait)?;
                retries += 1;
            }
        }
    }
}

/// Runs the probe, or its retry without `Range`, and writes the bytes it may keep. The outcome is
/// decided in the header callback, before the first byte. A failure worth retrying comes back as
/// [`Retryable`]; a probe whose answer gave the file's total and was then cut off returns its
/// outcome, and the range scheduler fetches the rest.
fn probe(
    lp: &CurlLoop,
    request: FetchRequest,
    probe_end: u64,
    part: &mut PartFile,
    ck: &mut Checkpointer,
    control: &Control,
    policy: &RetryPolicy,
) -> Result<Result<Probed, Retryable>, DownloadError> {
    let probe_verdict = Arc::new(Mutex::new(None));
    let multipart = Arc::new(AtomicBool::new(false));
    let validators: Arc<Mutex<Validators>> = Arc::new(Mutex::new((None, None)));
    let retry_after = RetryAfter::default();
    let (verdict, saw_multipart, seen, asked) = (
        Arc::clone(&probe_verdict),
        Arc::clone(&multipart),
        Arc::clone(&validators),
        retry_after.clone(),
    );
    let fetch = lp.fetch(
        request.limited(&control.limits),
        move |head: &ResponseHead| {
            asked.note(head);
            *seen.lock().expect("the validators lock") = (
                head.header("etag").map(str::to_owned),
                head.header("last-modified").map(str::to_owned),
            );
            if is_multipart(head) {
                saw_multipart.store(true, Ordering::Release);
                return Gate::Abort("a multipart/byteranges answer to one range".to_owned());
            }
            let result = classify_probe(head, probe_end);
            let gate = match &result {
                Ok(_) => Gate::Accept,
                Err(e) => Gate::Abort(e.message.clone()),
            };
            *verdict.lock().expect("the probe verdict lock") = Some(result);
            gate
        },
    );
    // The probe's bytes start at 0; how far they may go depends on its outcome.
    let mut cursor = 0u64;
    let drained = drain(&fetch, control, policy, |chunk| {
        let (limit, total) = match probe_verdict
            .lock()
            .expect("the probe verdict lock")
            .as_ref()
        {
            Some(Ok(ProbeOutcome::RangesOk {
                received_end,
                total,
            })) => (Some(received_end + 1), Some(*total)),
            Some(Ok(ProbeOutcome::SequentialRanges { received_end })) => {
                (Some(received_end + 1), None)
            }
            Some(Ok(ProbeOutcome::SingleStream { length })) => (None, *length),
            _ => (None, None),
        };
        part.accept(&mut cursor, limit, chunk)?;
        control.report(part.written, total);
        // With a total, ranges work, so the probe's bytes can be resumed from: checkpoint them.
        let total = match probe_verdict
            .lock()
            .expect("the probe verdict lock")
            .as_ref()
        {
            Some(Ok(ProbeOutcome::RangesOk { total, .. })) => Some(*total),
            _ => None,
        };
        if let (Some(total), true) = (total, ck.due(part)) {
            (ck.etag, ck.last_modified) = validators.lock().expect("the validators lock").clone();
            ck.take_probe(part, total, control)?;
        }
        Ok(())
    });
    if matches!(drained, Err(DownloadError::Paused)) {
        let total = match probe_verdict
            .lock()
            .expect("the probe verdict lock")
            .as_ref()
        {
            Some(Ok(ProbeOutcome::RangesOk { total, .. })) => Some(*total),
            _ => None,
        };
        let validators = validators.lock().expect("the validators lock").clone();
        return Ok(Ok(Probed::Paused(total, validators)));
    }
    let probe_done = drained?;
    if multipart.load(Ordering::Acquire) {
        return Ok(Ok(Probed::Multipart));
    }
    let outcome = match probe_verdict.lock().expect("the probe verdict lock").take() {
        Some(Ok(o)) => o,
        Some(Err(e)) => return retryable(e, &retry_after),
        None => {
            let e = probe_done.failure(policy).unwrap_or_else(loop_stopped);
            return retryable(e, &retry_after);
        }
    };
    if let Some(e) = probe_done.failure(policy) {
        // Cut off. With the file's total, the scheduler fetches what is missing; otherwise the
        // probe is tried again (it writes only the bytes not yet covered).
        if !matches!(outcome, ProbeOutcome::RangesOk { .. }) {
            return retryable(e, &retry_after);
        }
    }
    let validators = validators.lock().expect("the validators lock").clone();
    Ok(Ok(Probed::Outcome(outcome, cursor, validators)))
}

/// The wait a `429` or `503` asked for in `Retry-After`, noted in a header callback.
#[derive(Clone, Default)]
struct RetryAfter(Arc<Mutex<Option<Duration>>>);

impl RetryAfter {
    fn note(&self, head: &ResponseHead) {
        if matches!(head.status, 429 | 503) {
            *self.0.lock().expect("the Retry-After lock") =
                failure::retry_after(head, SystemTime::now());
        }
    }

    fn get(&self) -> Option<Duration> {
        *self.0.lock().expect("the Retry-After lock")
    }
}

/// Counts the failures in a row of one part of a download, and decides each wait (#48).
struct Retrier {
    policy: RetryPolicy,
    failures: u32,
    /// The bytes written at the last failure: more since then means progress.
    written: u64,
    rng: fastrand::Rng,
}

impl Retrier {
    fn new(policy: RetryPolicy) -> Self {
        Retrier {
            policy,
            failures: 0,
            written: 0,
            rng: fastrand::Rng::new(),
        }
    }

    /// The wait before retrying after `error`, with `written` bytes written so far, or `None` to
    /// give up. New bytes since the last failure start the count again.
    fn wait(
        &mut self,
        error: &TransferError,
        retry_after: Option<Duration>,
        written: u64,
    ) -> Option<Duration> {
        if !error.retryable {
            return None;
        }
        if written > self.written {
            self.written = written;
            self.failures = 0;
        }
        self.failures += 1;
        self.policy
            .wait(error.class, self.failures, retry_after, &mut self.rng)
    }
}

/// Waits `wait` before a retry, showing Retrying, and stops early for a pause or a cancel.
pub(crate) fn wait_out(control: &Control, wait: Duration) -> Result<(), DownloadError> {
    control.show(DownloadStatus::Retrying);
    let until = Instant::now() + wait;
    loop {
        if control.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        if control.is_paused() {
            return Err(DownloadError::Paused);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        thread::sleep(left.min(CANCEL_CHECK));
    }
}

fn loop_stopped() -> TransferError {
    fetch_failure(FetchError::LoopStopped)
}

/// What a request silent for [`RetryPolicy::reset_after`] fails with.
fn stall_error(policy: &RetryPolicy) -> TransferError {
    TransferError::new(
        FailureClass::Transient,
        None,
        &format!(
            "No data arrived for {} s, so the connection was reset.",
            policy.reset_after.as_secs_f32()
        ),
    )
}

/// How a download over HTTP/2 uses its streams (#47, report section 5.5): as streams of one
/// connection first; when more streams stop helping, connections of their own are tried once, and
/// the faster way is kept.
#[derive(Clone, Copy, Debug)]
enum Multiplex {
    /// Streams of one connection, and nothing else tried yet.
    First,
    /// Trying connections of their own; what streams of one achieved: the best speed and count.
    Trying { best: f64, count: u32 },
    /// Decided: `true` for connections of their own.
    Chosen(bool),
}

/// The request one lease is fetched with.
struct Active {
    /// Where the request's range starts, and where its next byte lands.
    start: u64,
    position: u64,
    /// The header callback's verdict: the last byte its answer gives, or why it was refused.
    verdict: SegmentVerdict,
    /// Lowers where the request stops, when its lease is split.
    limit: FetchLimit,
    /// The answer ran past its own `Content-Range` (counted once).
    overran: bool,
    /// What a `429` or `503` asked for.
    retry_after: RetryAfter,
    /// No byte has come for [`RetryPolicy::stalled_after`].
    quiet: bool,
    /// Stopped because the download now uses fewer connections (#47): it ends at its next write,
    /// and the rest of its lease is leased again.
    retiring: bool,
    /// When the request was made, to measure the round trip to its answer (#47).
    asked: Instant,
}

type SegmentVerdict = Arc<Mutex<Option<Result<u64, SegmentRejection>>>>;

/// What a segment's forwarder passes to the downloader.
enum Segment {
    Fetch(FetchEvent),
    /// No byte for [`RetryPolicy::stalled_after`].
    Quiet,
    /// No byte for [`RetryPolicy::reset_after`]: the request was cancelled.
    Reset,
}

/// The last byte the verdict's accepted answer gives.
fn answered_end(verdict: &SegmentVerdict) -> Option<u64> {
    match verdict.lock().expect("the segment verdict lock").as_ref() {
        Some(Ok(end)) => Some(*end),
        _ => None,
    }
}

/// Starts the request for `lease`, `Range: bytes=start-(end−1)`, checked in the header callback.
fn request(
    lp: &CurlLoop,
    base: &FetchRequest,
    total: u64,
    lease: Lease,
    tx: mpsc::SyncSender<(LeaseId, Segment)>,
    policy: &RetryPolicy,
    control: &Control,
) -> Active {
    let verdict: SegmentVerdict = Arc::new(Mutex::new(None));
    let v = Arc::clone(&verdict);
    let retry_after = RetryAfter::default();
    let asked = retry_after.clone();
    let (a, b) = (lease.start, lease.end - 1);
    let fetch_request = FetchRequest {
        range: Some((a, Some(b))),
        ..base.clone()
    }
    .limited(&control.limits);
    let fetch = lp.fetch(fetch_request, move |head: &ResponseHead| {
        asked.note(head);
        let result = check_segment(head, a, b, total, &Representation::default());
        let gate = match &result {
            Ok(_) => Gate::Accept,
            Err(SegmentRejection::RangesIgnored) => {
                Gate::Abort("a 200 for a range after byte 0".to_owned())
            }
            Err(
                SegmentRejection::Invalid(e)
                | SegmentRejection::Anomaly(e)
                | SegmentRejection::Changed(e),
            ) => Gate::Abort(e.message.clone()),
        };
        *v.lock().expect("the segment verdict lock") = Some(result);
        gate
    });
    let limit = fetch.limit();
    forward(
        fetch,
        lease.id,
        tx,
        policy,
        Arc::clone(&control.peak_buffered),
    );
    Active {
        start: a,
        position: a,
        verdict,
        limit,
        overran: false,
        retry_after,
        quiet: false,
        retiring: false,
        asked: Instant::now(),
    }
}

/// Fetches `0..total` around what the probe covered, with the range scheduler (#42, report
/// section 5.2):
///
/// - each free connection takes a lease, an equal share of the unassigned bytes;
/// - with nothing unassigned, a free connection splits the largest remainder, and the old request
///   stops at the new boundary;
/// - a short or cut-off answer gives the rest of its lease back, to be leased again under a new id;
/// - a `200` for a range, or a second anomaly, fetches the rest as one stream that writes only the
///   missing bytes (report section 5.1, steps 4, 6 and 7);
/// - a failure worth retrying, or a request silent for too long, frees its connection for a wait,
///   and the rest of its lease is leased again after it (#48). A throttling server halves the
///   connections this download uses;
/// - the connections asked for may change while it runs ([`Control::set_connections`], #47): more
///   take new leases at once, and fewer stop the newest requests, whose rest is leased again;
/// - with [`DownloadOptions::adaptive`], it starts with one connection and adds one per window
///   while each helps, up to what it may use ([`Ramp`], #47).
///
/// Returns the fetches made, the splits and the mode.
fn fetch_ranges(
    lp: &CurlLoop,
    url: &str,
    total: u64,
    options: DownloadOptions,
    part: &mut PartFile,
    ck: &mut Checkpointer,
    control: &Control,
) -> Result<(u32, u32, Mode), DownloadError> {
    let policy = &options.retry;
    // The most connections a throttling server leaves this download (#48).
    let mut throttled_to = u32::MAX;
    let mut sched = Scheduler::new(total, &part.coverage, options.leases);
    if ck.resumed {
        // What a resumed download starts with came from a checkpoint: it is on disk already.
        sched.synced();
    }
    // Bounded, so a slow writer stops the forwarders, which stops libcurl: an unbounded channel
    // here once queued a whole file in memory (#38). It is sized for the starting connections;
    // connections added later share it, which only holds them back sooner (#47).
    let (tx, rx) = mpsc::sync_channel(QUEUED_PER_SEGMENT * options.connections.max(1) as usize);
    let mut active: HashMap<LeaseId, Active> = HashMap::new();
    // Connections waiting out a retry wait, by when each may be used again.
    let mut waiting: Vec<Instant> = Vec::new();
    let mut retrier = Retrier::new(*policy);
    let (mut fetches, mut splits, mut anomalies) = (0u32, 0u32, 0u32);
    let mut ranges_ignored = false;
    let mut failure: Option<TransferError> = None;
    let mut changed: Option<TransferError> = None;
    let mut ramp = options.adaptive.map(Ramp::new);
    // Answers came over HTTP/2, and how its streams are used (#47).
    let mut h2 = false;
    let mut multiplex = Multiplex::First;
    // Body bytes received, written or not, and the shortest wait for an answer: the ramp's
    // measures (#47).
    let mut received = 0u64;
    let mut rtt: Option<Duration> = None;
    loop {
        let now = Instant::now();
        waiting.retain(|until| *until > now);
        if failure.is_some() || changed.is_some() {
            // The download ends; nothing waits to be retried.
            waiting.clear();
        }
        let mut live: Vec<LeaseId> = active
            .iter()
            .filter(|(_, a)| !a.retiring)
            .map(|(id, _)| *id)
            .collect();
        live.sort_unstable_by_key(|id| id.0);
        // What was asked for, within the engine's budget and what a throttling server left; with
        // adaptive connections, what helps within that.
        let ceiling = control
            .connections(options.connections)
            .min(control.allowed())
            .min(throttled_to);
        if let Some(r) = ramp.as_mut() {
            r.step(now, received, ceiling, live.len() as u32, rtt);
        }
        // HTTP/2: when more streams stop helping, connections of their own are tried once, and
        // the faster way is kept. Changing the way stops every request, to measure it cleanly.
        let mut restart = false;
        if let (Some(r), Some(adaptive), true) = (ramp.as_mut(), options.adaptive, h2) {
            match multiplex {
                Multiplex::First if r.why(ceiling) == Some(Tuning::NoGain) => {
                    multiplex = Multiplex::Trying {
                        best: r.best_rate().unwrap_or(0.0),
                        count: r.target(),
                    };
                    *r = Ramp::new(adaptive);
                    restart = true;
                }
                Multiplex::Trying { best, count } if r.settled(ceiling) => {
                    let enough = 1.0 + f64::from(adaptive.add_gain_percent) / 100.0;
                    let better = r.best_rate().is_some_and(|b| b > best * enough);
                    multiplex = Multiplex::Chosen(better);
                    control.own_connections.store(better, Ordering::Release);
                    if !better {
                        *r = Ramp::holding_at(adaptive, count);
                        restart = true;
                    }
                }
                _ => {}
            }
        }
        let connections = ramp.as_ref().map_or(ceiling, Ramp::target);
        control.tune(
            ramp.as_ref()
                .and_then(|r| r.why(ceiling).map(|why| (r.target(), why))),
        );
        // Fewer connections than run now: the newest requests stop.
        let keep = if restart { 0 } else { connections as usize };
        for id in live.iter().skip(keep) {
            let a = active.get_mut(id).expect("a live request");
            a.retiring = true;
            a.limit.stop_at(0);
        }
        // Fill the free connections, unless ranges stopped working for this download.
        let mut base = FetchRequest::get(url.to_owned());
        if matches!(
            multiplex,
            Multiplex::Trying { .. } | Multiplex::Chosen(true)
        ) {
            base = base.on_own_connection();
        }
        let mut file_bound = false;
        while failure.is_none()
            && changed.is_none()
            && !ranges_ignored
            && anomalies < MAX_ANOMALIES
            && ((active.len() + waiting.len()) as u32) < connections
        {
            let idle = connections - (active.len() + waiting.len()) as u32;
            let lease = match sched.assign(idle) {
                Some(Assignment::Fresh(lease)) => lease,
                Some(Assignment::Split { new, shrunk }) => {
                    // The old request stops at its lease's new end.
                    if let (Some(old), Some(lease)) = (active.get(&shrunk), sched.lease(shrunk)) {
                        old.limit.stop_at(lease.end - old.start);
                    }
                    splits += 1;
                    new
                }
                None => {
                    // Nothing unassigned, and no remainder large enough to split.
                    file_bound = true;
                    break;
                }
            };
            fetches += 1;
            active.insert(
                lease.id,
                request(lp, &base, total, lease, tx.clone(), policy, control),
            );
        }
        control.running(active.len() as u32, file_bound);
        if active.is_empty() {
            if waiting.is_empty() {
                break;
            }
            control.show(DownloadStatus::Retrying);
        }
        if control.is_cancelled() {
            // Dropping `rx` ends the forwarders, and dropping their fetches cancels them.
            return Err(DownloadError::Cancelled);
        }
        if control.is_paused() {
            // What was written so far is synced and recorded; the requests stop as `rx` goes.
            ck.take(part, &mut sched, control)?;
            return Err(DownloadError::Paused);
        }
        let (id, event) = match rx.recv_timeout(CANCEL_CHECK) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // A failure worth retrying frees its connection for a wait; otherwise the download fails.
        let mut retry_later =
            |t: TransferError, retry_after: Option<Duration>, written: u64| match retrier.wait(
                &t,
                retry_after,
                written,
            ) {
                Some(wait) => {
                    waiting.push(Instant::now() + wait);
                    if t.class == FailureClass::Throttled {
                        throttled_to = retry::after_throttling(connections);
                        control.throttled_to.store(throttled_to, Ordering::Release);
                    }
                }
                None => failure = failure.take().or(Some(t)),
            };
        match event {
            Segment::Quiet => {
                if let Some(a) = active.get_mut(&id) {
                    a.quiet = true;
                }
                // Stalled only when no connection brings bytes; a slow writer never counts, as
                // the forwarders measure time spent waiting for the network alone.
                if active.values().all(|a| a.quiet) {
                    control.show(DownloadStatus::Stalled);
                }
            }
            Segment::Reset => {
                // The forwarder gave up on the request and cancelled it; its lease is ended, and
                // the rest of it is leased again after the wait.
                if let Some(a) = active.remove(&id) {
                    sched.ended(id);
                    if !a.retiring {
                        retry_later(stall_error(policy), None, part.written);
                    }
                }
            }
            Segment::Fetch(FetchEvent::Head(head)) => {
                h2 |= head.http2;
                if let Some(a) = active.get(&id) {
                    let waited = a.asked.elapsed();
                    rtt = Some(rtt.map_or(waited, |r| r.min(waited)));
                    if answered_end(&a.verdict).is_some() {
                        sched.start(id);
                    }
                }
            }
            Segment::Fetch(FetchEvent::Data(chunk)) => {
                received += chunk.len() as u64;
                let Some(a) = active.get_mut(&id) else {
                    continue;
                };
                a.quiet = false;
                let offset = a.position;
                a.position += chunk.len() as u64;
                let accepted =
                    sched.accept(id, offset, &chunk, |at, bytes| part.write_owned(at, bytes))?;
                part.wasted += accepted.wasted;
                // Bytes past the end its own Content-Range gave: the server overran its answer.
                let overran = answered_end(&a.verdict).is_some_and(|end| a.position > end + 1);
                if overran && !a.overran {
                    a.overran = true;
                    anomalies += 1;
                }
                control.report(part.written, Some(total));
                if ck.due(part) {
                    ck.take(part, &mut sched, control)?;
                }
            }
            Segment::Fetch(FetchEvent::Done(result)) => {
                let Some(a) = active.remove(&id) else {
                    continue;
                };
                let verdict = a.verdict.lock().expect("the segment verdict lock").take();
                let unfinished = sched.lease(id).is_some_and(|l| l.cursor < l.end);
                if a.retiring {
                    // Stopped on purpose: only a changed file still matters.
                    if let Some(Err(SegmentRejection::Changed(t))) = verdict {
                        changed = changed.or(Some(t));
                    }
                    sched.ended(id);
                    continue;
                }
                match (verdict, result) {
                    (Some(Err(SegmentRejection::RangesIgnored)), _) => ranges_ignored = true,
                    (Some(Err(SegmentRejection::Anomaly(_))), _) => anomalies += 1,
                    (Some(Err(SegmentRejection::Invalid(t))), _) => {
                        retry_later(t, a.retry_after.get(), part.written)
                    }
                    (Some(Err(SegmentRejection::Changed(t))), _) => changed = changed.or(Some(t)),
                    // Accepted, then cut off: the rest of the lease is leased again.
                    (Some(Ok(_)), Err(_)) => anomalies += 1,
                    // Complete by its own account, but it stopped before the end its Content-Range
                    // gave (a chunked body that ended early). One stopped at its lease's end, or a
                    // shorter but honest 206, is not an anomaly.
                    (Some(Ok(end)), Ok(())) => {
                        if unfinished && a.position <= end {
                            anomalies += 1;
                        }
                    }
                    // Never answered: the connection itself failed.
                    (None, Err(e)) => retry_later(fetch_failure(e), None, part.written),
                    (None, Ok(())) => {}
                }
                sched.ended(id);
            }
        }
    }
    // A changed file explains any other failure seen alongside it.
    if let Some(t) = changed {
        return Err(DownloadError::Changed(t));
    }
    if let Some(e) = failure {
        return Err(e.into());
    }
    if sched.is_complete() {
        return Ok((fetches, splits, Mode::Ranges));
    }
    if ranges_ignored || anomalies >= MAX_ANOMALIES {
        let n = stream_gaps(lp, url, Some(total), part, control, policy)?.1;
        return Ok((fetches + n, splits, Mode::RangesLost));
    }
    // C, the live leases and the unassigned bytes always make up the file, so with no lease left
    // and nothing to assign, the file is covered. Reaching this is a bug.
    Err(TransferError::new(
        FailureClass::Transient,
        None,
        "the range scheduler stopped before the file was covered",
    )
    .into())
}

/// Fetches the rest of a file whose total the server doesn't give: one open range at a time from
/// the first missing byte, until a `416` says the file ends there. A failure worth retrying asks
/// for the same range again after a wait (#48). Returns the fetches made, the file's length and
/// the mode.
fn fetch_sequential(
    lp: &CurlLoop,
    url: &str,
    start: u64,
    part: &mut PartFile,
    control: &Control,
    policy: &RetryPolicy,
) -> Result<(u32, u64, Mode), DownloadError> {
    control.running(1, false);
    let mut fetches = 0;
    let mut anomalies = 0;
    let mut cursor = start;
    let mut retrier = Retrier::new(*policy);
    loop {
        fetches += 1;
        let from = cursor;
        let verdict = Arc::new(Mutex::new(None));
        let v = Arc::clone(&verdict);
        let retry_after = RetryAfter::default();
        let asked = retry_after.clone();
        let fetch = lp.fetch(
            FetchRequest::range(url.to_owned(), from, None).limited(&control.limits),
            move |head: &ResponseHead| {
                asked.note(head);
                let result = check_continuation(head, from, &Representation::default());
                let gate = match &result {
                    Ok(_) => Gate::Accept,
                    Err(SegmentRejection::RangesIgnored) => {
                        Gate::Abort("a 200 for a range after byte 0".to_owned())
                    }
                    Err(
                        SegmentRejection::Invalid(e)
                        | SegmentRejection::Anomaly(e)
                        | SegmentRejection::Changed(e),
                    ) => Gate::Abort(e.message.clone()),
                };
                *v.lock().expect("the continuation verdict lock") = Some(result);
                gate
            },
        );
        let before = part.written;
        let mut overran = false;
        let done = drain(&fetch, control, policy, |chunk| {
            let limit = match verdict
                .lock()
                .expect("the continuation verdict lock")
                .as_ref()
            {
                Some(Ok(Continuation::More { end, .. })) => end + 1,
                _ => from,
            };
            overran |= part.accept(&mut cursor, Some(limit), chunk)? > 0;
            control.report(part.written, None);
            Ok(())
        })?;
        let result = verdict
            .lock()
            .expect("the continuation verdict lock")
            .take();
        // A failure worth retrying, after a wait; or the download's end.
        let mut retry = |t: TransferError, retry_after: Option<Duration>| {
            let wait = retrier.wait(&t, retry_after, part.written).ok_or(t)?;
            wait_out(control, wait)
        };
        match (result, done) {
            (Some(Ok(Continuation::End)), _) => {
                return Ok((fetches, cursor, Mode::SequentialRanges))
            }
            // A stall isn't a sign that ranges fail: the rest is asked for again.
            (Some(Ok(Continuation::More { .. })), Drained::Reset) => {
                retry(stall_error(policy), None)?;
                continue;
            }
            (Some(Ok(Continuation::More { end, total })), done) => {
                if total == Some(cursor) {
                    return Ok((fetches, cursor, Mode::SequentialRanges));
                }
                if !matches!(done, Drained::Complete) || cursor <= end || overran {
                    anomalies += 1;
                }
            }
            (Some(Err(SegmentRejection::RangesIgnored)), _) => {
                let (length, n) = stream_gaps(lp, url, None, part, control, policy)?;
                return Ok((fetches + n, length, Mode::RangesLost));
            }
            (Some(Err(SegmentRejection::Anomaly(_))), _) => anomalies += 1,
            (Some(Err(SegmentRejection::Invalid(t))), _) => {
                retry(t, retry_after.get())?;
                continue;
            }
            (Some(Err(SegmentRejection::Changed(t))), _) => return Err(DownloadError::Changed(t)),
            // Never answered: the connection itself failed.
            (None, done) => {
                retry(done.failure(policy).unwrap_or_else(loop_stopped), None)?;
                continue;
            }
        }
        if part.written == before {
            anomalies += 1;
        }
        if anomalies >= MAX_ANOMALIES {
            let (length, n) = stream_gaps(lp, url, None, part, control, policy)?;
            return Ok((fetches + n, length, Mode::RangesLost));
        }
    }
}

/// Fetches the whole file as one stream and writes only the bytes not yet covered. With a known
/// `total`, the stream must be that long. A failure worth retrying starts the stream again after
/// a wait (#48). Returns the stream's length and the fetches made.
fn stream_gaps(
    lp: &CurlLoop,
    url: &str,
    total: Option<u64>,
    part: &mut PartFile,
    control: &Control,
    policy: &RetryPolicy,
) -> Result<(u64, u32), DownloadError> {
    control.running(1, false);
    let mut retrier = Retrier::new(*policy);
    let mut fetches = 0;
    loop {
        fetches += 1;
        let refusal = Arc::new(Mutex::new(None));
        let r = Arc::clone(&refusal);
        let retry_after = RetryAfter::default();
        let asked = retry_after.clone();
        let fetch = lp.fetch(
            FetchRequest::get(url.to_owned()).limited(&control.limits),
            move |head: &ResponseHead| {
                asked.note(head);
                let length = head
                    .header("content-length")
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let identity = head
                    .header("content-encoding")
                    .is_none_or(|e| e.trim() == "identity");
                if head.status == 200 && identity && (total.is_none() || length == total) {
                    return Gate::Accept;
                }
                // An error status keeps its own class; any other answer isn't the same file.
                let e = failure::status_failure(head.status).unwrap_or_else(|| {
                    TransferError::new(
                        FailureClass::Permanent,
                        Some(head.status),
                        &format!(
                            "the single stream isn't the same file (status {}, length {length:?}, expected {total:?})",
                            head.status
                        ),
                    )
                });
                let msg = e.message.clone();
                *r.lock().expect("the refusal lock") = Some(e);
                Gate::Abort(msg)
            },
        );
        let mut offset = 0u64;
        let done = drain(&fetch, control, policy, |chunk| {
            part.accept_gaps(offset, chunk)?;
            offset += chunk.len() as u64;
            control.report(part.written, total);
            Ok(())
        })?;
        let failed = match refusal.lock().expect("the refusal lock").take() {
            Some(e) => e,
            None => match done.failure(policy) {
                None => return Ok((offset, fetches)),
                Some(e) => e,
            },
        };
        let wait = retrier
            .wait(&failed, retry_after.get(), part.written)
            .ok_or(failed)?;
        wait_out(control, wait)?;
    }
}

/// Moves a fetch's events to `tx`, tagged with `tag`, on its own thread. It measures the time the
/// fetch brings nothing (#48): after [`RetryPolicy::stalled_after`] it sends [`Segment::Quiet`],
/// after [`RetryPolicy::reset_after`] it cancels the fetch and sends [`Segment::Reset`]. Time
/// spent waiting for room in `tx` (a slow writer) doesn't count, as the clock runs only while it
/// waits for the fetch.
fn forward<T: Copy + Send + 'static>(
    fetch: Fetch,
    tag: T,
    tx: mpsc::SyncSender<(T, Segment)>,
    policy: &RetryPolicy,
    peak: Arc<AtomicU64>,
) {
    let (stalled, reset) = (policy.stalled_after, policy.reset_after);
    thread::spawn(move || {
        let mut quiet = false;
        loop {
            let wait = if quiet {
                reset.saturating_sub(stalled)
            } else {
                stalled
            };
            let segment = match fetch.next_event(wait) {
                Some(event) => {
                    quiet = false;
                    Segment::Fetch(event)
                }
                None if !quiet => {
                    quiet = true;
                    Segment::Quiet
                }
                None => Segment::Reset,
            };
            let last = matches!(
                segment,
                Segment::Fetch(FetchEvent::Done(_)) | Segment::Reset
            );
            if last {
                // Recorded before the downloader can see the end.
                note_peak(&peak, &fetch);
            }
            if tx.send((tag, segment)).is_err() || last {
                // Dropping an unfinished Fetch cancels it.
                return;
            }
        }
    });
}

fn note_peak(peak: &AtomicU64, fetch: &Fetch) {
    peak.fetch_max(fetch.peak_buffered_bytes() as u64, Ordering::AcqRel);
}

/// How a fetch read by [`drain`] ended.
enum Drained {
    Complete,
    Failed(FetchError),
    /// Silent for [`RetryPolicy::reset_after`]; cancelled.
    Reset,
}

impl Drained {
    /// The failure it stands for, if it didn't complete.
    fn failure(self, policy: &RetryPolicy) -> Option<TransferError> {
        match self {
            Drained::Complete => None,
            Drained::Failed(e) => Some(fetch_failure(e)),
            Drained::Reset => Some(stall_error(policy)),
        }
    }
}

/// Reads one fetch to its end, handing each body chunk to `on_data`. Checks for cancellation
/// while it waits. Silence is measured from the end of the last `on_data`, so time spent writing
/// never counts as a stall: after [`RetryPolicy::stalled_after`] the download shows Stalled, and
/// after [`RetryPolicy::reset_after`] the fetch is cancelled (#48).
fn drain(
    fetch: &Fetch,
    control: &Control,
    policy: &RetryPolicy,
    on_data: impl FnMut(&[u8]) -> Result<(), DownloadError>,
) -> Result<Drained, DownloadError> {
    let ended = drain_events(fetch, control, policy, on_data);
    control.note_buffered(fetch);
    ended
}

/// [`drain`]'s loop.
fn drain_events(
    fetch: &Fetch,
    control: &Control,
    policy: &RetryPolicy,
    mut on_data: impl FnMut(&[u8]) -> Result<(), DownloadError>,
) -> Result<Drained, DownloadError> {
    let mut last = Instant::now();
    loop {
        if control.is_cancelled() {
            fetch.cancel();
            return Err(DownloadError::Cancelled);
        }
        if control.is_paused() {
            fetch.cancel();
            return Err(DownloadError::Paused);
        }
        let Some(event) = fetch.next_event(CANCEL_CHECK) else {
            let quiet = last.elapsed();
            if quiet >= policy.reset_after {
                fetch.cancel();
                return Ok(Drained::Reset);
            }
            if quiet >= policy.stalled_after {
                control.show(DownloadStatus::Stalled);
            }
            continue;
        };
        match event {
            FetchEvent::Head(_) => {}
            FetchEvent::Data(chunk) => on_data(&chunk)?,
            FetchEvent::Done(Ok(())) => return Ok(Drained::Complete),
            FetchEvent::Done(Err(e)) => return Ok(Drained::Failed(e)),
        }
        last = Instant::now();
    }
}

fn io_error(what: &str, path: &Path, e: &io::Error) -> DownloadError {
    let detail = format!("couldn't {what} {}: {e}", path.display());
    DownloadError::Io(match failure::disk_message(e) {
        Some(message) => format!("{message} ({detail})"),
        None => detail,
    })
}

/// The part file this download owns: removed on drop unless it was finished.
/// Creates the part file at `path`, which must not exist yet.
fn new_part(path: &Path) -> Result<files::NewPart, DownloadError> {
    files::create_part(path).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => DownloadError::Io(format!(
            "{} already exists; it may belong to another download",
            path.display()
        )),
        _ => io_error("create the part file", path, &e),
    })
}

struct PartFile {
    path: PathBuf,
    file: Option<File>,
    coverage: Coverage,
    written: u64,
    wasted: u64,
    keep: bool,
    /// This run created it sparse (`files::create_part`).
    sparse: bool,
}

impl PartFile {
    /// A new part file at `path`, sparse on Windows. From here on it is the download's own:
    /// removed on a failure, renamed on success.
    fn create(path: PathBuf) -> Result<Self, DownloadError> {
        let new = new_part(&path)?;
        Ok(PartFile {
            path,
            file: Some(new.file),
            coverage: Coverage::default(),
            written: 0,
            wasted: 0,
            keep: false,
            sparse: new.sparse,
        })
    }

    /// The download's own part file, reopened to resume: only `durable` is trusted.
    fn resumed(path: PathBuf, file: File, durable: Coverage) -> Self {
        PartFile {
            path,
            file: Some(file),
            coverage: durable,
            written: 0,
            wasted: 0,
            keep: false,
            sparse: false,
        }
    }

    /// Discards what is in the part file and starts again with an empty one.
    fn start_over(&mut self) -> Result<(), DownloadError> {
        drop(self.file.take());
        discard(&self.path)?;
        let new = new_part(&self.path)?;
        self.file = Some(new.file);
        self.coverage = Coverage::default();
        self.written = 0;
        self.sparse = new.sparse;
        Ok(())
    }

    /// Syncs what was written to disk.
    fn sync(&self) -> Result<(), DownloadError> {
        self.file
            .as_ref()
            .expect("the part file is open")
            .sync_data()
            .map_err(|e| io_error("sync", &self.path, &e))
    }

    /// Writes `chunk` at `*cursor`, up to the exclusive `limit`, and advances the cursor. Bytes past
    /// the limit, or already covered, are counted as wasted. Returns how many were past the limit.
    fn accept(
        &mut self,
        cursor: &mut u64,
        limit: Option<u64>,
        chunk: &[u8],
    ) -> Result<u64, DownloadError> {
        let start = *cursor;
        let mut take = chunk.len() as u64;
        if let Some(limit) = limit {
            take = take.min(limit.saturating_sub(start));
        }
        self.accept_gaps(start, &chunk[..take as usize])?;
        let excess = chunk.len() as u64 - take;
        self.wasted += excess;
        *cursor += take;
        Ok(excess)
    }

    /// Writes bytes the range scheduler accepted at `offset`. They count as covered once the write
    /// returns.
    fn write_owned(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        write_at(
            self.file.as_ref().expect("the part file is open"),
            bytes,
            offset,
        )
        .map_err(|e| io_error("write to", &self.path, &e))?;
        self.coverage.insert(offset, offset + bytes.len() as u64);
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Writes the parts of `chunk` (which starts at `offset`) that aren't covered yet.
    fn accept_gaps(&mut self, offset: u64, chunk: &[u8]) -> Result<(), DownloadError> {
        let end = offset + chunk.len() as u64;
        let gaps = self.coverage.gaps(offset, end);
        let fresh: u64 = gaps.iter().map(|(a, b)| b - a).sum();
        for (a, b) in gaps {
            let slice = &chunk[(a - offset) as usize..(b - offset) as usize];
            write_at(self.file.as_ref().expect("the part file is open"), slice, a)
                .map_err(|e| io_error("write to", &self.path, &e))?;
            self.coverage.insert(a, b);
        }
        self.written += fresh;
        self.wasted += chunk.len() as u64 - fresh;
        Ok(())
    }

    /// Checks coverage, syncs the file to disk, and links it to `destination` without replacing an
    /// existing file. Removes the part file either way.
    fn finish(mut self, total: u64, destination: &Path) -> Result<(), DownloadError> {
        if !self.coverage.covers(0, total) {
            return Err(TransferError::new(
                FailureClass::Transient,
                None,
                &format!("only {} of {total} bytes arrived", self.written),
            )
            .into());
        }
        let file = self.file.take().expect("the part file is open");
        file.set_len(total)
            .map_err(|e| io_error("size", &self.path, &e))?;
        file.sync_all()
            .map_err(|e| io_error("sync", &self.path, &e))?;
        drop(file);
        // An existing file is never replaced, whatever appeared there during the download, and a
        // crash before this leaves only the part file: nothing that looks finished (#43).
        match files::rename_no_replace(&self.path, destination) {
            Ok(()) => {
                // The part file is the destination now; this guard is done.
                self.keep = true;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(DownloadError::FileExists),
            Err(e) => Err(io_error("move the finished file to", destination, &e)),
        }
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if !self.keep {
            drop(self.file.take());
            // Only the part file this download created is removed, never through a link. A
            // failure here leaves it behind, and the next download refuses to reuse it.
            let _ = files::remove_part(&self.path);
        }
    }
}

#[cfg(windows)]
fn write_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "the disk accepted no bytes",
            ));
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}
