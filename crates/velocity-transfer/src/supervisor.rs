//! The engine supervisor (#39, ADR-0012): runs the engine as a child process behind the
//! `DirectTransfer` contract and keeps it running. The restart loop, the hang checks and the cap
//! are `velocity_child`'s supervisor, shared with the torrent helper since #62; this module is
//! what it keeps for the engine ([`TransferBook`]).
//!
//! - **Crashes:** when the engine's stdout ends, the supervisor reports every transfer that was
//!   running as Failed with a retryable error, and starts a new engine after a backoff. The part
//!   files stay on disk for checkpoints to resume from (#43).
//! - **Hangs** (#48): an engine that stops answering, without exiting, is checked every few
//!   seconds; after a few checks in a row go unanswered, it is killed and handled like a crash.
//!   A worker waiting on a slow disk doesn't hold up the answers: the engine answers commands on
//!   its own thread, and its workers write without holding the engine's state.
//! - **The cap:** more than [`SupervisorOptions::max_restarts`] crashes within the restart window
//!   leave the engine unavailable until [`SupervisedEngine::restart`] is called, the "Restart
//!   transfer engine" action. An engine that is missing or doesn't match the app is unavailable
//!   at once, since restarting it can't help.
//! - **Limits:** the global speed limit last set is given to every new engine (#49).
//! - **Ids:** a new engine starts its ids after the highest one an earlier engine assigned, so an
//!   id never names two transfers. The supervisor answers for the transfers an earlier engine had
//!   until they are removed.
//!
//! No lock is held while talking to the engine: a command takes its own reference to the running
//! engine and releases the state first.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use velocity_child::ledger::Ledger;
use velocity_child::supervisor::{Book, Retired, Supervisor};
pub use velocity_child::supervisor::{EngineHealth, SupervisorOptions};
use velocity_core::domain::{Capabilities, DownloadStatus, ErrorCode};
use velocity_core::transfer::{
    DirectTransfer, EngineLimits, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};

use crate::stdio::ChildProcessEngine;

/// What a lost transfer fails with: after a crash, after a hang, and after a restart by hand. The
/// core defines them, so the manager knows a lost transfer from a failed one (#58).
use velocity_core::transfer::{
    ENGINE_HUNG as HUNG_MESSAGE, ENGINE_RESTARTED as RESTART_MESSAGE,
    ENGINE_STOPPED as CRASH_MESSAGE,
};

/// What the supervisor keeps for the engine, across engines.
pub struct TransferBook {
    capabilities: Capabilities,
    /// The transfers of the running engine, and the finished ones of earlier engines.
    transfers: Ledger<TransferSnapshot>,
    /// The global speed limit last set, given to every new engine (#49).
    global_limits: Limits,
    /// The engine limits last set, given to every new engine (#57).
    engine_limits: EngineLimits,
}

impl TransferBook {
    fn new() -> Self {
        TransferBook {
            capabilities: Capabilities {
                protocols: Vec::new(),
                live_connection_changes: false,
                metalink: false,
                max_connections_per_host: 1,
            },
            transfers: Ledger::default(),
            global_limits: Limits::default(),
            engine_limits: EngineLimits::DEFAULT,
        }
    }

    /// Records a transfer an engine assigned, if no event has yet; `current` says whether that
    /// engine still runs. Returns the events to pass on.
    fn added(&mut self, current: bool, id: TransferId) -> Vec<TransferEvent> {
        let queued = TransferSnapshot {
            id,
            status: DownloadStatus::Queued,
            progress: velocity_core::domain::Progress {
                completed_bytes: 0,
                total_bytes: None,
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: None,
            },
            wasted_bytes: 0,
            error: None,
            verification: None,
            file_name: None,
            content_type: None,
        };
        // When the engine stopped between assigning the id and this reply, the transfer is lost.
        self.transfers
            .added(current, queued, CRASH_MESSAGE)
            .map(|snapshot| TransferEvent::Changed { snapshot })
            .into_iter()
            .collect()
    }
}

impl Book<ChildProcessEngine> for TransferBook {
    type Setup = (Limits, EngineLimits);

    fn setup(&self) -> (Limits, EngineLimits) {
        (self.global_limits, self.engine_limits)
    }

    fn prepare(
        engine: &ChildProcessEngine,
        (global, limits): (Limits, EngineLimits),
    ) -> Result<(), ErrorCode> {
        // A new engine starts unlimited: it gets the global limit its predecessor had.
        if global != Limits::default() {
            engine.set_global_limits(global)?;
        }
        // And the profile's engine limits (#57).
        if limits != EngineLimits::DEFAULT {
            engine.set_engine_limits(limits)?;
        }
        Ok(())
    }

    fn highest_id(&self) -> u64 {
        self.transfers.highest_id()
    }

    fn installed(&mut self, engine: &ChildProcessEngine) {
        self.capabilities = engine.capabilities();
        self.transfers.installed();
    }

    fn event(&mut self, event: TransferEvent) -> Vec<TransferEvent> {
        match &event {
            TransferEvent::Changed { snapshot } => self.transfers.changed(snapshot),
            TransferEvent::Removed { id } => self.transfers.removed(*id),
            // Passed on for the app to save (#43); it changes no state here.
            TransferEvent::Checkpoint { .. } => {}
        }
        vec![event]
    }

    /// Reports the running engine's unfinished transfers as failed and keeps them as orphans.
    fn retired(&mut self, why: Retired) -> Vec<TransferEvent> {
        let message = match why {
            Retired::Crashed => CRASH_MESSAGE,
            Retired::Hung => HUNG_MESSAGE,
            Retired::Restarted => RESTART_MESSAGE,
        };
        self.transfers
            .retired(message)
            .into_iter()
            .map(|snapshot| TransferEvent::Changed { snapshot })
            .collect()
    }
}

/// The direct-download engine as a supervised child process.
pub struct SupervisedEngine {
    supervisor: Supervisor<ChildProcessEngine, TransferBook>,
}

impl SupervisedEngine {
    /// Starts `<program> serve` and supervises it. Fails if the first engine doesn't start.
    pub fn start(program: PathBuf, options: SupervisorOptions) -> Result<Self, ErrorCode> {
        Ok(SupervisedEngine {
            supervisor: Supervisor::start(program, options, TransferBook::new())?,
        })
    }

    pub fn health(&self) -> EngineHealth {
        self.supervisor.health()
    }

    /// A receiver for every later change of [`EngineHealth`].
    pub fn subscribe_health(&self) -> Receiver<EngineHealth> {
        self.supervisor.subscribe_health()
    }

    /// Stops the engine, if one runs, and starts a new one, clearing the crash count. Transfers
    /// that were running fail with a retryable error. [`EngineHealth`] reports the outcome.
    pub fn restart(&self) {
        self.supervisor.restart();
    }

    /// The running engine's process id, or `None` while none runs.
    pub fn process_id(&self) -> Option<u32> {
        self.supervisor.process_id()
    }

    /// The engine's last stderr lines, with URL logins hidden.
    pub fn log(&self) -> Vec<String> {
        self.supervisor.log()
    }

    fn engine(&self) -> Result<Arc<ChildProcessEngine>, ErrorCode> {
        self.supervisor.child().map(|(engine, _)| engine)
    }

    /// Whether an earlier engine had `id`. Such a transfer is finished, so only `remove` and
    /// `snapshot` apply to it.
    fn orphaned(&self, id: TransferId) -> bool {
        self.supervisor
            .update(|book, _| (book.transfers.is_orphan(id), Vec::new()))
    }

    fn running(&self, id: TransferId) -> Result<Arc<ChildProcessEngine>, ErrorCode> {
        if self.orphaned(id) {
            return Err(ErrorCode::InvalidState);
        }
        self.engine()
    }
}

impl DirectTransfer for SupervisedEngine {
    fn capabilities(&self) -> Capabilities {
        self.supervisor
            .update(|book, _| (book.capabilities.clone(), Vec::new()))
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        let (engine, generation) = self.supervisor.child()?;
        let id = engine.add(spec)?;
        self.supervisor
            .update(|book, current| ((), book.added(current == generation, id)));
        Ok(id)
    }

    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.running(id)?.pause(id)
    }

    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.running(id)?.resume(id)
    }

    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        self.running(id)?.cancel(id)
    }

    fn remove(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let orphan = self.supervisor.update(|book, _| {
            if book.transfers.remove_orphan(id) {
                (true, vec![TransferEvent::Removed { id }])
            } else {
                (false, Vec::new())
            }
        });
        if orphan {
            return Ok(());
        }
        self.engine()?.remove(id)
    }

    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        self.running(id)?.set_limits(id, limits)
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        self.running(id)?.set_connections(id, connections)
    }

    fn trust_host_key(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        self.engine()?.trust_host_key(host, port, fingerprint)
    }

    fn set_global_limits(&mut self, limits: Limits) -> Result<(), ErrorCode> {
        self.engine()?.set_global_limits(limits)?;
        self.supervisor.update(|book, _| {
            book.global_limits = limits;
            ((), Vec::new())
        });
        Ok(())
    }

    fn set_engine_limits(&mut self, limits: EngineLimits) -> Result<(), ErrorCode> {
        let engine = self.engine()?;
        engine.set_engine_limits(limits)?;
        // The capabilities were read when the engine started; the engine's report now has the new
        // cap.
        let capabilities = engine.capabilities();
        self.supervisor.update(|book, _| {
            book.engine_limits = limits;
            book.capabilities = capabilities;
            ((), Vec::new())
        });
        Ok(())
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        let mut transfers = self.engine()?.snapshot()?;
        let orphans: Vec<TransferSnapshot> = self
            .supervisor
            .update(|book, _| (book.transfers.orphans(), Vec::new()));
        transfers.extend(orphans);
        transfers.sort_by_key(|s| s.id);
        Ok(transfers)
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        self.supervisor.subscribe()
    }
}
