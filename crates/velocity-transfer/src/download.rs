//! The downloader (#37, #41): one file over HTTP(S), with a probe that is also the first segment
//! and decides between ranges, sequential ranges and one stream (docs/ENGINE_INTERNALS.md), static
//! parallel segments when ranges work, and a finish that never replaces an existing file. What it
//! doesn't do yet is the rest of R5: the lease scheduler (#42), retries (#48), checkpoints and
//! resume (#43), validators (#45), whole-file hashes (#46) and adaptive connections (#47).
//!
//! The byte rules (`.claude/rules/transfer-engine.md`, research report sections 5.1 and 5.2):
//! - a byte is accepted only from the segment that owns it, at its own offset, once;
//! - a response that isn't exactly what was asked for is aborted before any body byte is written;
//! - every received byte is either written or counted as wasted;
//! - the file is renamed into place only after its coverage is complete and synced to disk.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use velocity_core::domain::{FailureClass, TransferError};
use velocity_core::transfer::{Checkpoint, Resume, CHECKPOINT_FORMAT};
use velocity_curl::{
    CurlLoop, Fetch, FetchError, FetchEvent, FetchLimit, FetchRequest, Gate, ResponseHead,
};

use crate::files;
use crate::probe::{
    check_continuation, check_segment, classify_probe, is_multipart, Continuation, ProbeOutcome,
    SegmentRejection,
};
use crate::scheduler::{Assignment, Coverage, Lease, LeaseId, Scheduler, Sizes};

/// How long a fetch may stay silent before the prototype gives up on it. #48 replaces this with
/// stall detection.
const SILENCE_LIMIT: Duration = Duration::from_secs(60);
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

/// Lets a caller follow, pause and stop a running download (the engine, #37, #43).
#[derive(Default)]
pub struct Control {
    cancelled: AtomicBool,
    paused: AtomicBool,
    progress: Option<ProgressFn>,
    checkpoints: Option<CheckpointFn>,
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

    fn report(&self, written: u64, total: Option<u64>) {
        if let Some(p) = &self.progress {
            p(written, total);
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
    let mut outcome = match from {
        Some(checkpoint) => {
            resumed_bytes = part.coverage.len();
            ck.etag.clone_from(&checkpoint.etag);
            ck.last_modified.clone_from(&checkpoint.last_modified);
            let connections = options.connections.max(1);
            fetch_ranges(
                lp,
                url,
                checkpoint.total,
                connections,
                options.leases,
                &mut part,
                &mut ck,
                control,
                true,
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
    let request = FetchRequest::range(url.to_owned(), 0, Some(probe_end));
    let (outcome, cursor, validators) = match probe(lp, request, probe_end, part, control)? {
        Probed::Outcome(outcome, cursor, validators) => (outcome, cursor, validators),
        // Report 5.1: a multipart answer to one range is retried once without Range.
        Probed::Multipart => {
            fetches += 1;
            let request = FetchRequest::get(url.to_owned());
            match probe(lp, request, probe_end, part, control)? {
                Probed::Outcome(outcome, cursor, validators) => (outcome, cursor, validators),
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
            let (n, total, mode) = fetch_sequential(lp, url, received_end + 1, part, control)?;
            fetches += n;
            (mode, total)
        }
        ProbeOutcome::RangesOk { total, .. } => {
            let connections = options.connections.max(1);
            let (n, s, mode) = fetch_ranges(
                lp,
                url,
                total,
                connections,
                options.leases,
                part,
                ck,
                control,
                false,
            )?;
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
        }
    }

    fn due(&self, part: &PartFile) -> bool {
        part.written.saturating_sub(self.written_at_last) >= self.cadence.bytes
            || self.last_at.elapsed() >= self.cadence.interval
    }

    fn take(
        &mut self,
        part: &PartFile,
        sched: &mut Scheduler,
        control: &Control,
    ) -> Result<(), DownloadError> {
        part.sync()?;
        sched.synced();
        control.checkpoint(Checkpoint {
            format: CHECKPOINT_FORMAT,
            source: self.source.clone(),
            total: sched.total(),
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
            durable: sched.durable().intervals().to_vec(),
        });
        self.sent += 1;
        self.last_at = Instant::now();
        self.written_at_last = part.written;
        Ok(())
    }
}

/// `<destination>.vdmpart`.
fn part_path(destination: &Path) -> PathBuf {
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
}

/// Runs the probe, or its retry without `Range`, and writes the bytes it may keep. The outcome is
/// decided in the header callback, before the first byte.
fn probe(
    lp: &CurlLoop,
    request: FetchRequest,
    probe_end: u64,
    part: &mut PartFile,
    control: &Control,
) -> Result<Probed, DownloadError> {
    let probe_verdict = Arc::new(Mutex::new(None));
    let multipart = Arc::new(AtomicBool::new(false));
    let validators: Arc<Mutex<Validators>> = Arc::new(Mutex::new((None, None)));
    let (verdict, saw_multipart, seen) = (
        Arc::clone(&probe_verdict),
        Arc::clone(&multipart),
        Arc::clone(&validators),
    );
    let fetch = lp.fetch(request, move |head: &ResponseHead| {
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
    });
    // The probe's bytes start at 0; how far they may go depends on its outcome.
    let mut cursor = 0u64;
    let probe_done = drain(&fetch, control, |chunk| {
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
        Ok(())
    })?;
    if multipart.load(Ordering::Acquire) {
        return Ok(Probed::Multipart);
    }
    let outcome = match probe_verdict.lock().expect("the probe verdict lock").take() {
        Some(Ok(o)) => o,
        Some(Err(e)) => return Err(e.into()),
        None => {
            return Err(fetch_failure(probe_done.err().unwrap_or(FetchError::LoopStopped)).into())
        }
    };
    if let Err(e) = probe_done {
        return Err(fetch_failure(e).into());
    }
    let validators = validators.lock().expect("the validators lock").clone();
    Ok(Probed::Outcome(outcome, cursor, validators))
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
}

type SegmentVerdict = Arc<Mutex<Option<Result<u64, SegmentRejection>>>>;

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
    url: &str,
    total: u64,
    lease: Lease,
    tx: mpsc::SyncSender<(LeaseId, FetchEvent)>,
) -> Active {
    let verdict: SegmentVerdict = Arc::new(Mutex::new(None));
    let v = Arc::clone(&verdict);
    let (a, b) = (lease.start, lease.end - 1);
    let fetch = lp.fetch(
        FetchRequest::range(url.to_owned(), a, Some(b)),
        move |head: &ResponseHead| {
            let result = check_segment(head, a, b, total);
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
        },
    );
    let limit = fetch.limit();
    forward(fetch, lease.id, tx);
    Active {
        start: a,
        position: a,
        verdict,
        limit,
        overran: false,
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
///   missing bytes (report section 5.1, steps 4, 6 and 7).
///
/// Returns the fetches made, the splits and the mode.
fn fetch_ranges(
    lp: &CurlLoop,
    url: &str,
    total: u64,
    connections: u32,
    sizes: Sizes,
    part: &mut PartFile,
    ck: &mut Checkpointer,
    control: &Control,
    resumed: bool,
) -> Result<(u32, u32, Mode), DownloadError> {
    let mut sched = Scheduler::new(total, &part.coverage, sizes);
    if resumed {
        // What a resumed download starts with came from a checkpoint: it is on disk already.
        sched.synced();
    }
    // Bounded, so a slow writer stops the forwarders, which stops libcurl: an unbounded channel
    // here once queued a whole file in memory (#38).
    let (tx, rx) = mpsc::sync_channel(QUEUED_PER_SEGMENT * connections as usize);
    let mut active: HashMap<LeaseId, Active> = HashMap::new();
    let (mut fetches, mut splits, mut anomalies) = (0u32, 0u32, 0u32);
    let mut ranges_ignored = false;
    let mut failure: Option<TransferError> = None;
    let mut changed: Option<TransferError> = None;
    loop {
        // Fill the free connections, unless ranges stopped working for this download.
        while failure.is_none()
            && changed.is_none()
            && !ranges_ignored
            && anomalies < MAX_ANOMALIES
            && (active.len() as u32) < connections
        {
            let idle = connections - active.len() as u32;
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
                None => break,
            };
            fetches += 1;
            active.insert(lease.id, request(lp, url, total, lease, tx.clone()));
        }
        if active.is_empty() {
            break;
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
        match event {
            FetchEvent::Head(_) => {
                if active
                    .get(&id)
                    .is_some_and(|a| answered_end(&a.verdict).is_some())
                {
                    sched.start(id);
                }
            }
            FetchEvent::Data(chunk) => {
                let Some(a) = active.get_mut(&id) else {
                    continue;
                };
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
            FetchEvent::Done(result) => {
                let Some(a) = active.remove(&id) else {
                    continue;
                };
                let verdict = a.verdict.lock().expect("the segment verdict lock").take();
                let unfinished = sched.lease(id).is_some_and(|l| l.cursor < l.end);
                match (verdict, result) {
                    (Some(Err(SegmentRejection::RangesIgnored)), _) => ranges_ignored = true,
                    (Some(Err(SegmentRejection::Anomaly(_))), _) => anomalies += 1,
                    (Some(Err(SegmentRejection::Invalid(t))), _) => failure = failure.or(Some(t)),
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
                    // Never answered: the connection itself failed. Retries are #48.
                    (None, Err(e)) => failure = failure.or(Some(fetch_failure(e))),
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
        stream_gaps(lp, url, Some(total), part, control)?;
        return Ok((fetches + 1, splits, Mode::RangesLost));
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
/// the first missing byte, until a `416` says the file ends there. Returns the fetches made, the
/// file's length and the mode.
fn fetch_sequential(
    lp: &CurlLoop,
    url: &str,
    start: u64,
    part: &mut PartFile,
    control: &Control,
) -> Result<(u32, u64, Mode), DownloadError> {
    let mut fetches = 0;
    let mut anomalies = 0;
    let mut cursor = start;
    loop {
        fetches += 1;
        let from = cursor;
        let verdict = Arc::new(Mutex::new(None));
        let v = Arc::clone(&verdict);
        let fetch = lp.fetch(
            FetchRequest::range(url.to_owned(), from, None),
            move |head: &ResponseHead| {
                let result = check_continuation(head, from);
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
        let done = drain(&fetch, control, |chunk| {
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
        match result {
            Some(Ok(Continuation::End)) => return Ok((fetches, cursor, Mode::SequentialRanges)),
            Some(Ok(Continuation::More { end, total })) => {
                if total == Some(cursor) {
                    return Ok((fetches, cursor, Mode::SequentialRanges));
                }
                if done.is_err() || cursor <= end || overran {
                    anomalies += 1;
                }
            }
            Some(Err(SegmentRejection::RangesIgnored)) => {
                fetches += 1;
                let length = stream_gaps(lp, url, None, part, control)?;
                return Ok((fetches, length, Mode::RangesLost));
            }
            Some(Err(SegmentRejection::Anomaly(_))) => anomalies += 1,
            Some(Err(SegmentRejection::Invalid(t))) => return Err(t.into()),
            Some(Err(SegmentRejection::Changed(t))) => return Err(DownloadError::Changed(t)),
            None => return Err(fetch_failure(done.err().unwrap_or(FetchError::LoopStopped)).into()),
        }
        if part.written == before {
            anomalies += 1;
        }
        if anomalies >= MAX_ANOMALIES {
            fetches += 1;
            let length = stream_gaps(lp, url, None, part, control)?;
            return Ok((fetches, length, Mode::RangesLost));
        }
    }
}

/// Fetches the whole file as one stream and writes only the bytes not yet covered. With a known
/// `total`, the stream must be that long. Returns the stream's length.
fn stream_gaps(
    lp: &CurlLoop,
    url: &str,
    total: Option<u64>,
    part: &mut PartFile,
    control: &Control,
) -> Result<u64, DownloadError> {
    let refusal = Arc::new(Mutex::new(None));
    let r = Arc::clone(&refusal);
    let fetch = lp.fetch(
        FetchRequest::get(url.to_owned()),
        move |head: &ResponseHead| {
            let length = head
                .header("content-length")
                .and_then(|v| v.trim().parse::<u64>().ok());
            let identity = head
                .header("content-encoding")
                .is_none_or(|e| e.trim() == "identity");
            if head.status == 200 && identity && (total.is_none() || length == total) {
                Gate::Accept
            } else {
                let e = TransferError::new(
                    FailureClass::Permanent,
                    Some(head.status),
                    &format!(
                        "the single stream isn't the same file (status {}, length {length:?}, expected {total:?})",
                        head.status
                    ),
                );
                let msg = e.message.clone();
                *r.lock().expect("the refusal lock") = Some(e);
                Gate::Abort(msg)
            }
        },
    );
    let mut offset = 0u64;
    let done = drain(&fetch, control, |chunk| {
        part.accept_gaps(offset, chunk)?;
        offset += chunk.len() as u64;
        control.report(part.written, total);
        Ok(())
    })?;
    if let Some(e) = refusal.lock().expect("the refusal lock").take() {
        return Err(e.into());
    }
    done.map_err(|e| DownloadError::Transfer(fetch_failure(e)))?;
    Ok(offset)
}

/// Moves a fetch's events to `tx`, tagged with `tag`, on its own thread.
fn forward<T: Copy + Send + 'static>(fetch: Fetch, tag: T, tx: mpsc::SyncSender<(T, FetchEvent)>) {
    thread::spawn(move || loop {
        let event = fetch
            .next_event(SILENCE_LIMIT)
            .unwrap_or(FetchEvent::Done(Err(FetchError::Curl {
                code: -1,
                message: format!("no data for {} s", SILENCE_LIMIT.as_secs()),
            })));
        let done = matches!(event, FetchEvent::Done(_));
        if tx.send((tag, event)).is_err() || done {
            // Dropping an unfinished Fetch cancels it.
            return;
        }
    });
}

/// Reads one fetch to its end, handing each body chunk to `on_data`. Checks for cancellation
/// while it waits.
fn drain(
    fetch: &Fetch,
    control: &Control,
    mut on_data: impl FnMut(&[u8]) -> Result<(), DownloadError>,
) -> Result<Result<(), FetchError>, DownloadError> {
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
            if last.elapsed() < SILENCE_LIMIT {
                continue;
            }
            fetch.cancel();
            return Ok(Err(FetchError::Curl {
                code: -1,
                message: format!("no data for {} s", SILENCE_LIMIT.as_secs()),
            }));
        };
        last = Instant::now();
        match event {
            FetchEvent::Head(_) => {}
            FetchEvent::Data(chunk) => on_data(&chunk)?,
            FetchEvent::Done(done) => return Ok(done),
        }
    }
}

/// A libcurl-side failure, as the contract describes it: retryable, without a status.
fn fetch_failure(e: FetchError) -> TransferError {
    match e {
        FetchError::Aborted(reason) => TransferError::new(FailureClass::Permanent, None, &reason),
        FetchError::Cancelled => {
            TransferError::new(FailureClass::Permanent, None, "the transfer was cancelled")
        }
        FetchError::Curl { code, message } => TransferError::new(
            FailureClass::Transient,
            None,
            &format!("libcurl error {code}: {message}"),
        ),
        FetchError::LoopStopped => {
            TransferError::new(FailureClass::Transient, None, "the libcurl loop stopped")
        }
    }
}

fn io_error(what: &str, path: &Path, e: &io::Error) -> DownloadError {
    DownloadError::Io(format!("couldn't {what} {}: {e}", path.display()))
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
