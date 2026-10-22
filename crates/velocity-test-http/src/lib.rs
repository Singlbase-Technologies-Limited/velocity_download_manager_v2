//! A local HTTP/1.1 server for V2's engine tests (#37). It serves seeded files over keep-alive
//! connections and misbehaves on demand, the way real servers do (research report section 5.4),
//! and counts what it sends so a test can prove, for example, that no second full transfer was
//! made. Standard library only. Test code: it trusts its callers and isn't hardened.

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
}

/// What the server does, beyond serving its files.
#[derive(Clone, Debug, Default)]
pub struct ServerOptions {
    pub range_mode: RangeMode,
    /// Limits every connection to this many body bytes per second.
    pub throttle_bytes_per_sec: Option<u64>,
    /// Paths answered with `302 Found` and this `Location`, for example to test that a redirect
    /// to a protocol outside the engine's allowlist is refused.
    pub redirects: Vec<(String, String)>,
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
        let acceptor = {
            let (counters, stop) = (Arc::clone(&counters), Arc::clone(&stop));
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    counters.lock().unwrap().connections += 1;
                    let (files, counters, options) =
                        (Arc::clone(&files), Arc::clone(&counters), options.clone());
                    thread::spawn(move || serve_connection(stream, &files, &counters, &options));
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

fn serve_connection(
    stream: TcpStream,
    files: &HashMap<String, Arc<Vec<u8>>>,
    counters: &Mutex<Counters>,
    options: &ServerOptions,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut out = stream;
    while let Some(req) = read_request(&mut reader) {
        let head = req.method == "HEAD";
        {
            let mut c = counters.lock().unwrap();
            if head {
                c.heads += 1;
            } else {
                c.gets += 1;
                c.ranges.push(req.header("Range").map(str::to_owned));
            }
        }
        let close = req
            .header("Connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        if let Some((_, location)) = options.redirects.iter().find(|(p, _)| *p == req.path) {
            let _ = out.write_all(
                format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
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
        let total = data.len() as u64;
        let range = req.header("Range").and_then(parse_range);
        let mode = options.range_mode;
        let accept_ranges = if mode == RangeMode::Ignore {
            ""
        } else {
            "Accept-Ranges: bytes\r\n"
        };

        // (status line, extra headers, body slice)
        let (status, extra, body): (&str, String, &[u8]) = match (mode, range) {
            (RangeMode::Ignore, _) | (_, None) => ("200 OK", String::new(), &data[..]),
            (RangeMode::FullBodyForNonZeroStart, Some((start, _))) if start > 0 => {
                ("200 OK", String::new(), &data[..])
            }
            (_, Some((start, _))) if start >= total => (
                "416 Range Not Satisfiable",
                format!("Content-Range: bytes */{total}\r\n"),
                &[][..],
            ),
            (_, Some((start, end))) => {
                let last = end.unwrap_or(total - 1).min(total - 1);
                let content_range = if mode == RangeMode::MalformedContentRange {
                    format!("Content-Range: bytes {start}-oops/{total}\r\n")
                } else {
                    format!("Content-Range: bytes {start}-{last}/{total}\r\n")
                };
                (
                    "206 Partial Content",
                    content_range,
                    &data[start as usize..=last as usize],
                )
            }
        };
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{accept_ranges}{extra}\r\n",
            body.len()
        );
        if out.write_all(header.as_bytes()).is_err() {
            return;
        }
        if !head && write_body(&mut out, body, options.throttle_bytes_per_sec, counters).is_err() {
            return;
        }
        if close {
            return;
        }
    }
}

/// Writes `body` in 16 KiB chunks, pacing them to `rate` bytes per second when set.
fn write_body(
    out: &mut TcpStream,
    body: &[u8],
    rate: Option<u64>,
    counters: &Mutex<Counters>,
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
