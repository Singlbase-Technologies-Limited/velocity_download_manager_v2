//! The engine supervisor (#39, ADR-0012): runs the engine as a child process behind the
//! `DirectTransfer` contract and keeps it running.
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
//! No lock is held while talking to the engine: a command takes its own reference to the
//! running engine and releases the state first.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use velocity_core::domain::{Capabilities, DownloadStatus, ErrorCode, FailureClass, TransferError};
use velocity_core::transfer::{
    DirectTransfer, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};

use crate::engine_log::EngineLog;
use crate::stdio::{ChildProcessEngine, SpawnOptions};

/// What a transfer that a crash lost fails with.
const CRASH_MESSAGE: &str = "the transfer engine stopped unexpectedly";
/// What a transfer fails with when its engine stopped responding and was killed.
const HUNG_MESSAGE: &str = "the transfer engine stopped responding and was restarted";
/// How often a watchdog checks whether its engine was replaced.
const WATCH_STEP: Duration = Duration::from_millis(50);
/// What a transfer fails with when the engine is restarted by hand.
const RESTART_MESSAGE: &str = "the transfer engine was restarted";

/// How the supervisor restarts the engine. The defaults are V1's: at most 5 restarts within 5
/// minutes, waiting 1, 2, 4, 8 and 16 seconds before each (up to 32). An engine that stops
/// answering is restarted too (#48): it is asked for its state every `check_every`, and after
/// `missed_checks` checks in a row without an answer within `check_timeout`, it is killed. The
/// check values are starting values (PO decision on #48).
#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    pub max_restarts: usize,
    pub restart_window: Duration,
    pub first_backoff: Duration,
    pub max_backoff: Duration,
    pub check_every: Duration,
    pub check_timeout: Duration,
    pub missed_checks: u32,
}

impl Default for SupervisorOptions {
    fn default() -> Self {
        SupervisorOptions {
            max_restarts: 5,
            restart_window: Duration::from_secs(300),
            first_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(32),
            check_every: Duration::from_secs(5),
            check_timeout: Duration::from_secs(5),
            missed_checks: 2,
        }
    }
}

impl SupervisorOptions {
    /// The wait before restart number `attempt` (from 1): doubling from the first backoff.
    fn backoff(&self, attempt: usize) -> Duration {
        let step = u32::try_from(attempt).unwrap_or(u32::MAX);
        crate::retry::doubled(self.first_backoff, step).min(self.max_backoff)
    }
}

/// Whether the engine can take commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineHealth {
    Running,
    /// The engine stopped and is being started again. `attempt` counts the crashes within the
    /// restart window; it is 0 after a restart by hand.
    Restarting {
        attempt: u32,
    },
    /// The engine isn't running and won't be restarted on its own; [`SupervisedEngine::restart`]
    /// tries again.
    Unavailable {
        code: ErrorCode,
    },
}

enum Signal {
    /// The engine of this generation stopped: its stdout ended or broke the protocol.
    Stopped(u64),
    /// The engine of this generation stopped answering.
    Hung(u64),
    Restart,
    Stop,
}

struct State {
    engine: Option<Arc<ChildProcessEngine>>,
    /// Raised whenever an engine starts or is retired, so a retired engine's late events and its
    /// stop signal are ignored.
    generation: u64,
    health: EngineHealth,
    capabilities: Capabilities,
    /// The last state of each transfer the running engine has, from its events and replies.
    current: BTreeMap<TransferId, TransferSnapshot>,
    /// The transfers earlier engines had when they stopped. All are finished; the supervisor
    /// answers for them until they are removed.
    orphans: BTreeMap<TransferId, TransferSnapshot>,
    highest_id: u64,
    /// The global speed limit last set, given to every new engine (#49).
    global_limits: Limits,
    crashes: VecDeque<Instant>,
    subscribers: Vec<Sender<TransferEvent>>,
    health_subscribers: Vec<Sender<EngineHealth>>,
}

impl State {
    fn emit(&mut self, event: TransferEvent) {
        self.subscribers.retain(|s| s.send(event.clone()).is_ok());
    }

    fn set_health(&mut self, health: EngineHealth) {
        self.health = health;
        self.health_subscribers.retain(|s| s.send(health).is_ok());
    }

    fn saw(&mut self, id: TransferId) {
        self.highest_id = self.highest_id.max(id.0);
    }

    /// Records a transfer that the engine of `generation` assigned, if no event has yet.
    fn added(&mut self, generation: u64, id: TransferId) {
        self.saw(id);
        let mut queued = TransferSnapshot {
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
        };
        if generation == self.generation {
            self.current.entry(id).or_insert(queued);
        } else if let Entry::Vacant(slot) = self.orphans.entry(id) {
            // The engine stopped between assigning the id and this reply.
            lose(&mut queued, CRASH_MESSAGE);
            slot.insert(queued.clone());
            self.emit(TransferEvent::Changed { snapshot: queued });
        }
    }
}

/// Stops a retired engine, outside the state lock. A command still in flight may hold another
/// reference, so the engine is asked to stop first; it must not go on downloading beside its
/// successor. Dropping the last reference then waits for it to exit, or kills it.
fn stop(engine: Option<Arc<ChildProcessEngine>>) {
    if let Some(engine) = engine {
        engine.stop();
    }
}

/// Marks an unfinished transfer as failed because its engine went away.
fn lose(snapshot: &mut TransferSnapshot, message: &str) {
    snapshot.status = DownloadStatus::Failed;
    snapshot.progress.download_speed_bps = 0;
    snapshot.progress.connections = 0;
    snapshot.error = Some(TransferError::new(FailureClass::Transient, None, message));
}

struct Shared {
    program: PathBuf,
    options: SupervisorOptions,
    log: EngineLog,
    state: Mutex<State>,
    control: Sender<Signal>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // The state stays consistent between whole updates, so a panic elsewhere doesn't corrupt it.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn spawn(&self) -> Result<ChildProcessEngine, ErrorCode> {
        let options = SpawnOptions {
            first_transfer_id: TransferId(self.state().highest_id + 1),
            log: self.log.clone(),
        };
        ChildProcessEngine::spawn_with(&self.program, &options)
    }

    /// Makes `engine` the running engine, and forwards its events until its stdout ends.
    fn install(self: &Arc<Self>, engine: ChildProcessEngine) -> Result<(), ErrorCode> {
        // A new engine starts unlimited: it gets the global limit its predecessor had.
        let global = self.state().global_limits;
        if global != Limits::default() {
            engine
                .set_global_limits(global)
                .map_err(|_| ErrorCode::EngineStartFailed)?;
        }
        let events = engine.subscribe();
        let capabilities = engine.capabilities();
        let engine = Arc::new(engine);
        let generation = {
            let mut state = self.state();
            state.generation += 1;
            state.engine = Some(Arc::clone(&engine));
            state.capabilities = capabilities;
            state.current.clear();
            state.generation
        };
        let shared = Arc::clone(self);
        let forwarder = thread::Builder::new()
            .name("velocity-engine-events".into())
            .spawn(move || {
                for event in events {
                    shared.forward(generation, event);
                }
                // The supervisor may have stopped already; then nobody needs to know.
                let _ = shared.control.send(Signal::Stopped(generation));
            });
        if forwarder.is_err() {
            // Without the forwarder, a crash would go unnoticed: don't run this engine.
            self.retire(Some(generation), CRASH_MESSAGE);
            return Err(ErrorCode::EngineStartFailed);
        }
        self.watch(generation, engine);
        self.state().set_health(EngineHealth::Running);
        Ok(())
    }

    /// Checks the engine of `generation` every `check_every` until it is replaced, and reports
    /// it as hung after `missed_checks` checks in a row go unanswered.
    fn watch(self: &Arc<Self>, generation: u64, engine: Arc<ChildProcessEngine>) {
        let shared = Arc::clone(self);
        let watchdog = thread::Builder::new()
            .name("velocity-engine-watchdog".into())
            .spawn(move || {
                let options = &shared.options;
                let mut missed = 0;
                loop {
                    let next = Instant::now() + options.check_every;
                    while Instant::now() < next {
                        if shared.state().generation != generation {
                            return;
                        }
                        thread::sleep(WATCH_STEP.min(options.check_every));
                    }
                    if engine.answers_within(options.check_timeout) {
                        missed = 0;
                        continue;
                    }
                    missed += 1;
                    if missed >= options.missed_checks.max(1) {
                        // The supervisor may have stopped already; then nobody needs to know.
                        let _ = shared.control.send(Signal::Hung(generation));
                        return;
                    }
                }
            });
        if watchdog.is_err() {
            // Not fatal: crashes are still noticed, only a hang isn't. Said in the engine's log.
            self.log
                .push("the engine watchdog couldn't start; a hung engine won't be restarted");
        }
    }

    fn forward(&self, generation: u64, event: TransferEvent) {
        let mut state = self.state();
        if state.generation != generation {
            return;
        }
        match &event {
            TransferEvent::Changed { snapshot } => {
                state.saw(snapshot.id);
                state.current.insert(snapshot.id, snapshot.clone());
            }
            TransferEvent::Removed { id } => {
                state.current.remove(id);
            }
            // Passed on for the app to save (#43); it changes no state here.
            TransferEvent::Checkpoint { .. } => {}
        }
        state.emit(event);
    }

    /// Takes the running engine away (only the engine of `generation`, if given), reports its
    /// unfinished transfers as failed, and stops it. Returns whether there was one to retire.
    fn retire(&self, generation: Option<u64>, message: &str) -> bool {
        let engine = {
            let mut state = self.state();
            if generation.is_some_and(|g| g != state.generation) || state.engine.is_none() {
                return false;
            }
            let engine = state.engine.take();
            state.generation += 1;
            for (id, mut snapshot) in std::mem::take(&mut state.current) {
                if !snapshot.status.is_terminal() {
                    lose(&mut snapshot, message);
                    state.emit(TransferEvent::Changed {
                        snapshot: snapshot.clone(),
                    });
                }
                state.orphans.insert(id, snapshot);
            }
            engine
        };
        stop(engine);
        true
    }

    /// Starts engines until one runs, the cap is reached or the supervisor stops. Returns false
    /// when the supervisor is stopping.
    fn recover(self: &Arc<Self>, signals: &Receiver<Signal>) -> bool {
        loop {
            let attempt = {
                let mut state = self.state();
                let window = self.options.restart_window;
                while state.crashes.front().is_some_and(|t| t.elapsed() > window) {
                    state.crashes.pop_front();
                }
                state.crashes.len()
            };
            if attempt > self.options.max_restarts {
                self.state().set_health(EngineHealth::Unavailable {
                    code: ErrorCode::EngineUnavailable,
                });
                return true;
            }
            self.state().set_health(EngineHealth::Restarting {
                attempt: attempt as u32,
            });
            if attempt > 0 {
                match signals.recv_timeout(self.options.backoff(attempt)) {
                    Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => return false,
                    Ok(Signal::Restart) => self.state().crashes.clear(),
                    Ok(Signal::Stopped(_) | Signal::Hung(_)) | Err(RecvTimeoutError::Timeout) => {}
                }
            }
            match self.spawn().and_then(|engine| self.install(engine)) {
                Ok(()) => return true,
                // Starting again can't fix a missing or mismatched engine.
                Err(code @ (ErrorCode::EngineMissing | ErrorCode::EngineIncompatible)) => {
                    self.state().set_health(EngineHealth::Unavailable { code });
                    return true;
                }
                // A failed start counts as a crash, so a broken engine reaches the cap.
                Err(_) => self.state().crashes.push_back(Instant::now()),
            }
        }
    }

    fn supervise(self: Arc<Self>, signals: Receiver<Signal>) {
        loop {
            let recovered = match signals.recv() {
                Ok(Signal::Stop) | Err(_) => return,
                Ok(Signal::Stopped(generation)) => {
                    if !self.retire(Some(generation), CRASH_MESSAGE) {
                        continue;
                    }
                    self.state().crashes.push_back(Instant::now());
                    self.recover(&signals)
                }
                // Stopping it kills it, as it can't answer a request to exit. Its stdout then
                // ends too, but that stop signal belongs to a retired generation and is ignored.
                Ok(Signal::Hung(generation)) => {
                    if !self.retire(Some(generation), HUNG_MESSAGE) {
                        continue;
                    }
                    self.state().crashes.push_back(Instant::now());
                    self.recover(&signals)
                }
                Ok(Signal::Restart) => {
                    self.retire(None, RESTART_MESSAGE);
                    self.state().crashes.clear();
                    self.recover(&signals)
                }
            };
            if !recovered {
                return;
            }
        }
    }
}

/// The direct-download engine as a supervised child process.
pub struct SupervisedEngine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SupervisedEngine {
    /// Starts `<program> serve` and supervises it. Fails if the first engine doesn't start.
    pub fn start(program: PathBuf, options: SupervisorOptions) -> Result<Self, ErrorCode> {
        let (control, signals) = channel();
        let shared = Arc::new(Shared {
            program,
            options,
            log: EngineLog::new(),
            state: Mutex::new(State {
                engine: None,
                generation: 0,
                health: EngineHealth::Restarting { attempt: 0 },
                capabilities: Capabilities {
                    protocols: Vec::new(),
                    live_connection_changes: false,
                    validator_resume: false,
                    metalink: false,
                    max_connections_per_host: 1,
                },
                current: BTreeMap::new(),
                orphans: BTreeMap::new(),
                highest_id: 0,
                global_limits: Limits::default(),
                crashes: VecDeque::new(),
                subscribers: Vec::new(),
                health_subscribers: Vec::new(),
            }),
            control,
        });
        let engine = shared.spawn()?;
        shared.install(engine)?;
        let supervisor = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("velocity-engine-supervisor".into())
            .spawn(move || supervisor.supervise(signals))
            .map_err(|_| ErrorCode::EngineStartFailed)?;
        Ok(SupervisedEngine {
            shared,
            thread: Some(thread),
        })
    }

    pub fn health(&self) -> EngineHealth {
        self.shared.state().health
    }

    /// A receiver for every later change of [`EngineHealth`].
    pub fn subscribe_health(&self) -> Receiver<EngineHealth> {
        let (tx, rx) = channel();
        self.shared.state().health_subscribers.push(tx);
        rx
    }

    /// Stops the engine, if one runs, and starts a new one, clearing the crash count. Transfers
    /// that were running fail with a retryable error. [`EngineHealth`] reports the outcome.
    pub fn restart(&self) {
        // The supervisor thread only ends when this engine is dropped.
        let _ = self.shared.control.send(Signal::Restart);
    }

    /// The running engine's process id, or `None` while none runs.
    pub fn process_id(&self) -> Option<u32> {
        self.shared.state().engine.as_ref().map(|e| e.process_id())
    }

    /// The engine's last stderr lines, with URL logins hidden.
    pub fn log(&self) -> Vec<String> {
        self.shared.log.lines()
    }

    fn engine(&self) -> Result<(Arc<ChildProcessEngine>, u64), ErrorCode> {
        let state = self.shared.state();
        let engine = state.engine.clone().ok_or(ErrorCode::EngineUnavailable)?;
        Ok((engine, state.generation))
    }

    /// Whether an earlier engine had `id`. Such a transfer is finished, so only `remove` and
    /// `snapshot` apply to it.
    fn orphaned(&self, id: TransferId) -> bool {
        self.shared.state().orphans.contains_key(&id)
    }

    fn running(&self, id: TransferId) -> Result<Arc<ChildProcessEngine>, ErrorCode> {
        if self.orphaned(id) {
            return Err(ErrorCode::InvalidState);
        }
        self.engine().map(|(engine, _)| engine)
    }
}

impl DirectTransfer for SupervisedEngine {
    fn capabilities(&self) -> Capabilities {
        self.shared.state().capabilities.clone()
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        let (engine, generation) = self.engine()?;
        let id = engine.add(spec)?;
        self.shared.state().added(generation, id);
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
        {
            let mut state = self.shared.state();
            if state.orphans.remove(&id).is_some() {
                state.emit(TransferEvent::Removed { id });
                return Ok(());
            }
        }
        self.engine()?.0.remove(id)
    }

    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        self.running(id)?.set_limits(id, limits)
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        self.running(id)?.set_connections(id, connections)
    }

    fn set_global_limits(&mut self, limits: Limits) -> Result<(), ErrorCode> {
        self.engine()?.0.set_global_limits(limits)?;
        self.shared.state().global_limits = limits;
        Ok(())
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        let mut transfers = self.engine()?.0.snapshot()?;
        let state = self.shared.state();
        transfers.extend(state.orphans.values().cloned());
        transfers.sort_by_key(|s| s.id);
        Ok(transfers)
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        self.shared.state().subscribers.push(tx);
        rx
    }
}

impl Drop for SupervisedEngine {
    fn drop(&mut self) {
        let _ = self.shared.control.send(Signal::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // A graceful stop: the engine cancels its transfers and removes their part files.
        let engine = {
            let mut state = self.shared.state();
            state.generation += 1;
            state.engine.take()
        };
        stop(engine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_doubles_up_to_its_cap() {
        let options = SupervisorOptions::default();
        let waits: Vec<u64> = (1..=7).map(|a| options.backoff(a).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 32]);
        assert_eq!(options.backoff(10_000), options.max_backoff);
    }
}
