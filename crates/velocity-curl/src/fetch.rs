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
//! - **Speed limits:** a fetch draws from the [`RateLimit`]s in its request. The write callback
//!   pauses a transfer whose limit has no tokens, and the loop thread resumes it once all its
//!   limits have tokens again (`rate.rs`, #49).
//! - **Safe defaults:** only `http`, `https` and `ftp`, redirects only to `http` and `https`, an
//!   explicitly empty proxy so `http_proxy` and the like are ignored (ADR-0008), no
//!   `Accept-Encoding`, and TLS verification as libcurl's default (on).

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use curl::easy::{Easy2, Handler, WriteError};
use curl::multi::{Easy2Handle, Multi, MultiWaker};

use crate::rate::{self, RateLimit};
use crate::runtime::Libcurl;

/// Protocols a fetch may use, and the ones a redirect may lead to. SFTP joins when #53 builds it.
const PROTOCOLS: &str = "http,https,ftp";
const REDIRECT_PROTOCOLS: &str = "http,https";
const MAX_REDIRECTS: u32 = 10;
/// The longest the loop waits for sockets before checking commands and timers again. Commands also
/// wake it at once; this only bounds a missed wakeup.
const POLL_CAP: Duration = Duration::from_millis(100);
/// How often the loop checks whether transfers paused by a speed limit may go on (#49).
const RATE_CHECK: Duration = Duration::from_millis(10);
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
    /// The speed limits the fetch draws from, such as its download's and the global one (#49).
    pub limits: Vec<RateLimit>,
    /// Opens a connection of its own, closed when the fetch ends, instead of sharing one as an
    /// HTTP/2 stream (`CURLOPT_FRESH_CONNECT`, `CURLOPT_FORBID_REUSE`; #47).
    pub own_connection: bool,
}

impl FetchRequest {
    pub fn get(url: String) -> Self {
        FetchRequest {
            url,
            range: None,
            limits: Vec::new(),
            own_connection: false,
        }
    }

    pub fn range(url: String, start: u64, end: Option<u64>) -> Self {
        FetchRequest {
            url,
            range: Some((start, end)),
            limits: Vec::new(),
            own_connection: false,
        }
    }

    /// The same request, on a connection of its own (#47).
    pub fn on_own_connection(mut self) -> Self {
        self.own_connection = true;
        self
    }

    /// The same request, drawing from `limits`.
    pub fn limited(mut self, limits: &[RateLimit]) -> Self {
        self.limits = limits.to_vec();
        self
    }
}

/// The final response's status line and headers (after any redirects).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// It came over HTTP/2: its status line says `HTTP/2` (#47).
    pub http2: bool,
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
    /// libcurl reported an error: what kind it is, and its `CURLcode` and description for
    /// diagnostics. The code is an `i64` because `CURLcode` is `u32` on Linux and `i32` on
    /// Windows; `i64` holds either without a cast.
    Curl {
        kind: CurlFailure,
        code: i64,
        message: String,
    },
    /// The loop stopped before the fetch finished.
    LoopStopped,
}

/// What kind of failure libcurl reported, so that callers can decide about retries without
/// knowing libcurl's codes (#48).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurlFailure {
    /// The host name didn't resolve.
    Resolve,
    /// The connection couldn't be made, or was refused.
    Connect,
    /// libcurl's own timeout passed (the connect timeout).
    Timeout,
    /// The connection broke while the request or the response was under way.
    Interrupted,
    /// The TLS handshake failed.
    TlsHandshake,
    /// The server's certificate wasn't trusted. Verification is never turned off (#69).
    Certificate,
    /// More redirects than allowed.
    TooManyRedirects,
    /// A URL or redirect outside the protocol allowlist, or a malformed URL.
    Refused,
    /// The server's answer broke the protocol.
    Protocol,
    /// A local problem: memory, or the write side.
    Local,
    /// Any other code.
    Other,
}

impl CurlFailure {
    fn of(e: &curl::Error) -> Self {
        if e.is_couldnt_resolve_host() || e.is_couldnt_resolve_proxy() {
            CurlFailure::Resolve
        } else if e.is_couldnt_connect() {
            CurlFailure::Connect
        } else if e.is_operation_timedout() {
            CurlFailure::Timeout
        } else if e.is_got_nothing()
            || e.is_send_error()
            || e.is_recv_error()
            || e.is_partial_file()
            || e.is_http2_error()
            || e.is_http2_stream_error()
        {
            CurlFailure::Interrupted
        } else if e.is_ssl_connect_error() {
            CurlFailure::TlsHandshake
        } else if e.is_peer_failed_verification()
            || e.is_ssl_cacert_badfile()
            || e.is_ssl_certproblem()
            || e.is_ssl_issuer_error()
        {
            CurlFailure::Certificate
        } else if e.is_too_many_redirects() {
            CurlFailure::TooManyRedirects
        } else if e.is_unsupported_protocol() || e.is_url_malformed() {
            CurlFailure::Refused
        } else if e.code() == curl_sys::CURLE_FTP_WEIRD_SERVER_REPLY || e.is_bad_content_encoding()
        {
            // curl-sys names code 8 by its old FTP name; libcurl uses it for any protocol.
            CurlFailure::Protocol
        } else if e.is_out_of_memory() || e.is_write_error() {
            CurlFailure::Local
        } else {
            CurlFailure::Other
        }
    }
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
    /// How long a fetch may take to connect (`CURLOPT_CONNECTTIMEOUT_MS`). There is no limit on
    /// the whole transfer: a large file may take hours, and stalls are caught by the downloader.
    pub connect_timeout: Duration,
    /// The most connections open to one host name and port at once
    /// (`CURLMOPT_MAX_HOST_CONNECTIONS`). Fetches past it wait for a free connection. The engine's
    /// own budget keeps below it; this is the backstop (#47).
    pub max_connections_per_host: usize,
    /// Speaks HTTP/2 to `http://` URLs without asking first (prior knowledge), for the HTTP/2 tests
    /// against a local server without TLS. Test builds only (PO decision on #47).
    #[cfg(feature = "test-h2c")]
    pub http2_prior_knowledge: bool,
}

impl Default for LoopOptions {
    fn default() -> Self {
        // Starting values, not requirements: the buffer from report section 5.5, measured later
        // (#49); the connect timeout from V1's `connect_timeout_secs` (PO decision on #48); 8
        // connections per host from report section 5.5 (PO decision on #47).
        LoopOptions {
            buffer_bytes_per_fetch: 1024 * 1024,
            connect_timeout: Duration::from_secs(30),
            max_connections_per_host: 8,
            #[cfg(feature = "test-h2c")]
            http2_prior_knowledge: false,
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
struct Flow {
    buffered: AtomicUsize,
    peak: AtomicUsize,
    paused: AtomicBool,
    /// Paused because a speed limit had no tokens; the loop thread resumes it (#49).
    rate_paused: AtomicBool,
    /// The most body bytes the transfer may deliver ([`Fetch::stop_at`]).
    limit: AtomicU64,
}

impl Default for Flow {
    fn default() -> Self {
        Flow {
            buffered: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
            rate_paused: AtomicBool::new(false),
            limit: AtomicU64::new(u64::MAX),
        }
    }
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
            .spawn(move || run_loop(inbox, waker_tx, options))
            .map_err(|e| LoopError(e.to_string()))?;
        // The Multi is created on the loop thread (it isn't Send); only its waker comes back.
        let waker = waker_rx
            .recv()
            .map_err(|_| LoopError("the loop thread ended while starting".into()))?
            .map_err(LoopError)?;
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
            limits: request.limits.clone(),
            lines: Vec::new(),
            accepted: false,
            abort: None,
            delivered: 0,
            stopped_at_limit: false,
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
        match configure(collector, &request, &self.options) {
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

    /// Stops the transfer once it has delivered `body_bytes` body bytes; it then ends with
    /// `Done(Ok)`. The limit may be lowered while the fetch runs, for example when its range is
    /// split (#42); bytes already delivered are not taken back.
    pub fn stop_at(&self, body_bytes: u64) {
        self.limit().stop_at(body_bytes);
    }

    /// A handle that sets [`Fetch::stop_at`] from another thread than the one reading the fetch.
    pub fn limit(&self) -> FetchLimit {
        FetchLimit(Arc::clone(&self.flow))
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

/// Lowers a fetch's body limit ([`Fetch::stop_at`]); it may be kept by another thread than the
/// fetch's reader.
#[derive(Clone)]
pub struct FetchLimit(Arc<Flow>);

impl FetchLimit {
    /// See [`Fetch::stop_at`].
    pub fn stop_at(&self, body_bytes: u64) {
        // Only ever lowered. A transfer paused for a slow reader sees it when it next writes.
        self.0.limit.fetch_min(body_bytes, Ordering::AcqRel);
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
    /// The speed limits this fetch draws from.
    limits: Vec<RateLimit>,
    /// Header lines of the response being received.
    lines: Vec<String>,
    accepted: bool,
    abort: Option<String>,
    /// Body bytes delivered so far.
    delivered: u64,
    /// The transfer was stopped at its limit: it ends as completed, not as an error.
    stopped_at_limit: bool,
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
        // Up to the limit, which another thread may have lowered since the last write.
        let room = self
            .flow
            .limit
            .load(Ordering::Acquire)
            .saturating_sub(self.delivered);
        let take = data.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        let buffered = self.flow.buffered.load(Ordering::Acquire);
        if take > 0 && buffered > 0 && buffered + take > self.cap {
            self.flow.paused.store(true, Ordering::Release);
            return Err(WriteError::Pause);
        }
        if take > 0 && !rate::take(&self.limits, take) {
            // Out of tokens: libcurl keeps these bytes and offers them again once resumed.
            self.flow.rate_paused.store(true, Ordering::Release);
            return Err(WriteError::Pause);
        }
        if take > 0 {
            let now = self.flow.buffered.fetch_add(take, Ordering::AcqRel) + take;
            self.flow.peak.fetch_max(now, Ordering::AcqRel);
            let _ = self.events.send(FetchEvent::Data(data[..take].to_vec()));
            self.delivered += take as u64;
        }
        if take < data.len() {
            // A short count makes libcurl stop the transfer (CURLE_WRITE_ERROR).
            self.stopped_at_limit = true;
        }
        Ok(take)
    }
}

fn parse_head(lines: &[String]) -> Option<ResponseHead> {
    let status = lines.first()?.split_whitespace().nth(1)?.parse().ok()?;
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(ResponseHead {
        status,
        headers,
        http2: lines.first()?.starts_with("HTTP/2"),
    })
}

fn configure(
    collector: Collector,
    request: &FetchRequest,
    options: &LoopOptions,
) -> Result<Easy2<Collector>, curl::Error> {
    let mut easy = Easy2::new(collector);
    easy.url(&request.url)?;
    easy.connect_timeout(options.connect_timeout)?;
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
    if request.own_connection {
        // A connection of its own, closed at the end, so no other fetch shares it (#47).
        easy.fresh_connect(true)?;
        easy.forbid_reuse(true)?;
    } else {
        // HTTP/2 multiplexing first (report section 5.5): wait for a connection being set up to
        // the same host, to share it if it turns out to speak HTTP/2, instead of opening another.
        easy.pipewait(true)?;
    }
    #[cfg(feature = "test-h2c")]
    if options.http2_prior_knowledge {
        easy.http_version(curl::easy::HttpVersion::V2PriorKnowledge)?;
    }
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
        kind: CurlFailure::of(e),
        code: i64::from(e.code()),
        message: e.description().to_owned(),
    }
}

/// The loop thread: owns the `Multi`, starts and stops transfers, and reports how each ended.
fn run_loop(
    inbox: Receiver<Command>,
    waker_tx: Sender<Result<MultiWaker, String>>,
    options: LoopOptions,
) {
    let mut multi = Multi::new();
    if let Err(e) = multi.set_max_host_connections(options.max_connections_per_host) {
        let _ = waker_tx.send(Err(format!("CURLMOPT_MAX_HOST_CONNECTIONS: {e}")));
        return;
    }
    if waker_tx.send(Ok(multi.waker())).is_err() {
        return;
    }
    let mut active: HashMap<usize, Easy2Handle<Collector>> = HashMap::new();
    // Where the next round of resuming rate-paused transfers starts (#49).
    let mut turn = 0usize;
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
                        .map_err(|e| (i64::from(e.code()), e.to_string()))
                        .and_then(|mut handle| {
                            handle
                                .set_token(token)
                                .map(|()| handle)
                                .map_err(|e| (i64::from(e.code()), e.to_string()))
                        });
                    match started {
                        Ok(handle) => {
                            active.insert(token, handle);
                        }
                        Err((code, message)) => {
                            let _ = events.send(FetchEvent::Done(Err(FetchError::Curl {
                                kind: CurlFailure::Local,
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
        // Transfers paused by a speed limit go on once all their limits have tokens again.
        // Resuming one hands it the bytes libcurl kept at once, so whoever is resumed first takes
        // a shared limit's tokens first: the order turns each round, so no transfer is always
        // last.
        let mut waiting: Vec<usize> = active
            .iter()
            .filter(|(_, h)| h.get_ref().flow.rate_paused.load(Ordering::Acquire))
            .map(|(token, _)| *token)
            .collect();
        let rate_paused = !waiting.is_empty();
        if rate_paused {
            waiting.sort_unstable();
            let start = turn % waiting.len();
            waiting.rotate_left(start);
            turn = turn.wrapping_add(1);
        }
        for token in waiting {
            let handle = &active[&token];
            let collector = handle.get_ref();
            if rate::ready(&collector.limits) {
                collector.flow.rate_paused.store(false, Ordering::Release);
                let _ = handle.unpause_write();
            }
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
                    // Stopped at its limit: the range it was asked for is complete.
                    Err(_) if handle.get_ref().stopped_at_limit => Ok(()),
                    Err(e) => match &handle.get_ref().abort {
                        Some(reason) => Err(FetchError::Aborted(reason.clone())),
                        None => Err(curl_error(&e)),
                    },
                };
                finish(&multi, handle, outcome);
            }
        }
        let cap = if rate_paused { RATE_CHECK } else { POLL_CAP };
        let wait = multi
            .get_timeout()
            .ok()
            .flatten()
            .map_or(cap, |t| t.min(cap));
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
            kind: CurlFailure::Local,
            code: i64::from(e.code()),
            message: format!("the transfer couldn't be removed: {e}"),
        })),
    };
    // The reader may have dropped its Fetch; then nobody is waiting for this event.
    let _ = events.send(FetchEvent::Done(outcome));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(code: curl_sys::CURLcode) -> CurlFailure {
        CurlFailure::of(&curl::Error::new(code))
    }

    #[test]
    fn libcurl_codes_map_to_failure_kinds() {
        let cases = [
            (curl_sys::CURLE_COULDNT_RESOLVE_HOST, CurlFailure::Resolve),
            (curl_sys::CURLE_COULDNT_RESOLVE_PROXY, CurlFailure::Resolve),
            (curl_sys::CURLE_COULDNT_CONNECT, CurlFailure::Connect),
            (curl_sys::CURLE_OPERATION_TIMEDOUT, CurlFailure::Timeout),
            (curl_sys::CURLE_GOT_NOTHING, CurlFailure::Interrupted),
            (curl_sys::CURLE_SEND_ERROR, CurlFailure::Interrupted),
            (curl_sys::CURLE_RECV_ERROR, CurlFailure::Interrupted),
            (curl_sys::CURLE_PARTIAL_FILE, CurlFailure::Interrupted),
            (curl_sys::CURLE_HTTP2, CurlFailure::Interrupted),
            (curl_sys::CURLE_HTTP2_STREAM, CurlFailure::Interrupted),
            (curl_sys::CURLE_SSL_CONNECT_ERROR, CurlFailure::TlsHandshake),
            (
                curl_sys::CURLE_PEER_FAILED_VERIFICATION,
                CurlFailure::Certificate,
            ),
            (curl_sys::CURLE_SSL_CACERT_BADFILE, CurlFailure::Certificate),
            (curl_sys::CURLE_SSL_ISSUER_ERROR, CurlFailure::Certificate),
            (
                curl_sys::CURLE_TOO_MANY_REDIRECTS,
                CurlFailure::TooManyRedirects,
            ),
            (curl_sys::CURLE_UNSUPPORTED_PROTOCOL, CurlFailure::Refused),
            (curl_sys::CURLE_URL_MALFORMAT, CurlFailure::Refused),
            (
                curl_sys::CURLE_FTP_WEIRD_SERVER_REPLY,
                CurlFailure::Protocol,
            ),
            (curl_sys::CURLE_BAD_CONTENT_ENCODING, CurlFailure::Protocol),
            (curl_sys::CURLE_OUT_OF_MEMORY, CurlFailure::Local),
            (curl_sys::CURLE_WRITE_ERROR, CurlFailure::Local),
            (curl_sys::CURLE_FAILED_INIT, CurlFailure::Other),
        ];
        for (code, expected) in cases {
            assert_eq!(kind(code), expected, "CURLcode {code}");
        }
    }

    #[test]
    fn the_connect_timeout_defaults_to_30_seconds() {
        assert_eq!(
            LoopOptions::default().connect_timeout,
            Duration::from_secs(30)
        );
    }
}
