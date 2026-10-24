//! SFTP fetches on russh (#53, ADR-0013), as a [`Transport`]: the engine fetches from an
//! `sftp://` link exactly as from an `https://` one.
//!
//! Each fetch runs as a task on this transport's own tokio runtime:
//!
//! 1. **Connect,** within the connect timeout, to the link's host and port (22 by default).
//! 2. **The host key,** checked against V2's known_hosts file ([`KnownHosts`]) before anything
//!    else is sent. An unknown key is refused and remembered, so the user can trust exactly that
//!    key ([`SftpTransport::trust_host_key`]); a changed key is always refused. Either ends the
//!    fetch with [`FetchError::HostKey`], which carries the fingerprint.
//! 3. **Log in** as the user of the request's login, with its private key when it has one,
//!    otherwise its password.
//! 4. **The file's size and time** (`stat`) become the head, as for FTP
//!    ([`velocity_curl::file_head`]): `200`, `206` for a range, `416` past the end. The gate decides
//!    before any byte is read.
//! 5. **Read** from the range's start, through a [`FetchPipe`]: the same bounded buffer, speed
//!    limits and body limit as libcurl's fetches.
//!
//! Cancelling the fetch, or dropping it, stops the task at its next step.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use russh::client::{self, AuthResult};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::StatusCode;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Notify;
use velocity_curl::{
    file_head, CurlFailure, Fetch, FetchControl, FetchError, FetchPipe, FetchRequest, Gate, GateFn,
    HostKeyProblem, Offered, RequestOptions, Transport,
};

use crate::known_hosts::{fingerprint, KnownHosts, Verdict};

/// The bytes read from the server at a time.
const READ_CHUNK: usize = 64 * 1024;
/// How often a fetch held back by a speed limit checks for tokens (as libcurl's loop does).
const RATE_CHECK: Duration = Duration::from_millis(10);

/// How SFTP fetches behave.
#[derive(Clone, Debug)]
pub struct SftpOptions {
    /// The most body bytes one fetch keeps for its reader before it waits, as libcurl's loop.
    pub buffer_bytes_per_fetch: usize,
    /// How long connecting, the key exchange included, may take.
    pub connect_timeout: Duration,
}

impl Default for SftpOptions {
    fn default() -> Self {
        // The libcurl loop's starting values (`LoopOptions::default`), so both transports hold the
        // same memory per fetch and give up connecting at the same time.
        SftpOptions {
            buffer_bytes_per_fetch: 1024 * 1024,
            connect_timeout: Duration::from_secs(30),
        }
    }
}

/// Why a host key couldn't be trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustError {
    /// No fetch saw a key with this fingerprint from this host and port.
    NotSeen,
    /// The host has another key pinned: a changed key is never trusted.
    Changed,
    /// The known_hosts file couldn't be written.
    Io(String),
}

/// The last untrusted key each server presented, by host and port.
type Seen = Arc<Mutex<HashMap<(String, u16), PublicKey>>>;

/// The SFTP transport: its own runtime, V2's known_hosts file, and the keys awaiting the user.
pub struct SftpTransport {
    runtime: tokio::runtime::Runtime,
    known_hosts: KnownHosts,
    options: SftpOptions,
    seen: Seen,
}

impl SftpTransport {
    /// Starts the transport, with `known_hosts` as the file of trusted host keys.
    pub fn start(known_hosts: KnownHosts, options: SftpOptions) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("velocity-sftp")
            .enable_all()
            .build()?;
        Ok(SftpTransport {
            runtime,
            known_hosts,
            options,
            seen: Arc::default(),
        })
    }

    /// Pins the key a fetch last saw from `host` and `port`, once the user confirmed that its
    /// fingerprint is `fingerprint`. Only a key a fetch actually saw can be trusted, so the user
    /// always confirms the key that will be pinned.
    pub fn trust_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint_: &str,
    ) -> Result<(), TrustError> {
        let seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let key = seen
            .get(&(host.to_ascii_lowercase(), port))
            .filter(|key| fingerprint(key) == fingerprint_)
            .ok_or(TrustError::NotSeen)?;
        match self.known_hosts.check(host, port, key) {
            Ok(Verdict::Changed { .. }) => return Err(TrustError::Changed),
            Ok(_) => {}
            Err(e) => return Err(TrustError::Io(e.to_string())),
        }
        self.known_hosts
            .trust(host, port, key)
            .map_err(|e| TrustError::Io(e.to_string()))
    }
}

impl Transport for SftpTransport {
    fn fetch(&self, request: FetchRequest, gate: GateFn) -> Fetch {
        let control = Arc::new(Control::default());
        let (pipe, fetch) = FetchPipe::open(
            &request,
            self.options.buffer_bytes_per_fetch,
            gate,
            Arc::clone(&control) as Arc<dyn FetchControl>,
        );
        let job = Job {
            request,
            known_hosts: self.known_hosts.clone(),
            seen: Arc::clone(&self.seen),
            options: self.options.clone(),
        };
        self.runtime.spawn(run(job, pipe, control));
        fetch
    }
}

/// How the fetch's reader reaches its task.
#[derive(Default)]
struct Control {
    cancelled: AtomicBool,
    cancel: Notify,
    room: Notify,
}

impl FetchControl for Control {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel.notify_one();
    }

    fn resume(&self) {
        self.room.notify_one();
    }
}

/// What one fetch's task needs.
struct Job {
    request: FetchRequest,
    known_hosts: KnownHosts,
    seen: Seen,
    options: SftpOptions,
}

async fn run(job: Job, mut pipe: FetchPipe, control: Arc<Control>) {
    let outcome = {
        let work = transfer(&job, &mut pipe, &control);
        tokio::select! {
            outcome = work => outcome,
            () = cancelled(&control) => Err(FetchError::Cancelled),
        }
    };
    pipe.finish(outcome);
}

async fn cancelled(control: &Control) {
    while !control.cancelled.load(Ordering::Acquire) {
        control.cancel.notified().await;
    }
}

/// The fetch, up to its outcome.
async fn transfer(job: &Job, pipe: &mut FetchPipe, control: &Control) -> Result<(), FetchError> {
    let target = Target::parse(&job.request.url)?;
    let sftp = connect(job, &target).await?;
    let meta = sftp
        .metadata(target.path.clone())
        .await
        .map_err(sftp_failure)?;
    let modified = meta
        .mtime
        .map(|t| UNIX_EPOCH + Duration::from_secs(u64::from(t)));
    let head = file_head(job.request.range, meta.size, modified, &job.request.url);
    let status = head.status;
    let start = job.request.range.map_or(0, |(start, _)| start);
    if let Gate::Abort(reason) = pipe.head(head) {
        return Err(FetchError::Aborted(reason));
    }
    if status == 416 {
        // Nothing at the range's start: the answer has no body, like an HTTP 416.
        return Ok(());
    }
    if status == 200 && start > 0 {
        // Without a size, nothing proved the range: its bytes are never delivered as the file's.
        return Err(FetchError::Aborted(
            "a 200 for a range after byte 0".to_owned(),
        ));
    }
    let mut file = sftp.open(target.path.clone()).await.map_err(sftp_failure)?;
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|e| failure(CurlFailure::Interrupted, format!("seeking to {start}: {e}")))?;
    }
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|e| failure(CurlFailure::Interrupted, format!("reading the file: {e}")))?;
        if read == 0 {
            return Ok(());
        }
        let mut chunk = &buffer[..read];
        while !chunk.is_empty() {
            match pipe.offer(chunk) {
                // The body limit: the range asked for is complete.
                Offered::Took(took) if took < chunk.len() => return Ok(()),
                Offered::Took(took) => chunk = &chunk[took..],
                Offered::Full => control.room.notified().await,
                Offered::Throttled => {
                    while !pipe.rate_ready() {
                        tokio::time::sleep(RATE_CHECK).await;
                    }
                }
            }
        }
    }
}

/// Connects, checks the host key, logs in, and opens the SFTP subsystem.
async fn connect(job: &Job, target: &Target) -> Result<SftpSession, FetchError> {
    let timeout = job.options.connect_timeout;
    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        timeout,
        tokio::net::lookup_host((target.host.as_str(), target.port)),
    )
    .await
    .map_err(|_| failure(CurlFailure::Timeout, "resolving the host timed out"))?
    .map_err(|e| failure(CurlFailure::Resolve, format!("{}: {e}", target.host)))?
    .collect();
    let problem: Arc<Mutex<Option<HostKeyProblem>>> = Arc::default();
    let handler = HostKeyCheck {
        known_hosts: job.known_hosts.clone(),
        host: target.host.clone(),
        port: target.port,
        seen: Arc::clone(&job.seen),
        problem: Arc::clone(&problem),
    };
    let config = Arc::new(client::Config::default());
    let connected = tokio::time::timeout(timeout, client::connect(config, &addrs[..], handler))
        .await
        .map_err(|_| failure(CurlFailure::Timeout, "connecting timed out"))?;
    let mut session = match connected {
        Ok(session) => session,
        Err(e) => {
            let problem = problem.lock().unwrap_or_else(|p| p.into_inner()).take();
            return Err(match problem {
                Some(problem) => FetchError::HostKey(problem),
                None => russh_failure(&e),
            });
        }
    };
    log_in(&mut session, &job.request.options).await?;
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| russh_failure(&e))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| russh_failure(&e))?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(sftp_failure)
}

/// Logs in as the request's user: with its private key when it has one, else its password.
async fn log_in(
    session: &mut client::Handle<HostKeyCheck>,
    options: &RequestOptions,
) -> Result<(), FetchError> {
    let Some((user, password)) = &options.login else {
        return Err(failure(
            CurlFailure::AccessDenied,
            "an SFTP login needs a user name",
        ));
    };
    let result = match &options.ssh_key {
        Some(key_file) => {
            let key =
                load_secret_key(&key_file.path, key_file.passphrase.as_deref()).map_err(|e| {
                    failure(
                        CurlFailure::AccessDenied,
                        format!("the private key couldn't be opened: {e}"),
                    )
                })?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|e| russh_failure(&e))?
                .flatten();
            session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
        }
        None => session.authenticate_password(user, password).await,
    };
    match result.map_err(|e| russh_failure(&e))? {
        AuthResult::Success => Ok(()),
        AuthResult::Failure { .. } => Err(failure(
            CurlFailure::AccessDenied,
            "the server refused the login",
        )),
    }
}

/// russh's callbacks for one connection: only the host-key check.
struct HostKeyCheck {
    known_hosts: KnownHosts,
    host: String,
    port: u16,
    seen: Seen,
    problem: Arc<Mutex<Option<HostKeyProblem>>>,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Host certificates need a trusted CA, which V2 doesn't have: refused.
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            return Ok(false);
        };
        let verdict = self.known_hosts.check(&self.host, self.port, key)?;
        let pinned = match verdict {
            Verdict::Known => return Ok(true),
            Verdict::Unknown => {
                self.seen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert((self.host.clone(), self.port), key.clone());
                None
            }
            Verdict::Changed { pinned } => Some(pinned),
        };
        *self.problem.lock().unwrap_or_else(|e| e.into_inner()) = Some(HostKeyProblem {
            host: self.host.clone(),
            port: self.port,
            fingerprint: fingerprint(key),
            pinned,
        });
        Ok(false)
    }
}

/// An `sftp://` link's host, port and path.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
    /// The path, percent-decoded, as the server's file system names it.
    path: String,
}

impl Target {
    fn parse(url: &str) -> Result<Self, FetchError> {
        let refused = |why: &str| failure(CurlFailure::Refused, format!("not an SFTP link: {why}"));
        let rest = url
            .split_once("://")
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("sftp"))
            .ok_or_else(|| refused("the scheme isn't sftp"))?
            .1;
        let (authority, path) = rest.split_at(rest.find('/').ok_or_else(|| refused("no path"))?);
        if authority.contains('@') {
            return Err(refused("a login in the link"));
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, after) = v6.split_once(']').ok_or_else(|| refused("a bad host"))?;
                (host, after.strip_prefix(':'))
            }
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            },
        };
        let port = match port {
            Some(p) => p.parse().map_err(|_| refused("a bad port"))?,
            None => 22,
        };
        let path = percent_decode(path.split(['?', '#']).next().unwrap_or(""))
            .ok_or_else(|| refused("a bad escape in the path"))?;
        if host.is_empty() || path.len() < 2 {
            return Err(refused("no host or no file"));
        }
        Ok(Target {
            host: host.to_ascii_lowercase(),
            port,
            path,
        })
    }
}

/// `%XX` escapes decoded, as UTF-8; `None` for a broken escape or bytes that aren't UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn failure(kind: CurlFailure, message: impl Into<String>) -> FetchError {
    FetchError::Sftp {
        kind,
        message: message.into(),
    }
}

/// What kind of failure a russh error is.
fn russh_failure(e: &russh::Error) -> FetchError {
    let kind = match e {
        russh::Error::IO(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            CurlFailure::Connect
        }
        russh::Error::IO(_)
        | russh::Error::Disconnect
        | russh::Error::HUP
        | russh::Error::SendError
        | russh::Error::RecvError => CurlFailure::Interrupted,
        russh::Error::InactivityTimeout | russh::Error::Elapsed(_) => CurlFailure::Timeout,
        _ => CurlFailure::Protocol,
    };
    failure(kind, e.to_string())
}

/// What kind of failure an SFTP error is: a missing file and a refused one are the server's
/// answer; a broken connection may be retried.
fn sftp_failure(e: russh_sftp::client::error::Error) -> FetchError {
    use russh_sftp::client::error::Error;
    let kind = match &e {
        Error::Status(s) if s.status_code == StatusCode::NoSuchFile => CurlFailure::NotFound,
        Error::Status(s) if s.status_code == StatusCode::PermissionDenied => {
            CurlFailure::AccessDenied
        }
        Error::IO(_) => CurlFailure::Interrupted,
        Error::Timeout => CurlFailure::Timeout,
        _ => CurlFailure::Protocol,
    };
    failure(kind, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> Result<Target, FetchError> {
        Target::parse(url)
    }

    #[test]
    fn links_give_their_host_port_and_decoded_path() {
        assert_eq!(
            target("sftp://Files.Example.org/pub/a%20b.zip").unwrap(),
            Target {
                host: "files.example.org".to_owned(),
                port: 22,
                path: "/pub/a b.zip".to_owned()
            }
        );
        let local = target("sftp://127.0.0.1:2222/C:/Users/x/f.bin").unwrap();
        assert_eq!(
            (local.port, local.path.as_str()),
            (2222, "/C:/Users/x/f.bin")
        );
        let v6 = target("sftp://[::1]:2200/f").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("::1", 2200));
        assert_eq!(target("sftp://[::1]/f").unwrap().port, 22);
    }

    #[test]
    fn links_that_arent_sftp_files_are_refused() {
        for url in [
            "https://h/f",
            "sftp://h",
            "sftp://h/",
            "sftp://user:pw@h/f",
            "sftp://h:notaport/f",
            "sftp://h/bad%zzescape",
            "sftp:///f",
        ] {
            assert!(
                matches!(
                    target(url),
                    Err(FetchError::Sftp {
                        kind: CurlFailure::Refused,
                        ..
                    })
                ),
                "{url}"
            );
        }
    }
}
