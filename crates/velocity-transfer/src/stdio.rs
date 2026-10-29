//! The engine as a child process (ADR-0012): the same [`InProcessEngine`] behind the
//! `DirectTransfer` contract, reached over the child's stdin and stdout with the framed, versioned
//! protocol `velocity_child` shares with the torrent helper (#39, #62, docs/ENGINE_PROTOCOL.md).
//!
//! - the child writes `hello` with its protocol and contract versions;
//! - the app checks them and answers `start` with its own, and the first transfer id to assign;
//! - the app then sends `request`s, each answered by one `reply`, and the child pushes `event`s.
//!
//! When the app closes stdin, or goes away, the child pauses every transfer, keeping its part file
//! for a resume (#43), waits briefly for the workers to stop, and exits. The child gets one
//! argument, `serve`, and a cleared environment: nothing secret can reach its arguments or
//! environment (`.claude/rules/security.md`).

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use velocity_child::client::ChildProcess;
pub use velocity_child::client::SpawnOptions;
use velocity_child::serve::Server;
use velocity_core::domain::{Capabilities, ErrorCode};
use velocity_core::transfer::{
    dispatch, DirectTransfer, EngineLimits, EngineSettings, Limits, TransferCommand, TransferEvent,
    TransferId, TransferReply, TransferSnapshot, TransferSpec,
};

use crate::engine::InProcessEngine;
use crate::protocol::TransferContract;

/// How long a child that lost its parent waits for paused transfers to stop.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The engine behind the protocol, in the child.
struct EngineServer(InProcessEngine);

impl Server<TransferContract> for EngineServer {
    fn handle(&mut self, command: TransferCommand) -> Result<TransferReply, ErrorCode> {
        dispatch(&mut self.0, command)
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        self.0.subscribe()
    }

    fn shut_down(&mut self) {
        // Paused, not cancelled: the part files stay for the app to resume them from (#43).
        self.0.pause_all();
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !self.0.idle() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Runs the child's side: serves the engine on stdin and stdout until stdin closes.
pub fn serve() -> Result<(), String> {
    velocity_child::serve::serve::<TransferContract, _>(
        std::io::stdin().lock(),
        std::io::stdout(),
        |first| InProcessEngine::start_from(TransferId(first)).map(EngineServer),
    )
}

/// A `DirectTransfer` client for an engine running as a child process. Its methods take `&self`
/// as well, so a supervisor can share one engine between threads.
pub struct ChildProcessEngine {
    process: ChildProcess<TransferContract>,
    /// What the engine reported at the start, and again after its limits changed (#57).
    capabilities: Mutex<Capabilities>,
}

impl ChildProcessEngine {
    /// Starts `<program> serve` with the default options.
    pub fn spawn(program: &Path) -> Result<Self, ErrorCode> {
        Self::spawn_with(program, &SpawnOptions::default())
    }

    /// Starts `<program> serve`, completes the handshake and asks for its capabilities. On a
    /// failure the child is dropped, which stops it.
    pub fn spawn_with(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode> {
        let engine = ChildProcessEngine {
            process: ChildProcess::spawn(program, options)?,
            capabilities: Mutex::new(Capabilities {
                protocols: Vec::new(),
                live_connection_changes: false,
                metalink: false,
                max_connections_per_host: 1,
                libcurl: None,
            }),
        };
        engine.refresh_capabilities()?;
        Ok(engine)
    }

    /// Asks the engine for its capabilities and keeps them.
    fn refresh_capabilities(&self) -> Result<(), ErrorCode> {
        match self.process.request(TransferCommand::Capabilities {})? {
            TransferReply::Capabilities { capabilities } => {
                *lock(&self.capabilities) = capabilities;
                Ok(())
            }
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    /// Asks the engine to stop by closing its stdin: it pauses its transfers and exits. Commands
    /// fail with `EngineUnavailable` from then on, even while another thread still holds the
    /// engine.
    pub fn stop(&self) {
        self.process.stop();
    }

    /// Whether the engine answers a request for its state within `timeout`: the supervisor's
    /// check for an engine that hangs without exiting (#48). An error reply is still an answer.
    pub fn answers_within(&self, timeout: Duration) -> bool {
        self.process.answers_within(timeout)
    }

    /// The child's operating-system process id, for diagnostics and measurements.
    pub fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    pub fn capabilities(&self) -> Capabilities {
        lock(&self.capabilities).clone()
    }

    pub fn add(&self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        match self.process.request(TransferCommand::Add {
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

    /// Sets the engine's limits, then reads its capabilities again: the per-host cap they report
    /// is the new one.
    pub fn set_engine_limits(&self, limits: EngineLimits) -> Result<(), ErrorCode> {
        self.done(TransferCommand::SetEngineLimits { limits })?;
        self.refresh_capabilities()
    }

    /// Sets how the engine retries, times out, shares connections and writes (#30).
    pub fn set_engine_settings(&self, settings: EngineSettings) -> Result<(), ErrorCode> {
        self.done(TransferCommand::SetEngineSettings { settings })
    }

    pub fn set_connections(&self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        match self
            .process
            .request(TransferCommand::SetConnections { id, connections })?
        {
            TransferReply::Connections { connections } => Ok(connections),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    pub fn trust_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        self.done(TransferCommand::TrustHostKey {
            host: host.to_owned(),
            port,
            fingerprint: fingerprint.to_owned(),
        })
    }

    pub fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        match self.process.request(TransferCommand::Snapshot {})? {
            TransferReply::Snapshot { transfers } => Ok(transfers),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    /// A receiver for the engine's events. It disconnects when the engine's stdout ends, which is
    /// how a supervisor notices a crash.
    pub fn subscribe(&self) -> Receiver<TransferEvent> {
        self.process.subscribe()
    }

    fn done(&self, command: TransferCommand) -> Result<(), ErrorCode> {
        match self.process.request(command)? {
            TransferReply::Done {} => Ok(()),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }
}

impl velocity_child::supervisor::Child for ChildProcessEngine {
    type Event = TransferEvent;
    const NAME: &'static str = "engine";

    fn spawn(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode> {
        ChildProcessEngine::spawn_with(program, options)
    }

    fn subscribe(&self) -> Receiver<TransferEvent> {
        ChildProcessEngine::subscribe(self)
    }

    fn answers_within(&self, timeout: Duration) -> bool {
        ChildProcessEngine::answers_within(self, timeout)
    }

    fn stop(&self) {
        ChildProcessEngine::stop(self)
    }

    fn process_id(&self) -> u32 {
        ChildProcessEngine::process_id(self)
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

    fn set_engine_limits(&mut self, limits: EngineLimits) -> Result<(), ErrorCode> {
        ChildProcessEngine::set_engine_limits(self, limits)
    }

    fn set_engine_settings(&mut self, settings: EngineSettings) -> Result<(), ErrorCode> {
        ChildProcessEngine::set_engine_settings(self, settings)
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        ChildProcessEngine::set_connections(self, id, connections)
    }

    fn trust_host_key(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        ChildProcessEngine::trust_host_key(self, host, port, fingerprint)
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        ChildProcessEngine::snapshot(self)
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        ChildProcessEngine::subscribe(self)
    }
}
