//! The libcurl loop over HTTP/2 (#47), against the local HTTP/2 test server with prior knowledge
//! (the `test-h2c` feature; PO decision on #47). Real libcurl, real sockets.

use std::time::{Duration, Instant};

use velocity_curl::{
    CurlLoop, Fetch, FetchEvent, FetchRequest, Gate, Libcurl, LoopOptions, ResponseHead,
};
use velocity_test_http::{seeded_bytes, Http2Options, Http2Server};

const WAIT: Duration = Duration::from_secs(20);

fn h2c_loop() -> CurlLoop {
    let curl = Libcurl::start().expect("libcurl starts");
    CurlLoop::start(
        &curl,
        LoopOptions {
            http2_prior_knowledge: true,
            ..LoopOptions::default()
        },
    )
    .expect("the loop starts")
}

fn accept_all(_: &ResponseHead) -> Gate {
    Gate::Accept
}

/// Reads `fetch` to its end: (head, body bytes, finished cleanly).
fn finish(fetch: &Fetch) -> (Option<ResponseHead>, Vec<u8>, bool) {
    let (mut head, mut data) = (None, Vec::new());
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match fetch.next_event(left).expect("the fetch ends in time") {
            FetchEvent::Head(h) => head = Some(h),
            FetchEvent::Data(chunk) => data.extend_from_slice(&chunk),
            FetchEvent::Done(done) => return (head, data, done.is_ok()),
        }
    }
}

fn serve(options: Http2Options) -> (Http2Server, Vec<u8>) {
    let data = seeded_bytes(400_000, 61);
    (
        Http2Server::start(vec![("f.bin".into(), data.clone())], options),
        data,
    )
}

/// Every fetch's stream runs at 1 MB/s, so 4 at once overlap.
fn slow_streams() -> Http2Options {
    Http2Options {
        throttle_stream_bytes_per_sec: Some(1_000_000),
        ..Http2Options::default()
    }
}

#[test]
fn concurrent_fetches_share_one_http2_connection() {
    let (s, data) = serve(slow_streams());
    let l = h2c_loop();
    let fetches: Vec<Fetch> = (0..4)
        .map(|_| l.fetch(FetchRequest::get(s.url("f.bin")), accept_all))
        .collect();
    for f in &fetches {
        let (head, body, ok) = finish(f);
        assert!(ok);
        assert!(body == data);
        assert!(head.expect("a head").http2, "answered over HTTP/2");
    }
    let c = s.counters();
    assert_eq!(c.peak_answering, 4, "the four ran at once");
    assert_eq!(c.connections, 1, "as streams on one connection");
}

#[test]
fn a_fetch_on_its_own_connection_opens_a_new_one() {
    let (s, data) = serve(slow_streams());
    let l = h2c_loop();
    let fetches: Vec<Fetch> = (0..2)
        .map(|_| {
            l.fetch(
                FetchRequest::get(s.url("f.bin")).on_own_connection(),
                accept_all,
            )
        })
        .collect();
    for f in &fetches {
        let (_, body, ok) = finish(f);
        assert!(ok && body == data);
    }
    assert_eq!(s.counters().connections, 2, "one connection each");
}

#[test]
fn stopping_a_fetch_resets_only_its_stream() {
    // #42's criterion, moved to #47: a split stops the old request at the new boundary. Over
    // HTTP/2 that resets the stream and keeps the connection.
    let (s, data) = serve(slow_streams());
    let l = h2c_loop();
    let stopped = l.fetch(FetchRequest::get(s.url("f.bin")), accept_all);
    stopped.stop_at(100_000);
    let (_, body, ok) = finish(&stopped);
    assert!(ok, "a fetch stopped at its limit ends cleanly");
    assert!(body == data[..100_000]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while s.counters().streams_reset == 0 {
        assert!(Instant::now() < deadline, "the stream was never reset");
        std::thread::sleep(Duration::from_millis(20));
    }
    let again = l.fetch(FetchRequest::get(s.url("f.bin")), accept_all);
    let (_, body, ok) = finish(&again);
    assert!(ok && body == data);
    let c = s.counters();
    assert_eq!(c.streams_reset, 1);
    assert_eq!(
        c.connections, 1,
        "the connection stayed open and was reused"
    );
}
