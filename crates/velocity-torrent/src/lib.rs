//! The torrent engine of V2 (#62, ADR-0014): libtorrent behind the [`TorrentEngine`] contract,
//! run as the torrent helper, a supervised child process (`velocity-torrent serve`). Only
//! `velocity-lt` calls libtorrent; this crate decides what reaches it and what its state means.
//! The child-process protocol and supervisor are `velocity_child`'s, shared with the
//! direct-download engine.
//!
//! [`TorrentEngine`]: velocity_core::torrent_engine::TorrentEngine

use velocity_core::torrent_engine::{
    TorrentCommand, TorrentEvent, TorrentReply, TORRENT_CONTRACT_VERSION,
};

#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "engine")]
pub mod portmap;
pub mod stdio;
pub mod supervisor;

/// The torrent engine contract, as the torrent helper speaks it.
#[derive(Debug, PartialEq)]
pub struct TorrentContract;

impl velocity_child::Contract for TorrentContract {
    type Command = TorrentCommand;
    type Reply = TorrentReply;
    type Event = TorrentEvent;
    const VERSION: u32 = TORRENT_CONTRACT_VERSION;
    const NAME: &'static str = "torrent";
    fn health_check() -> TorrentCommand {
        TorrentCommand::Version {}
    }
}
