//! A torrent's details beyond the snapshot (#63; V1's `inspect_torrent_file`, `get_torrent_files`,
//! trackers, re-announce and upload limit): asked for while they are shown, not sent with every
//! snapshot (PO decision on #63).
//!
//! - **Its contents** come from the metadata the manager stored (#58), or, before adding, from the
//!   `.torrent` file itself, with the download it would repeat.
//! - **Trackers and peers** come from the torrent helper, which redacts tracker passkeys (#62).
//! - **Changing the files** of a torrent keeps the files dropped from it, unless the user asks to
//!   delete them; then only the torrent's own are deleted ([`crate::torrent_files::delete_owned`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::adding::{hash_keys, read_torrent_file, Duplicate};
use super::batch::kept_file;
use super::finished::DELETE_WAIT;
use super::Manager;
use crate::domain::{ErrorCode, KeptFile};
use crate::seeding::SeedPolicy;
use crate::torrent::{parse_metadata, TorrentMeta, TorrentVersion};
use crate::torrent_engine::{PeerStatus, TorrentId, TrackerStatus, MAX_RATE_LIMIT};
use crate::torrent_files::{delete_owned, owned_paths};

/// A torrent's identity and files, from its metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentContents {
    pub name: String,
    pub info_hash_v1: Option<String>,
    pub info_hash_v2: Option<String>,
    pub version: TorrentVersion,
    pub private: bool,
    /// Bytes of real data, padding left out.
    pub total_bytes: u64,
    /// The files a user can choose, in the torrent's order; padding files left out.
    pub files: Vec<ContentFile>,
}

/// One file of a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentFile {
    /// What choosing files takes: libtorrent's index plus one.
    pub index: u32,
    /// Its path inside the download folder, with `/` between the parts.
    pub path: String,
    pub length: u64,
}

/// What the details panel shows of a torrent in the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentDetails {
    pub seeding: SeedPolicy,
    /// Bytes per second; 0 for no limit.
    pub upload_limit_bps: u64,
    /// `None` until its metadata is known.
    pub contents: Option<TorrentContents>,
    /// The files chosen, by index; `None` for every file.
    pub chosen: Option<Vec<u32>>,
}

/// What adding a `.torrent` file would add.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TorrentPreview {
    pub contents: TorrentContents,
    /// The download it would repeat, if any.
    pub duplicate: Option<Duplicate>,
}

impl TorrentDetails {
    /// A fixed example for the contract fixtures and UI tests (`torrent.json`).
    pub fn sample() -> Self {
        Self {
            seeding: SeedPolicy::Ratio { ratio: 2.0 },
            upload_limit_bps: 65_536,
            contents: Some(TorrentContents::sample()),
            chosen: Some(vec![1, 3]),
        }
    }
}

impl TorrentContents {
    /// A fixed example: a hybrid torrent of three files.
    pub fn sample() -> Self {
        Self {
            name: "album".to_owned(),
            info_hash_v1: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
            info_hash_v2: Some("ab".repeat(32)),
            version: TorrentVersion::Hybrid,
            private: false,
            total_bytes: 3_300_000,
            files: vec![
                ContentFile {
                    index: 1,
                    path: "album/a.bin".to_owned(),
                    length: 3_000_000,
                },
                ContentFile {
                    index: 2,
                    path: "album/sub/b.bin".to_owned(),
                    length: 200_000,
                },
                ContentFile {
                    index: 3,
                    path: "album/c.txt".to_owned(),
                    length: 100_000,
                },
            ],
        }
    }
}

/// A fixed tracker and peer for the contract fixtures and UI tests (`torrent.json`).
pub fn sample_swarm() -> (Vec<TrackerStatus>, Vec<PeerStatus>) {
    use crate::torrent_engine::TrackerState;
    let tracker = |url: &str, tier, state, message: Option<&str>| TrackerStatus {
        url: url.to_owned(),
        tier,
        state,
        message: message.map(str::to_owned),
        fails: if state == TrackerState::Error { 2 } else { 0 },
        next_announce_secs: Some(1800),
        seeds: (state == TrackerState::Working).then_some(4),
        peers: (state == TrackerState::Working).then_some(9),
    };
    (
        vec![
            tracker(
                "https://tracker.example/announce?passkey=REDACTED",
                0,
                TrackerState::Working,
                None,
            ),
            tracker(
                "udp://backup.example:80/announce",
                1,
                TrackerState::Error,
                Some("timed out"),
            ),
        ],
        vec![PeerStatus {
            ip: "203.0.113.7".to_owned(),
            port: 51413,
            download_speed_bps: 131_072,
            upload_speed_bps: 4096,
            seed: true,
            progress_permille: 1000,
            client: "Transmission 4.0".to_owned(),
        }],
    )
}

impl From<&TorrentMeta> for TorrentContents {
    fn from(meta: &TorrentMeta) -> Self {
        Self {
            name: meta.name.clone(),
            info_hash_v1: meta.info_hash.clone(),
            info_hash_v2: meta.info_hash_v2.clone(),
            version: meta.version,
            private: meta.private,
            total_bytes: meta.total_length,
            files: meta
                .user_files()
                .map(|f| ContentFile {
                    index: f.index,
                    path: f.path.join("/"),
                    length: f.length,
                })
                .collect(),
        }
    }
}

impl Manager {
    /// The torrent's seeding policy, upload limit, files and the files chosen. `Unsupported` for a
    /// direct download.
    pub fn torrent_details(&self, id: &str) -> Result<TorrentDetails, ErrorCode> {
        let options = &self.torrent_job(id)?.options;
        Ok(TorrentDetails {
            seeding: options.seeding,
            upload_limit_bps: options.upload_limit,
            contents: self.stored_meta(id)?.as_ref().map(TorrentContents::from),
            chosen: options.files.clone(),
        })
    }

    /// What adding the `.torrent` file at `path` would add, read as adding reads it, and the
    /// download it would repeat.
    pub fn preview_torrent(&self, path: &Path) -> Result<TorrentPreview, ErrorCode> {
        let meta = read_torrent_file(path)?;
        let known = self.known_downloads()?;
        let keys = hash_keys(meta.info_hash.as_ref(), meta.info_hash_v2.as_ref());
        Ok(TorrentPreview {
            contents: TorrentContents::from(&meta),
            duplicate: keys.iter().find_map(|k| known.hashes.get(k)).cloned(),
        })
    }

    /// The torrent's trackers, in tier order, with passkeys redacted. `InvalidState` while the
    /// helper doesn't have it.
    pub fn torrent_trackers(&self, id: &str) -> Result<Vec<TrackerStatus>, ErrorCode> {
        self.torrents.trackers(self.helpers_torrent(id)?)
    }

    /// The torrent's connected peers. `InvalidState` while the helper doesn't have it.
    pub fn torrent_peers(&self, id: &str) -> Result<Vec<PeerStatus>, ErrorCode> {
        self.torrents.peers(self.helpers_torrent(id)?)
    }

    /// Announces the torrent to its trackers now. `InvalidState` while the helper doesn't have it.
    pub fn reannounce(&mut self, id: &str) -> Result<(), ErrorCode> {
        let torrent = self.helpers_torrent(id)?;
        self.torrents.reannounce(torrent)
    }

    /// Sets the torrent's upload limit, in bytes per second; 0 for none. It applies at once when
    /// the helper has the torrent, and is stored. `InvalidInput` above [`MAX_RATE_LIMIT`].
    pub fn set_upload_limit(&mut self, id: &str, upload: u64) -> Result<(), ErrorCode> {
        if upload > MAX_RATE_LIMIT {
            return Err(ErrorCode::InvalidInput);
        }
        let job = self.torrent_job(id)?;
        let download = job.options.download_limit;
        if let Some(torrent) = job.torrent {
            self.torrents.set_limits(torrent, upload, download)?;
        }
        self.torrent_job_mut(id)?.options.upload_limit = upload;
        self.store_torrent_options(id);
        Ok(())
    }

    /// Downloads exactly `files`, as [`Manager::choose_files`] does, and with `delete_dropped`
    /// deletes the torrent's own files that are no longer chosen. Returns the files kept, with why.
    pub fn change_files(
        &mut self,
        id: &str,
        files: Vec<u32>,
        delete_dropped: bool,
    ) -> Result<Vec<KeptFile>, ErrorCode> {
        let before = self.torrent_job(id)?.options.files.clone();
        self.choose_files(id, files.clone())?;
        if !delete_dropped {
            return Ok(Vec::new());
        }
        let Some(meta) = self.stored_meta(id)? else {
            return Ok(Vec::new());
        };
        let dropped: Vec<u32> = meta
            .user_files()
            .map(|f| f.index)
            .filter(|i| before.as_ref().is_none_or(|b| b.contains(i)) && !files.contains(i))
            .collect();
        let root = PathBuf::from(&self.downloads[id].record.directory);
        let mut owned = owned_paths(&root, &meta, &dropped);
        // Only the dropped files: the folders and part files still hold the chosen ones.
        owned.folders.clear();
        owned.parts.clear();
        let done = delete_owned(&owned, DELETE_WAIT);
        Ok(done
            .kept
            .into_iter()
            .map(|(path, why)| kept_file(&path, why))
            .collect())
    }

    /// The torrent's metadata as the manager stored it; `None` before it is known.
    fn stored_meta(&self, id: &str) -> Result<Option<TorrentMeta>, ErrorCode> {
        let stored = self.database.torrent(id).map_err(|e| e.code)?;
        Ok(stored
            .and_then(|s| s.metadata)
            .and_then(|bytes| parse_metadata(&bytes).ok()))
    }

    /// The helper's id for the torrent; `InvalidState` while the helper doesn't have it.
    fn helpers_torrent(&self, id: &str) -> Result<TorrentId, ErrorCode> {
        self.torrent_job(id)?.torrent.ok_or(ErrorCode::InvalidState)
    }
}
