//! A local HTTP/1.1 server for V2's engine tests (#37). It serves seeded files over keep-alive
//! connections and misbehaves on demand, the way real servers do (research report section 5.4),
//! and counts what it sends so a test can prove, for example, that no second full transfer was
//! made. It can also freeze a process, for the supervisor's hang test (#48). Standard library
//! only, plus `windows-sys` on Windows for that. [`Http2Server`] serves the same files and range
//! modes over HTTP/2 (#47), [`ftp::FtpServer`] serves FTP and FTPS (#51), and [`ssh::SshServer`]
//! runs the machine's OpenSSH server for SFTP (#53). [`leaks::LeakScan`] looks for test secrets
//! wherever V2 writes or shows something (#71). Test code: it trusts its callers and isn't
//! hardened.

pub mod ftp;
mod http2;
pub mod leaks;
pub mod proxy;
pub mod ssh;
pub mod tls;

pub use http2::{Http2Options, Http2Server};

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How the server answers a `Range` request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeMode {
    /// RFC 9110 ranges: `206` with `Content-Range`, `416` past the end.
    #[default]
    Honour,
    /// Ignores `Range` and sends the whole file as `200`, without `Accept-Ranges` (RFC 9110 §14.2).
    Ignore,
    /// Answers `206` with the requested bytes but a `Content-Range` that doesn't parse.
    MalformedContentRange,
    /// Honours a range that starts at byte 0, but answers any other range with the whole file as
    /// `200`: the case the engine must abort before writing a byte (report section 5.1, step 4).
    FullBodyForNonZeroStart,
    /// Honours ranges, but never says the total: `Content-Range: bytes a-b/*`.
    UnknownTotal,
    /// Sends at most this many bytes for any range, with a `Content-Range` that says so: RFC 9110
    /// §14.2 lets a server send fewer bytes than asked.
    ShortRanges(u64),
    /// Answers any range with a `multipart/byteranges` `206`, and a request without `Range` with
    /// the whole file.
    Multipart,
    /// A malformed `Content-Range` on the first `n` responses to ranges after byte 0.
    MalformedLaterRanges(u32),
    /// On the first `count` responses to ranges after byte 0, closes the connection after `after`
    /// body bytes, although `Content-Length` promised the whole range.
    CutLaterBodies { count: u32, after: u64 },
    /// On the first `count` responses to ranges after byte 0, sends the `206` chunked, with
    /// `extra` bytes past the end its `Content-Range` gives.
    OverrunLaterRanges { count: u32, extra: u64 },
    /// Answers every request with this status and an empty body (`Retry-After: 1` with 429 and
    /// 503).
    Status(u16),
}

/// What the server does, beyond serving its files.
#[derive(Clone, Debug, Default)]
pub struct ServerOptions {
    pub range_mode: RangeMode,
    /// Limits every connection to this many body bytes per second.
    pub throttle_bytes_per_sec: Option<u64>,
    /// Limits all connections together to this many body bytes per second, the way a server that
    /// throttles per client IP does (#47).
    pub throttle_total_bytes_per_sec: Option<u64>,
    /// Limits only the first response to a range after byte 0 to this many body bytes per second,
    /// so that one connection is slower than the others (a range split, #42).
    pub throttle_first_later_range: Option<u64>,
    /// Paths answered with `302 Found` and this `Location`, for example to test that a redirect
    /// to a protocol outside the engine's allowlist is refused.
    pub redirects: Vec<(String, String)>,
    /// Answers the first requests with an error status, before any other mode (#48).
    pub first_statuses: Option<FirstStatuses>,
    /// Closes the first this many connections after reading a request, without answering (#48).
    pub drop_first: u32,
    /// Freezes the first response to a range after byte 0: it sends `after` body bytes, then
    /// nothing for `hold`, then closes the connection (#48).
    pub freeze_later_range: Option<Freeze>,
    /// Headers sent with every file: validators (#45).
    pub file_headers: FileHeaders,
    /// The file changes on the server part way, once per entry, in order of `after` (#45).
    pub changes: Vec<Change>,
    /// Closes the connection after this many body bytes of the first GET's answer, although its
    /// `Content-Length` promised more (#45).
    pub cut_first: Option<u64>,
    /// The first response over HTTP/1.1 whose body holds this byte of the file sends it with its
    /// bits flipped; later ones send it as it is: a piece that is bad once (#46).
    pub corrupt_once: Option<u64>,
}

/// Headers the server sends with a file, each as given and only when set (#45).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHeaders {
    /// `"v1"` or `W/"v1"`.
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub date: Option<String>,
    /// Sent as `Content-Encoding`, without encoding anything: the bytes are what the client stores.
    pub content_encoding: Option<String>,
    /// Sent as `Content-Disposition`, as given (#40).
    pub content_disposition: Option<String>,
    /// Sent as `Content-Type`, as given (#54).
    pub content_type: Option<String>,
}

/// The body of every redirect the server sends (#40).
pub const REDIRECT_BODY: &str = "<html><body>Moved</body></html>";

/// One request as the server read it: its path and headers (#40).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenRequest {
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl SeenRequest {
    /// The first header with this name, compared without case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// After the first `after` GETs, every GET is answered from `data` with `headers` instead, whatever
/// file it asks for. Another ETag is a changed file; the same bytes without one, a dropped ETag; other
/// bytes with the same ETag, a dishonest server (#45).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub after: u32,
    pub data: Vec<u8>,
    pub headers: FileHeaders,
}

/// The first `count` requests are answered with `status` and an empty body, with this
/// `Retry-After` value if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstStatuses {
    pub status: u16,
    pub count: u32,
    pub retry_after: Option<String>,
}

/// See [`ServerOptions::freeze_later_range`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freeze {
    pub after: u64,
    pub hold: Duration,
}

/// What the server has seen and sent so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub connections: u64,
    pub gets: u64,
    pub heads: u64,
    /// Response body bytes written to sockets (headers not counted).
    pub body_bytes_sent: u64,
    /// The `Range` header of each GET, in arrival order (`None` without one).
    pub ranges: Vec<Option<String>>,
    /// The `If-Range` header of each GET, in the same order (#45).
    pub if_ranges: Vec<Option<String>>,
    /// Responses that carried a limited misbehaviour, such as [`RangeMode::MalformedLaterRanges`].
    pub faults: u32,
    /// Whether the response that [`ServerOptions::throttle_first_later_range`] slows was sent.
    pub slowed: bool,
    /// When each GET arrived, in order.
    pub request_times: Vec<Instant>,
    /// Requests answered by [`ServerOptions::first_statuses`].
    pub first_statuses_sent: u32,
    /// Connections closed by [`ServerOptions::drop_first`].
    pub dropped: u32,
    /// Whether the response that [`ServerOptions::freeze_later_range`] freezes was sent.
    pub frozen: bool,
    /// Whether the byte [`ServerOptions::corrupt_once`] names was sent flipped.
    pub corrupted: bool,
    /// Responses to file requests being sent now, from their status line to their last byte.
    pub answering: u32,
    /// The most responses to file requests that were sent at once (#47).
    pub peak_answering: u32,
    /// HTTP/2 streams the client reset (`RST_STREAM`) before their last byte (#47).
    pub streams_reset: u32,
    /// Every HTTP/1.1 request, redirected ones included, in arrival order (#40).
    pub requests: Vec<SeenRequest>,
}

/// Counts one response in [`Counters::answering`] while it lives.
struct Answering<'a>(&'a Mutex<Counters>);

impl<'a> Answering<'a> {
    fn start(counters: &'a Mutex<Counters>) -> Self {
        let mut c = counters.lock().unwrap();
        c.answering += 1;
        c.peak_answering = c.peak_answering.max(c.answering);
        Answering(counters)
    }
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().answering -= 1;
    }
}

/// A running server on 127.0.0.1 and a free port. It stops when dropped.
pub struct TestServer {
    addr: SocketAddr,
    counters: Arc<Mutex<Counters>>,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Serves `files` (name, bytes) at `/<name>`.
    pub fn start(files: Vec<(String, Vec<u8>)>, options: ServerOptions) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local address");
        let files: Arc<HashMap<String, Arc<Vec<u8>>>> =
            Arc::new(files.into_iter().map(|(n, d)| (n, Arc::new(d))).collect());
        let counters = Arc::new(Mutex::new(Counters::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let pace = Arc::new(Pace::new(options.throttle_total_bytes_per_sec));
        let acceptor = {
            let (counters, stop) = (Arc::clone(&counters), Arc::clone(&stop));
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    counters.lock().unwrap().connections += 1;
                    let (files, counters, options, pace) = (
                        Arc::clone(&files),
                        Arc::clone(&counters),
                        options.clone(),
                        Arc::clone(&pace),
                    );
                    thread::spawn(move || {
                        serve_connection(stream, &files, &counters, &options, &pace)
                    });
                }
            })
        };
        TestServer {
            addr,
            counters,
            stop,
            acceptor: Some(acceptor),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port>/<name>`.
    pub fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.addr)
    }

    pub fn counters(&self) -> Counters {
        self.counters.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so the acceptor thread sees the flag and ends.
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.acceptor.take() {
            let _ = h.join();
        }
    }
}

/// Kills a process the way a crash ends it, without warning: `kill -9` on Linux, `taskkill /F` on
/// Windows. Returns once the kill command has finished. For crash tests and measurements (#38, #39).
pub fn kill_process(pid: u32) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("taskkill");
        c.args(["/F", "/PID", &pid.to_string()]);
        c
    } else {
        let mut c = std::process::Command::new("kill");
        c.args(["-9", &pid.to_string()]);
        c
    };
    let status = command
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} failed"))
    }
}

/// Whether a process with this id exists: `kill -0` on Linux, `tasklist` on Windows.
pub fn process_exists(pid: u32) -> Result<bool, String> {
    if cfg!(windows) {
        let filter = format!("PID eq {pid}");
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &filter, "/FO", "CSV", "/NH"])
            .output()
            .map_err(|e| e.to_string())?;
        // One CSV line per match, with the id quoted; "INFO: No tasks..." otherwise.
        Ok(String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")))
    } else {
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?;
        Ok(status.success())
    }
}

/// A process frozen by [`freeze_process`]; dropping it lets the process run again.
pub struct Frozen {
    pid: u32,
}

/// Freezes a process: it keeps running in the eyes of the system, but none of its threads run,
/// the way a hung process looks to the program that started it (#48). On Linux it is sent
/// `SIGSTOP` (`kill -STOP`), and dropping the guard sends `SIGCONT`. On Windows the calling thread
/// attaches as its debugger and never lets it continue (`DebugActiveProcess`); dropping the guard,
/// on the same thread, detaches. A killed process may not finish exiting on Windows until then.
pub fn freeze_process(pid: u32) -> Result<Frozen, String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            DebugActiveProcess, DebugSetProcessKillOnExit,
        };
        // SAFETY: plain Win32 calls that take a process id or a flag by value; no pointers.
        unsafe {
            if DebugActiveProcess(pid) == 0 {
                return Err(format!(
                    "DebugActiveProcess({pid}) failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // If the test ends first, the process isn't killed with it.
            DebugSetProcessKillOnExit(0);
        }
        Ok(Frozen { pid })
    }
    #[cfg(not(windows))]
    {
        signal(pid, "-STOP")?;
        Ok(Frozen { pid })
    }
}

#[cfg(not(windows))]
fn signal(pid: u32, which: &str) -> Result<(), String> {
    let status = std::process::Command::new("kill")
        .args([which, &pid.to_string()])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill {which} {pid} failed"))
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        // The process may be gone already; either way there is nothing left to undo.
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Diagnostics::Debug::DebugActiveProcessStop;
            // SAFETY: a plain Win32 call that takes a process id by value.
            unsafe {
                DebugActiveProcessStop(self.pid);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = signal(self.pid, "-CONT");
        }
    }
}

/// `len` deterministic pseudo-random bytes (xorshift64*), different for each seed.
pub fn seeded_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    Some(Request {
        method,
        path,
        headers,
    })
}

/// `bytes=a-b` or `bytes=a-` as an inclusive start and an optional inclusive end.
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start = a.trim().parse().ok()?;
    let end = if b.trim().is_empty() {
        None
    } else {
        Some(b.trim().parse().ok()?)
    };
    Some((start, end))
}

/// Paces the body bytes of all connections together ([`ServerOptions::throttle_total_bytes_per_sec`]).
struct Pace {
    rate: Option<u64>,
    /// When the current run of sending started, and the bytes reserved since.
    run: Mutex<(Instant, u64)>,
}

impl Pace {
    fn new(rate: Option<u64>) -> Self {
        Pace {
            rate: rate.filter(|r| *r > 0),
            run: Mutex::new((Instant::now(), 0)),
        }
    }

    /// Reserves `bytes` and returns when they may be sent, or `None` without a rate. Reservations
    /// are taken in turn, so the senders share the rate; an idle server doesn't save up a burst.
    fn reserve(&self, bytes: usize) -> Option<Instant> {
        let rate = self.rate?;
        let mut run = self.run.lock().unwrap();
        let now = Instant::now();
        if run.0 + Duration::from_secs_f64(run.1 as f64 / rate as f64) < now {
            *run = (now, 0);
        }
        let due = run.0 + Duration::from_secs_f64(run.1 as f64 / rate as f64);
        run.1 += bytes as u64;
        Some(due)
    }

    /// Waits until `bytes` more may be sent.
    fn wait(&self, bytes: usize) {
        if let Some(wait) = self
            .reserve(bytes)
            .and_then(|due| due.checked_duration_since(Instant::now()))
        {
            thread::sleep(wait);
        }
    }
}

fn serve_connection(
    stream: TcpStream,
    files: &HashMap<String, Arc<Vec<u8>>>,
    counters: &Mutex<Counters>,
    options: &ServerOptions,
    pace: &Pace,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut out = stream;
    while let Some(req) = read_request(&mut reader) {
        let head = req.method == "HEAD";
        let gets = {
            let mut c = counters.lock().unwrap();
            c.requests.push(SeenRequest {
                path: req.path.clone(),
                headers: req.headers.clone(),
            });
            if head {
                c.heads += 1;
            } else {
                c.gets += 1;
                c.ranges.push(req.header("Range").map(str::to_owned));
                c.if_ranges.push(req.header("If-Range").map(str::to_owned));
                c.request_times.push(Instant::now());
            }
            c.gets
        };
        let close = req
            .header("Connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        if !head && take_count(counters, |c| &mut c.dropped, options.drop_first) {
            // Close without a byte of answer: the client sees an empty reply.
            return;
        }
        if let Some(first) = options.first_statuses.as_ref() {
            if !head && take_count(counters, |c| &mut c.first_statuses_sent, first.count) {
                let retry = first
                    .retry_after
                    .as_ref()
                    .map(|v| format!("Retry-After: {v}\r\n"))
                    .unwrap_or_default();
                let code = first.status;
                let _ = out.write_all(
                    format!("HTTP/1.1 {code} Status\r\nContent-Length: 0\r\n{retry}\r\n")
                        .as_bytes(),
                );
                if close {
                    return;
                }
                continue;
            }
        }
        if let Some((_, location)) = options.redirects.iter().find(|(p, _)| *p == req.path) {
            // With a body, as servers send one: a client must never take it for the file's bytes.
            let body = REDIRECT_BODY;
            let _ = out.write_all(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Type: text/html\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            if close {
                return;
            }
            continue;
        }
        let Some(data) = files.get(req.path.trim_start_matches('/')) else {
            let _ = out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            if close {
                return;
            }
            continue;
        };
        let (data, file_headers) = match options.changes.iter().rfind(|c| gets > u64::from(c.after))
        {
            Some(change) => (&change.data, &change.headers),
            None => (&**data, &options.file_headers),
        };
        let range = req.header("Range").and_then(parse_range);
        let mode = options.range_mode;
        let fault = takes_fault(mode, range, counters);
        let served = Served {
            data,
            headers: file_headers,
            if_range: req.header("If-Range"),
        };
        let answer = answer(&served, range, mode, fault);
        let status = format!("{} {}", answer.status, answer.reason);
        let extra: String = answer
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect();
        let body = answer.body;
        if let RangeMode::Status(_) = mode {
            let _ = out.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n{extra}\r\n").as_bytes(),
            );
            if close {
                return;
            }
            continue;
        }
        let _answering = Answering::start(counters);
        if let (RangeMode::OverrunLaterRanges { extra: over, .. }, true) = (mode, fault) {
            let header = format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\n{extra}\r\n");
            if out.write_all(header.as_bytes()).is_err() {
                return;
            }
            let mut payload = body.to_vec();
            payload.resize(body.len() + over as usize, 0);
            if head || write_chunked(&mut out, &payload, counters).is_err() {
                return;
            }
            continue;
        }
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}\r\n",
            body.len()
        );
        if out.write_all(header.as_bytes()).is_err() {
            return;
        }
        if let (Some(after), 1, false) = (options.cut_first, gets, head) {
            let cut = (after as usize).min(body.len());
            let _ = write_body(&mut out, &body[..cut], None, counters, None);
            return;
        }
        if let (RangeMode::CutLaterBodies { after, .. }, true) = (mode, fault) {
            // Send part of the body, then drop the connection.
            let cut = (after as usize).min(body.len());
            let _ = write_body(
                &mut out,
                &body[..cut],
                options.throttle_bytes_per_sec,
                counters,
                Some(pace),
            );
            return;
        }
        if let (Some(freeze), Some((start, _))) = (options.freeze_later_range, range) {
            let first = {
                let mut c = counters.lock().unwrap();
                let first = start > 0 && !c.frozen;
                c.frozen |= first;
                first
            };
            if first {
                let cut = (freeze.after as usize).min(body.len());
                if write_body(&mut out, &body[..cut], None, counters, None).is_ok() {
                    thread::sleep(freeze.hold);
                }
                return;
            }
        }
        let flipped;
        let mut body = body;
        if let Some(at) = options.corrupt_once {
            // The body is the file from the range's start for a 206, and from byte 0 otherwise.
            let from = match (answer.status, range) {
                (206, Some((start, _))) => start,
                _ => 0,
            };
            if (from..from + body.len() as u64).contains(&at) {
                let mut c = counters.lock().unwrap();
                if !c.corrupted {
                    c.corrupted = true;
                    let mut copy = body.to_vec();
                    copy[(at - from) as usize] ^= 0xFF;
                    flipped = copy;
                    body = &flipped;
                }
            }
        }
        let mut rate = options.throttle_bytes_per_sec;
        if let (Some(slow), Some((start, _))) = (options.throttle_first_later_range, range) {
            let mut c = counters.lock().unwrap();
            if start > 0 && !c.slowed {
                c.slowed = true;
                rate = Some(slow);
            }
        }
        if !head && write_body(&mut out, body, rate, counters, Some(pace)).is_err() {
            return;
        }
        if close {
            return;
        }
    }
}

/// Counts one more of the first `limit` requests in the counter `field` picks, if there is room.
fn take_count(
    counters: &Mutex<Counters>,
    field: impl Fn(&mut Counters) -> &mut u32,
    limit: u32,
) -> bool {
    let mut c = counters.lock().unwrap();
    let n = field(&mut c);
    if *n < limit {
        *n += 1;
        true
    } else {
        false
    }
}

/// Counts one more faulty response, if fewer than `limit` were sent so far.
fn take_fault(counters: &Mutex<Counters>, limit: u32) -> bool {
    let mut c = counters.lock().unwrap();
    if c.faults < limit {
        c.faults += 1;
        true
    } else {
        false
    }
}

/// Whether this response carries `mode`'s limited misbehaviour. They apply only to ranges after
/// byte 0, so the probe stays honest.
fn takes_fault(
    mode: RangeMode,
    range: Option<(u64, Option<u64>)>,
    counters: &Mutex<Counters>,
) -> bool {
    match (mode, range) {
        (
            RangeMode::MalformedLaterRanges(n)
            | RangeMode::CutLaterBodies { count: n, .. }
            | RangeMode::OverrunLaterRanges { count: n, .. },
            Some((start, _)),
        ) if start > 0 => take_fault(counters, n),
        _ => false,
    }
}

/// What a server answers to a request for a file, the same over HTTP/1.1 and HTTP/2.
pub(crate) struct Answer<'a> {
    pub status: u16,
    pub reason: &'static str,
    /// Headers besides `Content-Length` (and the HTTP/1.1 framing).
    pub headers: Vec<(&'static str, String)>,
    pub body: &'a [u8],
}

/// A file as one request gets it: its bytes and headers, and the request's `If-Range`.
pub(crate) struct Served<'a> {
    pub data: &'a [u8],
    pub headers: &'a FileHeaders,
    pub if_range: Option<&'a str>,
}

impl Served<'_> {
    /// Whether `If-Range` lets a range through (RFC 9110 §13.1.5): a strong ETag equal to the
    /// file's own, or the file's exact `Last-Modified`. A weak tag never matches.
    fn if_range_holds(&self) -> bool {
        let Some(condition) = self.if_range.map(str::trim) else {
            return true;
        };
        if condition.starts_with('"') {
            self.headers.etag.as_deref() == Some(condition)
        } else {
            !condition.starts_with("W/") && self.headers.last_modified.as_deref() == Some(condition)
        }
    }
}

/// The answer to a request for `served` with `range` (inclusive, `(start, None)` is `start-`),
/// in `mode`; `fault` is whether this response carries the mode's limited misbehaviour. A range
/// whose `If-Range` doesn't hold gets the whole file as `200`.
pub(crate) fn answer<'a>(
    served: &Served<'a>,
    range: Option<(u64, Option<u64>)>,
    mode: RangeMode,
    fault: bool,
) -> Answer<'a> {
    let data = served.data;
    let total = data.len() as u64;
    let range = range.filter(|_| served.if_range_holds());
    if let RangeMode::Status(code) = mode {
        let mut headers = Vec::new();
        if matches!(code, 429 | 503) {
            headers.push(("Retry-After", "1".to_owned()));
        }
        return Answer {
            status: code,
            reason: "Status",
            headers,
            body: &[],
        };
    }
    let mut headers = Vec::new();
    if mode != RangeMode::Ignore {
        headers.push(("Accept-Ranges", "bytes".to_owned()));
    }
    let file = served.headers;
    headers.extend(file.etag.clone().map(|v| ("ETag", v)));
    headers.extend(file.last_modified.clone().map(|v| ("Last-Modified", v)));
    headers.extend(file.date.clone().map(|v| ("Date", v)));
    headers.extend(
        file.content_encoding
            .clone()
            .map(|v| ("Content-Encoding", v)),
    );
    headers.extend(
        file.content_disposition
            .clone()
            .map(|v| ("Content-Disposition", v)),
    );
    headers.extend(file.content_type.clone().map(|v| ("Content-Type", v)));
    let (status, reason, body): (u16, &'static str, &[u8]) = match (mode, range) {
        (RangeMode::Ignore, _) | (_, None) => (200, "OK", data),
        (RangeMode::FullBodyForNonZeroStart, Some((start, _))) if start > 0 => (200, "OK", data),
        (_, Some((start, _))) if start >= total => {
            headers.push(("Content-Range", format!("bytes */{total}")));
            (416, "Range Not Satisfiable", &[][..])
        }
        (_, Some((start, end))) => {
            let mut last = end.unwrap_or(total - 1).min(total - 1);
            if let RangeMode::ShortRanges(max) = mode {
                last = last.min(start + max.max(1) - 1);
            }
            headers.push(match mode {
                RangeMode::MalformedContentRange => {
                    ("Content-Range", format!("bytes {start}-oops/{total}"))
                }
                RangeMode::MalformedLaterRanges(_) if fault => {
                    ("Content-Range", format!("bytes {start}-oops/{total}"))
                }
                RangeMode::UnknownTotal => ("Content-Range", format!("bytes {start}-{last}/*")),
                RangeMode::Multipart => (
                    "Content-Type",
                    "multipart/byteranges; boundary=v2test".to_owned(),
                ),
                _ => ("Content-Range", format!("bytes {start}-{last}/{total}")),
            });
            (
                206,
                "Partial Content",
                &data[start as usize..=last as usize],
            )
        }
    };
    Answer {
        status,
        reason,
        headers,
        body,
    }
}

/// Writes `body` with chunked transfer coding, in 16 KiB chunks.
fn write_chunked(
    out: &mut TcpStream,
    body: &[u8],
    counters: &Mutex<Counters>,
) -> std::io::Result<()> {
    for chunk in body.chunks(16 * 1024) {
        counters.lock().unwrap().body_bytes_sent += chunk.len() as u64;
        out.write_all(format!("{:x}\r\n", chunk.len()).as_bytes())?;
        out.write_all(chunk)?;
        out.write_all(b"\r\n")?;
    }
    out.write_all(b"0\r\n\r\n")?;
    out.flush()
}

/// Writes `body` in 16 KiB chunks, pacing them to `rate` bytes per second when set, and to the
/// server-wide `pace` when given.
fn write_body(
    out: &mut TcpStream,
    body: &[u8],
    rate: Option<u64>,
    counters: &Mutex<Counters>,
    pace: Option<&Pace>,
) -> std::io::Result<()> {
    const CHUNK: usize = 16 * 1024;
    let started = Instant::now();
    let mut sent = 0u64;
    for chunk in body.chunks(CHUNK) {
        if let Some(rate) = rate.filter(|r| *r > 0) {
            let due = Duration::from_secs_f64(sent as f64 / rate as f64);
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                thread::sleep(wait);
            }
        }
        if let Some(pace) = pace {
            pace.wait(chunk.len());
        }
        // Counted before the write, so a client that has read these bytes always sees them
        // counted; counting after let a test read the counter first (#38). A failed write takes
        // its chunk back out.
        counters.lock().unwrap().body_bytes_sent += chunk.len() as u64;
        if let Err(e) = out.write_all(chunk) {
            counters.lock().unwrap().body_bytes_sent -= chunk.len() as u64;
            return Err(e);
        }
        sent += chunk.len() as u64;
    }
    out.flush()
}
