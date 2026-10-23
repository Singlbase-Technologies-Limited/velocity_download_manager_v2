//! The test server does exactly what each mode claims, checked with raw HTTP over a socket
//! (no client library), so engine tests can trust it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use velocity_test_http::{
    seeded_bytes, FirstStatuses, Freeze, RangeMode, ServerOptions, TestServer,
};

/// One parsed HTTP/1.1 response.
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Sends one request on `stream` and reads one full response (by Content-Length).
fn request(stream: &mut TcpStream, method: &str, path: &str, extra: &[(&str, &str)]) -> Response {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    // HTTP/1.1 requires CRLF line endings; a bare LF here is a server bug.
    assert!(line.ends_with("\r\n"), "status line without CRLF: {line:?}");
    let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').unwrap();
        headers.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = vec![0; if method == "HEAD" { 0 } else { len }];
    reader.read_exact(&mut body).unwrap();
    Response {
        status,
        headers,
        body,
    }
}

fn connect(server: &TestServer) -> TcpStream {
    let s = TcpStream::connect(server.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

const SIZE: usize = 100_000;

fn server(range_mode: RangeMode) -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(SIZE, 7);
    let s = TestServer::start(
        vec![("file.bin".into(), data.clone())],
        ServerOptions {
            range_mode,
            ..ServerOptions::default()
        },
    );
    (s, data)
}

#[test]
fn seeded_bytes_are_deterministic_and_vary_with_the_seed() {
    assert_eq!(seeded_bytes(1000, 1), seeded_bytes(1000, 1));
    assert_ne!(seeded_bytes(1000, 1), seeded_bytes(1000, 2));
    assert_eq!(seeded_bytes(1000, 1).len(), 1000);
}

#[test]
fn serves_the_whole_file_without_a_range() {
    let (s, data) = server(RangeMode::Honour);
    let r = request(&mut connect(&s), "GET", "/file.bin", &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, data);
    assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
    assert_eq!(s.counters().gets, 1);
    assert_eq!(s.counters().body_bytes_sent, SIZE as u64);
}

#[test]
fn honours_a_closed_and_an_open_range() {
    let (s, data) = server(RangeMode::Honour);
    let mut c = connect(&s);
    let r = request(&mut c, "GET", "/file.bin", &[("Range", "bytes=10-19")]);
    assert_eq!(r.status, 206);
    assert_eq!(
        r.header("Content-Range"),
        Some(format!("bytes 10-19/{SIZE}").as_str())
    );
    assert_eq!(r.body, &data[10..20]);
    // Same connection (keep-alive), an open-ended range.
    let r = request(&mut c, "GET", "/file.bin", &[("Range", "bytes=99990-")]);
    assert_eq!(r.status, 206);
    assert_eq!(r.body, &data[99_990..]);
    assert_eq!(s.counters().gets, 2);
    assert_eq!(s.counters().connections, 1);
    assert_eq!(
        s.counters().ranges,
        vec![
            Some("bytes=10-19".to_owned()),
            Some("bytes=99990-".to_owned())
        ]
    );
}

#[test]
fn shortens_a_range_past_the_end_and_refuses_one_that_starts_past_it() {
    let (s, data) = server(RangeMode::Honour);
    let mut c = connect(&s);
    let r = request(
        &mut c,
        "GET",
        "/file.bin",
        &[("Range", "bytes=99995-200000")],
    );
    assert_eq!(r.status, 206);
    assert_eq!(r.body, &data[99_995..]);
    let r = request(&mut c, "GET", "/file.bin", &[("Range", "bytes=200000-")]);
    assert_eq!(r.status, 416);
    assert_eq!(
        r.header("Content-Range"),
        Some(format!("bytes */{SIZE}").as_str())
    );
}

#[test]
fn ignores_ranges_in_ignore_mode() {
    let (s, data) = server(RangeMode::Ignore);
    let r = request(
        &mut connect(&s),
        "GET",
        "/file.bin",
        &[("Range", "bytes=10-19")],
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.body, data);
    assert_eq!(r.header("Accept-Ranges"), None);
}

#[test]
fn sends_a_malformed_content_range_in_that_mode() {
    let (s, _) = server(RangeMode::MalformedContentRange);
    let r = request(
        &mut connect(&s),
        "GET",
        "/file.bin",
        &[("Range", "bytes=0-9")],
    );
    assert_eq!(r.status, 206);
    let cr = r.header("Content-Range").unwrap();
    assert!(!cr.starts_with("bytes 0-9/"), "{cr}");
    assert_eq!(r.body.len(), 10);
}

#[test]
fn answers_a_non_zero_range_with_the_whole_file_in_that_mode() {
    let (s, data) = server(RangeMode::FullBodyForNonZeroStart);
    let mut c = connect(&s);
    let r = request(&mut c, "GET", "/file.bin", &[("Range", "bytes=0-9")]);
    assert_eq!((r.status, r.body.as_slice()), (206, &data[..10]));
    let r = request(&mut c, "GET", "/file.bin", &[("Range", "bytes=10-19")]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, data);
}

#[test]
fn answers_head_without_a_body_and_counts_it_apart() {
    let (s, _) = server(RangeMode::Honour);
    let r = request(&mut connect(&s), "HEAD", "/file.bin", &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Length"), Some(SIZE.to_string().as_str()));
    assert_eq!((s.counters().heads, s.counters().gets), (1, 0));
}

#[test]
fn reports_an_unknown_file_as_404() {
    let (s, _) = server(RangeMode::Honour);
    assert_eq!(
        request(&mut connect(&s), "GET", "/nope.bin", &[]).status,
        404
    );
}

#[test]
fn throttles_each_connection() {
    let data = seeded_bytes(200_000, 3);
    let s = TestServer::start(
        vec![("f".into(), data)],
        ServerOptions {
            throttle_bytes_per_sec: Some(1_000_000),
            ..ServerOptions::default()
        },
    );
    let started = std::time::Instant::now();
    let r = request(&mut connect(&s), "GET", "/f", &[]);
    assert_eq!(r.body.len(), 200_000);
    // 200 kB at 1 MB/s takes about 200 ms; allow for scheduling, but it must not be instant.
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn redirects_a_configured_path() {
    let data = seeded_bytes(10, 1);
    let s = TestServer::start(
        vec![("f".into(), data)],
        ServerOptions {
            redirects: vec![("/go".into(), "file:///etc/passwd".into())],
            ..ServerOptions::default()
        },
    );
    let r = request(&mut connect(&s), "GET", "/go", &[]);
    assert_eq!(r.status, 302);
    assert_eq!(r.header("Location"), Some("file:///etc/passwd"));
}

fn server_with(options: ServerOptions) -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(SIZE, 7);
    (
        TestServer::start(vec![("file.bin".into(), data.clone())], options),
        data,
    )
}

#[test]
fn answers_the_first_requests_with_a_status_and_its_retry_after() {
    let (s, data) = server_with(ServerOptions {
        first_statuses: Some(FirstStatuses {
            status: 429,
            count: 2,
            retry_after: Some("5".into()),
        }),
        ..ServerOptions::default()
    });
    let mut c = connect(&s);
    for _ in 0..2 {
        let r = request(&mut c, "GET", "/file.bin", &[]);
        assert_eq!((r.status, r.header("Retry-After")), (429, Some("5")));
    }
    let r = request(&mut c, "GET", "/file.bin", &[]);
    assert_eq!(r.status, 200);
    assert!(r.body == data);
    let counters = s.counters();
    assert_eq!(counters.request_times.len(), 3);
    assert!(counters.request_times.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn drops_the_first_connections_without_answering() {
    let (s, _) = server_with(ServerOptions {
        drop_first: 1,
        ..ServerOptions::default()
    });
    let mut c = connect(&s);
    c.write_all(b"GET /file.bin HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut buf = [0u8; 16];
    // Closed without a byte of answer: end of stream, or a reset on some systems.
    assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)));
    assert_eq!(
        request(&mut connect(&s), "GET", "/file.bin", &[]).status,
        200
    );
    assert_eq!((s.counters().dropped, s.counters().gets), (1, 2));
}

#[test]
fn freezes_the_first_later_range_after_some_bytes() {
    let hold = Duration::from_millis(800);
    let (s, data) = server_with(ServerOptions {
        freeze_later_range: Some(Freeze { after: 1000, hold }),
        ..ServerOptions::default()
    });
    let mut c = connect(&s);
    c.write_all(b"GET /file.bin HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=5000-\r\n\r\n")
        .unwrap();
    let started = std::time::Instant::now();
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match c.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
    assert!(started.elapsed() >= hold, "{:?}", started.elapsed());
    let body_at = got
        .windows(4)
        .position(|w| w == [13, 10, 13, 10]) // the CRLF CRLF that ends the head
        .expect("a head")
        + 4;
    assert!(got.starts_with(b"HTTP/1.1 206"), "{:?}", &got[..12]);
    assert!(
        got[body_at..] == data[5000..6000],
        "only the first 1000 bytes, then nothing"
    );
    assert!(s.counters().frozen);
    // Only the first later range freezes.
    let r = request(
        &mut connect(&s),
        "GET",
        "/file.bin",
        &[("Range", "bytes=5000-")],
    );
    assert!(r.body == data[5000..]);
}

#[test]
fn counts_the_responses_it_sends_at_once() {
    // Two throttled responses overlap: each takes about 200 ms.
    let s = TestServer::start(
        vec![("f".into(), seeded_bytes(200_000, 3))],
        ServerOptions {
            throttle_bytes_per_sec: Some(1_000_000),
            ..ServerOptions::default()
        },
    );
    let clients: Vec<_> = (0..2)
        .map(|_| {
            let mut stream = connect(&s);
            std::thread::spawn(move || request(&mut stream, "GET", "/f", &[]).body.len())
        })
        .collect();
    for c in clients {
        assert_eq!(c.join().unwrap(), 200_000);
    }
    let c = s.counters();
    assert_eq!(c.peak_answering, 2);
    assert_eq!(c.answering, 0, "both responses ended");
}
