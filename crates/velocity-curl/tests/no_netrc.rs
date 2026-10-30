//! V2 never reads a `.netrc` file (#67): a login goes only where the user gave it. The libcurl V2
//! builds has `.netrc` support compiled out (`scripts/native/build.mjs`). Its own test binary,
//! because it changes the process environment.

use std::fs;
use std::time::Duration;

use velocity_curl::{CurlLoop, FetchEvent, FetchRequest, Gate, Libcurl, LoopOptions};
use velocity_test_http::{seeded_bytes, ServerOptions, TestServer};

#[test]
fn a_netrc_file_is_never_read() {
    let data = seeded_bytes(1_000, 67);
    let server = TestServer::start(
        vec![("a.bin".into(), data.clone())],
        ServerOptions::default(),
    );
    // A .netrc (and Windows' _netrc) with a login for the test server, where libcurl would look.
    let home = std::env::temp_dir().join(format!("velocity-no-netrc-{}", std::process::id()));
    fs::create_dir_all(&home).expect("a home folder");
    let netrc = "machine 127.0.0.1 login netrc-user password netrc-secret\n";
    for name in [".netrc", "_netrc"] {
        fs::write(home.join(name), netrc).expect("a netrc file");
    }
    for (var, value) in [
        ("HOME", home.clone()),
        ("USERPROFILE", home.clone()),
        ("CURL_HOME", home.clone()),
        ("NETRC", home.join(".netrc")),
    ] {
        // SAFETY: this test binary has one test and sets the variables before starting any thread
        // that reads the environment.
        unsafe { std::env::set_var(var, value) };
    }
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
                assert_eq!(done, Ok(()));
                break;
            }
        }
    }
    let _ = fs::remove_dir_all(&home);
    assert!(body == data);
    let seen = &server.counters().requests[0];
    assert_eq!(
        seen.header("Authorization"),
        None,
        "the login came from the .netrc file"
    );
}
