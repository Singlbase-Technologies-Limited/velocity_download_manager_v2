//! The child-process layer V2's engines share (#39, ADR-0012; made generic in #62): the framed,
//! versioned protocol over a child's stdin and stdout, the child's serve loop, the app's client,
//! and the supervisor that restarts a crashed or hung child. The direct-download engine
//! (`velocity-transfer`) and the torrent helper (`velocity-torrent`) each supply a [`Contract`],
//! and a [`supervisor::Book`] for what their supervisor keeps. The protocol is described in
//! docs/ENGINE_PROTOCOL.md.

use std::sync::{Mutex, MutexGuard};

pub mod client;
pub mod frame;
pub mod ledger;
pub mod log;
pub mod message;
pub mod serve;
pub mod supervisor;

pub use message::{Contract, PROTOCOL_VERSION};

/// Locks `m`, ignoring poisoning: the data stays consistent between whole updates.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
