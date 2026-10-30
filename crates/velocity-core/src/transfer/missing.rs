//! The engine the app runs with when its engine program couldn't be started (#58): it is missing,
//! or doesn't match the app. Nothing can be downloaded, and the app says why and where it looked,
//! instead of failing to open (PO decision on #58: the engines are looked for beside the app).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::{
    DirectTransfer, EngineLimits, EngineSettings, Limits, TransferEvent, TransferId,
    TransferSnapshot, TransferSpec,
};
use crate::domain::{Capabilities, EngineHealth, ErrorCode};

/// No engine: every command is refused as `EngineUnavailable`, and [`DirectTransfer::health`]
/// gives the reason (`EngineMissing` or `EngineIncompatible`).
pub struct MissingEngine {
    code: ErrorCode,
    program: PathBuf,
    /// Kept so the manager's receiver stays open and quiet, rather than disconnected.
    subscribers: Vec<Sender<TransferEvent>>,
}

impl MissingEngine {
    /// No engine, because starting `program` failed with `code`.
    pub fn new(code: ErrorCode, program: PathBuf) -> Self {
        MissingEngine {
            code,
            program,
            subscribers: Vec::new(),
        }
    }

    /// Where the engine program was looked for.
    pub fn program(&self) -> &Path {
        &self.program
    }
}

const UNAVAILABLE: ErrorCode = ErrorCode::EngineUnavailable;

impl DirectTransfer for MissingEngine {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            protocols: Vec::new(),
            live_connection_changes: false,
            metalink: false,
            max_connections_per_host: 1,
            libcurl: None,
        }
    }
    fn add(&mut self, _spec: TransferSpec) -> Result<TransferId, ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn pause(&mut self, _id: TransferId) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn resume(&mut self, _id: TransferId) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn cancel(&mut self, _id: TransferId) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn remove(&mut self, _id: TransferId) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn set_limits(&mut self, _id: TransferId, _limits: Limits) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn set_global_limits(&mut self, _limits: Limits) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn set_engine_limits(&mut self, _limits: EngineLimits) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn set_engine_settings(&mut self, _settings: EngineSettings) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn set_proxy(&mut self, _proxy: Option<crate::proxy::Proxy>) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn set_connections(&mut self, _id: TransferId, _connections: u32) -> Result<u32, ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn trust_host_key(
        &mut self,
        _host: &str,
        _port: u16,
        _fingerprint: &str,
    ) -> Result<(), ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn snapshot(&self) -> Result<Vec<TransferSnapshot>, ErrorCode> {
        Err(UNAVAILABLE)
    }
    fn subscribe(&mut self) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        self.subscribers.push(tx);
        rx
    }
    fn health(&self) -> EngineHealth {
        EngineHealth::Unavailable { code: self.code }
    }
    /// Starting it again can't help: the program is still missing or mismatched.
    fn restart(&mut self) -> Result<(), ErrorCode> {
        Err(self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DownloadKind;
    use crate::transfer::Resume;

    fn missing() -> MissingEngine {
        MissingEngine::new(ErrorCode::EngineMissing, PathBuf::from("velocity-transfer"))
    }

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
            resume: Some(Resume::DiscardPartFile {}),
            trust: Default::default(),
        }
    }

    #[test]
    fn it_reports_why_there_is_no_engine() {
        let e = missing();
        assert_eq!(
            e.health(),
            EngineHealth::Unavailable {
                code: ErrorCode::EngineMissing
            }
        );
        assert!(
            e.capabilities().protocol(DownloadKind::Http).is_none(),
            "it does nothing"
        );
    }

    #[test]
    fn every_command_is_refused_as_unavailable() {
        let mut e = missing();
        let id = TransferId(1);
        let unavailable = Some(ErrorCode::EngineUnavailable);
        assert_eq!(e.add(spec()).err(), unavailable);
        assert_eq!(e.pause(id).err(), unavailable);
        assert_eq!(e.resume(id).err(), unavailable);
        assert_eq!(e.cancel(id).err(), unavailable);
        assert_eq!(e.remove(id).err(), unavailable);
        assert_eq!(e.set_limits(id, Limits::default()).err(), unavailable);
        assert_eq!(e.set_global_limits(Limits::default()).err(), unavailable);
        assert_eq!(
            e.set_engine_limits(EngineLimits::DEFAULT).err(),
            unavailable
        );
        assert_eq!(e.set_connections(id, 2).err(), unavailable);
        assert_eq!(e.trust_host_key("h", 22, "SHA256:x").err(), unavailable);
        assert_eq!(e.snapshot().err(), unavailable);
    }

    #[test]
    fn restarting_it_says_what_is_wrong_and_where() {
        let mut e = missing();
        assert_eq!(e.restart(), Err(ErrorCode::EngineMissing));
        assert_eq!(e.program(), std::path::Path::new("velocity-transfer"));
    }

    #[test]
    fn its_event_stream_stays_open_and_quiet() {
        let mut e = missing();
        let events = e.subscribe();
        assert!(matches!(
            events.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }
}
