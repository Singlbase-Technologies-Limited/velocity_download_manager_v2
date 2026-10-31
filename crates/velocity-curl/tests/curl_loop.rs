//! The libcurl loop against the local test server (#37): real libcurl, real sockets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use velocity_curl::{
    AddressProblem, CurlFailure, CurlLoop, Fetch, FetchError, FetchEvent, FetchRequest, Gate,
    Libcurl, LoopOptions, RateLimit, RequestOptions, ResponseHead,
};
use velocity_test_http::{seeded_bytes, RangeMode, ServerOptions, TestServer};

const WAIT: Duration = Duration::from_secs(20);

struct Outcome {
    head: Option<ResponseHead>,
    data: Vec<u8>,
    data_events: usize,
    done: Result<(), FetchError>,
}

/// Reads every event of `fetch` until Done, optionally pausing between events.
fn collect(fetch: &Fetch, pause_per_event: Option<Duration>) -> Outcome {
    let (mut head, mut data, mut data_events) = (None, Vec::new(), 0);
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match fetch
            .next_event(left)
            .expect("the fetch should finish in time")
        {
            FetchEvent::Head(h) => head = Some(h),
            FetchEvent::Data(chunk) => {
                data_events += 1;
                data.extend_from_slice(&chunk);
                if let Some(p) = pause_per_event {
                    std::thread::sleep(p);
                }
            }
            FetchEvent::Done(done) => {
                return Outcome {
                    head,
                    data,
                    data_events,
                    done,
                }
            }
        }
    }
}

fn start_loop(options: LoopOptions) -> CurlLoop {
    let curl = Libcurl::start().expect("libcurl starts");
    CurlLoop::start(&curl, options).expect("the loop starts")
}

fn serve(name: &str, len: usize, options: ServerOptions) -> (TestServer, Vec<u8>) {
    let data = seeded_bytes(len, 11);
    (
        TestServer::start(vec![(name.into(), data.clone())], options),
        data,
    )
}

fn accept_all(_: &ResponseHead) -> Gate {
    Gate::Accept
}

#[test]
fn fetches_a_whole_file() {
    let (s, data) = serve("a.bin", 300_000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let o = collect(
        &l.fetch(FetchRequest::get(s.url("a.bin")), accept_all),
        None,
    );
    assert_eq!(o.done, Ok(()));
    assert_eq!(o.head.map(|h| h.status), Some(200));
    assert!(o.data == data, "the body differs from the source");
    assert_eq!(s.counters().gets, 1);
}

#[test]
fn sends_the_requested_range_and_no_accept_encoding() {
    let (s, data) = serve("a.bin", 50_000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let o = collect(
        &l.fetch(
            FetchRequest::range(s.url("a.bin"), 100, Some(199)),
            accept_all,
        ),
        None,
    );
    assert_eq!(o.done, Ok(()));
    let head = o.head.unwrap();
    assert_eq!(head.status, 206);
    assert_eq!(head.header("content-range"), Some("bytes 100-199/50000"));
    assert_eq!(o.data, &data[100..200]);
    assert_eq!(s.counters().ranges, vec![Some("bytes=100-199".to_owned())]);
    assert_eq!(s.counters().if_ranges, vec![None]);
}

#[test]
fn sends_if_range_with_the_range_when_given_a_validator() {
    let (s, _) = serve("a.bin", 50_000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let validators = ["\"v1\"", "Sun, 06 Nov 1994 08:49:37 GMT"];
    for validator in validators {
        let request = FetchRequest::range(s.url("a.bin"), 100, Some(199))
            .if_range(Some(validator.to_owned()));
        assert_eq!(collect(&l.fetch(request, accept_all), None).done, Ok(()));
    }
    let c = s.counters();
    assert_eq!(c.ranges, vec![Some("bytes=100-199".to_owned()); 2]);
    assert_eq!(c.if_ranges, validators.map(|v| Some(v.to_owned())));
}

#[test]
fn refuses_an_if_range_value_that_would_break_the_request() {
    // A validator comes from a server: a line break in it must never add a header.
    let (s, _) = serve("a.bin", 50_000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    for bad in ["\"v1\"\r\nX-Injected: 1", "\"v1\"\n", "\"v\0\""] {
        let request =
            FetchRequest::range(s.url("a.bin"), 0, Some(9)).if_range(Some(bad.to_owned()));
        let done = collect(&l.fetch(request, accept_all), None).done;
        assert!(done.is_err(), "{bad:?}: {done:?}");
    }
    assert_eq!(s.counters().gets, 0, "nothing was sent");
}

#[test]
fn a_gate_that_aborts_delivers_no_body_bytes() {
    let (s, _) = serve(
        "a.bin",
        2_000_000,
        ServerOptions {
            range_mode: RangeMode::FullBodyForNonZeroStart,
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let reject_200 = |h: &ResponseHead| {
        if h.status == 200 {
            Gate::Abort("a 200 for a range that doesn't start at 0".into())
        } else {
            Gate::Accept
        }
    };
    let o = collect(
        &l.fetch(FetchRequest::range(s.url("a.bin"), 1000, None), reject_200),
        None,
    );
    assert_eq!(
        o.done,
        Err(FetchError::Aborted(
            "a 200 for a range that doesn't start at 0".into()
        ))
    );
    assert_eq!(o.head.map(|h| h.status), Some(200));
    assert_eq!(
        (o.data_events, o.data.len()),
        (0, 0),
        "no body byte may reach the reader"
    );
}

#[test]
fn runs_concurrent_fetches_on_one_loop() {
    let (s, data) = serve("a.bin", 400_000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let parts: Vec<(u64, u64)> = vec![
        (0, 99_999),
        (100_000, 199_999),
        (200_000, 299_999),
        (300_000, 399_999),
    ];
    let fetches: Vec<Fetch> = parts
        .iter()
        .map(|&(a, b)| l.fetch(FetchRequest::range(s.url("a.bin"), a, Some(b)), accept_all))
        .collect();
    for (f, &(a, b)) in fetches.iter().zip(&parts) {
        let o = collect(f, None);
        assert_eq!(o.done, Ok(()));
        assert!(
            o.data == data[a as usize..=b as usize],
            "part {a}-{b} differs"
        );
    }
    assert_eq!(s.counters().gets, 4);
    assert!(
        s.counters().connections <= 4,
        "{} connections",
        s.counters().connections
    );
}

#[test]
fn a_slow_reader_bounds_the_buffered_bytes() {
    let (s, data) = serve("a.bin", 2_000_000, ServerOptions::default());
    let cap = 64 * 1024;
    let l = start_loop(LoopOptions {
        buffer_bytes_per_fetch: cap,
        ..LoopOptions::default()
    });
    let f = l.fetch(FetchRequest::get(s.url("a.bin")), accept_all);
    let o = collect(&f, Some(Duration::from_millis(1)));
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data, "the body differs from the source");
    // libcurl hands over at most 16 KiB per write callback (CURL_MAX_WRITE_SIZE).
    assert!(
        f.peak_buffered_bytes() <= cap + 16 * 1024,
        "peak {} with a cap of {cap}",
        f.peak_buffered_bytes()
    );
}

#[test]
fn a_fetch_stops_cleanly_at_its_limit() {
    // A range split (#42, report section 5.2): the old request stops at the new boundary, and
    // that is a completed range, not an error.
    let (s, data) = serve("a.bin", 300_000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let f = l.fetch(FetchRequest::get(s.url("a.bin")), accept_all);
    f.stop_at(1234);
    let o = collect(&f, None);
    assert_eq!(o.done, Ok(()));
    assert_eq!(
        o.data,
        data[..1234].to_vec(),
        "exactly the bytes up to the limit"
    );
}

#[test]
fn a_limit_lowered_while_the_fetch_runs_stops_it_there() {
    let (s, data) = serve(
        "a.bin",
        4_000_000,
        ServerOptions {
            throttle_bytes_per_sec: Some(1_000_000),
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let f = l.fetch(FetchRequest::get(s.url("a.bin")), accept_all);
    let mut received = Vec::new();
    loop {
        match f.next_event(WAIT).expect("an event") {
            FetchEvent::Data(chunk) => {
                received.extend_from_slice(&chunk);
                break;
            }
            FetchEvent::Done(d) => panic!("finished before the limit was set: {d:?}"),
            FetchEvent::Head(_) => {}
        }
    }
    let limit = received.len() as u64 + 100_000;
    f.stop_at(limit);
    let started = Instant::now();
    let o = collect(&f, None);
    received.extend_from_slice(&o.data);
    assert_eq!(o.done, Ok(()));
    assert_eq!(received.len() as u64, limit);
    assert!(
        received == data[..limit as usize],
        "the bytes are the file's"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it stopped at the limit instead of running to the end ({:?})",
        started.elapsed()
    );
}

#[test]
fn a_cancelled_fetch_ends_and_the_loop_keeps_working() {
    let (s, data) = serve(
        "a.bin",
        4_000_000,
        ServerOptions {
            throttle_bytes_per_sec: Some(500_000),
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let f = l.fetch(FetchRequest::get(s.url("a.bin")), accept_all);
    // Wait for the first body bytes, then cancel.
    loop {
        match f.next_event(WAIT).expect("an event") {
            FetchEvent::Data(_) => break,
            FetchEvent::Done(d) => panic!("finished before cancel: {d:?}"),
            FetchEvent::Head(_) => {}
        }
    }
    let started = Instant::now();
    f.cancel();
    let o = collect(&f, None);
    assert_eq!(o.done, Err(FetchError::Cancelled));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel took {:?}",
        started.elapsed()
    );
    // The same loop serves the next fetch.
    let o = collect(
        &l.fetch(FetchRequest::range(s.url("a.bin"), 0, Some(9)), accept_all),
        None,
    );
    assert_eq!((o.done, o.data), (Ok(()), data[..10].to_vec()));
}

/// `file://`, `scp://` and `sftp://` are never reached through a redirect (#67; CVE-2026-86410
/// was a redirect to SFTP in the curl tool).
#[test]
fn a_redirect_to_a_protocol_outside_the_allowlist_is_refused() {
    for to in [
        "file:///etc/hosts",
        "scp://127.0.0.1:9/etc/hosts",
        "sftp://127.0.0.1:9/etc/hosts",
    ] {
        let (s, _) = serve(
            "a.bin",
            10,
            ServerOptions {
                redirects: vec![("/go".into(), to.into())],
                ..ServerOptions::default()
            },
        );
        let l = start_loop(LoopOptions::default());
        let o = collect(&l.fetch(FetchRequest::get(s.url("go")), accept_all), None);
        assert!(
            matches!(
                o.done,
                Err(FetchError::RedirectRefused {
                    kind: CurlFailure::Refused,
                    ..
                })
            ),
            "{to}: {:?}",
            o.done
        );
        assert!(o.data.is_empty(), "{to}");
    }
}

/// A request that must confirm local addresses (#67, ADR-0006).
fn confirming_local_addresses() -> Arc<RequestOptions> {
    Arc::new(RequestOptions {
        confirm_local_addresses: true,
        ..RequestOptions::default()
    })
}

#[test]
fn a_request_that_must_confirm_local_addresses_stops_at_one_before_sending() {
    let (s, _) = serve("a.bin", 100, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let request = FetchRequest::get(s.url("a.bin")).with_options(confirming_local_addresses());
    let o = collect(&l.fetch(request, accept_all), None);
    assert_eq!(
        o.done,
        Err(FetchError::LocalAddress(AddressProblem {
            host: "127.0.0.1".to_owned(),
            address: "127.0.0.1".to_owned(),
        }))
    );
    assert!(o.head.is_none() && o.data.is_empty());
    assert_eq!(s.counters().gets, 0, "no request reached the server");
}

/// The check is on the address connected to, not on the name: a name that resolves to this
/// computer is caught too, as DNS rebinding would make it.
#[test]
fn a_name_that_resolves_to_this_computer_is_caught_on_its_address() {
    let (s, _) = serve("a.bin", 100, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let url = format!("http://localhost:{}/a.bin", s.addr().port());
    let request = FetchRequest::get(url).with_options(confirming_local_addresses());
    let o = collect(&l.fetch(request, accept_all), None);
    match o.done {
        Err(FetchError::LocalAddress(AddressProblem { host, address })) => {
            assert_eq!(host, "localhost");
            assert!(
                ["127.0.0.1", "::1"].contains(&address.as_str()),
                "{address}"
            );
        }
        other => panic!("expected a local address, got {other:?}"),
    }
    assert_eq!(s.counters().gets, 0, "no request reached the server");
}

#[test]
fn a_request_without_the_check_reaches_a_local_address() {
    let (s, data) = serve("a.bin", 100, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let o = collect(
        &l.fetch(FetchRequest::get(s.url("a.bin")), accept_all),
        None,
    );
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
}

#[test]
fn reports_an_error_status_to_the_gate() {
    let (s, _) = serve("a.bin", 10, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let only_success = |h: &ResponseHead| {
        if h.status >= 400 {
            Gate::Abort(format!("HTTP {}", h.status))
        } else {
            Gate::Accept
        }
    };
    let o = collect(
        &l.fetch(FetchRequest::get(s.url("missing.bin")), only_success),
        None,
    );
    assert_eq!(o.done, Err(FetchError::Aborted("HTTP 404".into())));
}

/// libcurl's own default would follow a redirect to FTP; V2 allows only HTTP(S) redirects, and
/// Velocity, which follows redirects itself (#40), refuses it before connecting.
#[test]
fn a_redirect_to_ftp_is_refused_by_the_redirect_allowlist() {
    let (s, _) = serve(
        "a.bin",
        10,
        ServerOptions {
            redirects: vec![("/go".into(), "ftp://127.0.0.1:9/x".into())],
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let o = collect(&l.fetch(FetchRequest::get(s.url("go")), accept_all), None);
    match o.done {
        Err(FetchError::RedirectRefused { kind, message }) => {
            assert_eq!(kind, CurlFailure::Refused);
            assert!(message.contains("ftp://"), "{message}");
        }
        other => panic!("expected a refused redirect, got {other:?}"),
    }
    assert!(o.data.is_empty());
}

/// What a fetch sends with each request (#40): the login as Basic, `alice:hunter2`.
fn options() -> Arc<RequestOptions> {
    Arc::new(RequestOptions {
        user_agent: Some("VelocityTest/1".to_owned()),
        referrer: Some("https://example.com/page".to_owned()),
        headers: vec![
            ("X-Custom".to_owned(), "custom-value".to_owned()),
            ("Cookie".to_owned(), "session=secret".to_owned()),
            ("X-Empty".to_owned(), String::new()),
        ],
        login: Some(("alice".to_owned(), "hunter2".to_owned())),
        require_encryption: false,
        ssh_key: None,
        allow_insecure_redirect: false,
        confirm_local_addresses: false,
        proxy: None,
    })
}

const BASIC: &str = "Basic YWxpY2U6aHVudGVyMg==";

#[test]
fn sends_the_user_agent_referrer_custom_headers_and_login() {
    let (s, data) = serve("a.bin", 1000, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let request = FetchRequest::get(s.url("a.bin")).with_options(options());
    let o = collect(&l.fetch(request, accept_all), None);
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    let seen = &s.counters().requests[0];
    assert_eq!(seen.header("User-Agent"), Some("VelocityTest/1"));
    assert_eq!(seen.header("Referer"), Some("https://example.com/page"));
    assert_eq!(seen.header("X-Custom"), Some("custom-value"));
    assert_eq!(seen.header("Cookie"), Some("session=secret"));
    assert_eq!(seen.header("X-Empty"), Some(""));
    assert_eq!(seen.header("Authorization"), Some(BASIC));
}

#[test]
fn a_redirect_within_the_origin_keeps_everything_and_never_delivers_its_body() {
    let (s, data) = serve(
        "a.bin",
        1000,
        ServerOptions {
            redirects: vec![("/go".into(), "/a.bin".into())],
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let request = FetchRequest::get(s.url("go")).with_options(options());
    let o = collect(&l.fetch(request, accept_all), None);
    assert_eq!(o.done, Ok(()));
    assert!(
        o.data == data,
        "only the file's bytes, never the redirect's body"
    );
    let head = o.head.expect("the final head");
    assert_eq!(head.status, 200);
    assert_eq!(head.url, s.url("a.bin"));
    let requests = s.counters().requests;
    assert_eq!(
        requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        ["/go", "/a.bin"]
    );
    for seen in &requests {
        assert_eq!(seen.header("Authorization"), Some(BASIC), "{}", seen.path);
        assert_eq!(
            seen.header("X-Custom"),
            Some("custom-value"),
            "{}",
            seen.path
        );
        assert_eq!(
            seen.header("Cookie"),
            Some("session=secret"),
            "{}",
            seen.path
        );
    }
}

#[test]
fn a_redirect_to_another_origin_drops_the_login_cookie_and_custom_headers() {
    let (other, data) = serve("a.bin", 1000, ServerOptions::default());
    let (first, _) = serve(
        "unused.bin",
        10,
        ServerOptions {
            // Another port is another origin.
            redirects: vec![("/go".into(), other.url("a.bin"))],
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let request = FetchRequest::get(first.url("go")).with_options(options());
    let o = collect(&l.fetch(request, accept_all), None);
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    assert_eq!(o.head.expect("the final head").url, other.url("a.bin"));
    let sent = &first.counters().requests[0];
    assert_eq!(sent.header("Authorization"), Some(BASIC));
    let seen = &other.counters().requests[0];
    for header in ["Authorization", "Cookie", "X-Custom", "X-Empty"] {
        assert_eq!(
            seen.header(header),
            None,
            "{header} reached the other origin"
        );
    }
    // Not secrets: they still go.
    assert_eq!(seen.header("User-Agent"), Some("VelocityTest/1"));
    assert_eq!(seen.header("Referer"), Some("https://example.com/page"));
}

#[test]
fn a_redirect_loop_stops_after_ten_redirects() {
    let (s, _) = serve(
        "a.bin",
        10,
        ServerOptions {
            redirects: vec![("/loop".into(), "/loop".into())],
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions::default());
    let o = collect(&l.fetch(FetchRequest::get(s.url("loop")), accept_all), None);
    assert!(
        matches!(
            o.done,
            Err(FetchError::RedirectRefused {
                kind: CurlFailure::TooManyRedirects,
                ..
            })
        ),
        "{:?}",
        o.done
    );
    assert!(o.data.is_empty());
    // The first request and ten redirects followed.
    assert_eq!(s.counters().requests.len(), 11);
}

#[test]
fn refuses_a_header_that_would_break_the_request() {
    let (s, _) = serve("a.bin", 10, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let bad = Arc::new(RequestOptions {
        headers: vec![("X-A".to_owned(), "a\r\nInjected: 1".to_owned())],
        ..RequestOptions::default()
    });
    let o = collect(
        &l.fetch(
            FetchRequest::get(s.url("a.bin")).with_options(bad),
            accept_all,
        ),
        None,
    );
    assert!(
        matches!(o.done, Err(FetchError::Curl { .. })),
        "{:?}",
        o.done
    );
    assert_eq!(s.counters().requests.len(), 0, "nothing was sent");
}

/// Encryption is for FTP's `AUTH TLS` (#51): a plain http:// link can't have it, so a request that
/// requires it is refused rather than sent in clear.
#[test]
fn refuses_an_http_link_that_requires_encryption() {
    let (s, _) = serve("a.bin", 10, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let encrypted = Arc::new(RequestOptions {
        require_encryption: true,
        ..RequestOptions::default()
    });
    for url in [s.url("a.bin"), s.url("a.bin").replacen("http", "HTTP", 1)] {
        let o = collect(
            &l.fetch(
                FetchRequest::get(url.clone()).with_options(Arc::clone(&encrypted)),
                accept_all,
            ),
            None,
        );
        assert!(
            matches!(o.done, Err(FetchError::Curl { .. })),
            "{url}: {:?}",
            o.done
        );
    }
    assert_eq!(s.counters().requests.len(), 0, "nothing was sent");
}

/// `.invalid` never resolves (RFC 6761 §6.4), so this needs no network.
#[test]
fn a_host_that_does_not_resolve_is_a_resolve_failure() {
    let l = start_loop(LoopOptions::default());
    let o = collect(
        &l.fetch(
            FetchRequest::get("http://velocity-v2-test.invalid/a.bin".into()),
            accept_all,
        ),
        None,
    );
    match o.done {
        Err(FetchError::Curl { kind, .. }) => assert_eq!(kind, CurlFailure::Resolve),
        other => panic!("expected a resolve failure, got {other:?}"),
    }
}

#[test]
fn a_refused_connection_is_a_connect_failure() {
    // A port that was just free and has nothing listening on it.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let l = start_loop(LoopOptions::default());
    let o = collect(
        &l.fetch(
            FetchRequest::get(format!("http://127.0.0.1:{port}/a.bin")),
            accept_all,
        ),
        None,
    );
    match o.done {
        Err(FetchError::Curl { kind, .. }) => assert_eq!(kind, CurlFailure::Connect),
        other => panic!("expected a connect failure, got {other:?}"),
    }
}

/// Reads a whole fetch and returns how long it took.
fn timed(fetch: &Fetch) -> (Outcome, Duration) {
    let started = Instant::now();
    let o = collect(fetch, None);
    (o, started.elapsed())
}

#[test]
fn a_limited_fetch_holds_its_speed() {
    // #49: 2 MiB at 1 MiB/s takes about 2 s, within 10%.
    let (s, data) = serve("a.bin", 2 << 20, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let limit = RateLimit::new(Some(1 << 20));
    let request = FetchRequest::get(s.url("a.bin")).limited(&[limit]);
    let (o, took) = timed(&l.fetch(request, accept_all));
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data, "the body differs from the source");
    let secs = took.as_secs_f64();
    assert!((1.8..=2.2).contains(&secs), "took {secs:.2} s");
}

#[test]
fn a_changed_limit_applies_while_the_fetch_runs() {
    // 256 KiB/s for 1 s, then 2 MiB/s for the remaining 2.75 MiB: about 2.4 s in all.
    let (s, data) = serve("a.bin", 3 << 20, ServerOptions::default());
    let l = start_loop(LoopOptions::default());
    let limit = RateLimit::new(Some(256 << 10));
    let request = FetchRequest::get(s.url("a.bin")).limited(std::slice::from_ref(&limit));
    let fetch = l.fetch(request, accept_all);
    let changer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        limit.set(Some(2 << 20));
    });
    let (o, took) = timed(&fetch);
    changer.join().unwrap();
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data, "the body differs from the source");
    let expected = 1.0 + (3.0 - 0.25) / 2.0;
    let secs = took.as_secs_f64();
    assert!(
        (secs - expected).abs() <= expected * 0.1,
        "took {secs:.2} s, expected {expected:.2} s"
    );
}

#[test]
fn the_per_host_cap_changes_while_the_loop_runs() {
    // #57: the performance profile's cap reaches libcurl's own, without a new loop.
    let (s, data) = serve(
        "a.bin",
        200_000,
        ServerOptions {
            throttle_bytes_per_sec: Some(1_000_000),
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions {
        max_connections_per_host: 8,
        ..LoopOptions::default()
    });
    l.set_max_connections_per_host(2).expect("the cap changes");
    let fetches: Vec<Fetch> = (0..4)
        .map(|_| l.fetch(FetchRequest::get(s.url("a.bin")), accept_all))
        .collect();
    for f in &fetches {
        let o = collect(f, None);
        assert!(o.done.is_ok(), "{:?}", o.done);
        assert!(o.data == data);
    }
    assert_eq!(s.counters().peak_answering, 2, "held to the new cap");
}

#[test]
fn the_per_host_cap_queues_fetches_past_it() {
    // #47: CURLMOPT_MAX_HOST_CONNECTIONS is the backstop under the engine's own budget. Each
    // response takes about 200 ms, so the four would overlap without the cap.
    let (s, data) = serve(
        "a.bin",
        200_000,
        ServerOptions {
            throttle_bytes_per_sec: Some(1_000_000),
            ..ServerOptions::default()
        },
    );
    let l = start_loop(LoopOptions {
        max_connections_per_host: 2,
        ..LoopOptions::default()
    });
    let fetches: Vec<Fetch> = (0..4)
        .map(|_| l.fetch(FetchRequest::get(s.url("a.bin")), accept_all))
        .collect();
    for f in &fetches {
        let o = collect(f, None);
        assert!(o.done.is_ok(), "{:?}", o.done);
        assert!(o.data == data);
    }
    assert_eq!(s.counters().peak_answering, 2);
}
