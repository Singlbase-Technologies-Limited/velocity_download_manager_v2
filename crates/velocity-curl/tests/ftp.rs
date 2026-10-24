//! FTP and FTPS through the libcurl loop (#51), against the local FTP server: real libcurl, real
//! sockets. FTP has no response head, so the loop builds one from the control replies (`SIZE`,
//! `MDTM`, `REST`, `RETR`) before the first body byte: `200` for the file, `206` for a range, `416`
//! past its end. The gate then decides as it does for HTTP.

use std::sync::Arc;
use std::time::{Duration, Instant};

use velocity_curl::{
    CurlFailure, CurlLoop, Fetch, FetchError, FetchEvent, FetchRequest, Gate, Libcurl, LoopOptions,
    RequestOptions, ResponseHead,
};
use velocity_test_http::ftp::{FtpOptions, FtpServer, FtpTls};
use velocity_test_http::seeded_bytes;
use velocity_test_http::tls::Leaf;

const WAIT: Duration = Duration::from_secs(30);
/// [`velocity_test_http::ftp::MODIFIED`] as an HTTP date.
const MODIFIED: &str = "Thu, 02 Jan 2020 03:04:05 GMT";
const LEN: usize = 300_000;

struct Outcome {
    head: Option<ResponseHead>,
    data: Vec<u8>,
    done: Result<(), FetchError>,
}

fn collect(fetch: &Fetch) -> Outcome {
    let (mut head, mut data) = (None, Vec::new());
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match fetch.next_event(left).expect("the fetch ends in time") {
            FetchEvent::Head(h) => {
                assert!(head.is_none(), "a second head: {h:?}");
                assert!(data.is_empty(), "a head after body bytes: {h:?}");
                head = Some(h);
            }
            FetchEvent::Data(chunk) => data.extend_from_slice(&chunk),
            FetchEvent::Done(done) => return Outcome { head, data, done },
        }
    }
}

fn serve(options: FtpOptions) -> (FtpServer, Vec<u8>) {
    let data = seeded_bytes(LEN, 51);
    (
        FtpServer::start(vec![("/pub/f.bin".into(), data.clone())], options),
        data,
    )
}

fn start_loop(server: &FtpServer) -> CurlLoop {
    let curl = Libcurl::start().expect("libcurl starts");
    let options = LoopOptions {
        ca_file: server.ca_file().map(ToOwned::to_owned),
        ..LoopOptions::default()
    };
    CurlLoop::start(&curl, options).expect("the loop starts")
}

fn run(server: &FtpServer, request: FetchRequest) -> Outcome {
    let lp = start_loop(server);
    collect(&lp.fetch(request, |_: &ResponseHead| Gate::Accept))
}

fn with(request: FetchRequest, options: RequestOptions) -> FetchRequest {
    request.with_options(Arc::new(options))
}

fn login(user: &str, password: &str) -> RequestOptions {
    RequestOptions {
        login: Some((user.to_owned(), password.to_owned())),
        ..RequestOptions::default()
    }
}

fn failure(done: &Result<(), FetchError>) -> CurlFailure {
    match done {
        Err(FetchError::Curl { kind, .. }) => *kind,
        other => panic!("expected a libcurl failure, got {other:?}"),
    }
}

#[test]
fn a_whole_file_comes_with_its_size_and_modification_time_as_a_head() {
    let (server, data) = serve(FtpOptions::default());
    let url = server.url("pub/f.bin");
    let o = run(&server, FetchRequest::get(url.clone()));
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data, "the body differs from the file");
    let head = o.head.expect("a head before the body");
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("content-length"),
        Some(LEN.to_string().as_str())
    );
    assert_eq!(head.header("last-modified"), Some(MODIFIED));
    assert!(head.header("date").is_some(), "{head:?}");
    assert_eq!(head.header("content-range"), None);
    assert_eq!(head.url, url);
    assert!(!head.http2);
    let record = server.record();
    assert_eq!(
        (record.retrs, record.rests.clone()),
        (1, vec![]),
        "{record:?}"
    );
    assert_eq!(record.data_connections, vec![false]);
}

#[test]
fn a_range_from_byte_0_is_the_whole_file_as_a_206_without_rest() {
    let (server, data) = serve(FtpOptions::default());
    let o = run(
        &server,
        FetchRequest::range(server.url("pub/f.bin"), 0, Some(u64::MAX - 1)),
    );
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    let head = o.head.unwrap();
    assert_eq!(head.status, 206);
    let range = format!("bytes 0-{}/{LEN}", LEN - 1);
    assert_eq!(head.header("content-range"), Some(range.as_str()));
    assert_eq!(server.record().rests, Vec::<u64>::new());
}

#[test]
fn a_range_after_byte_0_restarts_the_transfer_there() {
    let (server, data) = serve(FtpOptions::default());
    let o = run(
        &server,
        FetchRequest::range(server.url("pub/f.bin"), 1000, None),
    );
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data[1000..], "not the bytes from 1000");
    let head = o.head.unwrap();
    assert_eq!(head.status, 206);
    let range = format!("bytes 1000-{}/{LEN}", LEN - 1);
    assert_eq!(head.header("content-range"), Some(range.as_str()));
    assert_eq!(
        head.header("content-length"),
        Some((LEN - 1000).to_string().as_str())
    );
    assert_eq!(server.record().rests, vec![1000]);
}

#[test]
fn a_range_with_an_end_stops_there() {
    let (server, data) = serve(FtpOptions::default());
    let o = run(
        &server,
        FetchRequest::range(server.url("pub/f.bin"), 1000, Some(1999)),
    );
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data[1000..2000], "{} bytes", o.data.len());
    let head = o.head.unwrap();
    assert_eq!(head.status, 206);
    assert_eq!(
        head.header("content-range"),
        Some(format!("bytes 1000-1999/{LEN}").as_str())
    );
}

#[test]
fn without_size_the_file_has_no_length_and_a_later_range_is_refused_unread() {
    let (server, data) = serve(FtpOptions {
        no_size: true,
        ..FtpOptions::default()
    });
    let whole = run(&server, FetchRequest::get(server.url("pub/f.bin")));
    assert_eq!(whole.done, Ok(()));
    assert!(whole.data == data);
    let head = whole.head.unwrap();
    assert_eq!((head.status, head.header("content-length")), (200, None));

    // Without the size, a range can't be proven: a 200, aborted before any byte, whatever the
    // gate says (a 200 for a range after byte 0 is never written).
    let later = run(
        &server,
        FetchRequest::range(server.url("pub/f.bin"), 1000, None),
    );
    assert_eq!(later.head.map(|h| h.status), Some(200));
    assert!(later.data.is_empty(), "{} bytes", later.data.len());
    assert!(
        matches!(later.done, Err(FetchError::Aborted(_))),
        "{:?}",
        later.done
    );
}

#[test]
fn a_refused_rest_is_a_200_and_the_range_is_aborted_before_any_byte() {
    let (server, data) = serve(FtpOptions {
        no_rest: true,
        ..FtpOptions::default()
    });
    let later = run(
        &server,
        FetchRequest::range(server.url("pub/f.bin"), 1000, None),
    );
    let head = later.head.unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("content-length"),
        Some(LEN.to_string().as_str())
    );
    assert!(later.data.is_empty());
    assert!(
        matches!(later.done, Err(FetchError::Aborted(_))),
        "{:?}",
        later.done
    );
    // The whole file still comes, without REST.
    let whole = run(&server, FetchRequest::get(server.url("pub/f.bin")));
    assert_eq!(whole.done, Ok(()));
    assert!(whole.data == data);
}

#[test]
fn without_mdtm_there_is_no_last_modified() {
    let (server, _) = serve(FtpOptions {
        no_mdtm: true,
        ..FtpOptions::default()
    });
    let o = run(&server, FetchRequest::get(server.url("pub/f.bin")));
    assert_eq!(o.done, Ok(()));
    assert_eq!(o.head.unwrap().header("last-modified"), None);
}

#[test]
fn a_missing_file_fails_as_not_found_without_a_head() {
    let (server, _) = serve(FtpOptions::default());
    let o = run(&server, FetchRequest::get(server.url("pub/missing.bin")));
    assert_eq!(failure(&o.done), CurlFailure::NotFound, "{:?}", o.done);
    assert!(o.head.is_none());
}

#[test]
fn a_range_past_the_end_is_a_416_with_the_size() {
    let (server, _) = serve(FtpOptions::default());
    let o = run(
        &server,
        FetchRequest::range(server.url("pub/f.bin"), LEN as u64 + 10, None),
    );
    let head = o.head.expect("a 416 head");
    assert_eq!(head.status, 416);
    assert_eq!(
        head.header("content-range"),
        Some(format!("bytes */{LEN}").as_str())
    );
    assert!(o.data.is_empty());
    assert_eq!(
        o.done,
        Ok(()),
        "a 416 ends like an HTTP one, without a body"
    );
}

#[test]
fn an_empty_file_is_a_416_for_byte_0_and_a_200_of_nothing() {
    let server = FtpServer::start(vec![("/e.bin".into(), Vec::new())], FtpOptions::default());
    let probe = run(
        &server,
        FetchRequest::range(server.url("e.bin"), 0, Some(99)),
    );
    let head = probe.head.unwrap();
    assert_eq!(
        (head.status, head.header("content-range")),
        (416, Some("bytes */0"))
    );
    assert_eq!(probe.done, Ok(()));
    let whole = run(&server, FetchRequest::get(server.url("e.bin")));
    let head = whole.head.unwrap();
    assert_eq!(
        (head.status, head.header("content-length")),
        (200, Some("0"))
    );
    assert_eq!(whole.done, Ok(()));
}

#[test]
fn the_login_is_sent_as_the_ftp_user_and_password() {
    let (server, data) = serve(FtpOptions {
        login: Some(("alice".into(), "TOPSECRETFTP".into())),
        ..FtpOptions::default()
    });
    let o = run(
        &server,
        with(
            FetchRequest::get(server.url("pub/f.bin")),
            login("alice", "TOPSECRETFTP"),
        ),
    );
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    assert_eq!(
        server.record().logins,
        vec![("alice".to_owned(), "TOPSECRETFTP".to_owned())]
    );
}

#[test]
fn a_refused_login_fails_as_access_denied() {
    let (server, _) = serve(FtpOptions {
        login: Some(("alice".into(), "right".into())),
        ..FtpOptions::default()
    });
    let o = run(
        &server,
        with(
            FetchRequest::get(server.url("pub/f.bin")),
            login("alice", "wrong"),
        ),
    );
    assert_eq!(failure(&o.done), CurlFailure::AccessDenied, "{:?}", o.done);
}

#[test]
fn a_cut_transfer_fails_as_interrupted_after_its_bytes() {
    let (server, data) = serve(FtpOptions {
        cut_first: Some(100_000),
        ..FtpOptions::default()
    });
    let o = run(&server, FetchRequest::get(server.url("pub/f.bin")));
    assert_eq!(failure(&o.done), CurlFailure::Interrupted, "{:?}", o.done);
    assert!(o.data == data[..o.data.len()]);
    assert!(o.data.len() <= 100_000);
}

#[test]
fn explicit_ftps_encrypts_the_login_and_the_file() {
    let (server, data) = serve(FtpOptions {
        login: Some(("alice".into(), "TOPSECRETFTP".into())),
        tls: FtpTls::Explicit(Leaf::Valid),
        ..FtpOptions::default()
    });
    let options = RequestOptions {
        require_encryption: true,
        ..login("alice", "TOPSECRETFTP")
    };
    let o = run(
        &server,
        with(FetchRequest::get(server.url("pub/f.bin")), options),
    );
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    let record = server.record();
    assert_eq!(record.tls_controls, 1, "{record:?}");
    assert_eq!(record.data_connections, vec![true], "{record:?}");
    // AUTH TLS came before the login.
    let auth = record.commands.iter().position(|c| c == "AUTH");
    let user = record.commands.iter().position(|c| c == "USER");
    assert!(auth < user && auth.is_some(), "{:?}", record.commands);
}

#[test]
fn implicit_ftps_is_encrypted_from_the_first_byte() {
    let (server, data) = serve(FtpOptions {
        tls: FtpTls::Implicit(Leaf::Valid),
        ..FtpOptions::default()
    });
    assert!(server.url("x").starts_with("ftps://"));
    let o = run(&server, FetchRequest::get(server.url("pub/f.bin")));
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    let record = server.record();
    assert_eq!(record.tls_controls, 1);
    assert_eq!(record.data_connections, vec![true], "{record:?}");
}

#[test]
fn requiring_encryption_from_a_plain_server_fails_before_the_login() {
    let (server, _) = serve(FtpOptions::default());
    let options = RequestOptions {
        require_encryption: true,
        ..login("alice", "TOPSECRETFTP")
    };
    let o = run(
        &server,
        with(FetchRequest::get(server.url("pub/f.bin")), options),
    );
    assert_eq!(
        failure(&o.done),
        CurlFailure::EncryptionRefused,
        "{:?}",
        o.done
    );
    let record = server.record();
    assert!(
        record.logins.is_empty(),
        "the login went out in clear: {record:?}"
    );
    assert_eq!(record.retrs, 0);
}

#[test]
fn an_untrusted_ftps_certificate_is_refused_before_the_login() {
    let (server, _) = serve(FtpOptions {
        tls: FtpTls::Explicit(Leaf::UntrustedIssuer),
        ..FtpOptions::default()
    });
    let options = RequestOptions {
        require_encryption: true,
        ..login("alice", "TOPSECRETFTP")
    };
    let o = run(
        &server,
        with(FetchRequest::get(server.url("pub/f.bin")), options),
    );
    assert_eq!(failure(&o.done), CurlFailure::Certificate, "{:?}", o.done);
    assert!(server.record().logins.is_empty());
}

#[test]
fn an_ftp_server_without_tls_is_plain_unless_encryption_is_required() {
    // Without the option an explicit-FTPS server is used in plain: nothing asks for AUTH TLS.
    let (server, data) = serve(FtpOptions {
        tls: FtpTls::Explicit(Leaf::Valid),
        ..FtpOptions::default()
    });
    let o = run(&server, FetchRequest::get(server.url("pub/f.bin")));
    assert_eq!(o.done, Ok(()));
    assert!(o.data == data);
    let record = server.record();
    assert_eq!(
        (record.tls_controls, record.data_connections),
        (0, vec![false])
    );
}
