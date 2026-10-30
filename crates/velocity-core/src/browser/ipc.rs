//! The local connection between the native host and the running app (#65), ported from V1's
//! `ipc.rs` with V2's endpoint ([`Paths::ipc_endpoint`], #27).
//!
//! - **Unix:** a socket in the data folder (folder 0700, socket 0600).
//! - **Windows:** a named pipe, created first (`first_pipe_instance`, so nothing else can hold the
//!   name before the app) and refusing remote clients.
//!
//! Each connection carries one line of JSON, an [`IpcEnvelope`], and gets one line back, a
//! [`BrowserResponse`]. The envelope must hold the token the app wrote at startup to a file only
//! this user can read; a wrong one is refused. Lines are bounded, connections time out, and at
//! most [`MAX_CONNECTIONS`] are handled at once. It is deliberately not an HTTP endpoint.

use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::protocol::{BrowserResponse, IpcEnvelope, MAX_MESSAGE_BYTES};
use crate::domain::ErrorCode;
use crate::paths::Paths;

/// The longest line: a message and its envelope.
const MAX_LINE: usize = MAX_MESSAGE_BYTES + 8 * 1024;
/// The longest a connection may take, from opening to its answer (V1's).
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
/// Connections handled at once (V1's); more are closed unanswered.
pub const MAX_CONNECTIONS: usize = 8;

/// The per-launch token: 32 random bytes from the operating system, as 64 hex digits. `Debug`
/// never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A new token, from the operating system's secure random source (`getrandom`, PO decision on
    /// #65). `Internal` if the system has none.
    pub fn generate() -> Result<Self, ErrorCode> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| ErrorCode::Internal)?;
        Ok(Token(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    /// A token read from somewhere else: `None` unless it is 64 hex digits.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Token(text.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Compares in time that doesn't depend on where the tokens differ.
    fn matches(&self, other: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), other.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Writes the token where the native host reads it: the data folder, readable by this user only.
pub fn write_token(paths: &Paths, token: &Token) -> Result<(), ErrorCode> {
    paths.ensure()?;
    write_private(&paths.ipc_token_file(), token.as_str().as_bytes())
}

/// The token the running app wrote. `NotFound` when there is none (the app isn't running, or
/// never ran); `InvalidState` when the file holds something else.
pub fn read_token(paths: &Paths) -> Result<Token, ErrorCode> {
    match std::fs::read_to_string(paths.ipc_token_file()) {
        Ok(text) => Token::parse(&text).ok_or(ErrorCode::InvalidState),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ErrorCode::NotFound),
        Err(_) => Err(ErrorCode::Io),
    }
}

/// Writes `bytes` to a new file beside `path`, owner-only on Unix, then renames it into place.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ErrorCode> {
    let temp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    };
    write().map_err(|_| ErrorCode::Io)
}

/// What answers each envelope with a valid token.
pub type Handler =
    Arc<dyn Fn(IpcEnvelope) -> Pin<Box<dyn Future<Output = BrowserResponse> + Send>> + Send + Sync>;

/// A [`Handler`] from an async function.
pub fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(IpcEnvelope) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = BrowserResponse> + Send + 'static,
{
    Arc::new(move |e| Box::pin(f(e)))
}

async fn read_line_limited<R: AsyncRead + Unpin>(r: R) -> std::io::Result<Vec<u8>> {
    let mut reader = BufReader::new(r.take(MAX_LINE as u64 + 1));
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf).await?;
    if buf.len() > MAX_LINE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "line too long",
        ));
    }
    while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        buf.pop();
    }
    Ok(buf)
}

async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    token: Arc<Token>,
    handler: Handler,
) {
    let (read, mut write) = tokio::io::split(stream);
    let work = async {
        let line = read_line_limited(read).await.ok()?;
        let response = match serde_json::from_slice::<IpcEnvelope>(&line) {
            Err(_) => BrowserResponse::err("", "INVALID_MESSAGE", "Malformed IPC envelope."),
            Ok(env) if !token.matches(&env.token) => {
                BrowserResponse::err("", "UNAUTHORIZED", "The native host is not authorized.")
            }
            Ok(env) => handler(env).await,
        };
        let mut out = serde_json::to_vec(&response).ok()?;
        out.push(b'\n');
        write.write_all(&out).await.ok()?;
        write.flush().await.ok()
    };
    let _ = tokio::time::timeout(CONNECTION_TIMEOUT, work).await;
}

/// Connections being handled, bounded by [`MAX_CONNECTIONS`].
#[derive(Clone, Default)]
struct Slots(Arc<AtomicUsize>);

/// One of [`Slots`], given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slots {
    fn take(&self) -> Option<Slot> {
        // Taken first, so two connections can't both see the last free one; given back when full.
        let slot = Slot(Arc::clone(&self.0));
        (self.0.fetch_add(1, Ordering::AcqRel) < MAX_CONNECTIONS).then_some(slot)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Starts listening on the endpoint, on the current tokio runtime. Returns once the endpoint is
/// held; the connections are served in the background until the task is aborted or the runtime
/// ends. `InvalidState` when another app already serves it; `Io` when it can't be created.
pub async fn serve(
    paths: &Paths,
    token: Token,
    handler: Handler,
) -> Result<tokio::task::JoinHandle<()>, ErrorCode> {
    platform::serve(
        paths.ipc_endpoint(),
        Arc::new(token),
        handler,
        Slots::default(),
    )
    .await
}

#[cfg(unix)]
mod platform {
    use super::*;
    use tokio::net::{UnixListener, UnixStream};

    pub(super) async fn serve(
        path: String,
        token: Arc<Token>,
        handler: Handler,
        slots: Slots,
    ) -> Result<tokio::task::JoinHandle<()>, ErrorCode> {
        let p = Path::new(&path);
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            if UnixStream::connect(p).await.is_ok() {
                return Err(ErrorCode::InvalidState);
            }
            // Only a stale socket of our own is removed, never a file or a link.
            use std::os::unix::fs::FileTypeExt;
            if !meta.file_type().is_socket() {
                return Err(ErrorCode::InvalidState);
            }
            std::fs::remove_file(p).map_err(|_| ErrorCode::Io)?;
        }
        let listener = UnixListener::bind(p).map_err(|_| ErrorCode::Io)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| ErrorCode::PermissionDenied)?;
        }
        Ok(tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let Some(slot) = slots.take() else {
                    continue;
                };
                let (token, handler) = (Arc::clone(&token), Arc::clone(&handler));
                tokio::spawn(async move {
                    handle_connection(stream, token, handler).await;
                    drop(slot);
                });
            }
        }))
    }

    pub(super) async fn connect(path: &str) -> std::io::Result<UnixStream> {
        UnixStream::connect(path).await
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, ServerOptions};

    pub(super) async fn serve(
        name: String,
        token: Arc<Token>,
        handler: Handler,
        slots: Slots,
    ) -> Result<tokio::task::JoinHandle<()>, ErrorCode> {
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&name)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::PermissionDenied => ErrorCode::InvalidState,
                _ => ErrorCode::Io,
            })?;
        Ok(tokio::spawn(async move {
            loop {
                let accepted = server.connect().await.is_ok();
                // A new instance every time: after a failed connect, such as a client that
                // already hung up, the old one can't be used again.
                let previous = server;
                server = match ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&name)
                {
                    Ok(s) => s,
                    Err(_) => return,
                };
                if !accepted {
                    continue;
                }
                let Some(slot) = slots.take() else {
                    continue;
                };
                let (token, handler) = (Arc::clone(&token), Arc::clone(&handler));
                tokio::spawn(async move {
                    handle_connection(previous, token, handler).await;
                    drop(slot);
                });
            }
        }))
    }

    pub(super) async fn connect(name: &str) -> std::io::Result<NamedPipeClient> {
        ClientOptions::new().open(name)
    }
}

/// Why the native host got no answer from the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcClientError {
    /// Nothing listens on the endpoint: the app isn't running.
    NotRunning,
    /// The connection broke or timed out.
    Io(String),
    /// The answer wasn't one.
    InvalidResponse,
}

impl fmt::Display for IpcClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("Velocity V2 is not running"),
            Self::Io(e) => write!(f, "Velocity V2 did not answer: {e}"),
            Self::InvalidResponse => f.write_str("Velocity V2 sent an invalid answer"),
        }
    }
}

/// Sends one envelope to the running app and reads its answer: the native host's side.
pub async fn send(
    paths: &Paths,
    envelope: &IpcEnvelope,
) -> Result<BrowserResponse, IpcClientError> {
    let stream = platform::connect(&paths.ipc_endpoint())
        .await
        .map_err(|_| IpcClientError::NotRunning)?;
    let work = async {
        let (read, mut write) = tokio::io::split(stream);
        let mut line = serde_json::to_vec(envelope).map_err(|_| IpcClientError::InvalidResponse)?;
        line.push(b'\n');
        write
            .write_all(&line)
            .await
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        write
            .flush()
            .await
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        let answer = read_line_limited(read)
            .await
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        serde_json::from_slice::<BrowserResponse>(&answer)
            .map_err(|_| IpcClientError::InvalidResponse)
    };
    tokio::time::timeout(CONNECTION_TIMEOUT, work)
        .await
        .map_err(|_| IpcClientError::Io("timed out".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("v2-ipc-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }

        fn paths(&self) -> Paths {
            Paths::resolve_with(Some(self.0.clone().into()), None, "unused").unwrap()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn envelope(token: &str) -> IpcEnvelope {
        IpcEnvelope {
            token: token.to_owned(),
            caller: "c".into(),
            message: json!({}),
        }
    }

    #[test]
    fn a_token_is_64_random_hex_digits_and_never_printed() {
        let (a, b) = (Token::generate().unwrap(), Token::generate().unwrap());
        assert_ne!(a, b);
        assert!(Token::parse(a.as_str()).is_some());
        assert!(!format!("{a:?}").contains(a.as_str()));
        assert_eq!(Token::parse("abc"), None);
        assert_eq!(Token::parse(&"g".repeat(64)), None);
        assert!(a.matches(a.as_str()) && !a.matches(b.as_str()) && !a.matches(""));
    }

    #[test]
    fn the_token_file_is_read_back_and_is_private() {
        let dir = Dir::new("token");
        let paths = dir.paths();
        assert_eq!(read_token(&paths), Err(ErrorCode::NotFound));
        let token = Token::generate().unwrap();
        write_token(&paths, &token).unwrap();
        assert_eq!(read_token(&paths), Ok(token));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.ipc_token_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(paths.ipc_token_file(), "not a token").unwrap();
        assert_eq!(read_token(&paths), Err(ErrorCode::InvalidState));
    }

    #[test]
    fn only_an_envelope_with_the_token_reaches_the_handler() {
        let dir = Dir::new("round-trip");
        let paths = dir.paths();
        let token = Token::generate().unwrap();
        let good = token.as_str().to_owned();
        let rt = runtime();
        let _server = rt
            .block_on(serve(
                &paths,
                token,
                handler(|env: IpcEnvelope| async move {
                    BrowserResponse::ok("x", json!({ "caller": env.caller }))
                }),
            ))
            .unwrap();
        let ok = rt.block_on(send(&paths, &envelope(&good))).unwrap();
        assert!(ok.ok);
        assert_eq!(ok.result.unwrap()["caller"], "c");
        let wrong = rt
            .block_on(send(&paths, &envelope(&"0".repeat(64))))
            .unwrap();
        assert_eq!(wrong.error_code(), Some("UNAUTHORIZED"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.ipc_endpoint())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A second app can't take the endpoint while the first serves it.
        let second = rt.block_on(serve(
            &paths,
            Token::generate().unwrap(),
            handler(|_| async { BrowserResponse::ok("", json!(null)) }),
        ));
        assert_eq!(second.err(), Some(ErrorCode::InvalidState));
    }

    #[test]
    fn an_app_that_isnt_running_is_reported() {
        let dir = Dir::new("not-running");
        let e = runtime()
            .block_on(send(&dir.paths(), &envelope("t")))
            .unwrap_err();
        assert_eq!(e, IpcClientError::NotRunning);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_refused() {
        let rt = runtime();
        let long = vec![b'x'; MAX_LINE + 10];
        let read = rt.block_on(read_line_limited(&long[..]));
        assert!(read.is_err());
        let short = rt.block_on(read_line_limited(&b"{}\n"[..])).unwrap();
        assert_eq!(short, b"{}");
    }

    #[test]
    fn at_most_the_limit_of_connections_is_handled_at_once() {
        let slots = Slots::default();
        let taken: Vec<_> = (0..MAX_CONNECTIONS).map(|_| slots.take()).collect();
        assert!(taken.iter().all(Option::is_some));
        assert!(slots.take().is_none());
        drop(taken);
        assert!(slots.take().is_some());
    }
}
