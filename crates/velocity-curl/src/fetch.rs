//! The libcurl loop (#37): one thread owns a libcurl `Multi` and runs every transfer on it, as
//! `.claude/rules/transfer-engine.md` requires. Other threads talk to it through commands and a
//! wakeup. Each fetch reports its final response head, then its body, then how it ended.
//!
//! - **A gate decides before any body byte:** the caller's gate sees the final response head in
//!   libcurl's header callback and can abort the transfer there, so a body it rejects is never
//!   delivered (report section 5.1, steps 3 and 4).
//! - **Bounded memory, no blocking:** a fetch buffers at most `buffer_bytes_per_fetch` bytes that
//!   its reader hasn't taken yet. When that is reached, the write callback pauses the transfer
//!   (`CURL_WRITEFUNC_PAUSE`) instead of blocking or growing, and the reader resumes it once it has
//!   caught up (report section 5.2; #49).
//! - **Safe defaults:** only `http`, `https` and `ftp`, redirects only to `http` and `https`, an
//!   explicitly empty proxy so `http_proxy` and the like are ignored (ADR-0008), no
//!   `Accept-Encoding`, and TLS verification as libcurl's default (on).

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use curl::easy::{Easy2, Handler, WriteError};
use curl::multi::{Easy2Handle, Multi, MultiWaker};

use crate::runtime::Libcurl;

/// Protocols a fetch may use, and the ones a redirect may lead to. SFTP joins when #53 builds it.
const PROTOCOLS: &str = "http,https,ftp";
const REDIRECT_PROTOCOLS: &str = "http,https";
const MAX_REDIRECTS: u32 = 10;
/// The longest the loop waits for sockets before checking commands and timers again. Commands also
/// wake it at once; this only bounds a missed wakeup.
const POLL_CAP: Duration = Duration::from_millis(100);
/// `CURLOPT_PROTOCOLS_STR` and `CURLOPT_REDIR_PROTOCOLS_STR` (curl.h of curl 8.23.0:
/// `CURLOPTTYPE_STRINGPOINT` + 318 and + 319). curl-sys 0.4.91 doesn't define them.
const CURLOPT_PROTOCOLS_STR: curl_sys::CURLoption = 10_318;
const CURLOPT_REDIR_PROTOCOLS_STR: curl_sys::CURLoption = 10_319;

/// One HTTP(S) or FTP GET.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    /// An inclusive byte range: `(start, None)` is `start-`.
    pub range: Option<(u64, Option<u64>)>,
}

impl FetchRequest {
    pub fn get(url: String) -> Self {
        FetchRequest { url, range: None }
    }

    pub fn range(url: String, start: u64, end: Option<u64>) -> Self {
        FetchRequest {
            url,
            range: Some((start, end)),
        }
    }
}

/// The final response's status line and headers (after any redirects).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl ResponseHead {
    /// The first header with this name, compared without case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What a gate decides about a final response head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    Accept,
    /// Ends the transfer before any body byte is delivered, with this reason.
    Abort(String),
}

/// How a fetch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The gate refused the response.
    Aborted(String),
    /// The caller cancelled the fetch.
    Cancelled,
    /// libcurl reported an error (`CURLcode` and its description).
    Curl { code: i32, message: String },
    /// The loop stopped before the fetch finished.
    LoopStopped,
}

/// One step of a fetch, in order: at most one `Head`, any number of `Data`, then one `Done`.
#[derive(Debug)]
pub enum FetchEvent {
    Head(ResponseHead),
    Data(Vec<u8>),
    Done(Result<(), FetchError>),
}

#[derive(Clone, Copy, Debug)]
pub struct LoopOptions {
    /// The most body bytes one fetch keeps for its reader before it pauses the transfer.
    pub buffer_bytes_per_fetch: usize,
}

impl Default for LoopOptions {
    fn default() -> Self {
        // A starting value, not a requirement (report section 5.5): measured later (#49).
        LoopOptions {
            buffer_bytes_per_fetch: 1024 * 1024,
        }
    }
}

/// The loop thread couldn't start.
#[derive(Debug)]
pub struct LoopError(String);

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the libcurl loop couldn't start: {}", self.0)
    }
}

impl std::error::Error for LoopError {}

/// State a fetch shares between the loop's callbacks and its reader.
#[derive(Default)]
struct Flow {
    buffered: AtomicUsize,
    peak: AtomicUsize,
    paused: AtomicBool,
}

enum Command {
    Start(usize, Box<Easy2<Collector>>),
    Cancel(usize),
    Unpause(usize),
    Stop,
}

/// Owns the loop thread. Dropping it stops the loop; unfinished fetches end with `LoopStopped`.
pub struct CurlLoop {
    commands: Sender<Command>,
    waker: MultiWaker,
    next_token: AtomicUsize,
    options: LoopOptions,
    thread: Option<JoinHandle<()>>,
}

impl CurlLoop {
    /// Starts the loop. It needs a [`Libcurl`], so it can't run on a libcurl that failed the
    /// capability check.
    pub fn start(_curl: &Libcurl, options: LoopOptions) -> Result<Self, LoopError> {
        let (commands, inbox) = mpsc::channel();
        let (waker_tx, waker_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("velocity-curl-multi".into())
            .spawn(move || run_loop(inbox, waker_tx))
            .map_err(|e| LoopError(e.to_string()))?;
        // The Multi is created on the loop thread (it isn't Send); only its waker comes back.
        let waker = waker_rx
            .recv()
            .map_err(|_| LoopError("the loop thread ended while starting".into()))?;
        Ok(CurlLoop {
            commands,
            waker,
            next_token: AtomicUsize::new(1),
            options,
            thread: Some(thread),
        })
    }

    /// Starts a fetch. Its events arrive on the returned [`Fetch`].
    pub fn fetch(
        &self,
        request: FetchRequest,
        gate: impl Fn(&ResponseHead) -> Gate + Send + 'static,
    ) -> Fetch {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let (events_tx, events) = mpsc::channel();
        let flow = Arc::new(Flow::default());
        let collector = Collector {
            events: events_tx.clone(),
            gate: Box::new(gate),
            flow: Arc::clone(&flow),
            cap: self.options.buffer_bytes_per_fetch,
            lines: Vec::new(),
            accepted: false,
            abort: None,
        };
        let fetch = Fetch {
            token,
            events,
            flow,
            cap: self.options.buffer_bytes_per_fetch,
            commands: self.commands.clone(),
            waker: self.waker.clone(),
            done: AtomicBool::new(false),
        };
        match configure(collector, &request) {
            Ok(easy) => self.send(Command::Start(token, Box::new(easy))),
            Err(e) => {
                let _ = events_tx.send(FetchEvent::Done(Err(curl_error(&e))));
            }
        }
        fetch
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            let _ = self.waker.wakeup();
        }
    }
}

impl Drop for CurlLoop {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One fetch's events, read by one thread.
pub struct Fetch {
    token: usize,
    events: Receiver<FetchEvent>,
    flow: Arc<Flow>,
    cap: usize,
    commands: Sender<Command>,
    waker: MultiWaker,
    done: AtomicBool,
}

impl Fetch {
    /// The next event, or `None` if none arrives within `timeout`. Taking a `Data` event frees its
    /// bytes from the fetch's buffer, and resumes a paused transfer once half the buffer is free.
    pub fn next_event(&self, timeout: Duration) -> Option<FetchEvent> {
        let event = match self.events.recv_timeout(timeout) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => FetchEvent::Done(Err(FetchError::LoopStopped)),
        };
        match &event {
            FetchEvent::Data(chunk) => {
                let left =
                    self.flow.buffered.fetch_sub(chunk.len(), Ordering::AcqRel) - chunk.len();
                if left <= self.cap / 2 && self.flow.paused.swap(false, Ordering::AcqRel) {
                    self.send(Command::Unpause(self.token));
                }
            }
            FetchEvent::Done(_) => self.done.store(true, Ordering::Release),
            FetchEvent::Head(_) => {}
        }
        Some(event)
    }

    /// Stops the fetch; its last event is `Done(Err(Cancelled))`.
    pub fn cancel(&self) {
        self.send(Command::Cancel(self.token));
    }

    /// The most body bytes this fetch held for its reader at one time.
    pub fn peak_buffered_bytes(&self) -> usize {
        self.flow.peak.load(Ordering::Acquire)
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            let _ = self.waker.wakeup();
        }
    }
}

impl Drop for Fetch {
    /// An abandoned fetch is cancelled, so its transfer doesn't keep running.
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            self.send(Command::Cancel(self.token));
        }
    }
}

/// libcurl's callbacks for one fetch. They run on the loop thread: they never block and never
/// call back into the caller; they only send events.
struct Collector {
    events: Sender<FetchEvent>,
    gate: Box<dyn Fn(&ResponseHead) -> Gate + Send>,
    flow: Arc<Flow>,
    cap: usize,
    /// Header lines of the response being received.
    lines: Vec<String>,
    accepted: bool,
    abort: Option<String>,
}

impl Handler for Collector {
    fn header(&mut self, data: &[u8]) -> bool {
        let line = String::from_utf8_lossy(data).trim_end().to_owned();
        if line.starts_with("HTTP/") {
            self.lines.clear();
        }
        if !line.is_empty() {
            self.lines.push(line);
            return true;
        }
        // A blank line ends one response's headers.
        let Some(head) = parse_head(&self.lines) else {
            return true;
        };
        // 1xx and followed redirects are intermediate responses; libcurl continues past them.
        if head.status < 200
            || ((300..400).contains(&head.status) && head.header("location").is_some())
        {
            return true;
        }
        let gate = (self.gate)(&head);
        let _ = self.events.send(FetchEvent::Head(head));
        match gate {
            Gate::Accept => {
                self.accepted = true;
                true
            }
            Gate::Abort(reason) => {
                self.abort = Some(reason);
                false
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if !self.accepted {
            // A body without an accepted head (for example an FTP response): nothing to gate.
            self.accepted = true;
        }
        let buffered = self.flow.buffered.load(Ordering::Acquire);
        if buffered > 0 && buffered + data.len() > self.cap {
            self.flow.paused.store(true, Ordering::Release);
            return Err(WriteError::Pause);
        }
        let now = self.flow.buffered.fetch_add(data.len(), Ordering::AcqRel) + data.len();
        self.flow.peak.fetch_max(now, Ordering::AcqRel);
        let _ = self.events.send(FetchEvent::Data(data.to_vec()));
        Ok(data.len())
    }
}

fn parse_head(lines: &[String]) -> Option<ResponseHead> {
    let status = lines.first()?.split_whitespace().nth(1)?.parse().ok()?;
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(ResponseHead { status, headers })
}

fn configure(
    collector: Collector,
    request: &FetchRequest,
) -> Result<Easy2<Collector>, curl::Error> {
    let mut easy = Easy2::new(collector);
    easy.url(&request.url)?;
    easy.get(true)?;
    if let Some((start, end)) = request.range {
        easy.range(&match end {
            Some(end) => format!("{start}-{end}"),
            None => format!("{start}-"),
        })?;
    }
    easy.follow_location(true)?;
    easy.max_redirections(MAX_REDIRECTS)?;
    // An empty proxy disables proxies, including those from http_proxy and similar variables.
    easy.proxy("")?;
    set_string(&easy, CURLOPT_PROTOCOLS_STR, PROTOCOLS)?;
    set_string(&easy, CURLOPT_REDIR_PROTOCOLS_STR, REDIRECT_PROTOCOLS)?;
    Ok(easy)
}

/// Sets a string option that the curl crate doesn't wrap.
fn set_string(
    easy: &Easy2<Collector>,
    option: curl_sys::CURLoption,
    value: &str,
) -> Result<(), curl::Error> {
    let value = CString::new(value).expect("a constant without NUL bytes");
    // SAFETY: `easy.raw()` is a live easy handle owned by `easy`; libcurl copies string options
    // (CURLOPT_PROTOCOLS_STR and CURLOPT_REDIR_PROTOCOLS_STR) during the call, so `value` may be
    // freed afterwards.
    let code = unsafe { curl_sys::curl_easy_setopt(easy.raw(), option, value.as_ptr()) };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(curl::Error::new(code))
    }
}

fn curl_error(e: &curl::Error) -> FetchError {
    FetchError::Curl {
        code: e.code(),
        message: e.description().to_owned(),
    }
}

/// The loop thread: owns the `Multi`, starts and stops transfers, and reports how each ended.
fn run_loop(inbox: Receiver<Command>, waker_tx: Sender<MultiWaker>) {
    let multi = Multi::new();
    if waker_tx.send(multi.waker()).is_err() {
        return;
    }
    let mut active: HashMap<usize, Easy2Handle<Collector>> = HashMap::new();
    loop {
        // Commands first: with nothing running, wait for one instead of polling.
        let mut next = if active.is_empty() {
            inbox.recv().ok()
        } else {
            inbox.try_recv().ok()
        };
        if active.is_empty() && next.is_none() {
            return; // every sender is gone
        }
        while let Some(command) = next.take() {
            match command {
                Command::Start(token, easy) => {
                    // add2 consumes the handle even when it fails, so keep a way to report it.
                    let events = easy.get_ref().events.clone();
                    let started = multi
                        .add2(*easy)
                        .map_err(|e| (e.code(), e.to_string()))
                        .and_then(|mut handle| {
                            handle
                                .set_token(token)
                                .map(|()| handle)
                                .map_err(|e| (e.code(), e.to_string()))
                        });
                    match started {
                        Ok(handle) => {
                            active.insert(token, handle);
                        }
                        Err((code, message)) => {
                            let _ = events.send(FetchEvent::Done(Err(FetchError::Curl {
                                code,
                                message: format!("the transfer couldn't start: {message}"),
                            })));
                        }
                    }
                }
                Command::Cancel(token) => {
                    if let Some(handle) = active.remove(&token) {
                        finish(&multi, handle, Err(FetchError::Cancelled));
                    }
                }
                Command::Unpause(token) => {
                    if let Some(handle) = active.get(&token) {
                        let _ = handle.unpause_write();
                    }
                }
                Command::Stop => {
                    for (_, handle) in active.drain() {
                        finish(&multi, handle, Err(FetchError::LoopStopped));
                    }
                    return;
                }
            }
            next = inbox.try_recv().ok();
        }
        if active.is_empty() {
            continue;
        }
        let _ = multi.perform();
        let mut ended = Vec::new();
        multi.messages(|m| {
            if let Ok(token) = m.token() {
                if let Some(result) = active.get(&token).and_then(|h| m.result_for2(h)) {
                    ended.push((token, result));
                }
            }
        });
        for (token, result) in ended {
            if let Some(handle) = active.remove(&token) {
                let outcome = match result {
                    Ok(()) => Ok(()),
                    Err(e) => match &handle.get_ref().abort {
                        Some(reason) => Err(FetchError::Aborted(reason.clone())),
                        None => Err(curl_error(&e)),
                    },
                };
                finish(&multi, handle, outcome);
            }
        }
        let wait = multi
            .get_timeout()
            .ok()
            .flatten()
            .map_or(POLL_CAP, |t| t.min(POLL_CAP));
        let _ = multi.poll(&mut [], wait);
    }
}

/// Removes a transfer from the `Multi` (on the loop thread, as libcurl requires) and sends its
/// last event.
fn finish(multi: &Multi, handle: Easy2Handle<Collector>, outcome: Result<(), FetchError>) {
    let events = handle.get_ref().events.clone();
    // A failed removal is reported to the fetch rather than dropped; a successful outcome then
    // becomes that error.
    let outcome = match multi.remove2(handle) {
        Ok(_) => outcome,
        Err(e) => outcome.and(Err(FetchError::Curl {
            code: e.code(),
            message: format!("the transfer couldn't be removed: {e}"),
        })),
    };
    // The reader may have dropped its Fetch; then nobody is waiting for this event.
    let _ = events.send(FetchEvent::Done(outcome));
}
