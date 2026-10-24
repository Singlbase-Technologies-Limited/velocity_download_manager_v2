//! A local TLS front for the test servers (#69): it terminates TLS with a certificate from a test
//! CA made fresh for each front, and passes the bytes on to a cleartext backend, the HTTP/1.1
//! [`crate::TestServer`] or the HTTP/2 [`crate::Http2Server`], whose range modes and counters work
//! unchanged. HTTP/2 negotiated through ALPN is the same framing the HTTP/2 server speaks with prior
//! knowledge.
//!
//! - **Certificates** ([`Leaf`]): valid, without an extended key usage, for client authentication
//!   or code signing only, from an untrusted issuer, for another host, expired, or revoked.
//! - **Revocation:** every leaf names a CRL distribution point on a plain-HTTP server of its own,
//!   on a fresh port. Windows' Schannel refuses a certificate whose revocation it can't check
//!   ("the revocation status is unknown"), and caches the lists it fetched, so a fresh URL per
//!   front keeps one run's list from answering for another (prototype on #69).
//! - **Trust:** the client trusts the CA through [`TlsFront::ca_file`]; nothing is installed on the
//!   machine. Verification is never turned off.
//!
//! Each front runs on its own tokio runtime, which stops when the front is dropped.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rcgen::{
    date_time_ymd, BasicConstraints, CertificateParams, CertificateRevocationListParams,
    CrlDistributionPoint, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyIdMethod, KeyPair,
    KeyUsagePurpose, RevokedCertParams, SanType, SerialNumber,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio_rustls::TlsAcceptor;

/// The serial number of a [`Leaf::Revoked`] certificate, the one entry of its revocation list.
pub const REVOKED_SERIAL: [u8; 4] = [0x0F, 0x0F, 0x0F, 0x0F];

/// The server certificate a front presents. All are signed by the front's test CA, for
/// `localhost` and 127.0.0.1, valid from 1975 to 4096, unless the variant says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaf {
    /// Extended key usage: server authentication.
    Valid,
    /// No extended key usage extension: any purpose (RFC 5280 §4.2.1.12).
    NoExtendedKeyUsage,
    /// Client authentication only (the CVE-2026-8367 class: not for a server).
    ClientAuthOnly,
    /// Code signing and e-mail protection only.
    CodeSigningOnly,
    /// Valid, but signed by another CA that the client doesn't trust.
    UntrustedIssuer,
    /// Valid, but for `wrong-host.invalid` only.
    WrongHost,
    /// Valid from 2000-01-01 to 2000-01-02.
    Expired,
    /// Valid, and listed on its CA's revocation list.
    Revoked,
}

/// The application protocol the front offers through ALPN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpn {
    /// `http/1.1`, for the HTTP/1.1 backend.
    Http1,
    /// `h2`, for the HTTP/2 backend.
    Http2,
}

/// One TLS session the front completed: what was negotiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// rustls's name for the version: `TLSv1_2` or `TLSv1_3`.
    pub version: String,
    /// The protocol ALPN agreed on, if any.
    pub alpn: Option<String>,
}

/// A test CA made fresh, the leaf it signed, and the server of its revocation list on a fresh port
/// (#69). The TLS front uses one, and so does the FTPS server, which terminates TLS itself (#51).
/// It stops its revocation-list server, and removes its CA file, when dropped.
pub struct TestPki {
    ca_der: CertificateDer<'static>,
    ca_name: String,
    ca_dir: PathBuf,
    ca_file: PathBuf,
    crl_url: String,
    crl_requests: Arc<AtomicU32>,
    leaf_der: CertificateDer<'static>,
    leaf_key: PrivateKeyDer<'static>,
    runtime: Option<Runtime>,
}

impl TestPki {
    /// A new CA, `leaf` signed as its variant says, and the CA's revocation list served.
    pub fn start(leaf: Leaf) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a tokio runtime");
        let (crl_listener, crl_addr) = bind(&runtime);
        let crl_url = format!("http://{crl_addr}/ca.crl");
        // A name of its own, so a test that adds the CA to a trust store removes exactly it.
        let front = format!(
            "{}-{}",
            std::process::id(),
            NEXT_FRONT.fetch_add(1, Ordering::Relaxed)
        );
        let ca_name = format!("Velocity V2 test CA {front}");
        let pki = Pki::new(leaf, &ca_name, &crl_url);

        let ca_dir = std::env::temp_dir().join(format!("v2-tls-{front}"));
        std::fs::create_dir_all(&ca_dir).expect("a folder for the test CA");
        let ca_file = ca_dir.join("test-ca.pem");
        std::fs::write(&ca_file, &pki.ca_pem).expect("the test CA written");

        let crl_requests = Arc::new(AtomicU32::new(0));
        runtime.spawn(serve_crl(
            crl_listener,
            pki.crl_der,
            Arc::clone(&crl_requests),
        ));
        TestPki {
            ca_der: pki.ca_der,
            ca_name,
            ca_dir,
            ca_file,
            crl_url,
            crl_requests,
            leaf_der: pki.leaf_der,
            leaf_key: pki.leaf_key,
            runtime: Some(runtime),
        }
    }

    /// A rustls server configuration that presents the leaf, offering `alpn` if given.
    pub fn server_config(&self, alpn: Option<Alpn>) -> Arc<rustls::ServerConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("rustls's default versions")
            .with_no_client_auth()
            .with_single_cert(vec![self.leaf_der.clone()], self.leaf_key.clone_key())
            .expect("the leaf certificate and key");
        config.alpn_protocols = match alpn {
            Some(Alpn::Http1) => vec![b"http/1.1".to_vec()],
            Some(Alpn::Http2) => vec![b"h2".to_vec()],
            None => Vec::new(),
        };
        Arc::new(config)
    }

    /// The test CA as a PEM file, for the client to trust (`CURLOPT_CAINFO`).
    pub fn ca_file(&self) -> &Path {
        &self.ca_file
    }

    /// The test CA's common name, unique to this CA.
    pub fn ca_name(&self) -> &str {
        &self.ca_name
    }

    /// The test CA's certificate.
    pub fn ca_der(&self) -> CertificateDer<'static> {
        self.ca_der.clone()
    }

    /// Where the leaf's revocation list is served.
    pub fn crl_url(&self) -> &str {
        &self.crl_url
    }

    /// How many times the revocation list was fetched.
    pub fn crl_requests(&self) -> u32 {
        self.crl_requests.load(Ordering::Acquire)
    }

    fn runtime(&self) -> &Runtime {
        self.runtime.as_ref().expect("running until dropped")
    }
}

impl Drop for TestPki {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        let _ = std::fs::remove_dir_all(&self.ca_dir);
    }
}

/// A tokio listener on 127.0.0.1 and a free port, for `runtime`.
fn bind(runtime: &Runtime) -> (TcpListener, SocketAddr) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    listener
        .set_nonblocking(true)
        .expect("a non-blocking listener");
    let addr = listener.local_addr().expect("local address");
    let _entered = runtime.enter();
    (
        TcpListener::from_std(listener).expect("a tokio listener"),
        addr,
    )
}

/// A running TLS front on 127.0.0.1 and a free port, with its revocation-list server. It stops,
/// and removes its CA file, when dropped.
pub struct TlsFront {
    addr: SocketAddr,
    sessions: Arc<Mutex<Vec<Session>>>,
    pki: TestPki,
}

impl TlsFront {
    /// Starts a front for `backend` that presents `leaf` and offers `alpn`.
    pub fn start(backend: SocketAddr, leaf: Leaf, alpn: Alpn) -> Self {
        let pki = TestPki::start(leaf);
        let (listener, addr) = bind(pki.runtime());
        let acceptor = TlsAcceptor::from(pki.server_config(Some(alpn)));
        let sessions = Arc::<Mutex<Vec<Session>>>::default();
        pki.runtime()
            .spawn(accept(listener, acceptor, backend, Arc::clone(&sessions)));
        TlsFront {
            addr,
            sessions,
            pki,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `https://localhost:<port>/<name>`.
    pub fn url(&self, name: &str) -> String {
        format!("https://localhost:{}/{name}", self.addr.port())
    }

    /// The test CA as a PEM file, for the client to trust (`CURLOPT_CAINFO`).
    pub fn ca_file(&self) -> &Path {
        self.pki.ca_file()
    }

    /// The test CA's common name, unique to this front.
    pub fn ca_name(&self) -> &str {
        self.pki.ca_name()
    }

    /// The test CA's certificate.
    pub fn ca_der(&self) -> CertificateDer<'static> {
        self.pki.ca_der()
    }

    /// Where the leaf's revocation list is served.
    pub fn crl_url(&self) -> &str {
        self.pki.crl_url()
    }

    /// How many times the revocation list was fetched.
    pub fn crl_requests(&self) -> u32 {
        self.pki.crl_requests()
    }

    /// The TLS sessions completed so far, in order.
    pub fn sessions(&self) -> Vec<Session> {
        self.sessions.lock().unwrap().clone()
    }
}

static NEXT_FRONT: AtomicU64 = AtomicU64::new(0);

/// A front's certificates: its CA, the leaf it presents, and the CA's revocation list.
struct Pki {
    ca_pem: String,
    ca_der: CertificateDer<'static>,
    leaf_der: CertificateDer<'static>,
    leaf_key: PrivateKeyDer<'static>,
    crl_der: Vec<u8>,
}

impl Pki {
    fn new(leaf: Leaf, ca_name: &str, crl_url: &str) -> Self {
        let (ca, ca_issuer) = new_ca(ca_name);
        // An untrusted leaf is signed by a CA of its own, which the client never sees.
        let other = (leaf == Leaf::UntrustedIssuer).then(|| new_ca("Velocity V2 untrusted CA"));
        let signer = other.as_ref().map_or(&ca_issuer, |(_, issuer)| issuer);

        let host = if leaf == Leaf::WrongHost {
            "wrong-host.invalid"
        } else {
            "localhost"
        };
        let mut params = CertificateParams::new(vec![host.to_owned()]).expect("a host name");
        if leaf != Leaf::WrongHost {
            params
                .subject_alt_names
                .push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        }
        params.distinguished_name.push(DnType::CommonName, host);
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = match leaf {
            Leaf::NoExtendedKeyUsage => Vec::new(),
            Leaf::ClientAuthOnly => vec![ExtendedKeyUsagePurpose::ClientAuth],
            Leaf::CodeSigningOnly => vec![
                ExtendedKeyUsagePurpose::CodeSigning,
                ExtendedKeyUsagePurpose::EmailProtection,
            ],
            _ => vec![ExtendedKeyUsagePurpose::ServerAuth],
        };
        if leaf == Leaf::Expired {
            params.not_before = date_time_ymd(2000, 1, 1);
            params.not_after = date_time_ymd(2000, 1, 2);
        }
        if leaf == Leaf::Revoked {
            params.serial_number = Some(SerialNumber::from(REVOKED_SERIAL.to_vec()));
        }
        params.crl_distribution_points = vec![CrlDistributionPoint {
            uris: vec![crl_url.to_owned()],
        }];
        params.use_authority_key_identifier_extension = true;
        let leaf_key = KeyPair::generate().expect("a leaf key");
        let leaf_cert = params
            .signed_by(&leaf_key, signer)
            .expect("the leaf signed");

        let revoked_certs = if leaf == Leaf::Revoked {
            vec![RevokedCertParams {
                serial_number: SerialNumber::from(REVOKED_SERIAL.to_vec()),
                revocation_time: date_time_ymd(2001, 1, 1),
                reason_code: None,
                invalidity_date: None,
            }]
        } else {
            Vec::new()
        };
        let crl = CertificateRevocationListParams {
            this_update: date_time_ymd(2001, 1, 1),
            next_update: date_time_ymd(4000, 1, 1),
            crl_number: SerialNumber::from(vec![1]),
            issuing_distribution_point: None,
            revoked_certs,
            key_identifier_method: KeyIdMethod::Sha256,
        }
        .signed_by(&ca_issuer)
        .expect("the revocation list signed");

        Pki {
            ca_pem: ca.pem(),
            ca_der: ca.der().clone(),
            leaf_der: leaf_cert.der().clone(),
            leaf_key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
            crl_der: crl.der().to_vec(),
        }
    }
}

/// A self-signed test CA named `name`, and the issuer that signs with it.
fn new_ca(name: &str) -> (rcgen::Certificate, Issuer<'static, KeyPair>) {
    let key = KeyPair::generate().expect("a CA key");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, name);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key).expect("the CA self-signed");
    (cert, Issuer::new(params, key))
}

/// Accepts TLS connections and passes each one's bytes on to `backend`.
async fn accept(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    backend: SocketAddr,
    sessions: Arc<Mutex<Vec<Session>>>,
) {
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            continue;
        };
        let (acceptor, sessions) = (acceptor.clone(), Arc::clone(&sessions));
        tokio::spawn(async move {
            // A client that refuses the certificate ends the handshake: nothing to pass on.
            let Ok(mut tls) = acceptor.accept(sock).await else {
                return;
            };
            let (_, conn) = tls.get_ref();
            sessions.lock().unwrap().push(Session {
                version: conn
                    .protocol_version()
                    .map(|v| format!("{v:?}"))
                    .unwrap_or_default(),
                alpn: conn
                    .alpn_protocol()
                    .map(|p| String::from_utf8_lossy(p).into_owned()),
            });
            let Ok(mut plain) = TcpStream::connect(backend).await else {
                return;
            };
            let _ = tokio::io::copy_bidirectional(&mut tls, &mut plain).await;
        });
    }
}

/// Serves `crl` at any path as `application/pkix-crl`, over plain HTTP/1.1.
async fn serve_crl(listener: TcpListener, crl: Vec<u8>, requests: Arc<AtomicU32>) {
    let crl = Arc::new(crl);
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            continue;
        };
        let (crl, requests) = (Arc::clone(&crl), Arc::clone(&requests));
        tokio::spawn(async move {
            // Read the request head; its path doesn't matter.
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            requests.fetch_add(1, Ordering::AcqRel);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/pkix-crl\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                crl.len()
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.write_all(&crl).await;
            let _ = sock.shutdown().await;
        });
    }
}
