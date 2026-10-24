//! The TLS fixture itself (#69): a client that trusts only the test CA reaches the backend's files
//! through the TLS front, ALPN is offered as asked, and the revocation list is served.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::ServerName;
use velocity_test_http::tls::{Alpn, Leaf, TlsFront};
use velocity_test_http::{ServerOptions, TestServer};

/// One HTTP/1.1 GET of `path` through `front`, trusting only its CA; the response as text.
fn get(front: &TlsFront, path: &str) -> (String, Option<Vec<u8>>, String) {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(front.ca_der())
        .expect("the test CA is a valid root");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let name = ServerName::try_from("localhost").unwrap();
    let conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let sock = TcpStream::connect(front.addr()).unwrap();
    let mut tls = rustls::StreamOwned::new(conn, sock);
    write!(
        tls,
        "GET /{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    // A close without close_notify ends the read with an error once the body is in.
    let _ = tls.read_to_end(&mut response);
    let alpn = tls.conn.alpn_protocol().map(<[u8]>::to_vec);
    let version = format!("{:?}", tls.conn.protocol_version());
    (
        String::from_utf8_lossy(&response).into_owned(),
        alpn,
        version,
    )
}

#[test]
fn a_client_trusting_the_test_ca_reaches_the_backend_through_the_front() {
    let backend = TestServer::start(
        vec![("f.txt".into(), b"hello over tls".to_vec())],
        ServerOptions::default(),
    );
    let front = TlsFront::start(backend.addr(), Leaf::Valid, Alpn::Http1);
    let (response, alpn, version) = get(&front, "f.txt");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("hello over tls"), "{response}");
    assert_eq!(alpn.as_deref(), Some(&b"http/1.1"[..]));
    assert_eq!(version, "Some(TLSv1_3)");
    let sessions = front.sessions();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0].version, "TLSv1_3");
    assert_eq!(sessions[0].alpn.as_deref(), Some("http/1.1"));
    assert_eq!(
        front.url("f.txt"),
        format!("https://localhost:{}/f.txt", front.addr().port())
    );
}

#[test]
fn the_revocation_list_is_served_and_names_only_a_revoked_leaf() {
    let backend = TestServer::start(Vec::new(), ServerOptions::default());
    let valid = TlsFront::start(backend.addr(), Leaf::Valid, Alpn::Http1);
    let revoked = TlsFront::start(backend.addr(), Leaf::Revoked, Alpn::Http1);
    for (front, entries) in [(&valid, 0), (&revoked, 1)] {
        let crl = fetch_crl(front.crl_url());
        assert_eq!(revoked_entries(&crl), entries, "{}", front.crl_url());
        assert_eq!(front.crl_requests(), 1);
    }
    // Each front serves its own list on a fresh port: no cached list answers for another.
    assert_ne!(valid.crl_url(), revoked.crl_url());
}

/// The body of `GET <url>` from the plain-HTTP revocation-list server.
fn fetch_crl(url: &str) -> Vec<u8> {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let mut sock = TcpStream::connect(host).unwrap();
    write!(
        sock,
        "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    sock.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&response[..split]);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("application/pkix-crl"), "{head}");
    response[split + 4..].to_vec()
}

/// How many revoked-certificate entries a DER CRL holds, by counting the serials of its
/// `revokedCertificates` sequence the way a parser would (RFC 5280 §5.1): enough for a fixture.
fn revoked_entries(der: &[u8]) -> usize {
    // A CRL from this fixture revokes at most one certificate, with serial 0x0F0F0F0F.
    usize::from(
        der.windows(6)
            .any(|w| w == [0x02, 0x04, 0x0F, 0x0F, 0x0F, 0x0F]),
    )
}
