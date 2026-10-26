//! A deterministic fake [`TorrentEngine`] for manager tests (#58), like the direct engine's
//! (`transfer::fake`). Nothing happens on its own: commands change state as the contract says,
//! and the test scripts everything else through a [`FakeTorrentControl`]. Compiled for tests only.

use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{
    Chunk, NetworkStatus, PeerStatus, PortMapState, TorrentEngine, TorrentEvent, TorrentId,
    TorrentSettings, TorrentSnapshot, TorrentSource, TorrentSpec, TrackerStatus, CHUNK_BYTES,
};
use crate::domain::{DownloadStatus, EngineHealth, ErrorCode, Progress, TransferError};
use crate::seeding::SeedPolicy;

struct Torrent {
    spec: TorrentSpec,
    snapshot: TorrentSnapshot,
    metadata: Option<Vec<u8>>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    torrents: BTreeMap<TorrentId, Torrent>,
    subscribers: Vec<Sender<TorrentEvent>>,
    /// Every command received, in order, for assertions.
    log: Vec<String>,
    /// The resume data uploaded for the next `Resume` add.
    uploaded: Vec<u8>,
    unavailable: Option<ErrorCode>,
    health: Option<EngineHealth>,
}

impl State {
    fn emit(&mut self, event: TorrentEvent) {
        self.subscribers.retain(|s| s.send(event.clone()).is_ok());
    }

    fn changed(&mut self, id: TorrentId) {
        let snapshot = Box::new(self.torrents[&id].snapshot.clone());
        self.emit(TorrentEvent::Changed { snapshot });
    }

    fn command(&mut self, entry: String) -> Result<(), ErrorCode> {
        self.log.push(entry);
        self.unavailable.map_or(Ok(()), Err)
    }

    fn torrent(&mut self, id: TorrentId) -> Result<&mut Torrent, ErrorCode> {
        self.torrents.get_mut(&id).ok_or(ErrorCode::NotFound)
    }

    fn set_status(&mut self, id: TorrentId, status: DownloadStatus) {
        if let Some(t) = self.torrents.get_mut(&id) {
            t.snapshot.status = status;
            self.changed(id);
        }
    }
}

/// The fake engine. Hand it to the code under test; keep a [`FakeTorrentControl`] to script it.
pub struct FakeTorrentEngine {
    state: Arc<Mutex<State>>,
}

/// The test's handle on a [`FakeTorrentEngine`] it has given away.
#[derive(Clone)]
pub struct FakeTorrentControl {
    state: Arc<Mutex<State>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .expect("fake torrent engine state is not poisoned")
}

impl FakeTorrentEngine {
    pub fn new() -> Self {
        Self {
            state: Arc::default(),
        }
    }

    pub fn control(&self) -> FakeTorrentControl {
        FakeTorrentControl {
            state: Arc::clone(&self.state),
        }
    }
}

impl FakeTorrentControl {
    /// The commands the engine received, such as `"pause 1"`.
    pub fn log(&self) -> Vec<String> {
        lock(&self.state).log.clone()
    }

    pub fn spec(&self, id: TorrentId) -> TorrentSpec {
        lock(&self.state).torrents[&id].spec.clone()
    }

    /// The resume data the last `Resume` add was given.
    pub fn uploaded(&self) -> Vec<u8> {
        lock(&self.state).uploaded.clone()
    }

    /// The torrent's metadata is known: its name, hashes and size, and `metadata` for
    /// [`TorrentEngine::metadata`].
    pub fn metadata_known(
        &self,
        id: TorrentId,
        name: &str,
        info_hash: &str,
        total: u64,
        metadata: Vec<u8>,
    ) {
        let mut s = lock(&self.state);
        let t = s.torrents.get_mut(&id).expect("known torrent");
        t.snapshot.name = Some(name.to_owned());
        t.snapshot.info_hash_v1 = Some(info_hash.to_owned());
        t.snapshot.progress.total_bytes = Some(total);
        t.metadata = Some(metadata);
        if t.snapshot.status == DownloadStatus::FetchingMetadata {
            t.snapshot.status = if t.spec.choose_files {
                DownloadStatus::ChoosingFiles
            } else {
                DownloadStatus::Downloading
            };
        }
        s.changed(id);
    }

    pub fn progress(&self, id: TorrentId, completed_bytes: u64) {
        let mut s = lock(&self.state);
        let t = s.torrents.get_mut(&id).expect("known torrent");
        t.snapshot.status = DownloadStatus::Downloading;
        t.snapshot.progress.completed_bytes = completed_bytes;
        t.snapshot.error = None;
        s.changed(id);
    }

    /// Complete; seeding if its policy allows, otherwise Completed.
    pub fn complete(&self, id: TorrentId) {
        let mut s = lock(&self.state);
        let t = s.torrents.get_mut(&id).expect("known torrent");
        let total = t.snapshot.progress.total_bytes.unwrap_or(0);
        t.snapshot.progress.completed_bytes = total;
        t.snapshot.status = if t.spec.seeding == (SeedPolicy::Never {}) {
            DownloadStatus::Completed
        } else {
            DownloadStatus::Seeding
        };
        s.changed(id);
    }

    pub fn fail(&self, id: TorrentId, error: TransferError) {
        let mut s = lock(&self.state);
        let t = s.torrents.get_mut(&id).expect("known torrent");
        t.snapshot.status = DownloadStatus::Failed;
        t.snapshot.error = Some(error);
        s.changed(id);
    }

    /// Sends `bytes` as the torrent's resume data, in chunks as the helper does.
    pub fn resume_data(&self, id: TorrentId, bytes: &[u8]) {
        let mut s = lock(&self.state);
        let mut offset = 0;
        loop {
            let chunk = Chunk::of(bytes, offset as u64, CHUNK_BYTES);
            let len = chunk.hex.len() / 2;
            s.emit(TorrentEvent::ResumeData { id, chunk });
            offset += len;
            if offset >= bytes.len() {
                break;
            }
        }
    }

    pub fn set_health(&self, health: EngineHealth) {
        lock(&self.state).health = Some(health);
    }
}

impl TorrentEngine for FakeTorrentEngine {
    fn version(&self) -> Result<String, ErrorCode> {
        Ok("fake".to_owned())
    }

    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command("set_settings".to_owned())?;
        settings.check()
    }

    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        let mut s = lock(&self.state);
        s.command("add".to_owned())?;
        s.next_id += 1;
        let id = TorrentId(s.next_id);
        let status = match (&spec.source, spec.paused, spec.choose_files) {
            (_, true, _) => DownloadStatus::Paused,
            (TorrentSource::Magnet { .. }, _, _) => DownloadStatus::FetchingMetadata,
            (_, _, true) => DownloadStatus::ChoosingFiles,
            _ => DownloadStatus::Queued,
        };
        let snapshot = TorrentSnapshot {
            id,
            status,
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
        s.torrents.insert(
            id,
            Torrent {
                spec,
                snapshot,
                metadata: None,
            },
        );
        s.changed(id);
        Ok(id)
    }

    fn restore(&mut self, id: TorrentId, spec: TorrentSpec) -> Result<(), ErrorCode> {
        lock(&self.state).command(format!("restore {}", id.0))?;
        let _ = spec;
        Err(ErrorCode::Unsupported)
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("pause {}", id.0))?;
        s.torrent(id)?;
        s.set_status(id, DownloadStatus::Paused);
        Ok(())
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("resume {}", id.0))?;
        s.torrent(id)?;
        s.set_status(id, DownloadStatus::Downloading);
        Ok(())
    }

    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("remove {}", id.0))?;
        s.torrents.remove(&id).ok_or(ErrorCode::NotFound)?;
        s.emit(TorrentEvent::Removed { id });
        Ok(())
    }

    fn set_files(&mut self, id: TorrentId, files: Vec<u32>) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("set_files {} {files:?}", id.0))?;
        let t = s.torrent(id)?;
        if t.metadata.is_none() {
            return Err(ErrorCode::InvalidState);
        }
        if files.is_empty() {
            return Err(ErrorCode::InvalidInput);
        }
        t.spec.files = Some(files);
        t.snapshot.status = DownloadStatus::Downloading;
        s.changed(id);
        Ok(())
    }

    fn metadata(&self, id: TorrentId, offset: u64) -> Result<Chunk, ErrorCode> {
        let s = lock(&self.state);
        let t = s.torrents.get(&id).ok_or(ErrorCode::NotFound)?;
        let metadata = t.metadata.as_ref().ok_or(ErrorCode::InvalidState)?;
        Ok(Chunk::of(metadata, offset, CHUNK_BYTES))
    }

    fn set_seeding(&mut self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("set_seeding {}", id.0))?;
        seeding.check()?;
        s.torrent(id)?.spec.seeding = seeding;
        Ok(())
    }

    fn upload(&mut self, chunk: Chunk) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("upload {}", chunk.offset))?;
        if chunk.offset == 0 {
            s.uploaded.clear();
        }
        let bytes = chunk.bytes()?;
        s.uploaded.extend(bytes);
        Ok(())
    }

    fn save_resume(&mut self) -> Result<Vec<TorrentId>, ErrorCode> {
        let mut s = lock(&self.state);
        s.command("save_resume".to_owned())?;
        Ok(s.torrents.keys().copied().collect())
    }

    fn set_limits(&mut self, id: TorrentId, upload: u64, download: u64) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("set_limits {} {upload} {download}", id.0))?;
        let t = s.torrent(id)?;
        t.spec.upload_limit = upload;
        t.spec.download_limit = download;
        Ok(())
    }

    fn trackers(&self, id: TorrentId) -> Result<Vec<TrackerStatus>, ErrorCode> {
        let s = lock(&self.state);
        s.torrents.get(&id).ok_or(ErrorCode::NotFound)?;
        Ok(Vec::new())
    }

    fn reannounce(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("reannounce {}", id.0))?;
        s.torrent(id)?;
        Ok(())
    }

    fn peers(&self, id: TorrentId) -> Result<Vec<PeerStatus>, ErrorCode> {
        let s = lock(&self.state);
        s.torrents.get(&id).ok_or(ErrorCode::NotFound)?;
        Ok(Vec::new())
    }

    fn network(&self) -> Result<NetworkStatus, ErrorCode> {
        Ok(NetworkStatus {
            listen_port: 6890,
            listen_error: None,
            port_mapping: PortMapState::Off {},
        })
    }

    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode> {
        let s = lock(&self.state);
        Ok(s.torrents.values().map(|t| t.snapshot.clone()).collect())
    }

    fn subscribe(&mut self) -> Receiver<TorrentEvent> {
        let (tx, rx) = channel();
        lock(&self.state).subscribers.push(tx);
        rx
    }

    fn health(&self) -> EngineHealth {
        lock(&self.state).health.unwrap_or(EngineHealth::Running)
    }
}
