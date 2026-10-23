//! The engine as a child process (ADR-0012): the same [`InProcessEngine`] behind the
//! `DirectTransfer` contract, reached over the child's stdin and stdout with the framed, versioned
//! protocol in [`crate::protocol`] (#39, docs/ENGINE_PROTOCOL.md).
//!
//! - the child writes `hello` with its protocol and contract versions;
//! - the app checks them and answers `start` with its own, and the first transfer id to assign;
//! - the app then sends `request`s, each answered by one `reply`, and the child pushes `event`s.
//!
//! When the app closes stdin, or goes away, the child pauses every transfer, keeping its part file
//! for a resume (#43), waits briefly for the workers to stop, and exits. The child gets one
//! argument, `serve`, and a cleared environment: nothing secret can reach its arguments or
//! environment (`.claude/rules/security.md`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use velocity_core::domain::{Capabilities, ErrorCode};
use velocity_core::transfer::{
    dispatch, DirectTransfer, Limits, TransferCommand, TransferEvent, TransferId, TransferReply,
    TransferSnapshot, TransferSpec, TRANSFER_CONTRACT_VERSION,
};

use crate::engine::InProcessEngine;
use crate::engine_log::EngineLog;
use crate::protocol::{
    check_versions, read_message, write_message, FrameError, FromChild, ToChild, PROTOCOL_VERSION,
};

/// How long the app waits for the child's `hello`, or for any reply.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a child that lost its parent waits for cancelled transfers to clean up.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
/// How long dropping the client waits for the child to exit before killing it.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// The environment variables the engine keeps. They are system settings, never secrets: Windows
/// sockets need `SystemRoot`, and OpenSSL on Linux reads the trust store's location from the two
/// `SSL_CERT_*` variables when they are set. Proxy variables are dropped too (ADR-0008).
const KEPT_ENV: &[&str] = if cfg!(windows) {
    &["SystemRoot", "windir"]
} else {
    &["SSL_CERT_FILE", "SSL_CERT_DIR"]
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs the child's side: serves the engine on stdin and stdout until stdin closes.
pub fn serve() -> Result<(), String> {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let mut input = std::io::stdin().lock();
    let hello = FromChild::Hello {
        protocol: PROTOCOL_VERSION,
        contract: TRANSFER_CONTRACT_VERSION,
    };
    write_message(&mut *lock(&out), &hello).map_err(|e| format!("writing hello: {e:?}"))?;
    let first_transfer_id = match read_message::<ToChild>(&mut input) {
        Ok(Some(ToChild::Start {
            protocol,
            contract,
            first_transfer_id,
        })) => {
            check_versions(protocol, contract).map_err(|_| {
                format!("the app speaks protocol {protocol} and contract {contract}; this engine speaks {PROTOCOL_VERSION} and {TRANSFER_CONTRACT_VERSION}")
            })?;
            first_transfer_id
        }
        Ok(Some(other)) => return Err(format!("expected start, got {other:?}")),
        Ok(None) => return Ok(()),
        Err(e) => return Err(format!("reading start: {e:?}")),
    };

    let mut engine = InProcessEngine::start_from(first_transfer_id)?;
    let events = engine.subscribe();
    let event_out = Arc::clone(&out);
    thread::spawn(move || {
        for event in events {
            match write_message(&mut *lock(&event_out), &FromChild::Event { event }) {
                Ok(()) => {}
                // One event can't exceed the limit today; if one ever does, say so and go on.
                Err(FrameError::TooLarge { len }) => {
                    eprintln!("an event of {len} bytes was dropped")
                }
                Err(_) => break,
            }
        }
    });

    let mut outcome = Ok(());
    loop {
        // The parent is the app, but its messages still cross a process boundary: anything that
        // isn't a well-formed request ends the session instead of being guessed at.
        let (seq, command) = match read_message::<ToChild>(&mut input) {
            Ok(Some(ToChild::Request { seq, command })) => (seq, command),
            Ok(None) => break,
            Ok(Some(other)) => {
                outcome = Err(format!("expected a request, got {other:?}"));
                break;
            }
            Err(e) => {
                outcome = Err(format!("a malformed request: {e:?}"));
                break;
            }
        };
        let result = dispatch(&mut engine, command);
        let written = match write_message(&mut *lock(&out), &FromChild::Reply { seq, result }) {
            Err(FrameError::TooLarge { len }) => {
                eprintln!("the reply to request {seq} is {len} bytes, over the limit");
                let refused = FromChild::Reply {
                    seq,
                    result: Err(ErrorCode::Internal),
                };
                write_message(&mut *lock(&out), &refused)
            }
            other => other,
        };
        if written.is_err() {
            break;
        }
    }

    // Paused, not cancelled: the part files stay for the app to resume them from (#43).
    engine.pause_all();
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    while !engine.idle() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    outcome
}

/// The command that starts the engine: one fixed argument and a cleared environment.
fn engine_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .arg("serve")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in KEPT_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The app has no console; without this flag Windows would open one for the engine.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

type ReplySender = Sender<Result<TransferReply, ErrorCode>>;

/// What the reader thread shares with the requesters.
struct Shared {
    /// The requests waiting for a reply; `None` once the reader has stopped.
    pending: Mutex<Option<HashMap<u64, ReplySender>>>,
    subscribers: Mutex<Vec<Sender<TransferEvent>>>,
}

/// Stops waiting for the reply to `seq`.
fn forget(shared: &Shared, seq: u64) {
    if let Some(pending) = lock(&shared.pending).as_mut() {
        pending.remove(&seq);
    }
}

/// The app's side of the protocol over any pair of streams; [`ChildProcessEngine`] runs it over a
/// child's pipes.
struct Connection {
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    shared: Arc<Shared>,
    next_seq: AtomicU64,
    capabilities: Capabilities,
    /// Disconnects when the reader stops: the engine's stdout ended or broke the protocol.
    closed: Mutex<Receiver<()>>,
}

impl Connection {
    /// Waits for the engine's `hello`, checks its versions, answers `start` and asks for its
    /// capabilities.
    fn open(
        stdin: impl Write + Send + 'static,
        stdout: impl Read + Send + 'static,
        first_transfer_id: TransferId,
    ) -> Result<Self, ErrorCode> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Some(HashMap::new())),
            subscribers: Mutex::new(Vec::new()),
        });
        let (hello_tx, hello_rx) = channel();
        let (closed_tx, closed) = channel::<()>();
        let reader_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("velocity-engine-reader".into())
            .spawn(move || {
                read_engine(stdout, &reader_shared, hello_tx);
                drop(closed_tx);
            })
            .map_err(|_| ErrorCode::EngineStartFailed)?;

        match hello_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(Ok((protocol, contract))) => check_versions(protocol, contract)?,
            Ok(Err(code)) => return Err(code),
            Err(_) => return Err(ErrorCode::EngineStartFailed),
        }
        let mut connection = Connection {
            stdin: Mutex::new(Some(Box::new(stdin))),
            shared,
            next_seq: AtomicU64::new(0),
            capabilities: Capabilities {
                protocols: Vec::new(),
                live_connection_changes: false,
                validator_resume: false,
                metalink: false,
                max_connections_per_host: 1,
            },
            closed: Mutex::new(closed),
        };
        let start = ToChild::Start {
            protocol: PROTOCOL_VERSION,
            contract: TRANSFER_CONTRACT_VERSION,
            first_transfer_id,
        };
        connection
            .send(&start)
            .map_err(|_| ErrorCode::EngineStartFailed)?;
        match connection.request(TransferCommand::Capabilities {})? {
            TransferReply::Capabilities { capabilities } => connection.capabilities = capabilities,
            _ => return Err(ErrorCode::EngineIncompatible),
        }
        Ok(connection)
    }

    fn send(&self, message: &ToChild) -> Result<(), ErrorCode> {
        let mut stdin = lock(&self.stdin);
        let out = stdin.as_mut().ok_or(ErrorCode::EngineUnavailable)?;
        write_message(out, message).map_err(|_| ErrorCode::EngineUnavailable)
    }

    fn request(&self, command: TransferCommand) -> Result<TransferReply, ErrorCode> {
        self.request_within(command, COMMAND_TIMEOUT)
    }

    /// [`Self::request`], waiting at most `timeout` for the reply.
    fn request_within(
        &self,
        command: TransferCommand,
        timeout: Duration,
    ) -> Result<TransferReply, ErrorCode> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = channel();
        match lock(&self.shared.pending).as_mut() {
            Some(pending) => pending.insert(seq, tx),
            // The engine is already gone: fail now rather than at the timeout.
            None => return Err(ErrorCode::EngineUnavailable),
        };
        if let Err(code) = self.send(&ToChild::Request { seq, command }) {
            forget(&self.shared, seq);
            return Err(code);
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                forget(&self.shared, seq);
                Err(ErrorCode::EngineUnavailable)
            }
            // The reader dropped every pending reply: the engine is gone.
            Err(RecvTimeoutError::Disconnected) => Err(ErrorCode::EngineUnavailable),
        }
    }

    fn subscribe(&self) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        lock(&self.shared.subscribers).push(tx);
        rx
    }

    /// Closes the engine's stdin, which asks it to cancel its transfers and exit, and waits up to
    /// `wait` for its stdout to end. Returns whether it ended.
    fn close(&self, wait: Duration) -> bool {
        lock(&self.stdin).take();
        matches!(
            lock(&self.closed).recv_timeout(wait),
            Err(RecvTimeoutError::Disconnected)
        )
    }
}

/// The app's reader: the first message must be the engine's `hello`; after it, replies go to
/// their requests and events to subscribers. A frame error, a second `hello` or the end of the
/// stream stops it, and every waiting request then fails as unavailable.
fn read_engine(
    mut stdout: impl Read,
    shared: &Shared,
    hello: Sender<Result<(u32, u32), ErrorCode>>,
) {
    let first = match read_message::<FromChild>(&mut stdout) {
        Ok(Some(FromChild::Hello { protocol, contract })) => Ok((protocol, contract)),
        // An engine that exits before saying hello failed to start.
        Ok(None) | Err(FrameError::Truncated) | Err(FrameError::Io(_)) => {
            Err(ErrorCode::EngineStartFailed)
        }
        // Anything else is an engine that speaks another protocol.
        Ok(Some(_)) | Err(FrameError::TooLarge { .. }) | Err(FrameError::Malformed(_)) => {
            Err(ErrorCode::EngineIncompatible)
        }
    };
    let started = first.is_ok();
    let _ = hello.send(first);
    if started {
        while let Ok(Some(message)) = read_message::<FromChild>(&mut stdout) {
            match message {
                FromChild::Hello { .. } => break,
                FromChild::Reply { seq, result } => {
                    if let Some(tx) = lock(&shared.pending).as_mut().and_then(|p| p.remove(&seq)) {
                        // The requester may have timed out and gone; that's not an error here.
                        let _ = tx.send(result);
                    }
                }
                FromChild::Event { event } => {
                    lock(&shared.subscribers).retain(|s| s.send(event.clone()).is_ok());
                }
            }
        }
    }
    lock(&shared.pending).take();
    lock(&shared.subscribers).clear();
}

/// How to start an engine.
#[derive(Clone)]
pub struct SpawnOptions {
    /// The first id the engine assigns. A supervisor raises it after a restart, so a new engine
    /// never reuses an id an earlier one assigned.
    pub first_transfer_id: TransferId,
    /// Where the engine's stderr goes.
    pub log: EngineLog,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        SpawnOptions {
            first_transfer_id: TransferId(1),
            log: EngineLog::new(),
        }
    }
}

/// A `DirectTransfer` client for an engine running as a child process. Its methods take `&self`
/// as well, so a supervisor can share one engine between threads.
pub struct ChildProcessEngine {
    child: Child,
    connection: Connection,
}

impl ChildProcessEngine {
    /// Starts `<program> serve` with the default options.
    pub fn spawn(program: &Path) -> Result<Self, ErrorCode> {
        Self::spawn_with(program, &SpawnOptions::default())
    }

    /// Starts `<program> serve`, completes the handshake and asks for its capabilities.
    pub fn spawn_with(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode> {
        let mut child = engine_command(program)
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ErrorCode::EngineMissing,
                _ => ErrorCode::EngineStartFailed,
            })?;
        let opened = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
            (Some(stdin), Some(stdout), Some(stderr)) => options
                .log
                .capture(stderr)
                .map_err(|_| ErrorCode::EngineStartFailed)
                .and_then(|()| Connection::open(stdin, stdout, options.first_transfer_id)),
            _ => Err(ErrorCode::EngineStartFailed),
        };
        match opened {
            Ok(connection) => Ok(ChildProcessEngine { child, connection }),
            Err(code) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(code)
            }
        }
    }

    /// Asks the engine to stop by closing its stdin: it cancels its transfers and exits. Commands
    /// fail with `EngineUnavailable` from then on, even while another thread still holds the
    /// engine.
    pub fn stop(&self) {
        lock(&self.connection.stdin).take();
    }

    /// Whether the engine answers a request for its state within `timeout`: the supervisor's
    /// check for an engine that hangs without exiting (#48). An error reply is still an answer.
    pub fn answers_within(&self, timeout: Duration) -> bool {
        !matches!(
            self.connection
                .request_within(TransferCommand::Snapshot {}, timeout),
            Err(ErrorCode::EngineUnavailable)
        )
    }

    /// The child's operating-system process id, for diagnostics and measurements.
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub fn capabilities(&self) -> Capabilities {
        self.connection.capabilities.clone()
    }

    pub fn add(&self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        match self.connection.request(TransferCommand::Add {
            spec: Box::new(spec),
        })? {
            TransferReply::Added { id } => Ok(id),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    pub fn pause(&self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Pause { id })
    }

    pub fn resume(&self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Resume { id })
    }

    pub fn cancel(&self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Cancel { id })
    }

    pub fn remove(&self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Remove { id })
    }

    pub fn set_limits(&self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        self.done(TransferCommand::SetLimits { id, limits })
    }

    pub fn set_global_limits(&self, limits: Limits) -> Result<(), ErrorCode> {
        self.done(TransferCommand::SetGlobalLimits { limits })
    }

    pub fn set_connections(&self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        match self
            .connection
            .request(TransferCommand::SetConnections { id, connections })?
        {
            TransferReply::Connections { connections } => Ok(connections),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    pub fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        match self.connection.request(TransferCommand::Snapshot {})? {
            TransferReply::Snapshot { transfers } => Ok(transfers),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    /// A receiver for the engine's events. It disconnects when the engine's stdout ends, which is
    /// how a supervisor notices a crash.
    pub fn subscribe(&self) -> Receiver<TransferEvent> {
        self.connection.subscribe()
    }

    fn done(&self, command: TransferCommand) -> Result<(), ErrorCode> {
        match self.connection.request(command)? {
            TransferReply::Done {} => Ok(()),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }
}

impl DirectTransfer for ChildProcessEngine {
    fn capabilities(&self) -> Capabilities {
        ChildProcessEngine::capabilities(self)
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        ChildProcessEngine::add(self, spec)
    }

    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        ChildProcessEngine::pause(self, id)
    }

    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        ChildProcessEngine::resume(self, id)
    }

    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        ChildProcessEngine::cancel(self, id)
    }

    fn remove(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        ChildProcessEngine::remove(self, id)
    }

    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        ChildProcessEngine::set_limits(self, id, limits)
    }

    fn set_global_limits(&mut self, limits: Limits) -> Result<(), ErrorCode> {
        ChildProcessEngine::set_global_limits(self, limits)
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        ChildProcessEngine::set_connections(self, id, connections)
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        ChildProcessEngine::snapshot(self)
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        ChildProcessEngine::subscribe(self)
    }
}

impl Drop for ChildProcessEngine {
    fn drop(&mut self) {
        // Closing stdin asks the child to cancel its transfers and exit; it's killed if it doesn't.
        let deadline = Instant::now() + EXIT_WAIT;
        if self.connection.close(EXIT_WAIT) {
            // Its stdout has ended, so it is exiting now.
            while Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => thread::sleep(Duration::from_millis(1)),
                    Err(_) => break,
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{pipe, PipeReader, PipeWriter};

    /// Opens a connection to a fake engine: `engine` runs on a thread with the app's stdin to read
    /// and the app's stdout to write.
    fn open_fake(
        engine: impl FnOnce(PipeReader, PipeWriter) + Send + 'static,
    ) -> Result<Connection, ErrorCode> {
        let (to_engine, app_stdin) = pipe().unwrap();
        let (app_stdout, from_engine) = pipe().unwrap();
        thread::spawn(move || engine(to_engine, from_engine));
        Connection::open(app_stdin, app_stdout, TransferId(5))
    }

    fn hello(out: &mut PipeWriter) {
        write_message(
            out,
            &FromChild::Hello {
                protocol: PROTOCOL_VERSION,
                contract: TRANSFER_CONTRACT_VERSION,
            },
        )
        .unwrap();
    }

    /// A fake engine's start: reads `start`, then answers the capabilities request.
    fn start(input: &mut PipeReader, out: &mut PipeWriter) -> ToChild {
        let start = read_message::<ToChild>(input).unwrap().unwrap();
        let Some(ToChild::Request { seq, .. }) = read_message::<ToChild>(input).unwrap() else {
            panic!("expected the capabilities request");
        };
        let reply = FromChild::Reply {
            seq,
            result: Ok(TransferReply::Capabilities {
                capabilities: Capabilities {
                    protocols: Vec::new(),
                    live_connection_changes: false,
                    validator_resume: false,
                    metalink: false,
                    max_connections_per_host: 3,
                },
            }),
        };
        write_message(out, &reply).unwrap();
        start
    }

    #[test]
    fn the_app_answers_hello_with_its_versions_and_the_first_transfer_id() {
        let (seen_tx, seen_rx) = channel();
        let connection = open_fake(move |mut input, mut out| {
            hello(&mut out);
            let start = start(&mut input, &mut out);
            seen_tx.send(start).unwrap();
            // Keep the streams open until the app closes stdin.
            let _ = read_message::<ToChild>(&mut input);
        })
        .expect("the handshake succeeds");
        assert_eq!(connection.capabilities.max_connections_per_host, 3);
        assert_eq!(
            seen_rx.recv().unwrap(),
            ToChild::Start {
                protocol: PROTOCOL_VERSION,
                contract: TRANSFER_CONTRACT_VERSION,
                first_transfer_id: TransferId(5),
            }
        );
    }

    #[test]
    fn an_engine_with_other_versions_is_incompatible() {
        for (protocol, contract) in [
            (PROTOCOL_VERSION + 1, TRANSFER_CONTRACT_VERSION),
            (PROTOCOL_VERSION, TRANSFER_CONTRACT_VERSION + 1),
        ] {
            let outcome = open_fake(move |_input, mut out| {
                write_message(&mut out, &FromChild::Hello { protocol, contract }).unwrap();
            });
            assert_eq!(outcome.err(), Some(ErrorCode::EngineIncompatible));
        }
    }

    #[test]
    fn an_engine_speaking_the_prototypes_json_lines_is_incompatible() {
        let outcome = open_fake(|_input, mut out| {
            out.write_all(b"{\"type\":\"ready\",\"protocol\":1}\n")
                .unwrap();
        });
        assert_eq!(outcome.err(), Some(ErrorCode::EngineIncompatible));
    }

    #[test]
    fn an_engine_that_exits_before_hello_failed_to_start() {
        let outcome = open_fake(|_input, out| drop(out));
        assert_eq!(outcome.err(), Some(ErrorCode::EngineStartFailed));
    }

    #[test]
    fn an_engine_that_breaks_the_protocol_is_dropped() {
        // After the handshake, the fake sends an event with a field the app doesn't know.
        let (go_tx, go_rx) = channel::<()>();
        let connection = open_fake(move |mut input, mut out| {
            hello(&mut out);
            start(&mut input, &mut out);
            go_rx.recv().unwrap();
            let body = br#"{"type":"event","event":{"type":"removed","id":1,"spy":true}}"#;
            out.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
            out.write_all(body).unwrap();
            // Stay alive: the app must stop on the message, not on the stream ending.
            let _ = read_message::<ToChild>(&mut input);
        })
        .expect("the handshake succeeds");
        let events = connection.subscribe();
        go_tx.send(()).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected),
            "the event is refused and the subscription ends"
        );
        let asked = Instant::now();
        assert_eq!(
            connection.request(TransferCommand::Snapshot {}),
            Err(ErrorCode::EngineUnavailable)
        );
        assert!(
            asked.elapsed() < COMMAND_TIMEOUT / 2,
            "a request to a dropped engine fails at once, not at the timeout"
        );
    }

    #[test]
    fn an_oversized_frame_from_the_engine_is_dropped_unread() {
        let (go_tx, go_rx) = channel::<()>();
        let connection = open_fake(move |mut input, mut out| {
            hello(&mut out);
            start(&mut input, &mut out);
            go_rx.recv().unwrap();
            out.write_all(&u32::MAX.to_le_bytes()).unwrap();
            let _ = read_message::<ToChild>(&mut input);
        })
        .expect("the handshake succeeds");
        let events = connection.subscribe();
        go_tx.send(()).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn the_engine_gets_one_argument_and_only_system_variables() {
        let command = engine_command(Path::new("velocity-transfer"));
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["serve"]);
        for (name, _) in command.get_envs() {
            assert!(
                KEPT_ENV.iter().any(|kept| name.eq_ignore_ascii_case(kept)),
                "{name:?} is passed to the engine"
            );
        }
    }
}
