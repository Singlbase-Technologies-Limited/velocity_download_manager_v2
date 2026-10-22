//! The prototype downloader (#37): one file over HTTP(S), with a probe that is also the first
//! segment, static parallel segments when ranges work, and a finish that never replaces an existing
//! file. What it doesn't do yet is the rest of R5: retries (#48), checkpoints and resume (#43),
//! validators (#45), whole-file hashes (#46) and adaptive connections (#47).
//!
//! The byte rules (`.claude/rules/transfer-engine.md`, research report sections 5.1 and 5.2):
//! - a byte is accepted only from the segment that owns it, at its own offset, once;
//! - a response that isn't exactly what was asked for is aborted before any body byte is written;
//! - every received byte is either written or counted as wasted;
//! - the file is renamed into place only after its coverage is complete and synced to disk.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use velocity_core::domain::{FailureClass, TransferError};
use velocity_curl::{CurlLoop, Fetch, FetchError, FetchEvent, FetchRequest, Gate, ResponseHead};

use crate::probe::{check_segment, classify_probe, ProbeOutcome, SegmentRejection};

/// How long a fetch may stay silent before the prototype gives up on it. #48 replaces this with
/// stall detection.
const SILENCE_LIMIT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug)]
pub struct DownloadOptions {
    /// Connections for a server that honours ranges: the probe, then up to this many segments at
    /// once (at least 1).
    pub connections: u32,
    /// The probe's size (the report's hypothesis S = 4 MiB; a starting value for #47 to tune).
    pub probe_bytes: u64,
}

/// How the file was fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ranges worked: the probe plus parallel segments.
    Ranges,
    /// Ranges worked for the probe, then a later range came back as `200`: the rest was fetched as
    /// one stream, writing only the missing bytes.
    RangesLost,
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
}

#[derive(Debug)]
pub enum DownloadError {
    /// The server or the network failed the transfer.
    Transfer(TransferError),
    /// The destination already exists; it is never replaced.
    FileExists,
    /// A local file operation failed.
    Io(String),
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
    let probe_verdict = Arc::new(Mutex::new(None));
    let verdict = Arc::clone(&probe_verdict);
    let probe = lp.fetch(
        FetchRequest::range(url.to_owned(), 0, Some(probe_end)),
        move |head: &ResponseHead| {
            let result = classify_probe(head, probe_end);
            let gate = match &result {
                Ok(_) => Gate::Accept,
                Err(e) => Gate::Abort(e.message.clone()),
            };
            *verdict.lock().expect("the probe verdict lock") = Some(result);
            gate
        },
    );
    // The probe's bytes start at 0; how far they may go depends on its outcome, decided before the
    // first byte (the gate ran in the header callback).
    let mut cursor = 0u64;
    let probe_done = drain(&probe, |chunk| {
        let limit = match probe_verdict
            .lock()
            .expect("the probe verdict lock")
            .as_ref()
        {
            Some(Ok(
                ProbeOutcome::RangesOk { received_end, .. }
                | ProbeOutcome::SequentialRanges { received_end },
            )) => Some(received_end + 1),
            _ => None,
        };
        guard.accept(&mut cursor, limit, chunk)
    })?;
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

    let mut fetches = 1;
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
        ProbeOutcome::SequentialRanges { .. } => {
            return Err(TransferError::new(FailureClass::Permanent, Some(206), "the server doesn't give the file's total size; the prototype can't continue such a download yet (#41)").into());
        }
        ProbeOutcome::RangesOk {
            received_end,
            total,
        } => {
            let segments = split(received_end + 1, total, options.connections.max(1));
            fetches += segments.len() as u32;
            let lost = fetch_segments(lp, url, total, &segments, &mut guard)?;
            if lost {
                fetches += 1;
                stream_gaps(lp, url, total, &mut guard)?;
                (Mode::RangesLost, total)
            } else {
                (Mode::Ranges, total)
            }
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
    })
}

/// `<destination>.vdmpart`.
fn part_path(destination: &Path) -> PathBuf {
    let mut p = destination.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

/// Splits `start..total` into at most `n` contiguous segments, as inclusive `(first, last)` pairs.
fn split(start: u64, total: u64, n: u32) -> Vec<(u64, u64)> {
    let remaining = total.saturating_sub(start);
    if remaining == 0 {
        return Vec::new();
    }
    let n = u64::from(n).min(remaining);
    let base = remaining / n;
    let extra = remaining % n;
    let mut out = Vec::new();
    let mut a = start;
    for i in 0..n {
        let len = base + u64::from(i < extra);
        out.push((a, a + len - 1));
        a += len;
    }
    out
}

/// Runs every segment at once. Returns whether any segment found that ranges were lost (a `200`).
fn fetch_segments(
    lp: &CurlLoop,
    url: &str,
    total: u64,
    segments: &[(u64, u64)],
    part: &mut PartFile,
) -> Result<bool, DownloadError> {
    let (tx, rx) = mpsc::channel();
    let mut rejections = Vec::new();
    for (index, &(a, b)) in segments.iter().enumerate() {
        let rejection = Arc::new(Mutex::new(None));
        rejections.push(Arc::clone(&rejection));
        let fetch = lp.fetch(
            FetchRequest::range(url.to_owned(), a, Some(b)),
            move |head: &ResponseHead| match check_segment(head, a, b, total) {
                Ok(_) => Gate::Accept,
                Err(r) => {
                    let reason = match &r {
                        SegmentRejection::RangesIgnored => {
                            "a 200 for a range after byte 0".to_owned()
                        }
                        SegmentRejection::Invalid(e) => e.message.clone(),
                    };
                    *rejection.lock().expect("the segment rejection lock") = Some(r);
                    Gate::Abort(reason)
                }
            },
        );
        forward(fetch, index, tx.clone());
    }
    drop(tx);
    let mut cursors: Vec<u64> = segments.iter().map(|&(a, _)| a).collect();
    let mut failure = None;
    for (index, event) in rx {
        match event {
            FetchEvent::Head(_) => {}
            FetchEvent::Data(chunk) => {
                let limit = Some(segments[index].1 + 1);
                part.accept(&mut cursors[index], limit, &chunk)?;
            }
            FetchEvent::Done(Ok(())) => {}
            FetchEvent::Done(Err(e)) => {
                let rejected = rejections[index]
                    .lock()
                    .expect("the segment rejection lock")
                    .take();
                match rejected {
                    Some(SegmentRejection::RangesIgnored) => {}
                    Some(SegmentRejection::Invalid(t)) => failure = failure.or(Some(t)),
                    None => failure = failure.or(Some(fetch_failure(e))),
                }
            }
        }
    }
    if let Some(e) = failure {
        return Err(e.into());
    }
    // A segment may also end short (a 206 with fewer bytes than asked for).
    Ok(!part.coverage.covers(0, total))
}

/// Fetches the whole file as one stream and writes only the bytes not yet covered.
fn stream_gaps(
    lp: &CurlLoop,
    url: &str,
    total: u64,
    part: &mut PartFile,
) -> Result<(), DownloadError> {
    let refusal = Arc::new(Mutex::new(None));
    let r = Arc::clone(&refusal);
    let fetch = lp.fetch(FetchRequest::get(url.to_owned()), move |head: &ResponseHead| {
        let length = head.header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
        if head.status == 200 && length == Some(total) && head.header("content-encoding").is_none_or(|e| e.trim() == "identity") {
            Gate::Accept
        } else {
            let e = TransferError::new(FailureClass::Permanent, Some(head.status), &format!("the single stream isn't the same {total}-byte file (status {}, length {length:?})", head.status));
            let msg = e.message.clone();
            *r.lock().expect("the refusal lock") = Some(e);
            Gate::Abort(msg)
        }
    });
    let mut offset = 0u64;
    let done = drain(&fetch, |chunk| {
        part.accept_gaps(offset, chunk)?;
        offset += chunk.len() as u64;
        Ok(())
    })?;
    if let Some(e) = refusal.lock().expect("the refusal lock").take() {
        return Err(e.into());
    }
    done.map_err(|e| DownloadError::Transfer(fetch_failure(e)))
}

/// Moves a fetch's events to `tx`, tagged with `index`, on its own thread.
fn forward(fetch: Fetch, index: usize, tx: mpsc::Sender<(usize, FetchEvent)>) {
    thread::spawn(move || loop {
        let event = fetch
            .next_event(SILENCE_LIMIT)
            .unwrap_or(FetchEvent::Done(Err(FetchError::Curl {
                code: -1,
                message: format!("no data for {} s", SILENCE_LIMIT.as_secs()),
            })));
        let done = matches!(event, FetchEvent::Done(_));
        if tx.send((index, event)).is_err() || done {
            // Dropping an unfinished Fetch cancels it.
            return;
        }
    });
}

/// Reads one fetch to its end, handing each body chunk to `on_data`.
fn drain(
    fetch: &Fetch,
    mut on_data: impl FnMut(&[u8]) -> Result<(), DownloadError>,
) -> Result<Result<(), FetchError>, DownloadError> {
    loop {
        let Some(event) = fetch.next_event(SILENCE_LIMIT) else {
            fetch.cancel();
            return Ok(Err(FetchError::Curl {
                code: -1,
                message: format!("no data for {} s", SILENCE_LIMIT.as_secs()),
            }));
        };
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

/// Which byte ranges are written, as sorted, merged, half-open intervals.
#[derive(Default)]
struct Coverage(Vec<(u64, u64)>);

impl Coverage {
    fn insert(&mut self, a: u64, b: u64) {
        if a >= b {
            return;
        }
        self.0.push((a, b));
        self.0.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.0.len());
        for &(s, e) in &self.0 {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.0 = merged;
    }

    fn covers(&self, a: u64, b: u64) -> bool {
        a >= b || self.0.iter().any(|&(s, e)| s <= a && b <= e)
    }

    /// The parts of `a..b` not yet covered.
    fn gaps(&self, a: u64, b: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut at = a;
        for &(s, e) in &self.0 {
            if e <= at || s >= b {
                continue;
            }
            if s > at {
                out.push((at, s.min(b)));
            }
            at = at.max(e);
            if at >= b {
                break;
            }
        }
        if at < b {
            out.push((at, b));
        }
        out
    }
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
    /// the limit, or already covered, are counted as wasted.
    fn accept(
        &mut self,
        cursor: &mut u64,
        limit: Option<u64>,
        chunk: &[u8],
    ) -> Result<(), DownloadError> {
        let start = *cursor;
        let mut take = chunk.len() as u64;
        if let Some(limit) = limit {
            take = take.min(limit.saturating_sub(start));
        }
        self.accept_gaps(start, &chunk[..take as usize])?;
        self.wasted += chunk.len() as u64 - take;
        *cursor += take;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_rest_into_contiguous_segments() {
        assert_eq!(
            split(100, 1000, 3),
            vec![(100, 399), (400, 699), (700, 999)]
        );
        assert_eq!(split(0, 10, 3), vec![(0, 3), (4, 6), (7, 9)]);
        assert_eq!(
            split(998, 1000, 8),
            vec![(998, 998), (999, 999)],
            "never more segments than bytes"
        );
        assert_eq!(split(1000, 1000, 4), vec![]);
    }

    #[test]
    fn coverage_merges_and_finds_gaps() {
        let mut c = Coverage::default();
        c.insert(10, 20);
        c.insert(30, 40);
        c.insert(20, 25);
        assert_eq!(c.0, vec![(10, 25), (30, 40)]);
        assert_eq!(c.gaps(0, 50), vec![(0, 10), (25, 30), (40, 50)]);
        assert_eq!(c.gaps(12, 22), vec![]);
        assert!(c.covers(10, 25) && !c.covers(10, 26));
        c.insert(0, 50);
        assert!(c.covers(0, 50));
    }
}
