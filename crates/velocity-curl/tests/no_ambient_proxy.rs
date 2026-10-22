//! V2 ignores proxy environment variables (ADR-0008): a proxy is only ever set from V2's own
//! settings. Its own test binary, because it changes the process environment.

use std::time::Duration;

use velocity_curl::{CurlLoop, FetchEvent, FetchRequest, Gate, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};

#[test]
fn proxy_environment_variables_are_ignored() {
    // Port 9 (discard) on loopback: nothing listens there, so a fetch that used this proxy would fail.
    for var in [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ] {
        // SAFETY: this test binary has one test and sets the variables before starting any thread
        // that reads the environment.
        unsafe { std::env::set_var(var, "http://127.0.0.1:9") };
    }
    let data = seeded_bytes(20_000, 5);
    let server = TestServer::start(
        vec![("a.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    let curl = Libcurl::start().expect("libcurl starts");
    let l = CurlLoop::start(&curl, LoopOptions::default()).expect("the loop starts");
    let fetch = l.fetch(FetchRequest::get(server.url("a.bin")), |_| Gate::Accept);
    let mut body = Vec::new();
    loop {
        match fetch
            .next_event(Duration::from_secs(20))
            .expect("an event in time")
        {
            FetchEvent::Head(_) => {}
            FetchEvent::Data(chunk) => body.extend_from_slice(&chunk),
            FetchEvent::Done(done) => {
                assert_eq!(
                    done,
                    Ok(()),
                    "the fetch went through the environment's proxy"
                );
                break;
            }
        }
    }
    assert!(body == data);
    assert_eq!(
        server.counters().gets,
        1,
        "the request reached the server directly"
    );
}
