//! The torrent helper as a child process (ADR-0014): [`LtEngine`] behind the torrent contract,
//! reached over the child's stdin and stdout with `velocity_child`'s protocol. The child gets one
//! argument, `serve`, and a cleared environment, as the direct-download engine does.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use velocity_child::client::{ChildProcess, SpawnOptions};
use velocity_child::serve::Server;
use velocity_core::domain::ErrorCode;
use velocity_core::seeding::SeedPolicy;
use velocity_core::torrent_engine::{
    dispatch, MetadataChunk, TorrentCommand, TorrentEngine, TorrentEvent, TorrentId, TorrentReply,
    TorrentSettings, TorrentSnapshot, TorrentSpec,
};

use crate::engine::LtEngine;
use crate::TorrentContract;

/// The engine behind the protocol, in the child.
struct TorrentServer(LtEngine);

impl Server<TorrentContract> for TorrentServer {
    fn handle(&mut self, command: TorrentCommand) -> Result<TorrentReply, ErrorCode> {
        dispatch(&mut self.0, command)
    }

    fn subscribe(&mut self) -> Receiver<TorrentEvent> {
        self.0.subscribe()
    }

    /// The session closes when the process exits; libtorrent's own files are left as they are
    /// for the next session to check.
    fn shut_down(&mut self) {}
}

/// How a helper starts: every kind of peer discovery off, so nothing reaches the network until
/// the app sends its settings, which the supervisor does right after the start
/// ([`crate::supervisor::TorrentBook`]).
pub const STARTING_SETTINGS: TorrentSettings = TorrentSettings {
    dht: false,
    peer_exchange: false,
    local_peer_discovery: false,
    max_active_seeds: 0,
};

/// Runs the child's side: serves the torrent engine on stdin and stdout until stdin closes.
pub fn serve() -> Result<(), String> {
    velocity_child::serve::serve::<TorrentContract, _>(
        std::io::stdin().lock(),
        std::io::stdout(),
        |first| LtEngine::start(first, STARTING_SETTINGS).map(TorrentServer),
    )
}

/// A [`TorrentEngine`] client for the torrent helper running as a child process. Its methods take
/// `&self` as well, so a supervisor can share one helper between threads.
pub struct ChildTorrentEngine {
    process: ChildProcess<TorrentContract>,
}

impl ChildTorrentEngine {
    /// Starts `<program> serve` with the default options.
    pub fn spawn(program: &Path) -> Result<Self, ErrorCode> {
        Self::spawn_with(program, &SpawnOptions::default())
    }

    /// Starts `<program> serve` and completes the handshake.
    pub fn spawn_with(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode> {
        Ok(ChildTorrentEngine {
            process: ChildProcess::spawn(program, options)?,
        })
    }

    /// Asks the helper to stop by closing its stdin. Commands fail with `EngineUnavailable` from
    /// then on.
    pub fn stop(&self) {
        self.process.stop();
    }

    pub fn answers_within(&self, timeout: Duration) -> bool {
        self.process.answers_within(timeout)
    }

    pub fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    pub fn version(&self) -> Result<String, ErrorCode> {
        match self.process.request(TorrentCommand::Version {})? {
            TorrentReply::Version { libtorrent } => Ok(libtorrent),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    pub fn set_settings(&self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        self.done(TorrentCommand::SetSettings { settings })
    }

    pub fn add(&self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        match self.process.request(TorrentCommand::Add { spec })? {
            TorrentReply::Added { id } => Ok(id),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    pub fn pause(&self, id: TorrentId) -> Result<(), ErrorCode> {
        self.done(TorrentCommand::Pause { id })
    }

    pub fn resume(&self, id: TorrentId) -> Result<(), ErrorCode> {
        self.done(TorrentCommand::Resume { id })
    }

    pub fn remove(&self, id: TorrentId) -> Result<(), ErrorCode> {
        self.done(TorrentCommand::Remove { id })
    }

    pub fn set_files(&self, id: TorrentId, files: Vec<u32>) -> Result<(), ErrorCode> {
        self.done(TorrentCommand::SetFiles { id, files })
    }

    pub fn metadata(&self, id: TorrentId, offset: u64) -> Result<MetadataChunk, ErrorCode> {
        match self
            .process
            .request(TorrentCommand::Metadata { id, offset })?
        {
            TorrentReply::Metadata { chunk } => Ok(chunk),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    pub fn set_seeding(&self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        self.done(TorrentCommand::SetSeeding { id, seeding })
    }

    pub fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode> {
        match self.process.request(TorrentCommand::Snapshot {})? {
            TorrentReply::Snapshot { torrents } => Ok(torrents),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }

    /// A receiver for the helper's events. It disconnects when the helper's stdout ends.
    pub fn subscribe(&self) -> Receiver<TorrentEvent> {
        self.process.subscribe()
    }

    fn done(&self, command: TorrentCommand) -> Result<(), ErrorCode> {
        match self.process.request(command)? {
            TorrentReply::Done {} => Ok(()),
            _ => Err(ErrorCode::EngineIncompatible),
        }
    }
}

impl TorrentEngine for ChildTorrentEngine {
    fn version(&self) -> Result<String, ErrorCode> {
        ChildTorrentEngine::version(self)
    }

    fn set_settings(&mut self, settings: TorrentSettings) -> Result<(), ErrorCode> {
        ChildTorrentEngine::set_settings(self, settings)
    }

    fn add(&mut self, spec: TorrentSpec) -> Result<TorrentId, ErrorCode> {
        ChildTorrentEngine::add(self, spec)
    }

    fn pause(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        ChildTorrentEngine::pause(self, id)
    }

    fn resume(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        ChildTorrentEngine::resume(self, id)
    }

    fn remove(&mut self, id: TorrentId) -> Result<(), ErrorCode> {
        ChildTorrentEngine::remove(self, id)
    }

    fn set_files(&mut self, id: TorrentId, files: Vec<u32>) -> Result<(), ErrorCode> {
        ChildTorrentEngine::set_files(self, id, files)
    }

    fn metadata(&self, id: TorrentId, offset: u64) -> Result<MetadataChunk, ErrorCode> {
        ChildTorrentEngine::metadata(self, id, offset)
    }

    fn set_seeding(&mut self, id: TorrentId, seeding: SeedPolicy) -> Result<(), ErrorCode> {
        ChildTorrentEngine::set_seeding(self, id, seeding)
    }

    fn snapshot(&self) -> Result<Vec<TorrentSnapshot>, ErrorCode> {
        ChildTorrentEngine::snapshot(self)
    }

    fn subscribe(&mut self) -> Receiver<TorrentEvent> {
        ChildTorrentEngine::subscribe(self)
    }
}

impl velocity_child::supervisor::Child for ChildTorrentEngine {
    type Event = TorrentEvent;
    const NAME: &'static str = "torrent";

    fn spawn(program: &Path, options: &SpawnOptions) -> Result<Self, ErrorCode> {
        ChildTorrentEngine::spawn_with(program, options)
    }

    fn subscribe(&self) -> Receiver<TorrentEvent> {
        ChildTorrentEngine::subscribe(self)
    }

    fn answers_within(&self, timeout: Duration) -> bool {
        ChildTorrentEngine::answers_within(self, timeout)
    }

    fn stop(&self) {
        ChildTorrentEngine::stop(self)
    }

    fn process_id(&self) -> u32 {
        ChildTorrentEngine::process_id(self)
    }
}
