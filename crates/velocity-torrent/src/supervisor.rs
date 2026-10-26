//! The torrent helper's supervisor (ADR-0014): `velocity_child`'s supervisor, with what it keeps
//! for torrents ([`TorrentBook`]). A crashed or hung helper is restarted with the engine's backoff
//! and cap; the torrents it had fail with a retryable error, and their files stay for the manager
//! to add them again. Every new helper gets the settings last set. The supervisor keeps each
//! torrent's latest resume data, put together from the helper's chunks.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use velocity_child::ledger::Ledger;
use velocity_child::supervisor::{Book, Retired, Supervisor};
pub use velocity_child::supervisor::{EngineHealth, SupervisorOptions};
use velocity_core::domain::{DownloadStatus, ErrorCode, Progress};
use velocity_core::seeding::SeedPolicy;
use velocity_core::torrent::MAX_RESUME_BYTES;
use velocity_core::torrent_engine::{
    Chunk, TorrentEngine, TorrentEvent, TorrentId, TorrentSettings, TorrentSnapshot, TorrentSpec,
};

use crate::stdio::ChildTorrentEngine;

/// What a torrent that a crash lost fails with.
const CRASH_MESSAGE: &str = "the torrent helper stopped unexpectedly";
/// What a torrent fails with when its helper stopped responding and was killed.
const HUNG_MESSAGE: &str = "the torrent helper stopped responding and was restarted";
/// What a torrent fails with when the helper is restarted by hand.
const RESTART_MESSAGE: &str = "the torrent helper was restarted";

/// One torrent's resume data as its chunks arrive.
#[derive(Debug, Default)]
struct Incoming {
    bytes: Vec<u8>,
}

impl Incoming {
    /// Adds `chunk`; returns the whole resume data once its last chunk is in. A chunk at offset 0
    /// starts it anew. A chunk out of order, with bad hex, or over [`MAX_RESUME_BYTES`] drops what
    /// came so far: the helper sends it all again next time.
    fn add(&mut self, chunk: &Chunk) -> Option<Vec<u8>> {
        if chunk.offset == 0 {
            self.bytes.clear();
        }
        let fits =
            chunk.total <= MAX_RESUME_BYTES as u64 && chunk.offset == self.bytes.len() as u64;
        match chunk.bytes() {
            Ok(bytes) if fits && (self.bytes.len() + bytes.len()) as u64 <= chunk.total => {
                self.bytes.extend(bytes);
            }
            _ => {
                self.bytes = Vec::new();
                return None;
            }
        }
        (self.bytes.len() as u64 == chunk.total).then(|| std::mem::take(&mut self.bytes))
    }
}

/// A torrent's latest complete resume data, and how many have arrived for it.
#[derive(Debug, Clone)]
struct Saved {
    count: u64,
    bytes: Vec<u8>,
}

/// What the supervisor keeps for the torrent helper, across helpers.
pub struct TorrentBook {
    torrents: Ledger<TorrentSnapshot>,
    /// The settings last set, given to every new helper.
    settings: TorrentSettings,
    incoming: BTreeMap<TorrentId, Incoming>,
    resume: BTreeMap<TorrentId, Saved>,
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
            TorrentEvent::Removed { id } => {
                self.torrents.removed(*id);
                self.incoming.remove(id);
                self.resume.remove(id);
            }
            TorrentEvent::ResumeData { id, chunk } => {
                if let Some(bytes) = self.incoming.entry(*id).or_default().add(chunk) {
                    self.incoming.remove(id);
                    let count = self.resume.get(id).map_or(0, |s| s.count) + 1;
                    self.resume.insert(*id, Saved { count, bytes });
                }
            }
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
            incoming: BTreeMap::new(),
            resume: BTreeMap::new(),
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

    /// The latest complete resume data the helpers sent for `id`.
    pub fn resume_data(&self, id: TorrentId) -> Option<Vec<u8>> {
        self.supervisor
            .update(|book, _| (book.resume.get(&id).map(|s| s.bytes.clone()), Vec::new()))
    }

    /// Asks the helper for every torrent's resume data and waits up to `timeout` for it, as the
    /// app does before it closes (#58). Returns the resume data that arrived, by torrent; a torrent
    /// the helper named that is missing had none in time.
    pub fn collect_resume_data(
        &self,
        timeout: Duration,
    ) -> Result<BTreeMap<TorrentId, Vec<u8>>, ErrorCode> {
        let count = |book: &TorrentBook, id: &TorrentId| book.resume.get(id).map_or(0, |s| s.count);
        let before: BTreeMap<TorrentId, u64> = self.supervisor.update(|book, _| {
            let counts = book
                .resume
                .keys()
                .map(|id| (*id, count(book, id)))
                .collect();
            (counts, Vec::new())
        });
        let ids = self.helper()?.save_resume()?;
        let deadline = Instant::now() + timeout;
        loop {
            let (fresh, all) = self.supervisor.update(|book, _| {
                let fresh: BTreeMap<TorrentId, Vec<u8>> = ids
                    .iter()
                    .filter(|id| count(book, id) > before.get(id).copied().unwrap_or(0))
                    .filter_map(|id| book.resume.get(id).map(|s| (*id, s.bytes.clone())))
                    .collect();
                let all = fresh.len() == ids.len();
                ((fresh, all), Vec::new())
            });
            if all || Instant::now() >= deadline {
                return Ok(fresh);
            }
            thread::sleep(Duration::from_millis(20));
        }
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
            uploaded_bytes: 0,
            seeding_secs: 0,
            seed_queued: false,
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

    fn set_files(&mut self, id: TorrentId, files: Vec<u32>) -> Result<(), ErrorCode> {
        self.running(id)?.set_files(id, files)
    }

    fn metadata(&self, id: TorrentId, offset: u64) -> Result<Chunk, ErrorCode> {
        self.running(id)?.metadata(id, offset)
    }

    fn set_seeding(&mut self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        self.running(id)?.set_seeding(id, seeding)
    }

    fn upload(&mut self, chunk: Chunk) -> Result<(), ErrorCode> {
        self.helper()?.upload(chunk)
    }

    fn save_resume(&mut self) -> Result<Vec<TorrentId>, ErrorCode> {
        self.helper()?.save_resume()
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

#[cfg(test)]
mod tests {
    use super::*;
    use velocity_core::torrent_engine::CHUNK_BYTES;

    #[test]
    fn resume_data_is_put_together_from_its_chunks_in_order() {
        let data: Vec<u8> = (0..10u8).collect();
        let chunk = |offset: u64, max| Chunk::of(&data, offset, max);
        let mut incoming = Incoming::default();
        assert_eq!(incoming.add(&chunk(0, 4)), None);
        assert_eq!(incoming.add(&chunk(4, 4)), None);
        assert_eq!(incoming.add(&chunk(8, 4)), Some(data.clone()));
        // In one chunk.
        assert_eq!(
            Incoming::default().add(&chunk(0, CHUNK_BYTES)),
            Some(data.clone())
        );
        // Offset 0 starts it anew, after a set the helper didn't finish.
        let mut incoming = Incoming::default();
        incoming.add(&chunk(0, 4));
        assert_eq!(incoming.add(&chunk(0, 6)), None);
        assert_eq!(incoming.add(&chunk(6, 6)), Some(data.clone()));
    }

    #[test]
    fn a_gap_bad_hex_or_too_much_drops_what_came_so_far() {
        let data: Vec<u8> = (0..10u8).collect();
        let chunk = |offset: u64, max| Chunk::of(&data, offset, max);
        // A gap: the rest never completes it.
        let mut incoming = Incoming::default();
        incoming.add(&chunk(0, 4));
        assert_eq!(incoming.add(&chunk(8, 4)), None);
        assert_eq!(incoming.add(&chunk(4, 4)), None);
        // Bad hex.
        let mut incoming = Incoming::default();
        incoming.add(&chunk(0, 4));
        let bad = Chunk {
            total: 10,
            offset: 4,
            hex: "zz".into(),
        };
        assert_eq!(incoming.add(&bad), None);
        assert_eq!(incoming.add(&chunk(4, 6)), None);
        // More than resume data may be.
        let huge = Chunk {
            total: MAX_RESUME_BYTES as u64 + 1,
            offset: 0,
            hex: "00".into(),
        };
        let mut incoming = Incoming::default();
        assert_eq!(incoming.add(&huge), None);
        assert!(incoming.bytes.is_empty());
        // Bytes beyond the total it announced.
        let over = Chunk {
            total: 1,
            offset: 0,
            hex: "0000".into(),
        };
        assert_eq!(Incoming::default().add(&over), None);
    }
}
