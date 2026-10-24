//! A deterministic fake `DirectTransfer` engine for manager tests (#36). Nothing happens on its
//! own: commands change state as the contract says, and the test scripts everything else
//! (progress, failures, completion) through a [`FakeControl`]. Compiled for tests only.

use super::{
    DirectTransfer, EngineLimits, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};
use crate::domain::{
    Capabilities, DownloadStatus, ErrorCode, Progress, TransferError, Verification,
};
use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Default)]
struct State {
    capabilities: Option<Capabilities>,
    next_id: u64,
    transfers: BTreeMap<TransferId, Transfer>,
    subscribers: Vec<Sender<TransferEvent>>,
    /// Every command received, in order, for assertions.
    log: Vec<String>,
    /// The limits from the last `set_global_limits`.
    global_limits: Limits,
    /// The limits from the last `set_engine_limits`.
    engine_limits: EngineLimits,
    /// Makes every command fail with this code, like an engine that stopped responding.
    unavailable: Option<ErrorCode>,
    /// Makes `add` refuse a transfer with this file name, as an engine refuses a name taken.
    refused_name: Option<(String, ErrorCode)>,
}

struct Transfer {
    spec: TransferSpec,
    snapshot: TransferSnapshot,
}

impl State {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
            .clone()
            .unwrap_or_else(Capabilities::sample)
    }

    fn emit(&mut self, event: TransferEvent) {
        self.subscribers.retain(|s| s.send(event.clone()).is_ok());
    }

    fn changed(&mut self, id: TransferId) {
        let snapshot = self.transfers[&id].snapshot.clone();
        self.emit(TransferEvent::Changed { snapshot });
    }

    fn command(&mut self, entry: String) -> Result<(), ErrorCode> {
        self.log.push(entry);
        self.unavailable.map_or(Ok(()), Err)
    }

    fn transfer(&mut self, id: TransferId) -> Result<&mut Transfer, ErrorCode> {
        self.transfers.get_mut(&id).ok_or(ErrorCode::NotFound)
    }

    /// Moves a transfer to `to` if its current status is one of `from`.
    fn transition(
        &mut self,
        id: TransferId,
        from: &[DownloadStatus],
        to: DownloadStatus,
    ) -> Result<(), ErrorCode> {
        let t = self.transfer(id)?;
        if !from.contains(&t.snapshot.status) {
            return Err(ErrorCode::InvalidState);
        }
        t.snapshot.status = to;
        if to != DownloadStatus::Downloading {
            t.snapshot.progress.download_speed_bps = 0;
            t.snapshot.progress.connections = 0;
        }
        self.changed(id);
        Ok(())
    }
}

/// Statuses in which a transfer is unfinished and may be paused or cancelled.
const ACTIVE: [DownloadStatus; 6] = [
    DownloadStatus::Queued,
    DownloadStatus::Connecting,
    DownloadStatus::Downloading,
    DownloadStatus::Retrying,
    DownloadStatus::Stalled,
    DownloadStatus::Verifying,
];

/// The fake engine. Hand it to the code under test; keep a [`FakeControl`] to script it.
pub struct FakeTransfer {
    state: Arc<Mutex<State>>,
}

/// The test's handle on a [`FakeTransfer`] it has given away.
#[derive(Clone)]
pub struct FakeControl {
    state: Arc<Mutex<State>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().expect("fake engine state is not poisoned")
}

impl FakeTransfer {
    pub fn new() -> Self {
        Self {
            state: Arc::default(),
        }
    }

    pub fn with_capabilities(capabilities: Capabilities) -> Self {
        let fake = Self::new();
        lock(&fake.state).capabilities = Some(capabilities);
        fake
    }

    pub fn control(&self) -> FakeControl {
        FakeControl {
            state: Arc::clone(&self.state),
        }
    }
}

impl FakeControl {
    /// The commands the engine received, such as `"pause 1"`.
    pub fn log(&self) -> Vec<String> {
        lock(&self.state).log.clone()
    }

    pub fn spec(&self, id: TransferId) -> TransferSpec {
        lock(&self.state).transfers[&id].spec.clone()
    }

    pub fn limits(&self, id: TransferId) -> Limits {
        self.spec(id).limits
    }

    pub fn global_limits(&self) -> Limits {
        lock(&self.state).global_limits
    }

    pub fn engine_limits(&self) -> EngineLimits {
        lock(&self.state).engine_limits
    }

    /// Reports bytes arriving: the transfer is Downloading with this progress.
    pub fn progress(&self, id: TransferId, completed_bytes: u64, total_bytes: Option<u64>) {
        let mut s = lock(&self.state);
        let connections = s.transfers[&id].spec.max_connections;
        let t = s.transfers.get_mut(&id).expect("known transfer");
        t.snapshot.status = DownloadStatus::Downloading;
        t.snapshot.progress = Progress {
            completed_bytes,
            total_bytes,
            download_speed_bps: 500_000,
            upload_speed_bps: 0,
            connections,
            connection_detail: None,
        };
        s.changed(id);
    }

    /// Reports any status, including ones a direct engine must never send.
    pub fn status(&self, id: TransferId, status: DownloadStatus) {
        let mut s = lock(&self.state);
        s.transfers
            .get_mut(&id)
            .expect("known transfer")
            .snapshot
            .status = status;
        s.changed(id);
    }

    pub fn fail(&self, id: TransferId, error: TransferError) {
        let mut s = lock(&self.state);
        let t = s.transfers.get_mut(&id).expect("known transfer");
        t.snapshot.status = DownloadStatus::Failed;
        t.snapshot.progress.download_speed_bps = 0;
        t.snapshot.progress.connections = 0;
        t.snapshot.error = Some(error);
        s.changed(id);
    }

    pub fn complete(&self, id: TransferId) {
        let mut s = lock(&self.state);
        let t = s.transfers.get_mut(&id).expect("known transfer");
        t.snapshot.status = DownloadStatus::Completed;
        // What the engine reports for a complete file without a checksum.
        t.snapshot.verification = Some(Verification::SizeVerified);
        let total = t.snapshot.progress.total_bytes.unwrap_or(0);
        t.snapshot.progress = Progress {
            completed_bytes: total,
            total_bytes: Some(total),
            download_speed_bps: 0,
            upload_speed_bps: 0,
            connections: 0,
            connection_detail: None,
        };
        s.changed(id);
    }

    /// The transfer's snapshot as the fake holds it now.
    pub fn snapshot(&self, id: TransferId) -> TransferSnapshot {
        lock(&self.state).transfers[&id].snapshot.clone()
    }

    /// Reports the name the file is saved under, as an engine does once it knows it (#40).
    pub fn named(&self, id: TransferId, file_name: &str) {
        let mut s = lock(&self.state);
        let t = s.transfers.get_mut(&id).expect("known transfer");
        t.snapshot.file_name = Some(file_name.to_owned());
        s.changed(id);
    }

    /// Sends an arbitrary event, for example one about a transfer that doesn't exist.
    pub fn emit(&self, event: TransferEvent) {
        lock(&self.state).emit(event);
    }

    /// From now on `add` refuses a transfer named `name` with `code`.
    pub fn refuse_file_name(&self, name: &str, code: ErrorCode) {
        lock(&self.state).refused_name = Some((name.to_owned(), code));
    }

    /// From now on every command fails with `code`; `None` makes the engine respond again.
    pub fn set_unavailable(&self, code: Option<ErrorCode>) {
        lock(&self.state).unavailable = code;
    }
}

impl DirectTransfer for FakeTransfer {
    fn capabilities(&self) -> Capabilities {
        lock(&self.state).capabilities()
    }

    fn add(&mut self, spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        let mut s = lock(&self.state);
        s.command("add".to_owned())?;
        if spec.urls.is_empty() {
            return Err(ErrorCode::InvalidInput);
        }
        if let Some((name, code)) = &s.refused_name {
            if spec.file_name.as_ref() == Some(name) {
                return Err(*code);
            }
        }
        s.next_id += 1;
        let id = TransferId(s.next_id);
        let snapshot = TransferSnapshot {
            id,
            status: DownloadStatus::Queued,
            progress: Progress {
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
            file_name: spec.file_name.clone(),
        };
        s.transfers.insert(id, Transfer { spec, snapshot });
        s.changed(id);
        Ok(id)
    }

    fn pause(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("pause {}", id.0))?;
        s.transition(id, &ACTIVE, DownloadStatus::Paused)
    }

    fn resume(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("resume {}", id.0))?;
        s.transition(id, &[DownloadStatus::Paused], DownloadStatus::Queued)
    }

    fn cancel(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("cancel {}", id.0))?;
        let mut from = ACTIVE.to_vec();
        from.push(DownloadStatus::Paused);
        s.transition(id, &from, DownloadStatus::Removed)
    }

    fn remove(&mut self, id: TransferId) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("remove {}", id.0))?;
        if !s.transfer(id)?.snapshot.status.is_terminal() {
            return Err(ErrorCode::InvalidState);
        }
        s.transfers.remove(&id);
        s.emit(TransferEvent::Removed { id });
        Ok(())
    }

    fn set_limits(&mut self, id: TransferId, limits: Limits) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("set_limits {}", id.0))?;
        limits.check()?;
        s.transfer(id)?.spec.limits = limits;
        Ok(())
    }

    fn set_global_limits(&mut self, limits: Limits) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command("set_global_limits".to_owned())?;
        limits.check()?;
        s.global_limits = limits;
        Ok(())
    }

    fn set_engine_limits(&mut self, limits: EngineLimits) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command("set_engine_limits".to_owned())?;
        limits.check()?;
        s.engine_limits = limits;
        Ok(())
    }

    fn set_connections(&mut self, id: TransferId, connections: u32) -> Result<u32, ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("set_connections {}", id.0))?;
        let applied = connections.clamp(1, s.capabilities().max_connections_per_host.max(1));
        s.transfer(id)?.spec.max_connections = applied;
        Ok(applied)
    }

    fn trust_host_key(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        let mut s = lock(&self.state);
        s.command(format!("trust_host_key {host}:{port} {fingerprint}"))
    }

    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        let mut s = lock(&self.state);
        s.command("snapshot".to_owned())?;
        Ok(s.transfers.values().map(|t| t.snapshot.clone()).collect())
    }

    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        lock(&self.state).subscribers.push(tx);
        rx
    }
}
