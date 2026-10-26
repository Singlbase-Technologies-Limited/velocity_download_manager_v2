//! The torrent helper's supervisor (ADR-0014): `velocity_child`'s supervisor, with what it keeps
//! for torrents ([`TorrentBook`]). A crashed or hung helper is restarted with the engine's backoff
//! and cap; the torrents it had fail with a retryable error, and their files stay for the manager
//! to add them again (resume data comes with #62's lifecycle part). Every new helper gets the
//! settings last set.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use velocity_child::ledger::Ledger;
use velocity_child::supervisor::{Book, Retired, Supervisor};
pub use velocity_child::supervisor::{EngineHealth, SupervisorOptions};
use velocity_core::domain::{DownloadStatus, ErrorCode, Progress};
use velocity_core::torrent_engine::{
    TorrentEngine, TorrentEvent, TorrentId, TorrentSettings, TorrentSnapshot, TorrentSpec,
};

use crate::stdio::ChildTorrentEngine;

/// What a torrent that a crash lost fails with.
const CRASH_MESSAGE: &str = "the torrent helper stopped unexpectedly";
/// What a torrent fails with when its helper stopped responding and was killed.
const HUNG_MESSAGE: &str = "the torrent helper stopped responding and was restarted";
/// What a torrent fails with when the helper is restarted by hand.
const RESTART_MESSAGE: &str = "the torrent helper was restarted";

/// What the supervisor keeps for the torrent helper, across helpers.
pub struct TorrentBook {
    torrents: Ledger<TorrentSnapshot>,
    /// The settings last set, given to every new helper.
    settings: TorrentSettings,
}

impl Book<ChildTorrentEngine> for TorrentBook {
    type Setup = TorrentSettings;

    fn setup(&self) -> TorrentSettings {
        self.settings
    }

    fn prepare(helper: &ChildTorrentEngine, settings: TorrentSettings) -> Result<(), ErrorCode> {
        helper.set_settings(settings)
    }

    fn highest_id(&self) -> u64 {
        self.torrents.highest_id()
    }

    fn installed(&mut self, _helper: &ChildTorrentEngine) {
        self.torrents.installed();
    }

    fn event(&mut self, event: TorrentEvent) -> Vec<TorrentEvent> {
        match &event {
            TorrentEvent::Changed { snapshot } => self.torrents.changed(snapshot),
            TorrentEvent::Removed { id } => self.torrents.removed(*id),
        }
        vec![event]
    }

    fn retired(&mut self, why: Retired) -> Vec<TorrentEvent> {
        let message = match why {
            Retired::Crashed => CRASH_MESSAGE,
            Retired::Hung => HUNG_MESSAGE,
            Retired::Restarted => RESTART_MESSAGE,
        };
        changed(self.torrents.retired(message))
    }
}

fn changed(snapshots: Vec<TorrentSnapshot>) -> Vec<TorrentEvent> {
    snapshots
        .into_iter()
        .map(|snapshot| TorrentEvent::Changed {
            snapshot: Box::new(snapshot),
        })
        .collect()
}

/// The torrent helper, kept running.
pub struct SupervisedTorrentEngine {
    supervisor: Supervisor<ChildTorrentEngine, TorrentBook>,
}

impl SupervisedTorrentEngine {
    /// Starts `<program> serve` with `settings` and supervises it. Fails if the first helper
    /// doesn't start.
    pub fn start(
        program: PathBuf,
        options: SupervisorOptions,
        settings: TorrentSettings,
    ) -> Result<Self, ErrorCode> {
        let book = TorrentBook {
            torrents: Ledger::default(),
            settings,
        };
        Ok(SupervisedTorrentEngine {
            supervisor: Supervisor::start(program, options, book)?,
        })
    }

    pub fn health(&self) -> EngineHealth {
        self.supervisor.health()
    }

    pub fn subscribe_health(&self) -> Receiver<EngineHealth> {
        self.supervisor.subscribe_health()
    }

    /// Stops the helper, if one runs, and starts a new one, clearing the crash count. Torrents
    /// that were running fail with a retryable error.
    pub fn restart(&self) {
        self.supervisor.restart();
    }

    pub fn process_id(&self) -> Option<u32> {
        self.supervisor.process_id()
    }

    /// The helper's last stderr lines, with URL logins hidden.
    pub fn log(&self) -> Vec<String> {
        self.supervisor.log()
    }

    fn helper(&self) -> Result<Arc<ChildTorrentEngine>, ErrorCode> {
        self.supervisor.child().map(|(helper, _)| helper)
    }

    /// The running helper, for a torrent it has: one an earlier helper had is finished.
    fn running(&self, id: TorrentId) -> Result<Arc<ChildTorrentEngine>, ErrorCode> {
        if self
            .supervisor
            .update(|book, _| (book.torrents.is_orphan(id), Vec::new()))
        {
            return Err(ErrorCode::InvalidState);
        }
        self.helper()
    }
}

impl TorrentEngine for SupervisedTorrentEngine {
    fn version(&self) -> Result<String, ErrorCode> {
        self.helper()?.version()
    }

    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        self.helper()?.set_settings(settings)?;
        self.supervisor.update(|book, _| {
            book.settings = settings;
            ((), Vec::new())
        });
        Ok(())
    }

    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        let (helper, generation) = self.supervisor.child()?;
        let id = helper.add(spec)?;
        let queued = TorrentSnapshot {
            id,
            status: DownloadStatus::Queued,
            name: None,
            info_hash_v1: None,
            info_hash_v2: None,
            progress: Progress {
                completed_bytes: 0,
                total_bytes: None,
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: None,
            },
            peers: 0,
            seeds: 0,
            error: None,
        };
        self.supervisor.update(|book, current| {
            // When the helper stopped between assigning the id and this reply, the torrent is lost.
            let lost = book
                .torrents
                .added(current == generation, queued, CRASH_MESSAGE);
            ((), changed(lost.into_iter().collect()))
        });
        Ok(id)
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        self.running(id)?.pause(id)
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        self.running(id)?.resume(id)
    }

    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let orphan = self.supervisor.update(|book, _| {
            if book.torrents.remove_orphan(id) {
                (true, vec![TorrentEvent::Removed { id }])
            } else {
                (false, Vec::new())
            }
        });
        if orphan {
            return Ok(());
        }
        self.helper()?.remove(id)
    }

    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode> {
        let mut torrents = self.helper()?.snapshot()?;
        let orphans = self
            .supervisor
            .update(|book, _| (book.torrents.orphans(), Vec::new()));
        torrents.extend(orphans);
        torrents.sort_by_key(|s| s.id);
        Ok(torrents)
    }

    fn subscribe(&mut self) -> Receiver<TorrentEvent> {
        self.supervisor.subscribe()
    }
}
