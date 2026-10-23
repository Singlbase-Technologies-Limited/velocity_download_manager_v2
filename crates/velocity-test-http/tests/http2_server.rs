//! The HTTP/2 test server does what each option claims (#47), checked with h2's own client over a
//! socket, so engine tests can trust it.

use std::future::{poll_fn, Future};
use std::pin::pin;
use std::task::Poll;
use std::time::{Duration, Instant};

use bytes::Bytes;
use h2::client::SendRequest;
use http::{header, Request};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use velocity_test_http::{seeded_bytes, Http2Options, Http2Server};

const SIZE: usize = 200_000;

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Runs two futures at once and returns both outputs (tokio's `join!` needs its macros feature).
async fn join2<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let (mut a, mut b) = (pin!(a), pin!(b));
    let (mut done_a, mut done_b) = (None, None);
    poll_fn(|cx| {
        if done_a.is_none() {
            if let Poll::Ready(v) = a.as_mut().poll(cx) {
                done_a = Some(v);
            }
        }
        if done_b.is_none() {
            if let Poll::Ready(v) = b.as_mut().poll(cx) {
                done_b = Some(v);
            }
        }
        if done_a.is_some() && done_b.is_some() {
            Poll::Ready((done_a.take().unwrap(), done_b.take().unwrap()))
        } else {
            Poll::Pending
        }
    })
    .await
}

/// One HTTP/2 connection to `server`, with prior knowledge.
async fn connect(server: &Http2Server) -> SendRequest<Bytes> {
    let tcp = TcpStream::connect(server.addr()).await.unwrap();
    let (client, connection) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// A GET of `/f` on `client`'s connection: (status, Content-Range, body).
async fn get(
    client: &SendRequest<Bytes>,
    server: &Http2Server,
    range: Option<&str>,
) -> (u16, Option<String>, Vec<u8>) {
    let mut client = client.clone().ready().await.unwrap();
    let mut request = Request::get(server.url("f"));
    if let Some(r) = range {
        request = request.header(header::RANGE, r);
    }
    let (response, _) = client
        .send_request(request.body(()).unwrap(), true)
        .unwrap();
    let response = response.await.unwrap();
    let status = response.status().as_u16();
    let content_range = response
        .headers()
        .get(header::CONTENT_RANGE)
        .map(|v| v.to_str().unwrap().to_owned());
    let mut body = response.into_body();
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        body.flow_control().release_capacity(chunk.len()).unwrap();
        data.extend_from_slice(&chunk);
    }
    (status, content_range, data)
}

fn server(options: Http2Options) -> (Http2Server, Vec<u8>) {
    let data = seeded_bytes(SIZE, 9);
    (
        Http2Server::start(vec![("f".into(), data.clone())], options),
        data,
    )
}

#[test]
fn serves_concurrent_streams_and_ranges_on_one_connection() {
    let (s, data) = server(Http2Options::default());
    let rt = runtime();
    rt.block_on(async {
        let client = connect(&s).await;
        let (whole, part) = join2(
            get(&client, &s, None),
            get(&client, &s, Some("bytes=100-199")),
        )
        .await;
        assert_eq!(whole.0, 200);
        assert!(whole.2 == data);
        assert_eq!(part.0, 206);
        assert_eq!(
            part.1.as_deref(),
            Some(format!("bytes 100-199/{SIZE}").as_str())
        );
        assert!(part.2 == data[100..200]);
    });
    let c = s.counters();
    assert_eq!(
        (c.connections, c.gets),
        (1, 2),
        "two streams on one connection"
    );
}

/// How long two concurrent GETs of the whole file take, on one connection or on two.
fn two_at_once(s: &Http2Server, one_connection: bool) -> Duration {
    let rt = runtime();
    rt.block_on(async {
        let first = connect(s).await;
        let second = if one_connection {
            first.clone()
        } else {
            connect(s).await
        };
        let started = Instant::now();
        let (a, b) = join2(get(&first, s, None), get(&second, s, None)).await;
        assert_eq!((a.2.len(), b.2.len()), (SIZE, SIZE));
        started.elapsed()
    })
}

#[test]
fn throttles_each_stream_or_each_connection() {
    // 1 MB/s: one 200 kB response takes about 200 ms.
    let per_stream = server(Http2Options {
        throttle_stream_bytes_per_sec: Some(1_000_000),
        ..Http2Options::default()
    })
    .0;
    let took = two_at_once(&per_stream, true);
    assert!(took >= Duration::from_millis(150), "per stream: {took:?}");
    assert!(
        took < Duration::from_millis(350),
        "per stream, streams don't share: {took:?}"
    );

    let per_connection = server(Http2Options {
        throttle_connection_bytes_per_sec: Some(1_000_000),
        ..Http2Options::default()
    })
    .0;
    let took = two_at_once(&per_connection, true);
    assert!(
        took >= Duration::from_millis(350),
        "one connection's streams share: {took:?}"
    );
    let took = two_at_once(&per_connection, false);
    assert!(
        took < Duration::from_millis(350),
        "two connections don't share: {took:?}"
    );
}

#[test]
fn counts_a_stream_the_client_resets_and_keeps_its_connection() {
    let (s, data) = server(Http2Options {
        throttle_stream_bytes_per_sec: Some(200_000),
        ..Http2Options::default()
    });
    let rt = runtime();
    rt.block_on(async {
        let client = connect(&s).await;
        let mut ready = client.clone().ready().await.unwrap();
        let (response, _) = ready
            .send_request(Request::get(s.url("f")).body(()).unwrap(), true)
            .unwrap();
        let mut body = response.await.unwrap().into_body();
        // Read a little, then drop the body: h2 resets the stream (CANCEL).
        let first = body.data().await.unwrap().unwrap();
        assert!(!first.is_empty());
        drop(body);
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.counters().streams_reset == 0 {
            assert!(Instant::now() < deadline, "the reset was never seen");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // The connection still serves.
        let (status, _, again) = get(&client, &s, Some("bytes=0-99")).await;
        assert_eq!(status, 206);
        assert!(again == data[..100]);
    });
    let c = s.counters();
    assert_eq!(c.streams_reset, 1);
    assert_eq!(
        c.connections, 1,
        "a reset stream leaves its connection open"
    );
}
