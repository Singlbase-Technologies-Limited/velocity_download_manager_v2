//! The SFTP layer of V2 (ADR-0013, #53): the only crate that calls russh. It answers a fetch with
//! the same events as the libcurl layer, through velocity-curl's [`velocity_curl::Transport`], so
//! the engine's probe, gate, checkpoints and completion gate are the same for SFTP.

mod client;
pub mod known_hosts;
mod proxy;

pub use client::{SftpOptions, SftpTransport, TrustError};
