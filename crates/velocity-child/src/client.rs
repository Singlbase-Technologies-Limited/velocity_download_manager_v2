//! The app's side of the protocol (#39, #62): starts `<program> serve` as a child process with one
//! argument and a cleared environment, completes the handshake, sends requests and receives
//! replies and events. Nothing secret can reach the child's arguments or environment
//! (`.claude/rules/security.md`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use velocity_core::domain::ErrorCode;

use crate::frame::{read_message, write_message, FrameError};
use crate::lock;
use crate::log::ChildLog;
use crate::message::{check_versions, Contract, FromChild, ToChild, PROTOCOL_VERSION};

/// How long the app waits for the child's `hello`, or for any reply.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How long dropping the client waits for the child to exit before killing it.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// The environment variables a child keeps. They are system settings, never secrets: Windows
/// sockets need `SystemRoot`, and OpenSSL on Linux reads the trust store's location from the two
/// `SSL_CERT_*` variables when they are set. Proxy variables are dropped too (ADR-0008).
pub const KEPT_ENV: &[&str] = if cfg!(windows) {
    &["SystemRoot", "windir"]
} else {
    &["SSL_CERT_FILE", "SSL_CERT_DIR"]
};

/// The command that starts a child: one fixed argument and a cleared environment.
pub fn child_command(program: &Path) -> Command {
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
        // The app has no console; without this flag Windows would open one for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

type ReplySender<C> = Sender<Result<<C as Contract>::Reply, ErrorCode>>;

/// What the reader thread shares with the requesters.
struct Shared<C: Contract> {
    /// The requests waiting for a reply; `None` once the reader has stopped.
    pending: Mutex<Option<HashMap<u64, ReplySender<C>>>>,
    subscribers: Mutex<Vec<Sender<C::Event>>>,
}

impl<C: Contract> Shared<C> {
    /// Stops waiting for the reply to `seq`.
    fn forget(&self, seq: u64) {
        if let Some(pending) = lock(&self.pending).as_mut() {
            pending.remove(&seq);
        }
    }
}

/// The app's side of the protocol over any pair of streams; [`ChildProcess`] runs it over a
/// child's pipes.
pub struct Connection<C: Contract> {
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    shared: Arc<Shared<C>>,
    next_seq: AtomicU64,
    /// Disconnects when the reader stops: the child's stdout ended or broke the protocol.
    closed: Mutex<Receiver<()>>,
}

impl<C: Contract> Connection<C> {
    /// Waits for the child's `hello`, checks its versions and answers `start`.
    pub fn open(
        stdin: impl Write + Send + 'static,
        stdout: impl Read + Send + 'static,
        first_id: u64,
    ) -> Result<Self, ErrorCode> {
        let shared = Arc::new(Shared::<C> {
            pending: Mutex::new(Some(HashMap::new())),
            subscribers: Mutex::new(Vec::new()),
        });
        let (hello_tx, hello_rx) = channel();
        let (closed_tx, closed) = channel::<()>();
        let reader_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name(format!("velocity-{}-reader", C::NAME))
            .spawn(move || {
                read_child(stdout, &reader_shared, hello_tx);
                drop(closed_tx);
            })
            .map_err(|_| ErrorCode::EngineStartFailed)?;

        match hello_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(Ok((protocol, contract))) => check_versions::<C>(protocol, contract)?,
            Ok(Err(code)) => return Err(code),
            Err(_) => return Err(ErrorCode::EngineStartFailed),
        }
        let connection = Connection {
            stdin: Mutex::new(Some(Box::new(stdin))),
            shared,
            next_seq: AtomicU64::new(0),
            closed: Mutex::new(closed),
        };
        let start = ToChild::<C>::Start {
            protocol: PROTOCOL_VERSION,
            contract: C::VERSION,
            first_id,
        };
        connection
            .send(&start)
            .map_err(|_| ErrorCode::EngineStartFailed)?;
        Ok(connection)
    }

    fn send(&self, message: &ToChild<C>) -> Result<(), ErrorCode> {
        let mut stdin = lock(&self.stdin);
        let out = stdin.as_mut().ok_or(ErrorCode::EngineUnavailable)?;
        write_message(out, message).map_err(|_| ErrorCode::EngineUnavailable)
    }

    /// Sends `command` and waits up to [`COMMAND_TIMEOUT`] for its reply.
    pub fn request(&self, command: C::Command) -> Result<C::Reply, ErrorCode> {
        self.request_within(command, COMMAND_TIMEOUT)
    }

    /// [`Self::request`], waiting at most `timeout` for the reply.
    pub fn request_within(
        &self,
        command: C::Command,
        timeout: Duration,
    ) -> Result<C::Reply, ErrorCode> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = channel();
        match lock(&self.shared.pending).as_mut() {
            Some(pending) => pending.insert(seq, tx),
            // The child is already gone: fail now rather than at the timeout.
            None => return Err(ErrorCode::EngineUnavailable),
        };
        if let Err(code) = self.send(&ToChild::Request { seq, command }) {
            self.shared.forget(seq);
            return Err(code);
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.shared.forget(seq);
                Err(ErrorCode::EngineUnavailable)
            }
            // The reader dropped every pending reply: the child is gone.
            Err(RecvTimeoutError::Disconnected) => Err(ErrorCode::EngineUnavailable),
        }
    }

    /// A receiver for the child's events. It disconnects when the child's stdout ends, which is
    /// how a supervisor notices a crash.
    pub fn subscribe(&self) -> Receiver<C::Event> {
        let (tx, rx) = channel();
        lock(&self.shared.subscribers).push(tx);
        rx
    }

    /// Closes the child's stdin, so requests fail as unavailable from now on.
    pub fn stop(&self) {
        lock(&self.stdin).take();
    }

    /// Closes the child's stdin, which asks it to shut down and exit, and waits up to `wait` for
    /// its stdout to end. Returns whether it ended.
    fn close(&self, wait: Duration) -> bool {
        self.stop();
        matches!(
            lock(&self.closed).recv_timeout(wait),
            Err(RecvTimeoutError::Disconnected)
        )
    }
}

/// The app's reader: the first message must be the child's `hello`; after it, replies go to their
/// requests and events to subscribers. A frame error, a second `hello` or the end of the stream
/// stops it, and every waiting request then fails as unavailable.
fn read_child<C: Contract>(
    mut stdout: impl Read,
    shared: &Shared<C>,
    hello: Sender<Result<(u32, u32), ErrorCode>>,
) {
    let first = match read_message::<FromChild<C>>(&mut stdout) {
        Ok(Some(FromChild::Hello { protocol, contract })) => Ok((protocol, contract)),
        // A child that exits before saying hello failed to start.
        Ok(None) | Err(FrameError::Truncated) | Err(FrameError::Io(_)) => {
            Err(ErrorCode::EngineStartFailed)
        }
        // Anything else is a child that speaks another protocol.
        Ok(Some(_)) | Err(FrameError::TooLarge { .. }) | Err(FrameError::Malformed(_)) => {
            Err(ErrorCode::EngineIncompatible)
        }
    };
    let started = first.is_ok();
    let _ = hello.send(first);
    if started {
        while let Ok(Some(message)) = read_message::<FromChild<C>>(&mut stdout) {
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

/// How to start a child.
#[derive(Clone)]
pub struct SpawnOptions {
    /// The first id the child assigns. A supervisor raises it after a restart, so a new child
    /// never reuses an id an earlier one assigned.
    pub first_id: u64,
    /// Where the child's stderr goes.
    pub log: ChildLog,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        SpawnOptions {
            first_id: 1,
            log: ChildLog::new(),
        }
    }
}

/// A child process speaking `C`. Its methods take `&self`, so a supervisor can share one child
/// between threads.
pub struct ChildProcess<C: Contract> {
    child: Child,
    connection: Connection<C>,
}

impl<C: Contract> ChildProcess<C> {
    /// Starts `<program> serve` and completes the handshake.
    pub fn spawn(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode> {
        let mut child = child_command(program).spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::EngineMissing,
            _ => ErrorCode::EngineStartFailed,
        })?;
        let opened = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
            (Some(stdin), Some(stdout), Some(stderr)) => options
                .log
                .capture(C::NAME, stderr)
                .map_err(|_| ErrorCode::EngineStartFailed)
                .and_then(|()| Connection::open(stdin, stdout, options.first_id)),
            _ => Err(ErrorCode::EngineStartFailed),
        };
        match opened {
            Ok(connection) => Ok(ChildProcess { child, connection }),
            Err(code) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(code)
            }
        }
    }

    /// Asks the child to stop by closing its stdin: it shuts down and exits. Commands fail with
    /// `EngineUnavailable` from then on, even while another thread still holds the child.
    pub fn stop(&self) {
        self.connection.stop();
    }

    /// Whether the child answers [`Contract::health_check`] within `timeout`: the supervisor's
    /// check for a child that hangs without exiting (#48). An error reply is still an answer.
    pub fn answers_within(&self, timeout: Duration) -> bool {
        !matches!(
            self.connection.request_within(C::health_check(), timeout),
            Err(ErrorCode::EngineUnavailable)
        )
    }

    /// The child's operating-system process id, for diagnostics and measurements.
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub fn request(&self, command: C::Command) -> Result<C::Reply, ErrorCode> {
        self.connection.request(command)
    }

    pub fn subscribe(&self) -> Receiver<C::Event> {
        self.connection.subscribe()
    }
}

impl<C: Contract> Drop for ChildProcess<C> {
    fn drop(&mut self) {
        // Closing stdin asks the child to shut down and exit; it's killed if it doesn't.
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
    use crate::message::tests::{Echo, EchoCommand, EchoEvent, EchoReply};
    use std::io::{pipe, PipeReader, PipeWriter};

    /// Opens a connection to a fake child: `child` runs on a thread with the app's stdin to read
    /// and the app's stdout to write.
    fn open_fake(
        child: impl FnOnce(PipeReader, PipeWriter) + Send + 'static,
    ) -> Result<Connection<Echo>, ErrorCode> {
        let (to_child, app_stdin) = pipe().unwrap();
        let (app_stdout, from_child) = pipe().unwrap();
        thread::spawn(move || child(to_child, from_child));
        Connection::open(app_stdin, app_stdout, 5)
    }

    fn hello(out: &mut PipeWriter) {
        write_message(
            out,
            &FromChild::<Echo>::Hello {
                protocol: PROTOCOL_VERSION,
                contract: Echo::VERSION,
            },
        )
        .unwrap();
    }

    #[test]
    fn the_app_answers_hello_with_its_versions_and_the_first_id() {
        let (seen_tx, seen_rx) = channel();
        let _connection = open_fake(move |mut input, mut out| {
            hello(&mut out);
            seen_tx
                .send(read_message::<ToChild<Echo>>(&mut input).unwrap().unwrap())
                .unwrap();
            // Keep the streams open until the app closes stdin.
            let _ = read_message::<ToChild<Echo>>(&mut input);
        })
        .expect("the handshake succeeds");
        assert_eq!(
            seen_rx.recv().unwrap(),
            ToChild::Start {
                protocol: PROTOCOL_VERSION,
                contract: Echo::VERSION,
                first_id: 5,
            }
        );
    }

    #[test]
    fn replies_reach_their_requests_and_events_their_subscribers() {
        let connection = open_fake(|mut input, mut out| {
            hello(&mut out);
            let _start = read_message::<ToChild<Echo>>(&mut input).unwrap();
            while let Ok(Some(ToChild::Request { seq, command })) =
                read_message::<ToChild<Echo>>(&mut input)
            {
                let EchoCommand::Say { text } = command else {
                    continue;
                };
                let event = FromChild::<Echo>::Event {
                    event: EchoEvent::Heard { text: text.clone() },
                };
                write_message(&mut out, &event).unwrap();
                let reply = FromChild::<Echo>::Reply {
                    seq,
                    result: Ok(EchoReply::Said { text, first_id: 5 }),
                };
                write_message(&mut out, &reply).unwrap();
            }
        })
        .unwrap();
        let events = connection.subscribe();
        assert_eq!(
            connection.request(EchoCommand::Say { text: "hi".into() }),
            Ok(EchoReply::Said {
                text: "hi".into(),
                first_id: 5
            })
        );
        assert_eq!(
            events.recv_timeout(Duration::from_secs(10)),
            Ok(EchoEvent::Heard { text: "hi".into() })
        );
    }

    #[test]
    fn a_child_with_other_versions_is_incompatible() {
        for (protocol, contract) in [
            (PROTOCOL_VERSION + 1, Echo::VERSION),
            (PROTOCOL_VERSION, Echo::VERSION + 1),
        ] {
            let outcome = open_fake(move |_input, mut out| {
                write_message(&mut out, &FromChild::<Echo>::Hello { protocol, contract }).unwrap();
            });
            assert_eq!(outcome.err(), Some(ErrorCode::EngineIncompatible));
        }
    }

    #[test]
    fn a_child_speaking_the_prototypes_json_lines_is_incompatible() {
        let outcome = open_fake(|_input, mut out| {
            out.write_all(b"{\"type\":\"ready\",\"protocol\":1}\n")
                .unwrap();
        });
        assert_eq!(outcome.err(), Some(ErrorCode::EngineIncompatible));
    }

    #[test]
    fn a_child_that_exits_before_hello_failed_to_start() {
        let outcome = open_fake(|_input, out| drop(out));
        assert_eq!(outcome.err(), Some(ErrorCode::EngineStartFailed));
    }

    #[test]
    fn a_child_that_breaks_the_protocol_is_dropped() {
        // After the handshake, the fake sends an event with a field the app doesn't know.
        let (go_tx, go_rx) = channel::<()>();
        let connection = open_fake(move |mut input, mut out| {
            hello(&mut out);
            let _start = read_message::<ToChild<Echo>>(&mut input);
            go_rx.recv().unwrap();
            let body = br#"{"type":"event","event":{"type":"heard","text":"x","spy":true}}"#;
            out.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
            out.write_all(body).unwrap();
            // Stay alive: the app must stop on the message, not on the stream ending.
            let _ = read_message::<ToChild<Echo>>(&mut input);
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
            connection.request(EchoCommand::Ping {}),
            Err(ErrorCode::EngineUnavailable)
        );
        assert!(
            asked.elapsed() < COMMAND_TIMEOUT / 2,
            "a request to a dropped child fails at once, not at the timeout"
        );
    }

    #[test]
    fn an_oversized_frame_from_the_child_is_dropped_unread() {
        let (go_tx, go_rx) = channel::<()>();
        let connection = open_fake(move |mut input, mut out| {
            hello(&mut out);
            let _start = read_message::<ToChild<Echo>>(&mut input);
            go_rx.recv().unwrap();
            out.write_all(&u32::MAX.to_le_bytes()).unwrap();
            let _ = read_message::<ToChild<Echo>>(&mut input);
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
    fn the_child_gets_one_argument_and_only_system_variables() {
        let command = child_command(Path::new("velocity-transfer"));
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["serve"]);
        for (name, _) in command.get_envs() {
            assert!(
                KEPT_ENV.iter().any(|kept| name.eq_ignore_ascii_case(kept)),
                "{name:?} is passed to the child"
            );
        }
    }
}
