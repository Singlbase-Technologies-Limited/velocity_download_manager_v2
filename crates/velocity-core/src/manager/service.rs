//! The manager on a thread of its own (#58), as the app runs it.
//!
//! - **One owner:** the thread owns the [`Manager`]; commands reach it through a bounded channel
//!   and wait, at most [`CALL_TIMEOUT`], for its answer. No lock is held while the manager talks
//!   to the engine or the database (`.claude/rules/rust-native.md`).
//! - **Ticks:** every [`TICK`] the manager applies the engine's events and the schedule at the
//!   local time ([`Manager::tick`]), and starts what may run.
//! - **Snapshots:** when the snapshot changed, it is sent, at most every [`SNAPSHOT_EVERY`]: the
//!   UI gets each change, and a busy download doesn't flood it (V1 polled every 250 ms to 5 s).

use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::Manager;
use crate::domain::{DownloadsSnapshot, ErrorCode};

/// How often the manager applies the engine's events and the schedule.
pub const TICK: Duration = Duration::from_millis(100);
/// The most often a snapshot is sent: V1 refreshed the list every 250 ms at its fastest.
pub const SNAPSHOT_EVERY: Duration = Duration::from_millis(250);
/// The longest a command waits for the manager. Its engine calls have their own, shorter bounds
/// (ENGINE_PROTOCOL.md); this one only guards against a thread that stopped.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Commands that may wait for the manager at once.
const PENDING_CALLS: usize = 64;

type Job = Box<dyn FnOnce(&mut Manager) + Send>;

/// The running manager. Dropping it stops the thread, once the command running finishes.
pub struct ManagerService {
    jobs: Option<SyncSender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl ManagerService {
    /// Runs `manager` on its own thread; `on_snapshot` is called there with each new snapshot,
    /// the first one at once.
    pub fn start(
        manager: Manager,
        on_snapshot: impl FnMut(&DownloadsSnapshot) + Send + 'static,
    ) -> Self {
        let (jobs, inbox) = sync_channel::<Job>(PENDING_CALLS);
        let thread = thread::Builder::new()
            .name("velocity-manager".to_owned())
            .spawn(move || run(manager, inbox, on_snapshot))
            .expect("the manager thread starts");
        ManagerService {
            jobs: Some(jobs),
            thread: Some(thread),
        }
    }

    /// Runs `f` on the manager's thread and returns what it returns. `Internal` if the manager
    /// stopped, or didn't answer within [`CALL_TIMEOUT`].
    pub fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Manager) -> R + Send + 'static,
    ) -> Result<R, ErrorCode> {
        let (answer, answered) = sync_channel(1);
        let job: Job = Box::new(move |m| {
            // The caller may have stopped waiting; the command ran all the same.
            let _ = answer.send(f(m));
        });
        self.jobs
            .as_ref()
            .ok_or(ErrorCode::Internal)?
            .send(job)
            .map_err(|_| ErrorCode::Internal)?;
        answered
            .recv_timeout(CALL_TIMEOUT)
            .map_err(|_| ErrorCode::Internal)
    }
}

impl Drop for ManagerService {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            // A panic on the manager's thread has already been reported by the panic hook.
            let _ = thread.join();
        }
    }
}

/// Milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn run(
    mut manager: Manager,
    inbox: Receiver<Job>,
    mut on_snapshot: impl FnMut(&DownloadsSnapshot),
) {
    let mut sent: Option<(DownloadsSnapshot, Instant)> = None;
    let mut next_tick = Instant::now();
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match inbox.recv_timeout(wait) {
            Ok(job) => job(&mut manager),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if Instant::now() >= next_tick {
            manager.tick(now_ms(), jiff::Zoned::now().datetime());
            next_tick = Instant::now() + TICK;
        }
        let due = sent
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() >= SNAPSHOT_EVERY);
        if due {
            let snapshot = manager.snapshot();
            if sent.as_ref().is_none_or(|(last, _)| *last != snapshot) {
                on_snapshot(&snapshot);
                sent = Some((snapshot, Instant::now()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::domain::DownloadKind;
    use crate::transfer::fake::FakeTransfer;
    use crate::transfer::{Limits, TransferId, TransferSpec};
    use std::path::PathBuf;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Arc;
    use std::time::Instant;

    fn spec() -> TransferSpec {
        TransferSpec {
            urls: vec!["https://example.com/a.zip".to_owned()],
            directory: PathBuf::from("downloads"),
            file_name: Some("a.zip".to_owned()),
            credentials: None,
            require_encryption: false,
            ssh_key: None,
            headers: Vec::new(),
            referrer: None,
            user_agent: None,
            checksum: None,
            pieces: None,
            limits: Limits::default(),
            max_connections: 4,
            resume: None,
        }
    }

    fn service() -> (
        ManagerService,
        crate::transfer::fake::FakeControl,
        Receiver<DownloadsSnapshot>,
    ) {
        let fake = FakeTransfer::new();
        let control = fake.control();
        let manager = Manager::open(
            Box::new(fake),
            Arc::new(Database::open_in_memory().unwrap()),
        )
        .unwrap();
        let (tx, rx) = channel();
        let service = ManagerService::start(manager, move |s| {
            let _ = tx.send(s.clone());
        });
        (service, control, rx)
    }

    /// The snapshots sent within `within`.
    fn collect(rx: &Receiver<DownloadsSnapshot>, within: Duration) -> Vec<DownloadsSnapshot> {
        let until = Instant::now() + within;
        let mut got = Vec::new();
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(s) => got.push(s),
                Err(_) => break,
            }
        }
        got
    }

    #[test]
    fn calls_run_on_the_managers_thread_and_answer() {
        let (service, control, _rx) = service();
        let id = service
            .call(|m| m.add(DownloadKind::Http, spec(), 1))
            .unwrap()
            .unwrap();
        assert_eq!(control.log(), ["add"]);
        let file = service
            .call(move |m| m.get(&id).and_then(|r| r.file_name.clone()))
            .unwrap();
        assert_eq!(file.as_deref(), Some("a.zip"));
    }

    #[test]
    fn the_engines_events_reach_a_snapshot_without_any_call() {
        let (service, control, rx) = service();
        service
            .call(|m| m.add(DownloadKind::Http, spec(), 1))
            .unwrap()
            .unwrap();
        control.progress(TransferId(1), 1234, Some(5000));
        let seen = collect(&rx, Duration::from_secs(2));
        assert!(
            seen.iter()
                .any(|s| s.downloads.first().map(|r| r.progress.completed_bytes) == Some(1234)),
            "the progress was sent"
        );
    }

    #[test]
    fn snapshots_are_sent_at_most_every_250_ms_and_only_when_something_changed() {
        let (service, control, rx) = service();
        service
            .call(|m| m.add(DownloadKind::Http, spec(), 1))
            .unwrap()
            .unwrap();
        let started = Instant::now();
        for n in 1..=40 {
            control.progress(TransferId(1), n * 100, Some(10_000));
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut seen = collect(&rx, Duration::from_millis(600));
        let elapsed = started.elapsed();
        // One for the add, then at most one per 250 ms of changes.
        let most = 2 + (elapsed.as_millis() / SNAPSHOT_EVERY.as_millis()) as usize;
        assert!(
            seen.len() <= most,
            "{} snapshots in {elapsed:?}, at most {most}",
            seen.len()
        );
        let last = seen.pop().expect("at least one snapshot");
        assert_eq!(
            last.downloads[0].progress.completed_bytes, 4000,
            "the last change is sent"
        );
        assert!(
            collect(&rx, Duration::from_millis(600)).is_empty(),
            "nothing changed, nothing sent"
        );
    }

    #[test]
    fn the_thread_ends_when_the_service_is_dropped() {
        let (service, _control, rx) = service();
        drop(service);
        // The sender went with the thread.
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
    }
}
