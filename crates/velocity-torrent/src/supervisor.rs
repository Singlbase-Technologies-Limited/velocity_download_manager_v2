//! The torrent helper's supervisor (ADR-0014): `velocity_child`'s supervisor, with what it keeps
//! for torrents ([`TorrentBook`]). A crashed or hung helper is restarted with the engine's backoff
//! and cap; the torrents it had fail with a retryable error, then the new helper gets them back
//! under their own ids (PO decision on #62): from each one's latest resume data, which the
//! supervisor puts together from the helper's chunks, or else from its link or file. One that
//! can't be added back stays failed. Every new helper gets the settings last set.

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
use velocity_core::proxy::Proxy;
use velocity_core::seeding::SeedPolicy;
use velocity_core::torrent::MAX_RESUME_BYTES;
use velocity_core::torrent_engine::{
    Chunk, NetworkStatus, PeerStatus, TorrentEngine, TorrentEvent, TorrentId, TorrentSettings,
    TorrentSnapshot, TorrentSource, TorrentSpec, TrackerStatus, CHUNK_BYTES,
};

use crate::stdio::ChildTorrentEngine;

/// What a lost torrent fails with: after a crash, after a hang, and after a restart by hand. The
/// core defines them, so the manager knows a lost torrent from a failed one (#58).
use velocity_core::torrent_engine::{
    HELPER_HUNG as HUNG_MESSAGE, HELPER_RESTARTED as RESTART_MESSAGE,
    HELPER_STOPPED as CRASH_MESSAGE,
};

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

/// A torrent a new helper gets back: its id, how it was added, as changed since, and its latest
/// resume data.
pub struct Lost {
    id: TorrentId,
    spec: TorrentSpec,
    resume: Option<Vec<u8>>,
}

/// What the supervisor keeps for the torrent helper, across helpers.
pub struct TorrentBook {
    torrents: Ledger<TorrentSnapshot>,
    /// The settings last set, given to every new helper.
    settings: TorrentSettings,
    /// The proxy last set, given to every new helper before its settings (#192), so nothing it
    /// does ever goes around the proxy.
    proxy: Option<Proxy>,
    incoming: BTreeMap<TorrentId, Incoming>,
    resume: BTreeMap<TorrentId, Saved>,
    /// How each torrent was added, with the choice, pause and policy set since: what a new
    /// helper adds it back with when there is no resume data.
    specs: BTreeMap<TorrentId, TorrentSpec>,
}

impl TorrentBook {
    fn forget(&mut self, id: TorrentId) {
        self.incoming.remove(&id);
        self.resume.remove(&id);
        self.specs.remove(&id);
    }
}

impl Book<ChildTorrentEngine> for TorrentBook {
    type Setup = (Option<Proxy>, TorrentSettings, Vec<Lost>);

    /// The proxy, the settings, and the torrents earlier helpers lost.
    fn setup(&self) -> (Option<Proxy>, TorrentSettings, Vec<Lost>) {
        let lost = self
            .torrents
            .orphans()
            .into_iter()
            .filter_map(|orphan| {
                let spec = self.specs.get(&orphan.id)?.clone();
                let resume = self.resume.get(&orphan.id).map(|s| s.bytes.clone());
                Some(Lost {
                    id: orphan.id,
                    spec,
                    resume,
                })
            })
            .collect();
        (self.proxy.clone(), self.settings.clone(), lost)
    }

    /// Gives the helper the proxy, then the settings, then the lost torrents. One the helper
    /// refuses stays failed; a helper that stops answering fails the start.
    fn prepare(
        helper: &ChildTorrentEngine,
        (proxy, settings, lost): (Option<Proxy>, TorrentSettings, Vec<Lost>),
    ) -> Result<(), ErrorCode> {
        if proxy.is_some() {
            helper.set_proxy(proxy)?;
        }
        helper.set_settings(settings)?;
        for lost in lost {
            // A torrent the helper refuses stays failed, as the ledger has it.
            if let Err(ErrorCode::EngineUnavailable) = add_back(helper, lost) {
                return Err(ErrorCode::EngineUnavailable);
            }
        }
        Ok(())
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
                self.forget(*id);
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

/// Adds a lost torrent back to `helper` under its id: from its resume data when there is some,
/// with the choice it holds, otherwise as it was added.
fn add_back(helper: &ChildTorrentEngine, lost: Lost) -> Result<(), ErrorCode> {
    let Some(resume) = lost.resume else {
        return helper.restore(lost.id, lost.spec);
    };
    let mut offset = 0;
    while offset < resume.len() {
        helper.upload(Chunk::of(&resume, offset as u64, CHUNK_BYTES))?;
        offset += CHUNK_BYTES;
    }
    let spec = TorrentSpec {
        source: TorrentSource::Resume {},
        choose_files: false,
        files: None,
        ..lost.spec
    };
    helper.restore(lost.id, spec)
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
        Self::start_with_proxy(program, options, settings, None)
    }

    /// [`Self::start`] with the user's proxy, which the first helper gets before its settings
    /// (#192): no peer or tracker connection is made before it.
    pub fn start_with_proxy(
        program: PathBuf,
        options: SupervisorOptions,
        settings: TorrentSettings,
        proxy: Option<Proxy>,
    ) -> Result<Self, ErrorCode> {
        if let Some(proxy) = &proxy {
            proxy.check()?;
        }
        let book = TorrentBook {
            torrents: Ledger::default(),
            settings,
            proxy,
            incoming: BTreeMap::new(),
            resume: BTreeMap::new(),
            specs: BTreeMap::new(),
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

    /// Changes how `id` is added back after a crash.
    fn keep(&self, id: TorrentId, change: impl FnOnce(&mut TorrentSpec)) {
        self.supervisor.update(|book, _| {
            if let Some(spec) = book.specs.get_mut(&id) {
                change(spec);
            }
            ((), Vec::new())
        });
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

    /// Kept first, so a helper that isn't running gets it when it starts; then given to the
    /// running one.
    fn set_proxy(&mut self, proxy: Option<Proxy>) -> Result<(), ErrorCode> {
        if let Some(proxy) = &proxy {
            proxy.check()?;
        }
        let kept = proxy.clone();
        self.supervisor.update(|book, _| {
            book.proxy = kept;
            ((), Vec::new())
        });
        self.helper()?.set_proxy(proxy)
    }

    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        self.helper()?.set_settings(settings.clone())?;
        self.supervisor.update(|book, _| {
            book.settings = settings;
            ((), Vec::new())
        });
        Ok(())
    }

    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        let (helper, generation) = self.supervisor.child()?;
        let kept = spec.clone();
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
            book.specs.insert(id, kept);
            // When the helper stopped between assigning the id and this reply, the torrent is lost.
            let lost = book
                .torrents
                .added(current == generation, queued, CRASH_MESSAGE);
            ((), changed(lost.into_iter().collect()))
        });
        Ok(id)
    }

    /// Gives the running helper a torrent under `id`; the supervisor does this itself after a
    /// crash, so the app has no need to.
    fn restore(&mut self, id: TorrentId, spec: TorrentSpec) -> Result<(), ErrorCode> {
        let kept = spec.clone();
        self.helper()?.restore(id, spec)?;
        self.supervisor.update(|book, _| {
            book.specs.insert(id, kept);
            ((), Vec::new())
        });
        Ok(())
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        self.running(id)?.pause(id)?;
        self.keep(id, |spec| spec.paused = true);
        Ok(())
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        self.running(id)?.resume(id)?;
        self.keep(id, |spec| spec.paused = false);
        Ok(())
    }

    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let orphan = self.supervisor.update(|book, _| {
            if book.torrents.remove_orphan(id) {
                book.forget(id);
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
        self.running(id)?.set_files(id, files.clone())?;
        // A `.torrent` file is added back with its choice; a magnet's is in its resume data.
        self.keep(id, |spec| {
            if let TorrentSource::File { .. } = spec.source {
                spec.files = Some(files);
                spec.choose_files = false;
            }
        });
        Ok(())
    }

    fn metadata(&self, id: TorrentId, offset: u64) -> Result<Chunk, ErrorCode> {
        self.running(id)?.metadata(id, offset)
    }

    fn set_seeding(&mut self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        self.running(id)?.set_seeding(id, seeding)?;
        self.keep(id, |spec| spec.seeding = seeding);
        Ok(())
    }

    fn upload(&mut self, chunk: Chunk) -> Result<(), ErrorCode> {
        self.helper()?.upload(chunk)
    }

    fn save_resume(&mut self) -> Result<Vec<TorrentId>, ErrorCode> {
        self.helper()?.save_resume()
    }

    fn set_limits(&mut self, id: TorrentId, upload: u64, download: u64) -> Result<(), ErrorCode> {
        self.running(id)?.set_limits(id, upload, download)?;
        self.keep(id, |spec| {
            spec.upload_limit = upload;
            spec.download_limit = download;
        });
        Ok(())
    }

    fn trackers(&self, id: TorrentId) -> Result<Vec<TrackerStatus>, ErrorCode> {
        self.running(id)?.trackers(id)
    }

    fn reannounce(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        self.running(id)?.reannounce(id)
    }

    fn peers(&self, id: TorrentId) -> Result<Vec<PeerStatus>, ErrorCode> {
        self.running(id)?.peers(id)
    }

    fn network(&self) -> Result<NetworkStatus, ErrorCode> {
        self.helper()?.network()
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

    fn health(&self) -> EngineHealth {
        self.supervisor.health()
    }

    fn restart(&mut self) -> Result<(), ErrorCode> {
        self.supervisor.restart();
        Ok(())
    }

    fn recent_log(&self) -> Vec<String> {
        self.log()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
