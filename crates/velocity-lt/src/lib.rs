//! A narrow bridge to one libtorrent 2.0 session.
//!
//! Only plain data crosses the boundary. Torrents are named by an id the shim
//! assigns; libtorrent never deletes files through this bridge (removal always
//! keeps data, and the application decides what to delete).

#[cxx::bridge(namespace = "velocity_lt")]
pub mod ffi {
    /// Session-wide settings. Applied at creation and again by `apply_config`.
    #[derive(Debug, Clone, Default)]
    pub struct SessionConfig {
        /// libtorrent `listen_interfaces`, e.g. `0.0.0.0:6882,[::]:6882`.
        pub listen_interfaces: String,
        pub enable_dht: bool,
        pub enable_lsd: bool,
        pub enable_upnp: bool,
        pub enable_natpmp: bool,
        /// `host:port,host:port`
        pub dht_bootstrap_nodes: String,
        pub user_agent: String,
        /// Eight-character peer id prefix such as `-VL0100-`.
        pub peer_fingerprint: String,
        /// Bytes per second, 0 = unlimited.
        pub upload_rate_limit: i32,
        pub download_rate_limit: i32,
        pub connections_limit: i32,
    }

    #[derive(Debug, Clone, Default)]
    pub struct AddParams {
        /// A whole `.torrent` file or `d4:info<info dict>e`. Empty for a magnet.
        pub torrent: Vec<u8>,
        pub magnet: String,
        pub save_path: String,
        /// One entry per file (padding files included): 0 = skip, 4 = normal.
        /// Empty means every file at normal priority.
        pub file_priorities: Vec<u8>,
        pub paused: bool,
        /// Bytes per second, 0 = unlimited.
        pub upload_limit: i32,
        pub download_limit: i32,
        /// 0 = libtorrent's default.
        pub max_connections: i32,
        pub trackers: Vec<String>,
        /// libtorrent resume data written earlier for this torrent.
        pub resume_data: Vec<u8>,
        pub disable_dht: bool,
        pub disable_lsd: bool,
        /// Peer exchange is switched per torrent.
        pub disable_pex: bool,
        /// Files start at priority 0 when a magnet's metadata arrives, so a
        /// metadata-only fetch never downloads payload.
        pub dont_download: bool,
    }

    #[derive(Debug, Clone, Default)]
    pub struct TorrentStatus {
        /// libtorrent `torrent_status::state_t`.
        pub state: u8,
        pub paused: bool,
        pub has_metadata: bool,
        pub is_finished: bool,
        pub is_seeding: bool,
        pub total_wanted: i64,
        pub total_wanted_done: i64,
        pub total_done: i64,
        pub all_time_upload: i64,
        pub all_time_download: i64,
        pub download_rate: i64,
        pub upload_rate: i64,
        pub num_peers: i32,
        pub num_seeds: i32,
        /// Seeds and peers reported by trackers (scrape), -1 when unknown.
        pub num_complete: i32,
        pub num_incomplete: i32,
        pub error: String,
        pub name: String,
        pub save_path: String,
        pub num_pieces: i32,
        pub piece_length: i32,
        /// Hex info hashes; empty when the torrent has no such version.
        pub info_hash_v1: String,
        pub info_hash_v2: String,
        pub seeding_secs: i64,
        pub need_save_resume: bool,
    }

    #[derive(Debug, Clone, Default)]
    pub struct FileEntry {
        /// Absolute path (save path joined with the path inside the torrent).
        pub path: String,
        pub size: i64,
        pub done: i64,
        pub priority: u8,
        pub pad: bool,
    }

    #[derive(Debug, Clone, Default)]
    pub struct PeerEntry {
        pub ip: String,
        pub port: u16,
        pub download_rate: i64,
        pub upload_rate: i64,
        pub seed: bool,
        pub am_choking: bool,
        pub peer_choking: bool,
        pub progress: f32,
        pub client: String,
    }

    #[derive(Debug, Clone, Default)]
    pub struct TrackerEntry {
        /// May contain a passkey: never log it unredacted.
        pub url: String,
        pub tier: u8,
        /// An announce succeeded and nothing has failed since.
        pub working: bool,
        pub updating: bool,
        pub fails: u8,
        /// Last tracker message or error, empty when none.
        pub message: String,
        /// Seconds until the next announce, -1 when none is scheduled.
        pub next_announce_secs: i64,
        pub scrape_complete: i32,
        pub scrape_incomplete: i32,
    }

    #[derive(Debug, Clone, Default)]
    pub struct AlertEntry {
        /// One of: listen_succeeded, listen_failed, portmap, portmap_error,
        /// metadata_received, torrent_finished, torrent_error, file_error,
        /// tracker_reply, tracker_error, tracker_warning, save_resume_data,
        /// save_resume_data_failed, torrent_removed, cache_flushed.
        pub kind: String,
        /// The torrent the alert is about, 0 for session alerts.
        pub id: u64,
        pub message: String,
        /// Resume data for `save_resume_data`.
        pub data: Vec<u8>,
        /// Port for listen and port-mapping alerts.
        pub port: i32,
        /// `upnp` or `natpmp` for port-mapping alerts, `tcp`/`udp` for listen alerts.
        pub transport: String,
        /// `tcp` or `udp` for port mapping.
        pub protocol: String,
        /// Tracker URL for tracker alerts (may contain a passkey).
        pub url: String,
    }

    unsafe extern "C++" {
        include!("velocity-lt/cpp/shim.h");

        type Session;

        fn new_session(cfg: &SessionConfig) -> Result<UniquePtr<Session>>;
        fn version() -> String;

        fn apply_config(self: &Session, cfg: &SessionConfig) -> Result<()>;
        fn add(self: &Session, p: &AddParams) -> Result<u64>;
        /// Keeps the files. Returns false for an unknown id.
        fn remove(self: &Session, id: u64) -> bool;
        fn status(self: &Session, id: u64) -> Result<TorrentStatus>;
        fn files(self: &Session, id: u64) -> Result<Vec<FileEntry>>;
        fn peers(self: &Session, id: u64) -> Result<Vec<PeerEntry>>;
        fn trackers(self: &Session, id: u64) -> Result<Vec<TrackerEntry>>;
        fn pause(self: &Session, id: u64) -> Result<()>;
        fn resume(self: &Session, id: u64) -> Result<()>;
        fn set_file_priorities(self: &Session, id: u64, priorities: &[u8]) -> Result<()>;
        fn set_limits(self: &Session, id: u64, upload: i32, download: i32) -> Result<()>;
        fn force_reannounce(self: &Session, id: u64) -> Result<()>;
        fn force_recheck(self: &Session, id: u64) -> Result<()>;
        /// Write out everything still queued for the torrent's files and close them;
        /// a `cache_flushed` alert follows.
        fn flush(self: &Session, id: u64) -> Result<()>;
        fn request_resume_data(self: &Session, id: u64) -> Result<()>;
        /// The info dictionary as `d4:info<info>e`, once metadata is known.
        fn metadata(self: &Session, id: u64) -> Result<Vec<u8>>;
        fn pop_alerts(self: &Session) -> Vec<AlertEntry>;
        /// Connect to a known peer (used for local peers and tests).
        fn connect_peer(self: &Session, id: u64, ip: &str, port: u16) -> Result<()>;
        /// The first TCP port the session listens on, 0 before it listens.
        fn listen_port(self: &Session) -> u16;
    }
}

// SAFETY: every `Session` method takes `&self`; `lt::session` is documented as thread-safe (its
// calls are posted to libtorrent's own network thread), and the shim guards its id table with
// `mu_`. No method hands out a reference into C++-owned memory.
unsafe impl Send for ffi::Session {}
// SAFETY: as for `Send` above.
unsafe impl Sync for ffi::Session {}

/// libtorrent `torrent_status::state_t` values.
pub mod state {
    pub const CHECKING_FILES: u8 = 1;
    pub const DOWNLOADING_METADATA: u8 = 2;
    pub const DOWNLOADING: u8 = 3;
    pub const FINISHED: u8 = 4;
    pub const SEEDING: u8 = 5;
    pub const CHECKING_RESUME_DATA: u8 = 7;
}

/// Error text the shim uses when a torrent with the same hash is still in the session.
pub const DUPLICATE_TORRENT: &str = "duplicate_torrent";

pub use cxx::UniquePtr;
pub use ffi::*;
