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
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use velocity_core::domain::{FailureClass, TransferError};
use velocity_curl::{
    CurlLoop, Fetch, FetchError, FetchEvent, FetchLimit, FetchRequest, Gate, ResponseHead,
};

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

/// Lets a caller follow and stop a running download (the engine, #37).
#[derive(Default)]
pub struct Control {
    cancelled: AtomicBool,
    progress: Option<ProgressFn>,
}

impl Control {
    /// `progress` is called with the bytes written so far and the total, once known.
    pub fn with_progress(progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static) -> Self {
        Control {
            cancelled: AtomicBool::new(false),
            progress: Some(Box::new(progress)),
        }
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

/// [`download`], followed and stoppable through `control`.
pub fn download_with(
    lp: &CurlLoop,
    url: &str,
    destination: &Path,
    options: DownloadOptions,
    control: &Control,
) -> Result<DownloadReport, DownloadError> {
    // symlink_metadata doesn't follow a link: a dangling link at the destination still counts.
    if fs::symlink_metadata(destination).is_ok() {
        return Err(DownloadError::FileExists);
    }
    let part = part_path(destination);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => DownloadError::Io(format!(
                "{} already exists; it may belong to another download",
                part.display()
            )),
            _ => io_error("create the part file", &part, &e),
        })?;
    // From here on the part file is ours: removed on any failure, renamed on success.
    let mut guard = PartFile {
        path: part,
        file: Some(file),
        coverage: Coverage::default(),
        written: 0,
        wasted: 0,
        keep: false,
    };

    let probe_end = options.probe_bytes.max(1) - 1;
    let mut fetches = 1;
    let mut splits = 0;
    let request = FetchRequest::range(url.to_owned(), 0, Some(probe_end));
    let (outcome, cursor) = match probe(lp, request, probe_end, &mut guard, control)? {
        Probed::Outcome(outcome, cursor) => (outcome, cursor),
        // Report 5.1: a multipart answer to one range is retried once without Range.
        Probed::Multipart => {
            fetches += 1;
            let request = FetchRequest::get(url.to_owned());
            match probe(lp, request, probe_end, &mut guard, control)? {
                Probed::Outcome(outcome, cursor) => (outcome, cursor),
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
                fetch_sequential(lp, url, received_end + 1, &mut guard, control)?;
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
                &mut guard,
                control,
            )?;
            fetches += n;
            splits = s;
            (mode, total)
        }
    };
    let (written, wasted) = (guard.written, guard.wasted);
    guard.finish(total, destination)?;
    Ok(DownloadReport {
        mode,
        total,
        fetches,
        written_bytes: written,
        wasted_bytes: wasted,
        splits,
    })
}

/// `<destination>.vdmpart`.
fn part_path(destination: &Path) -> PathBuf {
    let mut p = destination.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

/// What the probe found.
enum Probed {
    /// Its outcome, and how far its bytes reached.
    Outcome(ProbeOutcome, u64),
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
    let (verdict, saw_multipart) = (Arc::clone(&probe_verdict), Arc::clone(&multipart));
    let fetch = lp.fetch(request, move |head: &ResponseHead| {
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
    Ok(Probed::Outcome(outcome, cursor))
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
                Err(SegmentRejection::Invalid(e) | SegmentRejection::Anomaly(e)) => {
                    Gate::Abort(e.message.clone())
                }
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
    control: &Control,
) -> Result<(u32, u32, Mode), DownloadError> {
    let mut sched = Scheduler::new(total, &part.coverage, sizes);
    // Bounded, so a slow writer stops the forwarders, which stops libcurl: an unbounded channel
    // here once queued a whole file in memory (#38).
    let (tx, rx) = mpsc::sync_channel(QUEUED_PER_SEGMENT * connections as usize);
    let mut active: HashMap<LeaseId, Active> = HashMap::new();
    let (mut fetches, mut splits, mut anomalies) = (0u32, 0u32, 0u32);
    let mut ranges_ignored = false;
    let mut failure: Option<TransferError> = None;
    loop {
        // Fill the free connections, unless ranges stopped working for this download.
        while failure.is_none()
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
                    Err(SegmentRejection::Invalid(e) | SegmentRejection::Anomaly(e)) => {
                        Gate::Abort(e.message.clone())
                    }
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
struct PartFile {
    path: PathBuf,
    file: Option<File>,
    coverage: Coverage,
    written: u64,
    wasted: u64,
    keep: bool,
}

impl PartFile {
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
        // A hard link fails if the destination exists, so an existing file is never replaced,
        // whatever appeared there during the download (rename would replace it on Windows).
        match fs::hard_link(&self.path, destination) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(DownloadError::FileExists)
            }
            Err(e) => return Err(io_error("move the finished file to", destination, &e)),
        }
        // The destination now holds the data; the part file's name goes, and this guard is done.
        self.keep = true;
        fs::remove_file(&self.path).map_err(|e| io_error("remove the part file", &self.path, &e))
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if !self.keep {
            drop(self.file.take());
            // Only the part file this download created is removed. A failure here leaves it behind,
            // and the next download refuses to reuse it.
            let _ = fs::remove_file(&self.path);
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
