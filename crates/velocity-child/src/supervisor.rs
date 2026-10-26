//! The child supervisor (#39, ADR-0012; shared with the torrent helper since #62): runs a child
//! process and keeps it running. What a child is given when it starts, what its events mean and
//! what happens to its work when it goes away are the [`Book`]'s; the rest is here.
//!
//! - **Crashes:** when the child's stdout ends, the book is told the child was retired, and a new
//!   child starts after a backoff.
//! - **Hangs** (#48): a child that stops answering, without exiting, is checked every few seconds;
//!   after a few checks in a row go unanswered, it is killed and handled like a crash.
//! - **The cap:** more than [`SupervisorOptions::max_restarts`] crashes within the restart window
//!   leave the child unavailable until [`Supervisor::restart`] is called. A child that is missing
//!   or doesn't match the app is unavailable at once, since restarting it can't help.
//! - **Ids:** a new child starts its ids after the highest one an earlier child assigned, so an id
//!   never names two things.
//!
//! No lock is held while talking to a child: a command takes its own reference to the running
//! child and releases the state first.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use velocity_core::domain::ErrorCode;

use crate::client::SpawnOptions;
use crate::log::ChildLog;

/// How often a watchdog checks whether its child was replaced.
const WATCH_STEP: Duration = Duration::from_millis(50);

/// `first` doubled for each step after the first (`step` from 1), without overflowing.
pub fn doubled(first: Duration, step: u32) -> Duration {
    first.saturating_mul(1 << step.saturating_sub(1).min(31))
}

/// How the supervisor restarts a child. The defaults are V1's: at most 5 restarts within 5
/// minutes, waiting 1, 2, 4, 8 and 16 seconds before each (up to 32). A child that stops answering
/// is restarted too (#48): it is checked every `check_every`, and after `missed_checks` checks in a
/// row without an answer within `check_timeout`, it is killed. The check values are starting
/// values (PO decision on #48).
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
        doubled(self.first_backoff, step).min(self.max_backoff)
    }
}

/// Whether the child can take commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineHealth {
    Running,
    /// The child stopped and is being started again. `attempt` counts the crashes within the
    /// restart window; it is 0 after a restart by hand.
    Restarting {
        attempt: u32,
    },
    /// The child isn't running and won't be restarted on its own; [`Supervisor::restart`] tries
    /// again.
    Unavailable {
        code: ErrorCode,
    },
}

/// A child process the supervisor can run.
pub trait Child: Send + Sync + Sized + 'static {
    type Event: Clone + Send + 'static;
    /// A short name for threads and messages, such as `engine` or `torrent`.
    const NAME: &'static str;
    fn spawn(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode>;
    /// A receiver for the child's events. It disconnects when the child's stdout ends.
    fn subscribe(&self) -> Receiver<Self::Event>;
    /// Whether the child answers a cheap request within `timeout`.
    fn answers_within(&self, timeout: Duration) -> bool;
    /// Asks the child to stop: requests fail as unavailable from now on.
    fn stop(&self);
    fn process_id(&self) -> u32;
}

/// Why the running child was taken away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retired {
    /// Its stdout ended or broke the protocol.
    Crashed,
    /// It stopped answering and was killed.
    Hung,
    /// [`Supervisor::restart`] replaced it.
    Restarted,
}

/// What a supervisor keeps for its kind of child: the state the app sees, which outlives each
/// child. Every method but [`Book::prepare`] runs with the supervisor's state locked, so none may
/// talk to a child.
pub trait Book<Ch: Child>: Send + 'static {
    /// What a new child is given before it takes commands, such as the limits last set.
    type Setup: Send;
    fn setup(&self) -> Self::Setup;
    /// Gives a new child its setup. No lock is held. Events the child sends meanwhile reach
    /// [`Book::event`] once it is [`Book::installed`].
    fn prepare(child: &Ch, setup: Self::Setup) -> Result<(), ErrorCode>;
    /// The highest id any child assigned; a new child starts after it.
    fn highest_id(&self) -> u64;
    /// `child` is now the running child.
    fn installed(&mut self, child: &Ch);
    /// An event from the running child; returns the events to pass on to subscribers.
    fn event(&mut self, event: Ch::Event) -> Vec<Ch::Event>;
    /// The running child was taken away; returns the events to pass on, such as its unfinished
    /// work failing.
    fn retired(&mut self, why: Retired) -> Vec<Ch::Event>;
}

enum Signal {
    /// The child of this generation stopped: its stdout ended or broke the protocol.
    Stopped(u64),
    /// The child of this generation stopped answering.
    Hung(u64),
    Restart,
    Stop,
}

struct State<Ch: Child, B> {
    child: Option<Arc<Ch>>,
    /// Raised whenever a child starts or is retired, so a retired child's late events and its stop
    /// signal are ignored.
    generation: u64,
    health: EngineHealth,
    book: B,
    crashes: VecDeque<Instant>,
    subscribers: Vec<Sender<Ch::Event>>,
    health_subscribers: Vec<Sender<EngineHealth>>,
}

impl<Ch: Child, B> State<Ch, B> {
    fn emit(&mut self, events: Vec<Ch::Event>) {
        for event in events {
            self.subscribers.retain(|s| s.send(event.clone()).is_ok());
        }
    }

    fn set_health(&mut self, health: EngineHealth) {
        self.health = health;
        self.health_subscribers.retain(|s| s.send(health).is_ok());
    }
}

/// Stops a retired child, outside the state lock. A command still in flight may hold another
/// reference, so the child is asked to stop first; it must not go on working beside its
/// successor. Dropping the last reference then waits for it to exit, or kills it.
fn stop<Ch: Child>(child: Option<Arc<Ch>>) {
    if let Some(child) = child {
        child.stop();
    }
}

struct Shared<Ch: Child, B> {
    program: PathBuf,
    options: SupervisorOptions,
    log: ChildLog,
    state: Mutex<State<Ch, B>>,
    control: Sender<Signal>,
}

impl<Ch: Child, B: Book<Ch>> Shared<Ch, B> {
    fn state(&self) -> MutexGuard<'_, State<Ch, B>> {
        // The state stays consistent between whole updates, so a panic elsewhere doesn't corrupt it.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn spawn(&self) -> Result<Ch, ErrorCode> {
        let options = SpawnOptions {
            first_id: self.state().book.highest_id() + 1,
            log: self.log.clone(),
        };
        Ch::spawn(&self.program, &options)
    }

    /// Makes `child` the running child, and forwards its events until its stdout ends.
    fn install(self: &Arc<Self>, child: Ch) -> Result<(), ErrorCode> {
        let setup = self.state().book.setup();
        // Subscribed first, so nothing the child sends while it is prepared is lost.
        let events = child.subscribe();
        B::prepare(&child, setup).map_err(|_| ErrorCode::EngineStartFailed)?;
        let child = Arc::new(child);
        let generation = {
            let mut state = self.state();
            state.generation += 1;
            state.child = Some(Arc::clone(&child));
            state.book.installed(&child);
            state.generation
        };
        let shared = Arc::clone(self);
        let forwarder = thread::Builder::new()
            .name(format!("velocity-{}-events", Ch::NAME))
            .spawn(move || {
                for event in events {
                    shared.forward(generation, event);
                }
                // The supervisor may have stopped already; then nobody needs to know.
                let _ = shared.control.send(Signal::Stopped(generation));
            });
        if forwarder.is_err() {
            // Without the forwarder, a crash would go unnoticed: don't run this child.
            self.retire(Some(generation), Retired::Crashed);
            return Err(ErrorCode::EngineStartFailed);
        }
        self.watch(generation, &child);
        self.state().set_health(EngineHealth::Running);
        Ok(())
    }

    /// Checks the child of `generation` every `check_every` until it is replaced, and reports it
    /// as hung after `missed_checks` checks in a row go unanswered.
    ///
    /// The watchdog holds the child weakly, so the supervisor's own reference stays the last one:
    /// dropping it waits for the child to exit, and no thread keeps a retired child running.
    fn watch(self: &Arc<Self>, generation: u64, child: &Arc<Ch>) {
        let child = Arc::downgrade(child);
        let shared = Arc::clone(self);
        let watchdog = thread::Builder::new()
            .name(format!("velocity-{}-watchdog", Ch::NAME))
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
                    let Some(running) = Weak::upgrade(&child) else {
                        return;
                    };
                    let answered = running.answers_within(options.check_timeout);
                    drop(running);
                    if answered {
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
            // Not fatal: crashes are still noticed, only a hang isn't. Said in the child's log.
            self.log.push(&format!(
                "the {name} watchdog couldn't start; a hung {name} won't be restarted",
                name = Ch::NAME
            ));
        }
    }

    fn forward(&self, generation: u64, event: Ch::Event) {
        let mut state = self.state();
        if state.generation != generation {
            return;
        }
        let events = state.book.event(event);
        state.emit(events);
    }

    /// Takes the running child away (only the child of `generation`, if given), tells the book
    /// why, and stops it. Returns whether there was one to retire.
    fn retire(&self, generation: Option<u64>, why: Retired) -> bool {
        let child = {
            let mut state = self.state();
            if generation.is_some_and(|g| g != state.generation) || state.child.is_none() {
                return false;
            }
            let child = state.child.take();
            state.generation += 1;
            let events = state.book.retired(why);
            state.emit(events);
            child
        };
        stop(child);
        true
    }

    /// Starts children until one runs, the cap is reached or the supervisor stops. Returns false
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
            match self.spawn().and_then(|child| self.install(child)) {
                Ok(()) => return true,
                // Starting again can't fix a missing or mismatched child.
                Err(code @ (ErrorCode::EngineMissing | ErrorCode::EngineIncompatible)) => {
                    self.state().set_health(EngineHealth::Unavailable { code });
                    return true;
                }
                // A failed start counts as a crash, so a broken child reaches the cap.
                Err(_) => self.state().crashes.push_back(Instant::now()),
            }
        }
    }

    fn supervise(self: Arc<Self>, signals: Receiver<Signal>) {
        loop {
            let recovered = match signals.recv() {
                Ok(Signal::Stop) | Err(_) => return,
                Ok(Signal::Stopped(generation)) => {
                    if !self.retire(Some(generation), Retired::Crashed) {
                        continue;
                    }
                    self.state().crashes.push_back(Instant::now());
                    self.recover(&signals)
                }
                // Stopping it kills it, as it can't answer a request to exit. Its stdout then ends
                // too, but that stop signal belongs to a retired generation and is ignored.
                Ok(Signal::Hung(generation)) => {
                    if !self.retire(Some(generation), Retired::Hung) {
                        continue;
                    }
                    self.state().crashes.push_back(Instant::now());
                    self.recover(&signals)
                }
                Ok(Signal::Restart) => {
                    self.retire(None, Retired::Restarted);
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

/// A child process kept running, with its [`Book`].
pub struct Supervisor<Ch: Child, B: Book<Ch>> {
    shared: Arc<Shared<Ch, B>>,
    thread: Option<JoinHandle<()>>,
}

impl<Ch: Child, B: Book<Ch>> Supervisor<Ch, B> {
    /// Starts `<program> serve` and supervises it. Fails if the first child doesn't start.
    pub fn start(program: PathBuf, options: SupervisorOptions, book: B) -> Result<Self, ErrorCode> {
        let (control, signals) = channel();
        let shared = Arc::new(Shared {
            program,
            options,
            log: ChildLog::new(),
            state: Mutex::new(State {
                child: None,
                generation: 0,
                health: EngineHealth::Restarting { attempt: 0 },
                book,
                crashes: VecDeque::new(),
                subscribers: Vec::new(),
                health_subscribers: Vec::new(),
            }),
            control,
        });
        let child = shared.spawn()?;
        shared.install(child)?;
        let supervisor = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name(format!("velocity-{}-supervisor", Ch::NAME))
            .spawn(move || supervisor.supervise(signals))
            .map_err(|_| ErrorCode::EngineStartFailed)?;
        Ok(Supervisor {
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

    /// A receiver for the events the book passes on, from every child.
    pub fn subscribe(&self) -> Receiver<Ch::Event> {
        let (tx, rx) = channel();
        self.shared.state().subscribers.push(tx);
        rx
    }

    /// Stops the child, if one runs, and starts a new one, clearing the crash count.
    /// [`EngineHealth`] reports the outcome.
    pub fn restart(&self) {
        // The supervisor thread only ends when this supervisor is dropped.
        let _ = self.shared.control.send(Signal::Restart);
    }

    /// The running child's process id, or `None` while none runs.
    pub fn process_id(&self) -> Option<u32> {
        self.shared.state().child.as_ref().map(|c| c.process_id())
    }

    /// The child's last stderr lines, with URL logins hidden.
    pub fn log(&self) -> Vec<String> {
        self.shared.log.lines()
    }

    /// The running child and its generation, to send a command to with no lock held.
    pub fn child(&self) -> Result<(Arc<Ch>, u64), ErrorCode> {
        let state = self.shared.state();
        let child = state.child.clone().ok_or(ErrorCode::EngineUnavailable)?;
        Ok((child, state.generation))
    }

    /// Runs `f` on the book with the state locked; `f` gets the current generation and returns
    /// its result and the events to pass on. `f` must not talk to a child.
    pub fn update<R>(&self, f: impl FnOnce(&mut B, u64) -> (R, Vec<Ch::Event>)) -> R {
        let mut state = self.shared.state();
        let generation = state.generation;
        let (result, events) = f(&mut state.book, generation);
        state.emit(events);
        result
    }
}

impl<Ch: Child, B: Book<Ch>> Drop for Supervisor<Ch, B> {
    fn drop(&mut self) {
        let _ = self.shared.control.send(Signal::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // A graceful stop: the child shuts its work down itself.
        let child = {
            let mut state = self.shared.state();
            state.generation += 1;
            state.child.take()
        };
        stop(child);
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

    #[test]
    fn doubling_never_overflows() {
        assert_eq!(doubled(Duration::from_secs(1), 1), Duration::from_secs(1));
        assert_eq!(doubled(Duration::from_secs(1), 3), Duration::from_secs(4));
        assert_eq!(doubled(Duration::MAX, 40), Duration::MAX);
    }
}
