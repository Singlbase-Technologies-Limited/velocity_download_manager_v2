//! The engine as a child process (#37, #38): the same [`InProcessEngine`] behind the
//! `DirectTransfer` contract, reached over the child's stdin and stdout.
//!
//! The framing is JSON lines, one message per line, using the contract's wire forms
//! ([`TransferCommand`], [`TransferReply`], [`TransferEvent`]):
//!
//! - the child first writes `{"type":"ready","protocol":1}`;
//! - the parent writes `{"type":"request","seq":N,"command":{…}}`;
//! - the child answers each with `{"type":"reply","seq":N,"result":{"Ok":{…}}}` or
//!   `{"result":{"Err":"NOT_FOUND"}}`, and pushes `{"type":"event","event":{…}}` at any time.
//!
//! When the parent closes stdin, or goes away, the child cancels every transfer, waits briefly for
//! the part files to be removed, and exits. Nothing passes through process arguments or the
//! environment.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use velocity_core::domain::{Capabilities, ErrorCode};
use velocity_core::transfer::{
    dispatch, DirectTransfer, Limits, TransferCommand, TransferEvent, TransferId, TransferReply,
    TransferSnapshot, TransferSpec,
};

use crate::engine::InProcessEngine;

/// The framing's version. A child that announces another one is refused.
pub const PROTOCOL: u32 = 1;
/// How long the parent waits for the child's `ready`, or for any reply.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a child that lost its parent waits for cancelled transfers to clean up.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
/// How long dropping the client waits for the child to exit before killing it.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// A message from the parent to the child.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ToChild {
    Request { seq: u64, command: TransferCommand },
}

/// A message from the child to the parent.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum FromChild {
    Ready {
        protocol: u32,
    },
    Reply {
        seq: u64,
        result: Result<TransferReply, ErrorCode>,
    },
    Event {
        event: TransferEvent,
    },
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Writes one message as one line. The lock only keeps lines whole; no engine state is behind it.
fn write_line(out: &Mutex<impl Write>, message: &impl Serialize) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let mut out = lock(out);
    out.write_all(&line)?;
    out.flush()
}

/// Runs the child's side: serves `engine` on stdin and stdout until stdin closes.
pub fn serve() -> Result<(), String> {
    let mut engine = InProcessEngine::start()?;
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let events = engine.subscribe();
    let event_out = Arc::clone(&out);
    thread::spawn(move || {
        for event in events {
            if write_line(&event_out, &FromChild::Event { event }).is_err() {
                break;
            }
        }
    });
    write_line(&out, &FromChild::Ready { protocol: PROTOCOL }).map_err(|e| e.to_string())?;

    let mut outcome = Ok(());
    for line in std::io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                outcome = Err(format!("reading stdin: {e}"));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        // The parent is the app, but its messages still cross a process boundary: a line that
        // isn't a request ends the session instead of being guessed at.
        let request: ToChild = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                outcome = Err(format!("a malformed request: {e}"));
                break;
            }
        };
        let ToChild::Request { seq, command } = request;
        let result = dispatch(&mut engine, command);
        if write_line(&out, &FromChild::Reply { seq, result }).is_err() {
            break;
        }
    }

    engine.cancel_all();
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    while !engine.idle() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    outcome
}

struct Client {
    pending: Mutex<HashMap<u64, Sender<Result<TransferReply, ErrorCode>>>>,
    subscribers: Mutex<Vec<Sender<TransferEvent>>>,
}

/// A `DirectTransfer` client for an engine running as a child process.
pub struct ChildProcessEngine {
    child: Child,
    stdin: Option<Mutex<ChildStdin>>,
    client: Arc<Client>,
    next_seq: AtomicU64,
    capabilities: Capabilities,
}

impl ChildProcessEngine {
    /// Starts `<program> serve`, waits for its `ready`, and asks for its capabilities.
    pub fn spawn(program: &Path) -> Result<Self, ErrorCode> {
        let mut child = Command::new(program)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ErrorCode::EngineMissing,
                _ => ErrorCode::EngineStartFailed,
            })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(ErrorCode::EngineStartFailed);
        };
        let client = Arc::new(Client {
            pending: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
        });
        let (ready_tx, ready_rx) = channel();
        let reader_client = Arc::clone(&client);
        thread::spawn(move || read_child(BufReader::new(stdout), &reader_client, ready_tx));

        let mut engine = ChildProcessEngine {
            child,
            stdin: Some(Mutex::new(stdin)),
            client,
            next_seq: AtomicU64::new(0),
            capabilities: Capabilities {
                protocols: Vec::new(),
                live_connection_changes: false,
                validator_resume: false,
                metalink: false,
                max_connections_per_host: 1,
            },
        };
        match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(PROTOCOL) => {}
            Ok(_) => return Err(ErrorCode::EngineIncompatible),
            Err(_) => return Err(ErrorCode::EngineStartFailed),
        }
        match engine.request(TransferCommand::Capabilities)? {
            TransferReply::Capabilities { capabilities } => engine.capabilities = capabilities,
            _ => return Err(ErrorCode::EngineIncompatible),
        }
        Ok(engine)
    }

    /// The child's operating-system process id, for supervision (#39) and measurements (#38).
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    fn request(&self, command: TransferCommand) -> Result<TransferReply, ErrorCode> {
        let stdin = self.stdin.as_ref().ok_or(ErrorCode::EngineUnavailable)?;
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = channel();
        lock(&self.client.pending).insert(seq, tx);
        if write_line(stdin, &ToChild::Request { seq, command }).is_err() {
            lock(&self.client.pending).remove(&seq);
            return Err(ErrorCode::EngineUnavailable);
        }
        match rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.client.pending).remove(&seq);
                Err(ErrorCode::EngineUnavailable)
            }
            // The reader dropped every pending reply: the child is gone.
            Err(RecvTimeoutError::Disconnected) => Err(ErrorCode::EngineUnavailable),
        }
    }

    fn done(&self, command: TransferCommand) -> Result<(), ErrorCode> {
        match self.request(command)? {
            TransferReply::Done => Ok(()),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }
}

/// The parent's reader: routes replies to their requests and events to subscribers. When the
/// child's stdout ends or breaks the framing, every waiting request fails as unavailable.
fn read_child(stdout: impl BufRead, client: &Client, ready: Sender<u32>) {
    for line in stdout.lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<FromChild>(&line) else {
            break;
        };
        match message {
            FromChild::Ready { protocol } => {
                let _ = ready.send(protocol);
            }
            FromChild::Reply { seq, result } => {
                if let Some(tx) = lock(&client.pending).remove(&seq) {
                    // The requester may have timed out and gone; that's not an error here.
                    let _ = tx.send(result);
                }
            }
            FromChild::Event { event } => {
                lock(&client.subscribers).retain(|s| s.send(event.clone()).is_ok());
            }
        }
    }
    lock(&client.pending).clear();
    lock(&client.subscribers).clear();
}

impl DirectTransfer for ChildProcessEngine {
    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        match self.request(TransferCommand::Add { spec })? {
            TransferReply::Added { id } => Ok(id),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Pause { id })
    }

    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Resume { id })
    }

    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Cancel { id })
    }

    fn remove(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.done(TransferCommand::Remove { id })
    }

    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        self.done(TransferCommand::SetLimits { id, limits })
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        match self.request(TransferCommand::SetConnections { id, connections })? {
            TransferReply::Connections { connections } => Ok(connections),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        match self.request(TransferCommand::Snapshot)? {
            TransferReply::Snapshot { transfers } => Ok(transfers),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        lock(&self.client.subscribers).push(tx);
        rx
    }
}

impl Drop for ChildProcessEngine {
    fn drop(&mut self) {
        // Closing stdin asks the child to cancel its transfers and exit; it's killed if it doesn't.
        self.stdin = None;
        let deadline = Instant::now() + EXIT_WAIT;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_framing_uses_the_contracts_wire_forms() {
        let request = ToChild::Request {
            seq: 7,
            command: TransferCommand::Pause { id: TransferId(3) },
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"type":"request","seq":7,"command":{"command":"pause","id":3}}"#
        );
        let refused = FromChild::Reply {
            seq: 7,
            result: Err(ErrorCode::NotFound),
        };
        let json = serde_json::to_string(&refused).unwrap();
        assert_eq!(
            json,
            r#"{"type":"reply","seq":7,"result":{"Err":"NOT_FOUND"}}"#
        );
        assert!(matches!(
            serde_json::from_str::<FromChild>(&json).unwrap(),
            FromChild::Reply {
                seq: 7,
                result: Err(ErrorCode::NotFound)
            }
        ));
        assert_eq!(
            serde_json::to_string(&FromChild::Ready { protocol: PROTOCOL }).unwrap(),
            r#"{"type":"ready","protocol":1}"#
        );
    }
}
