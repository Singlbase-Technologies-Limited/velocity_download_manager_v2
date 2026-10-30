//! The libcurl loop (#37): one thread owns a libcurl `Multi` and runs every transfer on it, as
//! `.claude/rules/transfer-engine.md` requires. Other threads talk to it through commands and a
//! wakeup. Each fetch reports its final response head, then its body, then how it ended.
//!
//! - **A gate decides before any body byte:** the caller's gate sees the final response head in
//!   libcurl's header callback and can abort the transfer there, so a body it rejects is never
//!   delivered (report section 5.1, steps 3 and 4).
//! - **Bounded memory, no blocking:** a fetch buffers at most `buffer_bytes_per_fetch` bytes that
//!   its reader hasn't taken yet. When that is reached, the write callback pauses the transfer
//!   (`CURL_WRITEFUNC_PAUSE`) instead of blocking or growing, and the reader resumes it once it has
//!   caught up (report section 5.2; #49).
//! - **Speed limits:** a fetch draws from the [`RateLimit`]s in its request. The write callback
//!   pauses a transfer whose limit has no tokens, and the loop thread resumes it once all its
//!   limits have tokens again (`rate.rs`, #49).
//! - **Safe defaults:** only `http`, `https`, `ftp` and `ftps`, redirects only to `http` and
//!   `https` (from `https` to `http` only once the user confirmed it, #67), an explicitly empty
//!   proxy so `http_proxy` and the like are ignored (ADR-0008), no `Accept-Encoding`, and TLS
//!   verification as libcurl's default (on). libcurl is built without `.netrc` support.
//! - **Local addresses** (#67, ADR-0006): a request that must confirm them is stopped before it is
//!   sent when the address it connected to is this computer's or a private network's
//!   (`CURLOPT_PREREQFUNCTION`, `address.rs`).
//! - **FTP** (#51): its control replies become a response head before the first body byte
//!   (`ftp.rs`), so the gate works as for HTTP. A range is sent as `REST`, and its end is the
//!   fetch's body limit. Passive mode only, to the control connection's address.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use curl::easy::{Easy2, Handler, InfoType, List, WriteError};
use curl::multi::{Easy2Handle, Multi, MultiWaker};

use crate::address::is_local;
use crate::ftp::{is_ftp, FtpReplies};
use crate::pipe::{Fetch, FetchControl, Flow, GateFn, Offered, Sink, Transport};
use crate::rate::{self, RateLimit};
use crate::redirect::{next_hop, Refusal};
use crate::runtime::Libcurl;
use crate::url::{origin, Origin};

/// Protocols a fetch may use, and the ones a redirect may lead to. SFTP joins when #53 builds it.
const PROTOCOLS: &str = "http,https,ftp,ftps";
const REDIRECT_PROTOCOLS: &str = "http,https";
/// `CURLFTPAUTH_TLS` (curl.h of curl 8.23.0): explicit FTPS asks `AUTH TLS` first, not the older
/// `AUTH SSL`. curl-sys 0.4.91 doesn't define it.
const CURLFTPAUTH_TLS: std::os::raw::c_long = 2;
/// The longest the loop waits for sockets before checking commands and timers again. Commands also
/// wake it at once; this only bounds a missed wakeup.
const POLL_CAP: Duration = Duration::from_millis(100);
/// How often the loop checks whether transfers paused by a speed limit may go on (#49).
const RATE_CHECK: Duration = Duration::from_millis(10);
/// `CURLOPT_PROTOCOLS_STR` and `CURLOPT_REDIR_PROTOCOLS_STR` (curl.h of curl 8.23.0:
/// `CURLOPTTYPE_STRINGPOINT` + 318 and + 319). curl-sys 0.4.91 doesn't define them.
const CURLOPT_PROTOCOLS_STR: curl_sys::CURLoption = 10_318;
const CURLOPT_REDIR_PROTOCOLS_STR: curl_sys::CURLoption = 10_319;
/// `CURLOPT_PREREQFUNCTION` and `CURLOPT_PREREQDATA` (curl.h of curl 8.23.0:
/// `CURLOPTTYPE_FUNCTIONPOINT` + 312 and `CURLOPTTYPE_CBPOINT` + 313; libcurl 7.80+), and the
/// callback's answers `CURL_PREREQFUNC_OK` and `CURL_PREREQFUNC_ABORT`. curl-sys 0.4.91 doesn't
/// define them.
const CURLOPT_PREREQFUNCTION: curl_sys::CURLoption = 20_312;
const CURLOPT_PREREQDATA: curl_sys::CURLoption = 10_313;
const CURL_PREREQFUNC_OK: c_int = 0;
const CURL_PREREQFUNC_ABORT: c_int = 1;
/// `CURLE_PROXY` (curl.h of curl 8.23.0, libcurl 7.73+): a SOCKS proxy refused, its reason in
/// `CURLINFO_PROXY_ERROR`. curl-sys 0.4.91 doesn't define it.
const CURLE_PROXY: curl_sys::CURLcode = 97;

/// One HTTP(S) or FTP GET.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    /// An inclusive byte range: `(start, None)` is `start-`.
    pub range: Option<(u64, Option<u64>)>,
    /// The speed limits the fetch draws from, such as its download's and the global one (#49).
    pub limits: Vec<RateLimit>,
    /// Opens a connection of its own, closed when the fetch ends, instead of sharing one as an
    /// HTTP/2 stream (`CURLOPT_FRESH_CONNECT`, `CURLOPT_FORBID_REUSE`; #47).
    pub own_connection: bool,
    /// A strong validator sent as `If-Range` with the range (#45): a server whose file no longer
    /// matches it answers the whole new file as `200` instead (RFC 9110 §13.1.5).
    pub if_range: Option<String>,
    /// What the download adds to its requests (#40), shared by all of them.
    pub options: Arc<RequestOptions>,
}

/// What a download adds to its requests (#40). The login and the custom headers go only to the
/// first URL's origin; the user agent and the referrer go with every request. `Debug` never shows
/// a header value or the login.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RequestOptions {
    /// `User-Agent`; none is sent without one.
    pub user_agent: Option<String>,
    /// `Referer`.
    pub referrer: Option<String>,
    /// Custom headers, name and value; an empty value is sent as an empty header.
    pub headers: Vec<(String, String)>,
    /// User name and password, sent as HTTP Basic (or as the FTP login).
    pub login: Option<(String, String)>,
    /// FTP only: insist on TLS (`AUTH TLS`) for the login and the file, and fail if the server
    /// can't, never falling back to plain FTP (#51). `ftps://` is always encrypted.
    pub require_encryption: bool,
    /// SFTP only: log in with this private key, as the user of `login` (#53).
    pub ssh_key: Option<SshKeyFile>,
    /// Follow a redirect from `https` to `http`: the user confirmed it for this download (#67).
    pub allow_insecure_redirect: bool,
    /// Stop before sending a request whose connection reached this computer or a private
    /// network (#67, ADR-0006): a link from the browser that the user hasn't confirmed.
    pub confirm_local_addresses: bool,
    /// The proxy every connection goes through (#192); `None` connects directly. Proxy
    /// variables in the environment are never used either way.
    pub proxy: Option<ProxyConfig>,
}

/// A proxy, as the user set it (#192, ADR-0008). `Debug` never shows the login.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    /// A DNS name, an IPv4 address, or an IPv6 address in brackets.
    pub host: String,
    pub port: u16,
    /// The proxy's own login, sent only to the proxy.
    pub login: Option<(String, String)>,
}

/// What kind of proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    /// HTTP, through `CONNECT` tunnels.
    Http,
    /// HTTP over TLS to the proxy, its certificate verified like a server's.
    Https,
    /// SOCKS5, which also resolves the names connected to.
    Socks5,
}

impl ProxyConfig {
    /// The proxy as libcurl takes it: `http://`, `https://` or `socks5h://`, then host and port.
    /// `socks5h` lets the proxy resolve names, so no lookup leaves this computer.
    pub fn url(&self) -> String {
        let scheme = match self.kind {
            ProxyKind::Http => "http",
            ProxyKind::Https => "https",
            ProxyKind::Socks5 => "socks5h",
        };
        format!("{scheme}://{}:{}", self.host, self.port)
    }
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("login", &self.login.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A private key file for an SFTP login, and its passphrase if it is encrypted (#53). `Debug`
/// never shows the passphrase.
#[derive(Clone, PartialEq, Eq)]
pub struct SshKeyFile {
    pub path: std::path::PathBuf,
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for SshKeyFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshKeyFile")
            .field("path", &self.path)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl std::fmt::Debug for RequestOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("RequestOptions")
            .field("user_agent", &self.user_agent)
            .field("referrer", &self.referrer)
            .field("headers", &names)
            .field("login", &self.login.as_ref().map(|_| "<redacted>"))
            .field("require_encryption", &self.require_encryption)
            .field("ssh_key", &self.ssh_key)
            .field("allow_insecure_redirect", &self.allow_insecure_redirect)
            .field("confirm_local_addresses", &self.confirm_local_addresses)
            .field("proxy", &self.proxy)
            .finish()
    }
}

impl FetchRequest {
    pub fn get(url: String) -> Self {
        FetchRequest {
            url,
            range: None,
            limits: Vec::new(),
            own_connection: false,
            if_range: None,
            options: Arc::default(),
        }
    }

    /// The same request, with what its download adds to every request (#40).
    pub fn with_options(mut self, options: Arc<RequestOptions>) -> Self {
        self.options = options;
        self
    }

    pub fn range(url: String, start: u64, end: Option<u64>) -> Self {
        FetchRequest {
            range: Some((start, end)),
            ..FetchRequest::get(url)
        }
    }

    /// The same request, sending `validator` as `If-Range` (#45).
    pub fn if_range(mut self, validator: Option<String>) -> Self {
        self.if_range = validator;
        self
    }

    /// The same request, on a connection of its own (#47).
    pub fn on_own_connection(mut self) -> Self {
        self.own_connection = true;
        self
    }

    /// The same request, drawing from `limits`.
    pub fn limited(mut self, limits: &[RateLimit]) -> Self {
        self.limits = limits.to_vec();
        self
    }
}

/// The final response's status line and headers (after any redirects).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// It came over HTTP/2: its status line says `HTTP/2` (#47).
    pub http2: bool,
    /// The URL this response answered, after any redirects (#40): the final URL names a file
    /// that `Content-Disposition` doesn't.
    pub url: String,
}

impl ResponseHead {
    /// The first header with this name, compared without case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What a gate decides about a final response head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    Accept,
    /// Ends the transfer before any body byte is delivered, with this reason.
    Abort(String),
}

/// How a fetch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The gate refused the response.
    Aborted(String),
    /// The caller cancelled the fetch.
    Cancelled,
    /// libcurl reported an error: what kind it is, and its `CURLcode` and description for
    /// diagnostics. The code is an `i64` because `CURLcode` is `u32` on Linux and `i32` on
    /// Windows; `i64` holds either without a cast.
    Curl {
        kind: CurlFailure,
        code: i64,
        message: String,
    },
    /// Velocity refused to follow a redirect (`redirect.rs`, #40): too many, or to another
    /// protocol. The message names no URL.
    RedirectRefused { kind: CurlFailure, message: String },
    /// A redirect from `https` to `http` that the user hasn't confirmed for this download (#67):
    /// the host it goes to.
    InsecureRedirect { host: String },
    /// The connection reached this computer or a private network, and the request must confirm
    /// that first (#67, ADR-0006). Nothing was sent.
    LocalAddress(AddressProblem),
    /// SFTP failed (#53): what kind of failure it is, and what went wrong, for diagnostics.
    Sftp { kind: CurlFailure, message: String },
    /// The SFTP server's host key isn't one the user has trusted (#53): unknown, or changed.
    HostKey(HostKeyProblem),
    /// The loop stopped before the fetch finished.
    LoopStopped,
}

/// An SFTP server's host key that wasn't trusted (#53). An unknown key waits for the user to
/// confirm its fingerprint; a changed one is always refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyProblem {
    pub host: String,
    pub port: u16,
    /// The key the server presented, as OpenSSH prints its SHA-256 fingerprint: `SHA256:…`.
    pub fingerprint: String,
    /// The fingerprint of the key pinned for this server before, when the key changed.
    pub pinned: Option<String>,
}

/// A request stopped before it was sent because its connection reached this computer or a
/// private network (#67, ADR-0006).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressProblem {
    /// The host the URL names.
    pub host: String,
    /// The address libcurl connected to, as libcurl gives it.
    pub address: String,
}

/// What kind of failure libcurl reported, so that callers can decide about retries without
/// knowing libcurl's codes (#48).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurlFailure {
    /// The host name didn't resolve.
    Resolve,
    /// The connection couldn't be made, or was refused.
    Connect,
    /// libcurl's own timeout passed (the connect timeout).
    Timeout,
    /// The connection broke while the request or the response was under way.
    Interrupted,
    /// The TLS handshake failed.
    TlsHandshake,
    /// The server's certificate wasn't trusted. Verification is never turned off (#69).
    Certificate,
    /// More redirects than allowed.
    TooManyRedirects,
    /// A URL or redirect outside the protocol allowlist, or a malformed URL.
    Refused,
    /// A redirect from `https` to `http`, refused until the user can confirm one (#40, #67).
    InsecureRedirect,
    /// The server's answer broke the protocol.
    Protocol,
    /// The server has no such file (FTP's `550`, #51).
    NotFound,
    /// The server refused the login, or access to the file (FTP, #51).
    AccessDenied,
    /// Encryption was required, and the server can't encrypt the connection (FTP's `AUTH TLS`,
    /// #51). Nothing was sent in clear after the greeting.
    EncryptionRefused,
    /// The proxy refused its login, or refused to connect onward (#192): an HTTP proxy's `407`
    /// to `CONNECT`, or a SOCKS5 proxy's refusal.
    ProxyRefused,
    /// A local problem: memory, or the write side.
    Local,
    /// Any other code.
    Other,
}

/// Windows' certificate-trust errors (HRESULTs from winerror.h) that Schannel names in a connect
/// error's detail: the `CERT_E_*` range, revocation, and the `SEC_E_*` certificate errors.
const REFUSED_CERTIFICATE_CODES: &[&str] = &[
    "0x800b01",   // CERT_E_*: expired, untrusted root, wrong name, wrong usage, revoked…
    "0x80092010", // CRYPT_E_REVOKED
    "0x80092012", // CRYPT_E_NO_REVOCATION_CHECK
    "0x80092013", // CRYPT_E_REVOCATION_OFFLINE
    "0x80090322", // SEC_E_WRONG_PRINCIPAL
    "0x80090325", // SEC_E_UNTRUSTED_ROOT
    "0x80090327", // SEC_E_CERT_UNKNOWN
    "0x80090328", // SEC_E_CERT_EXPIRED
    "0x80090349", // SEC_E_CERT_WRONG_USAGE
];

/// Whether a connect error's detail says the server's certificate was refused.
fn refuses_certificate(detail: Option<&str>) -> bool {
    let Some(detail) = detail else {
        return false;
    };
    let detail = detail.to_ascii_lowercase();
    REFUSED_CERTIFICATE_CODES
        .iter()
        .any(|code| detail.contains(code))
}

impl CurlFailure {
    fn of(e: &curl::Error) -> Self {
        if e.is_couldnt_resolve_host() || e.is_couldnt_resolve_proxy() {
            CurlFailure::Resolve
        } else if e.is_couldnt_connect() {
            CurlFailure::Connect
        } else if e.is_operation_timedout() {
            CurlFailure::Timeout
        } else if e.is_got_nothing()
            || e.is_send_error()
            || e.is_recv_error()
            || e.is_partial_file()
            || e.is_http2_error()
            || e.is_http2_stream_error()
        {
            CurlFailure::Interrupted
        } else if e.is_ssl_connect_error() && refuses_certificate(e.extra_description()) {
            // Verifying against the Windows store, Schannel reports a refused certificate as a
            // connect error, with the reason only in the detail (#69).
            CurlFailure::Certificate
        } else if e.is_ssl_connect_error() {
            CurlFailure::TlsHandshake
        } else if e.is_peer_failed_verification()
            || e.is_ssl_cacert_badfile()
            || e.is_ssl_certproblem()
            || e.is_ssl_issuer_error()
        {
            CurlFailure::Certificate
        } else if e.is_too_many_redirects() {
            CurlFailure::TooManyRedirects
        } else if e.is_unsupported_protocol() || e.is_url_malformed() {
            CurlFailure::Refused
        } else if e.code() == curl_sys::CURLE_REMOTE_FILE_NOT_FOUND {
            CurlFailure::NotFound
        } else if e.is_login_denied() || e.is_remote_access_denied() {
            CurlFailure::AccessDenied
        } else if e.is_use_ssl_failed() {
            CurlFailure::EncryptionRefused
        } else if e.code() == curl_sys::CURLE_FTP_WEIRD_SERVER_REPLY || e.is_bad_content_encoding()
        {
            // curl-sys names code 8 by its old FTP name; libcurl uses it for any protocol.
            CurlFailure::Protocol
        } else if e.is_out_of_memory() || e.is_write_error() {
            CurlFailure::Local
        } else {
            CurlFailure::Other
        }
    }
}

/// One step of a fetch, in order: at most one `Head`, any number of `Data`, then one `Done`.
#[derive(Debug)]
pub enum FetchEvent {
    Head(ResponseHead),
    Data(Vec<u8>),
    Done(Result<(), FetchError>),
}

#[derive(Clone, Debug)]
pub struct LoopOptions {
    /// The most body bytes one fetch keeps for its reader before it pauses the transfer.
    pub buffer_bytes_per_fetch: usize,
    /// How long a fetch may take to connect (`CURLOPT_CONNECTTIMEOUT_MS`). There is no limit on
    /// the whole transfer: a large file may take hours, and stalls are caught by the downloader.
    pub connect_timeout: Duration,
    /// The most connections open to one host name and port at once
    /// (`CURLMOPT_MAX_HOST_CONNECTIONS`). Fetches past it wait for a free connection. The engine's
    /// own budget keeps below it; this is the backstop (#47).
    pub max_connections_per_host: usize,
    /// Speaks HTTP/2 to `http://` URLs without asking first (prior knowledge), for the HTTP/2 tests
    /// against a local server without TLS. Test builds only (PO decision on #47).
    #[cfg(feature = "test-h2c")]
    pub http2_prior_knowledge: bool,
    /// Trusts this PEM file's CA (`CURLOPT_CAINFO`) in place of the platform's store, for the TLS
    /// tests against the local test CA (#69). Verification and revocation checks stay on. Test
    /// builds only (PO decision on #69).
    #[cfg(feature = "test-ca")]
    pub ca_file: Option<std::path::PathBuf>,
}

impl Default for LoopOptions {
    fn default() -> Self {
        // Starting values, not requirements: the buffer from report section 5.5, measured later
        // (#49); the connect timeout from V1's `connect_timeout_secs` (PO decision on #48); 8
        // connections per host from report section 5.5 (PO decision on #47).
        LoopOptions {
            buffer_bytes_per_fetch: 1024 * 1024,
            connect_timeout: Duration::from_secs(30),
            max_connections_per_host: 8,
            #[cfg(feature = "test-h2c")]
            http2_prior_knowledge: false,
            #[cfg(feature = "test-ca")]
            ca_file: None,
        }
    }
}

/// The loop thread couldn't start.
#[derive(Debug)]
pub struct LoopError(String);

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the libcurl loop couldn't start: {}", self.0)
    }
}

impl std::error::Error for LoopError {}

enum Command {
    Start(usize, Box<Easy2<Collector>>),
    Cancel(usize),
    Unpause(usize),
    /// A new `CURLMOPT_MAX_HOST_CONNECTIONS`, and where to say whether libcurl took it.
    MaxHostConnections(usize, Sender<Result<(), String>>),
    Stop,
}

/// Owns the loop thread. Dropping it stops the loop; unfinished fetches end with `LoopStopped`.
pub struct CurlLoop {
    commands: Sender<Command>,
    waker: MultiWaker,
    next_token: AtomicUsize,
    options: LoopOptions,
    /// The connect timeout for requests that start from now, in milliseconds (#30); starts as
    /// `options.connect_timeout`.
    connect_timeout_ms: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl CurlLoop {
    /// Starts the loop. It needs a [`Libcurl`], so it can't run on a libcurl that failed the
    /// capability check.
    pub fn start(_curl: &Libcurl, options: LoopOptions) -> Result<Self, LoopError> {
        let (commands, inbox) = mpsc::channel();
        let (waker_tx, waker_rx) = mpsc::channel();
        let for_loop = options.clone();
        let thread = thread::Builder::new()
            .name("velocity-curl-multi".into())
            .spawn(move || run_loop(inbox, waker_tx, for_loop))
            .map_err(|e| LoopError(e.to_string()))?;
        // The Multi is created on the loop thread (it isn't Send); only its waker comes back.
        let waker = waker_rx
            .recv()
            .map_err(|_| LoopError("the loop thread ended while starting".into()))?
            .map_err(LoopError)?;
        Ok(CurlLoop {
            commands,
            waker,
            next_token: AtomicUsize::new(1),
            connect_timeout_ms: AtomicU64::new(
                u64::try_from(options.connect_timeout.as_millis()).unwrap_or(u64::MAX),
            ),
            options,
            thread: Some(thread),
        })
    }

    /// Starts a fetch. Its events arrive on the returned [`Fetch`].
    pub fn fetch(
        &self,
        request: FetchRequest,
        gate: impl Fn(&ResponseHead) -> Gate + Send + 'static,
    ) -> Fetch {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let (events_tx, events) = mpsc::channel();
        let flow = Arc::new(Flow::default());
        let ftp = is_ftp(&request.url).then(|| FtpReplies::new(request.range, request.url.clone()));
        if let (Some(_), Some((start, Some(end)))) = (&ftp, request.range) {
            // FTP's REST has no end: the transfer stops at it instead.
            flow.limit.store(
                end.saturating_sub(start).saturating_add(1),
                Ordering::Release,
            );
        }
        let cap = self.options.buffer_bytes_per_fetch;
        let collector = Collector {
            sink: Sink::new(
                events_tx.clone(),
                Arc::clone(&flow),
                cap,
                request.limits.clone(),
            ),
            gate: Box::new(gate),
            lines: Vec::new(),
            accepted: false,
            abort: None,
            stopped_at_limit: false,
            first: origin(&request.url),
            request,
            followed: 0,
            redirecting: false,
            ftp,
            local_address: Box::default(),
        };
        let control = CurlControl {
            token,
            commands: self.commands.clone(),
            waker: self.waker.clone(),
        };
        let fetch = Fetch::new(events, flow, cap, Arc::new(control));
        let options = LoopOptions {
            connect_timeout: Duration::from_millis(self.connect_timeout_ms.load(Ordering::Relaxed)),
            ..self.options.clone()
        };
        match configure(collector, &options) {
            Ok(easy) => self.send(Command::Start(token, Box::new(easy))),
            Err(e) => {
                let _ = events_tx.send(FetchEvent::Done(Err(curl_error(&e))));
            }
        }
        fetch
    }

    /// Changes `CURLMOPT_MAX_HOST_CONNECTIONS` while the loop runs (#57): the performance
    /// profile's per-host cap. Fetches past it wait for a connection.
    /// The longest a connection may take to open, for requests that start from now (#30).
    pub fn set_connect_timeout(&self, timeout: Duration) {
        let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        self.connect_timeout_ms.store(ms, Ordering::Relaxed);
    }

    pub fn set_max_connections_per_host(&self, connections: usize) -> Result<(), LoopError> {
        let (reply, answer) = mpsc::channel();
        self.send(Command::MaxHostConnections(connections, reply));
        answer
            .recv()
            .map_err(|_| LoopError("the loop thread has ended".into()))?
            .map_err(|e| LoopError(format!("CURLMOPT_MAX_HOST_CONNECTIONS: {e}")))
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            let _ = self.waker.wakeup();
        }
    }
}

impl Drop for CurlLoop {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Transport for CurlLoop {
    fn fetch(&self, request: FetchRequest, gate: GateFn) -> Fetch {
        CurlLoop::fetch(self, request, gate)
    }
}

/// How a fetch's reader reaches the loop thread: commands with the fetch's token, and a wakeup.
struct CurlControl {
    token: usize,
    commands: Sender<Command>,
    waker: MultiWaker,
}

impl CurlControl {
    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            let _ = self.waker.wakeup();
        }
    }
}

impl FetchControl for CurlControl {
    fn cancel(&self) {
        self.send(Command::Cancel(self.token));
    }

    fn resume(&self) {
        self.send(Command::Unpause(self.token));
    }
}

/// libcurl's callbacks for one fetch. They run on the loop thread: they never block and never
/// call back into the caller; they only send events.
struct Collector {
    /// The reader's buffer, the speed limits and the body limit (`pipe.rs`).
    sink: Sink,
    gate: Box<dyn Fn(&ResponseHead) -> Gate + Send>,
    /// Header lines of the response being received.
    lines: Vec<String>,
    accepted: bool,
    abort: Option<String>,
    /// The transfer was stopped at its limit: it ends as completed, not as an error.
    stopped_at_limit: bool,
    /// The request, with the URL of the current hop (#40).
    request: FetchRequest,
    /// The first URL's origin: the login and the custom headers go only there.
    first: Option<Origin>,
    /// Redirects followed so far.
    followed: u32,
    /// The response being received is a redirect: its body is never the file's, and the loop
    /// decides whether to follow it once it ends (`follow`).
    redirecting: bool,
    /// An FTP fetch's control replies, which make its head (#51); `None` for HTTP.
    ftp: Option<FtpReplies>,
    /// The local address that `prereq` stopped the request at (#67). Boxed: libcurl holds a
    /// pointer to it (`CURLOPT_PREREQDATA`), which must stay put while the handle lives.
    local_address: Box<OnceLock<String>>,
}

impl Collector {
    /// Shows the gate the final head and sends it on; `false` when the gate aborts.
    fn gate(&mut self, head: ResponseHead) -> bool {
        let gate = (self.gate)(&head);
        // FTP's 200 to a range after byte 0 starts at the REST offset, not at byte 0, whatever the
        // gate says: it never reaches the caller (#51).
        let unproven = self.ftp.is_some()
            && head.status == 200
            && self.request.range.is_some_and(|(start, _)| start > 0);
        let _ = self.sink.events.send(FetchEvent::Head(head));
        match gate {
            Gate::Accept if !unproven => {
                self.accepted = true;
                true
            }
            Gate::Accept => {
                self.abort = Some("a 200 for a range after byte 0".to_owned());
                false
            }
            Gate::Abort(reason) => {
                self.abort = Some(reason);
                false
            }
        }
    }
}

impl Handler for Collector {
    fn header(&mut self, data: &[u8]) -> bool {
        if let Some(ftp) = self.ftp.as_mut() {
            // A control reply, never a header: libcurl passes FTP's replies here (#51).
            let reply = String::from_utf8_lossy(data);
            return match ftp.received(&reply, SystemTime::now()) {
                Some(head) => self.gate(head),
                None => true,
            };
        }
        let line = String::from_utf8_lossy(data).trim_end().to_owned();
        if line.starts_with("HTTP/") {
            self.lines.clear();
            self.redirecting = false;
        }
        if !line.is_empty() {
            self.lines.push(line);
            return true;
        }
        // A blank line ends one response's headers.
        let Some(mut head) = parse_head(&self.lines) else {
            return true;
        };
        // 1xx responses are intermediate; libcurl continues past them.
        if head.status < 200 {
            return true;
        }
        // A redirect: Velocity follows it itself once this response ends (#40).
        if (300..400).contains(&head.status) && head.header("location").is_some() {
            self.redirecting = true;
            return true;
        }
        head.url = self.request.url.clone();
        self.gate(head)
    }

    /// libcurl's debug output, on only for FTP: the commands it sent, which the replies answer
    /// (#51). Nothing is printed or kept but a command's verb, so the password never is.
    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        if let (InfoType::HeaderOut, Some(ftp)) = (kind, self.ftp.as_mut()) {
            ftp.sent(data);
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if self.redirecting {
            // A redirect's own body, such as a "Moved" page: read and dropped.
            return Ok(data.len());
        }
        if !self.accepted && self.ftp.is_some() {
            // FTP bytes before the replies made a head: nothing gated them, so none is delivered.
            // A short count stops the transfer.
            self.abort = Some("FTP data before the server's reply to RETR".to_owned());
            return Ok(0);
        }
        if !self.accepted {
            // A body without a head: nothing to gate.
            self.accepted = true;
        }
        match self.sink.offer(data) {
            // The reader's buffer is full, or a speed limit is out of tokens: libcurl keeps these
            // bytes and offers them again once resumed (by the reader, or by the loop thread).
            Offered::Full | Offered::Throttled => Err(WriteError::Pause),
            Offered::Took(take) => {
                if take < data.len() {
                    // A short count makes libcurl stop the transfer (CURLE_WRITE_ERROR).
                    self.stopped_at_limit = true;
                }
                Ok(take)
            }
        }
    }
}

fn parse_head(lines: &[String]) -> Option<ResponseHead> {
    let status = lines.first()?.split_whitespace().nth(1)?.parse().ok()?;
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(ResponseHead {
        status,
        headers,
        http2: lines.first()?.starts_with("HTTP/2"),
        url: String::new(),
    })
}

fn configure(collector: Collector, options: &LoopOptions) -> Result<Easy2<Collector>, curl::Error> {
    let request = collector.request.clone();
    let mut easy = Easy2::new(collector);
    // The first URL is its own origin: the login and the custom headers go with it.
    set_options(&mut easy, &request, true, options)?;
    Ok(easy)
}

/// Sets every option of a request on a new or reset handle. `with_secrets` adds the login and the
/// custom headers: only for the first URL's origin (#40).
fn set_options(
    easy: &mut Easy2<Collector>,
    request: &FetchRequest,
    with_secrets: bool,
    options: &LoopOptions,
) -> Result<(), curl::Error> {
    let refused = || curl::Error::new(curl_sys::CURLE_BAD_FUNCTION_ARGUMENT);
    easy.url(&request.url)?;
    easy.connect_timeout(options.connect_timeout)?;
    easy.get(true)?;
    let ftp = is_ftp(&request.url);
    match request.range {
        // REST, from where the range starts; the fetch's body limit ends it (`CurlLoop::fetch`).
        Some((start, _)) if ftp => easy.resume_from(start)?,
        Some((start, end)) => easy.range(&match end {
            Some(end) => format!("{start}-{end}"),
            None => format!("{start}-"),
        })?,
        None => {}
    }
    let plain_http = request
        .url
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http"));
    if ftp {
        set_ftp_options(easy, request)?;
    } else if request.options.require_encryption && plain_http {
        // A plain http:// link can't be encrypted: refused rather than sent in clear.
        return Err(refused());
    }
    let mut lines = Vec::new();
    if let Some(validator) = &request.if_range {
        // The value comes from a server: a control character could end the header line and add
        // another header, so such a value is refused and nothing is sent.
        if validator.bytes().any(|b| b.is_ascii_control()) {
            return Err(refused());
        }
        lines.push(format!("If-Range: {validator}"));
    }
    let extra = &request.options;
    if with_secrets {
        for (name, value) in &extra.headers {
            // The engine checked them (velocity-core's check_headers); refused again here, where
            // the line is written, rather than trusted.
            if !is_header_line_safe(name, value) {
                return Err(refused());
            }
            // libcurl sends `Name;` as an empty header, and drops `Name:`.
            lines.push(if value.is_empty() {
                format!("{name};")
            } else {
                format!("{name}: {value}")
            });
        }
        if let Some((user, password)) = &extra.login {
            easy.username(user)?;
            easy.password(password)?;
            // Basic only: sent with the first request, never another scheme negotiated by the
            // server. CURLOPT_UNRESTRICTED_AUTH stays off.
            easy.http_auth(curl::easy::Auth::new().basic(true))?;
        }
    }
    if !lines.is_empty() {
        let mut list = List::new();
        for line in &lines {
            list.append(line)?;
        }
        easy.http_headers(list)?;
    }
    if let Some(agent) = &extra.user_agent {
        if !is_header_line_safe("User-Agent", agent) {
            return Err(refused());
        }
        easy.useragent(agent)?;
    }
    if let Some(referrer) = &extra.referrer {
        if !is_header_line_safe("Referer", referrer) {
            return Err(refused());
        }
        easy.referer(referrer)?;
    }
    // Velocity follows redirects itself (`follow`, `redirect.rs`): libcurl would send the custom
    // headers to another host.
    easy.follow_location(false)?;
    set_proxy(easy, extra.proxy.as_ref())?;
    set_string(easy, CURLOPT_PROTOCOLS_STR, PROTOCOLS)?;
    set_string(easy, CURLOPT_REDIR_PROTOCOLS_STR, REDIRECT_PROTOCOLS)?;
    if extra.confirm_local_addresses {
        match &extra.proxy {
            None => check_addresses(easy)?,
            // Through a proxy, the connection is to the proxy, which the user chose (PO decision
            // on #192): only a link that names this computer or a private network itself stops.
            Some(_) => {
                if let Some(host) = local_host(&request.url) {
                    let _ = easy.get_ref().local_address.set(host);
                    stop_before_request(easy)?;
                }
            }
        }
    }
    if request.own_connection {
        // A connection of its own, closed at the end, so no other fetch shares it (#47).
        easy.fresh_connect(true)?;
        easy.forbid_reuse(true)?;
    } else {
        // HTTP/2 multiplexing first (report section 5.5): wait for a connection being set up to
        // the same host, to share it if it turns out to speak HTTP/2, instead of opening another.
        easy.pipewait(true)?;
    }
    #[cfg(feature = "test-h2c")]
    if options.http2_prior_knowledge {
        easy.http_version(curl::easy::HttpVersion::V2PriorKnowledge)?;
    }
    // The test CA in place of the platform's store; verification stays on (#69).
    #[cfg(feature = "test-ca")]
    if let Some(ca_file) = &options.ca_file {
        easy.cainfo(ca_file)?;
        // And for an HTTPS proxy's certificate (#192).
        easy.proxy_cainfo(&ca_file.to_string_lossy())?;
    }
    Ok(())
}

/// The proxy, or explicitly none (#192, ADR-0008). An empty proxy disables proxies, those from
/// `http_proxy` and similar variables included. Through an HTTP or HTTPS proxy every connection is
/// a `CONNECT` tunnel, so the proxy never reads a request, its headers or a site's login, even for
/// `http://` and FTP. The proxy's login goes only to the proxy (`CURLOPT_PROXYUSERNAME`), never as
/// a site's `Authorization`; an HTTPS proxy's certificate is verified as a server's (#69).
fn set_proxy(easy: &mut Easy2<Collector>, proxy: Option<&ProxyConfig>) -> Result<(), curl::Error> {
    let Some(proxy) = proxy else {
        return easy.proxy("");
    };
    easy.proxy(&proxy.url())?;
    if proxy.kind != ProxyKind::Socks5 {
        easy.http_proxy_tunnel(true)?;
    }
    if let Some((user, password)) = &proxy.login {
        easy.proxy_username(user)?;
        easy.proxy_password(password)?;
        // Basic only, sent with the first CONNECT, as a site's login is (#40).
        easy.proxy_auth(curl::easy::Auth::new().basic(true))?;
    }
    Ok(())
}

/// The host of `url` when it names this computer or a private network itself: `localhost`, a
/// name under `.localhost`, or a local IP address (#67, ADR-0006). Through a proxy that is all
/// V2 can judge, since the proxy resolves names and makes the connection (#192).
fn local_host(url: &str) -> Option<String> {
    let host = crate::url::origin(url)?.host;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || bare.parse::<IpAddr>().is_ok_and(is_local);
    local.then_some(host)
}

/// Has libcurl stop the request once its connection is made, before anything is sent: through a
/// proxy, for a link [`local_host`] judged local (#192). The address was recorded already.
fn stop_before_request(easy: &mut Easy2<Collector>) -> Result<(), curl::Error> {
    let callback: extern "C" fn(*mut c_void, *mut c_char, *mut c_char, c_int, c_int) -> c_int =
        prereq_stop;
    // SAFETY: `easy.raw()` is a live easy handle owned by `easy`. CURLOPT_PREREQFUNCTION takes a
    // `curl_prereq_callback`, whose C signature `prereq_stop` has; it reads no data pointer.
    let code = unsafe { curl_sys::curl_easy_setopt(easy.raw(), CURLOPT_PREREQFUNCTION, callback) };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(curl::Error::new(code))
    }
}

/// The pre-request callback for a link already judged local (#192): always stops.
extern "C" fn prereq_stop(
    _data: *mut c_void,
    _primary_ip: *mut c_char,
    _local_ip: *mut c_char,
    _primary_port: c_int,
    _local_port: c_int,
) -> c_int {
    CURL_PREREQFUNC_ABORT
}

/// A proxy that refused its login or the connection onward (#192), told from the handle: an HTTP
/// proxy's `407` to `CONNECT`, or `CURLE_PROXY` from a SOCKS proxy. Otherwise `error` as it is.
fn proxy_refusal(
    handle: &mut Easy2Handle<Collector>,
    e: &curl::Error,
    error: FetchError,
) -> FetchError {
    let refused = e.code() == CURLE_PROXY || handle.http_connectcode().is_ok_and(|c| c == 407);
    match error {
        FetchError::Curl { code, message, .. } if refused => FetchError::Curl {
            kind: CurlFailure::ProxyRefused,
            code,
            message,
        },
        other => other,
    }
}

/// FTP's own options (#51).
///
/// - **Debug output** on, for the debug callback alone: it pairs each reply with its command
///   (`ftp.rs`). libcurl prints nothing itself once a debug callback is set.
/// - **`MDTM`**, the file's modification time, which becomes `Last-Modified`.
/// - **Passive mode to the control connection's address:** the address in a `PASV` reply is
///   ignored, so a server can't send the data connection elsewhere (libcurl's default since
///   7.74.0, set here to keep it).
/// - **Encryption:** an `ftps://` link is always encrypted. A plain `ftp://` link that requires
///   encryption asks `AUTH TLS` before the login and fails if the server refuses
///   (`CURLUSESSL_ALL`), never falling back to plain FTP. Certificates are verified as for HTTPS.
fn set_ftp_options(easy: &mut Easy2<Collector>, request: &FetchRequest) -> Result<(), curl::Error> {
    easy.verbose(true)?;
    easy.fetch_filetime(true)?;
    set_long(easy, curl_sys::CURLOPT_FTP_SKIP_PASV_IP, 1)?;
    set_long(easy, curl_sys::CURLOPT_FTPSSLAUTH, CURLFTPAUTH_TLS)?;
    let implicit = request
        .url
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("ftps"));
    if implicit || request.options.require_encryption {
        set_long(
            easy,
            curl_sys::CURLOPT_USE_SSL,
            std::os::raw::c_long::from(curl_sys::CURLUSESSL_ALL),
        )?;
    }
    Ok(())
}

/// A header name that is an RFC 9110 token and a value without control characters but tab, so
/// the line can't end early and start another header.
fn is_header_line_safe(name: &str, value: &str) -> bool {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    !name.is_empty()
        && name.chars().all(token)
        && !value.chars().any(|c| c.is_control() && c != '\t')
}

/// Follows the redirect a fetch's response ended with, when Velocity's rules allow it
/// (`redirect.rs`): the same handle, reset and set up again for the next URL, carries on under
/// the same token. On the loop thread. On a refusal, the fetch's events and why.
fn follow(
    multi: &Multi,
    handle: Easy2Handle<Collector>,
    token: usize,
    options: &LoopOptions,
) -> Result<Easy2Handle<Collector>, (Sender<FetchEvent>, FetchError)> {
    let events = handle.get_ref().sink.events.clone();
    let fail = |e: FetchError| (events.clone(), e);
    let local = |what: &str, code: i64, e: String| FetchError::Curl {
        kind: CurlFailure::Local,
        code,
        message: format!("the redirect couldn't be {what}: {e}"),
    };
    let mut easy = multi
        .remove2(handle)
        .map_err(|e| fail(local("followed", i64::from(e.code()), e.to_string())))?;
    // libcurl resolves the Location against the URL it answered (CURLINFO_REDIRECT_URL).
    let location = easy.redirect_url().ok().flatten().map(str::to_owned);
    let collector = easy.get_mut();
    let hop = match location {
        Some(to) => next_hop(
            collector.first.as_ref(),
            &collector.request.url,
            &to,
            collector.followed,
            collector.request.options.allow_insecure_redirect,
        ),
        None => Err(Refusal {
            kind: CurlFailure::Refused,
            message: "a redirect without a usable Location".to_owned(),
            host: None,
        }),
    }
    .map_err(|r| {
        fail(match r.host {
            Some(host) if r.kind == CurlFailure::InsecureRedirect => {
                FetchError::InsecureRedirect { host }
            }
            _ => FetchError::RedirectRefused {
                kind: r.kind,
                message: r.message,
            },
        })
    })?;
    collector.followed += 1;
    collector.request.url = hop.url;
    collector.redirecting = false;
    collector.lines.clear();
    let request = collector.request.clone();
    // Back to libcurl's defaults, so no option of the last hop, such as the login, carries over.
    easy.reset();
    set_options(&mut easy, &request, hop.same_origin, options).map_err(|e| fail(curl_error(&e)))?;
    let mut next = multi
        .add2(easy)
        .map_err(|e| fail(local("started", i64::from(e.code()), e.to_string())))?;
    next.set_token(token)
        .map_err(|e| fail(local("started", i64::from(e.code()), e.to_string())))?;
    Ok(next)
}

/// Has libcurl call [`prereq`] once each connection is made and before the request is sent, with
/// the address it connected to (#67, ADR-0006). Set again on every hop, since `follow` resets the
/// handle.
fn check_addresses(easy: &mut Easy2<Collector>) -> Result<(), curl::Error> {
    let data: *const OnceLock<String> = &*easy.get_ref().local_address;
    let callback: extern "C" fn(*mut c_void, *mut c_char, *mut c_char, c_int, c_int) -> c_int =
        prereq;
    // SAFETY: `easy.raw()` is a live easy handle owned by `easy`. CURLOPT_PREREQFUNCTION takes a
    // `curl_prereq_callback`, whose C signature `prereq` has. CURLOPT_PREREQDATA takes a pointer
    // that libcurl only passes back to the callback: it points into the collector's box, which
    // the handle owns, so it lives as long as the handle and never moves.
    let code = unsafe {
        let code = curl_sys::curl_easy_setopt(easy.raw(), CURLOPT_PREREQFUNCTION, callback);
        if code == curl_sys::CURLE_OK {
            curl_sys::curl_easy_setopt(easy.raw(), CURLOPT_PREREQDATA, data.cast::<c_void>())
        } else {
            code
        }
    };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(curl::Error::new(code))
    }
}

/// libcurl's pre-request callback (#67): lets the request go to a public address, and stops it
/// at a local one, which it records for the user to confirm. An address it can't read is stopped
/// too. On the loop thread; it never blocks.
extern "C" fn prereq(
    data: *mut c_void,
    primary_ip: *mut c_char,
    _local_ip: *mut c_char,
    _primary_port: c_int,
    _local_port: c_int,
) -> c_int {
    let address = if primary_ip.is_null() {
        String::new()
    } else {
        // SAFETY: libcurl passes the connected address as a NUL-terminated string that is valid
        // during the call; it is copied before returning.
        unsafe { CStr::from_ptr(primary_ip) }
            .to_string_lossy()
            .into_owned()
    };
    if address.parse::<IpAddr>().is_ok_and(|ip| !is_local(ip)) {
        return CURL_PREREQFUNC_OK;
    }
    if !data.is_null() {
        // SAFETY: `data` is the CURLOPT_PREREQDATA set by `check_addresses`: a live
        // `OnceLock<String>` in the collector's box, only read elsewhere on this thread.
        let found = unsafe { &*data.cast::<OnceLock<String>>() };
        let _ = found.set(address);
    }
    CURL_PREREQFUNC_ABORT
}

/// Sets a string option that the curl crate doesn't wrap.
fn set_string(
    easy: &Easy2<Collector>,
    option: curl_sys::CURLoption,
    value: &str,
) -> Result<(), curl::Error> {
    let value = CString::new(value).expect("a constant without NUL bytes");
    // SAFETY: `easy.raw()` is a live easy handle owned by `easy`; libcurl copies string options
    // (CURLOPT_PROTOCOLS_STR and CURLOPT_REDIR_PROTOCOLS_STR) during the call, so `value` may be
    // freed afterwards.
    let code = unsafe { curl_sys::curl_easy_setopt(easy.raw(), option, value.as_ptr()) };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(curl::Error::new(code))
    }
}

/// Sets a `long` option that the curl crate doesn't wrap.
fn set_long(
    easy: &Easy2<Collector>,
    option: curl_sys::CURLoption,
    value: std::os::raw::c_long,
) -> Result<(), curl::Error> {
    // SAFETY: `easy.raw()` is a live easy handle owned by `easy`, and every option passed here
    // (CURLOPT_FTP_SKIP_PASV_IP, CURLOPT_FTPSSLAUTH, CURLOPT_USE_SSL) takes a `long`.
    let code = unsafe { curl_sys::curl_easy_setopt(easy.raw(), option, value) };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(curl::Error::new(code))
    }
}

/// libcurl's error: its kind, its code, and its description with the detail from libcurl's error
/// buffer when there is one, such as which certificate check failed (#69).
fn curl_error(e: &curl::Error) -> FetchError {
    let message = match e.extra_description().map(str::trim) {
        Some(detail) if !detail.is_empty() => format!("{}: {detail}", e.description()),
        _ => e.description().to_owned(),
    };
    FetchError::Curl {
        kind: CurlFailure::of(e),
        code: i64::from(e.code()),
        message,
    }
}

/// The loop thread: owns the `Multi`, starts and stops transfers, and reports how each ended.
fn run_loop(
    inbox: Receiver<Command>,
    waker_tx: Sender<Result<MultiWaker, String>>,
    options: LoopOptions,
) {
    let mut multi = Multi::new();
    if let Err(e) = multi.set_max_host_connections(options.max_connections_per_host) {
        let _ = waker_tx.send(Err(format!("CURLMOPT_MAX_HOST_CONNECTIONS: {e}")));
        return;
    }
    if waker_tx.send(Ok(multi.waker())).is_err() {
        return;
    }
    let mut active: HashMap<usize, Easy2Handle<Collector>> = HashMap::new();
    // Where the next round of resuming rate-paused transfers starts (#49).
    let mut turn = 0usize;
    loop {
        // Commands first: with nothing running, wait for one instead of polling.
        let mut next = if active.is_empty() {
            inbox.recv().ok()
        } else {
            inbox.try_recv().ok()
        };
        if active.is_empty() && next.is_none() {
            return; // every sender is gone
        }
        while let Some(command) = next.take() {
            match command {
                Command::Start(token, easy) => {
                    // add2 consumes the handle even when it fails, so keep a way to report it.
                    let events = easy.get_ref().sink.events.clone();
                    let started = multi
                        .add2(*easy)
                        .map_err(|e| (i64::from(e.code()), e.to_string()))
                        .and_then(|mut handle| {
                            handle
                                .set_token(token)
                                .map(|()| handle)
                                .map_err(|e| (i64::from(e.code()), e.to_string()))
                        });
                    match started {
                        Ok(handle) => {
                            active.insert(token, handle);
                        }
                        Err((code, message)) => {
                            let _ = events.send(FetchEvent::Done(Err(FetchError::Curl {
                                kind: CurlFailure::Local,
                                code,
                                message: format!("the transfer couldn't start: {message}"),
                            })));
                        }
                    }
                }
                Command::Cancel(token) => {
                    if let Some(handle) = active.remove(&token) {
                        finish(&multi, handle, Err(FetchError::Cancelled));
                    }
                }
                Command::Unpause(token) => {
                    if let Some(handle) = active.get(&token) {
                        let _ = handle.unpause_write();
                    }
                }
                Command::MaxHostConnections(connections, reply) => {
                    let set = multi
                        .set_max_host_connections(connections)
                        .map_err(|e| e.to_string());
                    let _ = reply.send(set);
                }
                Command::Stop => {
                    for (_, handle) in active.drain() {
                        finish(&multi, handle, Err(FetchError::LoopStopped));
                    }
                    return;
                }
            }
            next = inbox.try_recv().ok();
        }
        if active.is_empty() {
            continue;
        }
        // Transfers paused by a speed limit go on once all their limits have tokens again.
        // Resuming one hands it the bytes libcurl kept at once, so whoever is resumed first takes
        // a shared limit's tokens first: the order turns each round, so no transfer is always
        // last.
        let mut waiting: Vec<usize> = active
            .iter()
            .filter(|(_, h)| h.get_ref().sink.flow.rate_paused.load(Ordering::Acquire))
            .map(|(token, _)| *token)
            .collect();
        let rate_paused = !waiting.is_empty();
        if rate_paused {
            waiting.sort_unstable();
            let start = turn % waiting.len();
            waiting.rotate_left(start);
            turn = turn.wrapping_add(1);
        }
        for token in waiting {
            let handle = &active[&token];
            let collector = handle.get_ref();
            if rate::ready(&collector.sink.limits) {
                collector
                    .sink
                    .flow
                    .rate_paused
                    .store(false, Ordering::Release);
                let _ = handle.unpause_write();
            }
        }
        let _ = multi.perform();
        let mut ended = Vec::new();
        multi.messages(|m| {
            if let Ok(token) = m.token() {
                if let Some(result) = active.get(&token).and_then(|h| m.result_for2(h)) {
                    ended.push((token, result));
                }
            }
        });
        for (token, result) in ended {
            if let Some(handle) = active.remove(&token) {
                if result.is_ok() && handle.get_ref().redirecting {
                    match follow(&multi, handle, token, &options) {
                        Ok(next) => {
                            active.insert(token, next);
                        }
                        // The reader may have dropped its Fetch; then nobody is waiting.
                        Err((events, refused)) => {
                            let _ = events.send(FetchEvent::Done(Err(refused)));
                        }
                    }
                    continue;
                }
                let collector = handle.get_ref();
                let outcome = match &result {
                    Ok(()) => Ok(()),
                    // Stopped at its limit: the range it was asked for is complete.
                    Err(_) if collector.stopped_at_limit => Ok(()),
                    Err(e) => match &collector.abort {
                        Some(reason) => Err(FetchError::Aborted(reason.clone())),
                        // Stopped before the request at a local address (#67).
                        None if collector.local_address.get().is_some() => {
                            Err(FetchError::LocalAddress(AddressProblem {
                                host: collector
                                    .first
                                    .as_ref()
                                    .map(|o| o.host.clone())
                                    .unwrap_or_default(),
                                address: collector.local_address.get().cloned().unwrap_or_default(),
                            }))
                        }
                        // An FTP range past the end ends without a body, like an HTTP 416.
                        None if collector
                            .ftp
                            .as_ref()
                            .is_some_and(FtpReplies::unsatisfiable) =>
                        {
                            Ok(())
                        }
                        None => Err(curl_error(e)),
                    },
                };
                let mut handle = handle;
                let outcome = match (outcome, &result) {
                    (Err(error), Err(e)) => Err(proxy_refusal(&mut handle, e, error)),
                    (outcome, _) => outcome,
                };
                finish(&multi, handle, outcome);
            }
        }
        let cap = if rate_paused { RATE_CHECK } else { POLL_CAP };
        let wait = multi
            .get_timeout()
            .ok()
            .flatten()
            .map_or(cap, |t| t.min(cap));
        let _ = multi.poll(&mut [], wait);
    }
}

/// Removes a transfer from the `Multi` (on the loop thread, as libcurl requires) and sends its
/// last event.
fn finish(multi: &Multi, handle: Easy2Handle<Collector>, outcome: Result<(), FetchError>) {
    let events = handle.get_ref().sink.events.clone();
    // A failed removal is reported to the fetch rather than dropped; a successful outcome then
    // becomes that error.
    let outcome = match multi.remove2(handle) {
        Ok(_) => outcome,
        Err(e) => outcome.and(Err(FetchError::Curl {
            kind: CurlFailure::Local,
            code: i64::from(e.code()),
            message: format!("the transfer couldn't be removed: {e}"),
        })),
    };
    // The reader may have dropped its Fetch; then nobody is waiting for this event.
    let _ = events.send(FetchEvent::Done(outcome));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(code: curl_sys::CURLcode) -> CurlFailure {
        CurlFailure::of(&curl::Error::new(code))
    }

    /// Regression (#69, found by the Windows store test on CI): verifying against the Windows
    /// store, Schannel reports a certificate it refuses as CURLE_SSL_CONNECT_ERROR, with the
    /// reason only in the detail. It was taken for a transient handshake failure and retried.
    #[test]
    fn a_connect_error_from_a_refused_certificate_is_a_certificate_failure() {
        let with = |code, detail: &str| {
            let mut e = curl::Error::new(code);
            e.set_extra(detail.to_owned());
            CurlFailure::of(&e)
        };
        for detail in [
            "schannel: next InitializeSecurityContext failed: CRYPT_E_REVOKED (0x80092010) - The certificate is revoked.",
            "schannel: next InitializeSecurityContext failed: SEC_E_UNTRUSTED_ROOT (0x80090325) - The certificate chain was issued by an authority that is not trusted.",
            "schannel: next InitializeSecurityContext failed: SEC_E_CERT_EXPIRED (0x80090328) - The received certificate has expired.",
            "schannel: next InitializeSecurityContext failed: SEC_E_WRONG_PRINCIPAL (0x80090322) - The target principal name is incorrect.",
            "schannel: next InitializeSecurityContext failed: Unknown error (0x800B0110)",
            "schannel: next InitializeSecurityContext failed: CRYPT_E_NO_REVOCATION_CHECK (0x80092012) - The revocation function was unable to check revocation for the certificate.",
            "schannel: next InitializeSecurityContext failed: CRYPT_E_REVOCATION_OFFLINE (0x80092013) - The revocation function was unable to check revocation because the revocation server was offline.",
        ] {
            assert_eq!(
                with(curl_sys::CURLE_SSL_CONNECT_ERROR, detail),
                CurlFailure::Certificate,
                "{detail}"
            );
        }
        // A handshake that broke for another reason stays a transient handshake failure.
        for detail in [
            "schannel: failed to receive handshake, SSL/TLS connection failed",
            "schannel: next InitializeSecurityContext failed: SEC_E_INTERNAL_ERROR (0x80090304)",
            "",
        ] {
            assert_eq!(
                with(curl_sys::CURLE_SSL_CONNECT_ERROR, detail),
                CurlFailure::TlsHandshake,
                "{detail}"
            );
        }
    }

    #[test]
    fn libcurl_codes_map_to_failure_kinds() {
        let cases = [
            (curl_sys::CURLE_COULDNT_RESOLVE_HOST, CurlFailure::Resolve),
            (curl_sys::CURLE_COULDNT_RESOLVE_PROXY, CurlFailure::Resolve),
            (curl_sys::CURLE_COULDNT_CONNECT, CurlFailure::Connect),
            (curl_sys::CURLE_OPERATION_TIMEDOUT, CurlFailure::Timeout),
            (curl_sys::CURLE_GOT_NOTHING, CurlFailure::Interrupted),
            (curl_sys::CURLE_SEND_ERROR, CurlFailure::Interrupted),
            (curl_sys::CURLE_RECV_ERROR, CurlFailure::Interrupted),
            (curl_sys::CURLE_PARTIAL_FILE, CurlFailure::Interrupted),
            (curl_sys::CURLE_HTTP2, CurlFailure::Interrupted),
            (curl_sys::CURLE_HTTP2_STREAM, CurlFailure::Interrupted),
            (curl_sys::CURLE_SSL_CONNECT_ERROR, CurlFailure::TlsHandshake),
            (
                curl_sys::CURLE_PEER_FAILED_VERIFICATION,
                CurlFailure::Certificate,
            ),
            (curl_sys::CURLE_SSL_CACERT_BADFILE, CurlFailure::Certificate),
            (curl_sys::CURLE_SSL_ISSUER_ERROR, CurlFailure::Certificate),
            (
                curl_sys::CURLE_TOO_MANY_REDIRECTS,
                CurlFailure::TooManyRedirects,
            ),
            (curl_sys::CURLE_UNSUPPORTED_PROTOCOL, CurlFailure::Refused),
            (curl_sys::CURLE_URL_MALFORMAT, CurlFailure::Refused),
            (
                curl_sys::CURLE_FTP_WEIRD_SERVER_REPLY,
                CurlFailure::Protocol,
            ),
            (curl_sys::CURLE_BAD_CONTENT_ENCODING, CurlFailure::Protocol),
            (curl_sys::CURLE_REMOTE_FILE_NOT_FOUND, CurlFailure::NotFound),
            (curl_sys::CURLE_LOGIN_DENIED, CurlFailure::AccessDenied),
            (
                curl_sys::CURLE_REMOTE_ACCESS_DENIED,
                CurlFailure::AccessDenied,
            ),
            (
                curl_sys::CURLE_USE_SSL_FAILED,
                CurlFailure::EncryptionRefused,
            ),
            (curl_sys::CURLE_OUT_OF_MEMORY, CurlFailure::Local),
            (curl_sys::CURLE_WRITE_ERROR, CurlFailure::Local),
            (curl_sys::CURLE_FAILED_INIT, CurlFailure::Other),
        ];
        for (code, expected) in cases {
            assert_eq!(kind(code), expected, "CURLcode {code}");
        }
    }

    #[test]
    fn a_failure_keeps_libcurls_own_detail() {
        let mut e = curl::Error::new(curl_sys::CURLE_PEER_FAILED_VERIFICATION);
        e.set_extra("schannel: the certificate has been revoked".to_owned());
        match curl_error(&e) {
            FetchError::Curl { message, .. } => {
                assert!(message.contains("has been revoked"), "{message}");
                assert!(message.contains("was not OK"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        // Without a detail, the code's description alone.
        let plain = curl::Error::new(curl_sys::CURLE_COULDNT_CONNECT);
        match curl_error(&plain) {
            FetchError::Curl { message, .. } => assert_eq!(message, plain.description()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_connect_timeout_defaults_to_30_seconds() {
        assert_eq!(
            LoopOptions::default().connect_timeout,
            Duration::from_secs(30)
        );
    }
}
