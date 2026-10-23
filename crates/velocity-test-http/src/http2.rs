//! A local HTTP/2 server over cleartext TCP, with prior knowledge (no TLS, no upgrade), for the
//! engine's HTTP/2 tests (#47). It serves the same files and range modes as the HTTP/1.1 server,
//! as concurrent streams on each connection, and can throttle each stream or each TCP connection,
//! so a test can tell multiplexing from separate connections. HTTP/2 over TLS (ALPN) is #69's
//! (PO decision on #47: Windows' Schannel refuses a test CA, as it can't check its revocation).
//!
//! It runs on its own tokio runtime, which stops when the server is dropped.

use std::collections::HashMap;
use std::future::poll_fn;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use h2::server::SendResponse;
use h2::{RecvStream, SendStream};
use http::{header, Method, Request, Response};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;

use crate::{answer, parse_range, takes_fault, Answering, Counters, Pace, RangeMode};

/// What the HTTP/2 server does, beyond serving its files.
#[derive(Clone, Debug, Default)]
pub struct Http2Options {
    pub range_mode: RangeMode,
    /// Limits every stream to this many body bytes per second, the way a server that throttles
    /// per request does.
    pub throttle_stream_bytes_per_sec: Option<u64>,
    /// Limits every TCP connection, all its streams together, to this many body bytes per second.
    pub throttle_connection_bytes_per_sec: Option<u64>,
    /// Limits only the first response to a range after byte 0 to this many body bytes per second,
    /// so that one stream is slower than the others (a range split, #42).
    pub throttle_first_later_range: Option<u64>,
}

/// A running HTTP/2 server on 127.0.0.1 and a free port. It stops when dropped.
pub struct Http2Server {
    addr: SocketAddr,
    counters: Arc<Mutex<Counters>>,
    runtime: Option<Runtime>,
}

type Files = Arc<HashMap<String, Arc<Vec<u8>>>>;

impl Http2Server {
    /// Serves `files` (name, bytes) at `/<name>`.
    pub fn start(files: Vec<(String, Vec<u8>)>, options: Http2Options) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a tokio runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local address");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let listener = {
            let _entered = runtime.enter();
            TcpListener::from_std(listener).expect("a tokio listener")
        };
        let files: Files = Arc::new(files.into_iter().map(|(n, d)| (n, Arc::new(d))).collect());
        let counters = Arc::new(Mutex::new(Counters::default()));
        runtime.spawn(accept(listener, files, Arc::clone(&counters), options));
        Http2Server {
            addr,
            counters,
            runtime: Some(runtime),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port>/<name>`, to be fetched with HTTP/2 prior knowledge.
    pub fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.addr)
    }

    /// What the server has seen and sent so far. `connections` counts TCP connections, and
    /// `gets` and `ranges` count streams.
    pub fn counters(&self) -> Counters {
        self.counters.lock().unwrap().clone()
    }
}

impl Drop for Http2Server {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

async fn accept(
    listener: TcpListener,
    files: Files,
    counters: Arc<Mutex<Counters>>,
    options: Http2Options,
) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            continue;
        };
        counters.lock().unwrap().connections += 1;
        tokio::spawn(serve_connection(
            socket,
            Arc::clone(&files),
            Arc::clone(&counters),
            options.clone(),
        ));
    }
}

/// Accepts the streams of one connection; polling `accept` also drives the connection.
async fn serve_connection(
    socket: TcpStream,
    files: Files,
    counters: Arc<Mutex<Counters>>,
    options: Http2Options,
) {
    let _ = socket.set_nodelay(true);
    let Ok(mut connection) = h2::server::handshake(socket).await else {
        return;
    };
    let pace = Arc::new(Pace::new(options.throttle_connection_bytes_per_sec));
    while let Some(Ok((request, respond))) = connection.accept().await {
        tokio::spawn(serve_stream(
            request,
            respond,
            Arc::clone(&files),
            Arc::clone(&counters),
            options.clone(),
            Arc::clone(&pace),
        ));
    }
}

async fn serve_stream(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    files: Files,
    counters: Arc<Mutex<Counters>>,
    options: Http2Options,
    pace: Arc<Pace>,
) {
    let head = request.method() == Method::HEAD;
    let range_header = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    {
        let mut c = counters.lock().unwrap();
        if head {
            c.heads += 1;
        } else {
            c.gets += 1;
            c.ranges.push(range_header.clone());
            c.request_times.push(Instant::now());
        }
    }
    let Some(data) = files
        .get(request.uri().path().trim_start_matches('/'))
        .cloned()
    else {
        let not_found = Response::builder()
            .status(404)
            .header(header::CONTENT_LENGTH, 0)
            .body(())
            .expect("a valid response");
        let _ = respond.send_response(not_found, true);
        return;
    };
    let range = range_header.as_deref().and_then(parse_range);
    let mode = options.range_mode;
    let fault = takes_fault(mode, range, &counters);
    let answer = answer(&data, range, mode, fault);
    let mut response = Response::builder()
        .status(answer.status)
        .header(header::CONTENT_LENGTH, answer.body.len());
    for (name, value) in &answer.headers {
        response = response.header(*name, value);
    }
    let response = response.body(()).expect("a valid response");
    let end = head || answer.body.is_empty();
    let Ok(mut stream) = respond.send_response(response, end) else {
        return;
    };
    if end {
        return;
    }
    let _answering = Answering::start(&counters);
    let mut rate = options.throttle_stream_bytes_per_sec;
    if let (Some(slow), Some((start, _))) = (options.throttle_first_later_range, range) {
        let mut c = counters.lock().unwrap();
        if start > 0 && !c.slowed {
            c.slowed = true;
            rate = Some(slow);
        }
    }
    if let Err(Stopped::Reset) = send_body(&mut stream, answer.body, rate, &counters, &pace).await {
        counters.lock().unwrap().streams_reset += 1;
    }
}

/// Why a body wasn't sent in full.
enum Stopped {
    /// The client reset the stream (`RST_STREAM`).
    Reset,
    /// The connection failed or closed.
    Gone,
}

/// Sends `body` in 16 KiB chunks within HTTP/2 flow control, pacing them to `rate` bytes per
/// second when set, and to the connection's `pace`.
async fn send_body(
    stream: &mut SendStream<Bytes>,
    body: &[u8],
    rate: Option<u64>,
    counters: &Mutex<Counters>,
    pace: &Pace,
) -> Result<(), Stopped> {
    const CHUNK: usize = 16 * 1024;
    let started = Instant::now();
    let mut sent = 0u64;
    let count = body.len().div_ceil(CHUNK);
    for (i, chunk) in body.chunks(CHUNK).enumerate() {
        if let Some(rate) = rate.filter(|r| *r > 0) {
            let due = started + Duration::from_secs_f64(sent as f64 / rate as f64);
            tokio::time::sleep_until(due.into()).await;
        }
        if let Some(due) = pace.reserve(chunk.len()) {
            tokio::time::sleep_until(due.into()).await;
        }
        let mut rest = Bytes::copy_from_slice(chunk);
        while !rest.is_empty() {
            stream.reserve_capacity(rest.len());
            let granted = match poll_fn(|cx| stream.poll_capacity(cx)).await {
                Some(Ok(n)) => n,
                Some(Err(e)) if e.is_reset() => return Err(Stopped::Reset),
                Some(Err(_)) => return Err(Stopped::Gone),
                // The stream can't send any more: find out whether the client reset it.
                None => {
                    return match poll_fn(|cx| stream.poll_reset(cx)).await {
                        Ok(_) => Err(Stopped::Reset),
                        Err(_) => Err(Stopped::Gone),
                    }
                }
            };
            let part = rest.split_to(granted.min(rest.len()));
            let last = rest.is_empty() && i + 1 == count;
            // Counted before the send, as the HTTP/1.1 server does (#38).
            counters.lock().unwrap().body_bytes_sent += part.len() as u64;
            if let Err(e) = stream.send_data(part, last) {
                return Err(if e.is_reset() {
                    Stopped::Reset
                } else {
                    Stopped::Gone
                });
            }
        }
        sent += chunk.len() as u64;
    }
    Ok(())
}
