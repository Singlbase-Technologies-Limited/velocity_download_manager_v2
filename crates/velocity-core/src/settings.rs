//! The settings the Settings screen edits (#30), as one: every section V1 had that V2 does
//! something with (docs/SETTINGS.md, docs/PARITY.md). Each section is saved under its own key of
//! the settings document ([`Section`]), read merged over its defaults so that settings saved by an
//! older V2 get the new fields' defaults (V1's `migrate_settings_json`), and checked in Rust
//! whatever the window sent.
//!
//! The window's look (`appearance`) and the first-run flag are saved on their own (#29), so the
//! Settings screen never overwrites a panel width the user just dragged.

use serde::{Deserialize, Serialize};

use crate::app_settings::{GeneralSettings, NotificationSettings};
use crate::database::settings::Section;
use crate::destination::FolderSettings;
use crate::domain::ErrorCode;
use crate::history::StorageSettings;
use crate::performance::PerformanceSettings;
use crate::queue::QueueSettings;
use crate::seeding::SeedPolicy;
use crate::torrent_engine::{TorrentSettings, MAX_RATE_LIMIT};
use crate::transfer::{EngineSettings, Limits};

/// What a download sends and which protocols are on (V1's `advanced` settings, and its global
/// speed limit from `queue`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedSettings {
    /// KiB/s all downloads share; 0 for no limit (#49).
    pub global_download_limit_kib: u64,
    /// Torrents and magnet links can be added (V1's `enable_bittorrent`).
    pub enable_bittorrent: bool,
    /// Metalink files can be added, and links to them followed (#54; V1's `enable_metalink`).
    pub enable_metalink: bool,
    /// The `User-Agent` new downloads send; `None` for V2's own (`VelocityDownloadManager/<version>`,
    /// PO decision on #40).
    pub user_agent: Option<String>,
}

impl AdvancedSettings {
    /// The longest user agent: V1's.
    pub const MAX_USER_AGENT_BYTES: usize = 256;
}

impl Default for AdvancedSettings {
    /// V1's: BitTorrent on, Metalink off, no limit, V2's own user agent.
    fn default() -> Self {
        Self {
            global_download_limit_kib: 0,
            enable_bittorrent: true,
            enable_metalink: false,
            user_agent: None,
        }
    }
}

impl Section for AdvancedSettings {
    const KEY: &'static str = "advanced";

    /// `InvalidInput` for a limit below 1 KiB/s or too large, or a user agent that is empty,
    /// longer than [`AdvancedSettings::MAX_USER_AGENT_BYTES`], or has a control character.
    fn checked(&self) -> Result<Self, ErrorCode> {
        Limits::from_kib(self.global_download_limit_kib)?;
        if let Some(agent) = &self.user_agent {
            let printable = agent.chars().all(|c| !c.is_control());
            if agent.trim().is_empty() || agent.len() > Self::MAX_USER_AGENT_BYTES || !printable {
                return Err(ErrorCode::InvalidInput);
            }
        }
        Ok(self.clone())
    }
}

/// What new torrents start with (V1's `choose_files_for_magnets`, `default_upload_limit_kib` and
/// `seeding`): the Add dialog's defaults, and the manager's for a torrent added without them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewTorrentSettings {
    /// A magnet waits in Choose files once its metadata arrives.
    pub choose_files_for_magnets: bool,
    pub seeding: SeedPolicy,
    /// KiB/s each new torrent uploads at most; 0 for no limit.
    pub upload_limit_kib: u64,
}

impl Default for NewTorrentSettings {
    /// V1's: no file choice, no seeding, no upload limit.
    fn default() -> Self {
        Self {
            choose_files_for_magnets: false,
            seeding: SeedPolicy::default(),
            upload_limit_kib: 0,
        }
    }
}

impl Section for NewTorrentSettings {
    const KEY: &'static str = "newTorrents";

    fn checked(&self) -> Result<Self, ErrorCode> {
        self.seeding.check()?;
        let ok = self
            .upload_limit_kib
            .checked_mul(1024)
            .is_some_and(|bps| bps <= MAX_RATE_LIMIT);
        if ok {
            Ok(self.clone())
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }
}

impl Section for QueueSettings {
    const KEY: &'static str = "queue";

    fn checked(&self) -> Result<Self, ErrorCode> {
        self.check()?;
        Ok(self.clone())
    }
}

impl Section for PerformanceSettings {
    const KEY: &'static str = "performance";

    fn checked(&self) -> Result<Self, ErrorCode> {
        self.check()?;
        Ok(*self)
    }
}

impl Section for EngineSettings {
    const KEY: &'static str = "engine";

    fn checked(&self) -> Result<Self, ErrorCode> {
        self.check()?;
        Ok(*self)
    }
}

impl Section for TorrentSettings {
    const KEY: &'static str = "torrent";

    fn checked(&self) -> Result<Self, ErrorCode> {
        self.check()?;
        Ok(self.clone())
    }
}

/// Every setting the Settings screen edits.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub general: GeneralSettings,
    pub notifications: NotificationSettings,
    pub downloads: FolderSettings,
    pub queue: QueueSettings,
    pub performance: PerformanceSettings,
    pub engine: EngineSettings,
    pub advanced: AdvancedSettings,
    pub torrent: TorrentSettings,
    pub new_torrents: NewTorrentSettings,
    pub storage: StorageSettings,
}

impl Settings {
    /// Each section as it is saved, or the first refusal: nothing is saved unless all pass.
    pub fn checked(&self) -> Result<Settings, ErrorCode> {
        Ok(Settings {
            general: self.general.checked()?,
            notifications: self.notifications.checked()?,
            downloads: self.downloads.checked()?,
            queue: self.queue.checked()?,
            performance: self.performance.checked()?,
            engine: self.engine.checked()?,
            advanced: self.advanced.checked()?,
            torrent: self.torrent.checked()?,
            new_torrents: self.new_torrents.checked()?,
            storage: self.storage.checked()?,
        })
    }

    /// Every section as JSON, under its key, for saving in one write.
    pub fn sections(&self) -> Result<Vec<(&'static str, serde_json::Value)>, ErrorCode> {
        fn json<S: Serialize>(
            key: &'static str,
            s: &S,
        ) -> Result<(&'static str, serde_json::Value), ErrorCode> {
            serde_json::to_value(s)
                .map(|v| (key, v))
                .map_err(|_| ErrorCode::Internal)
        }
        Ok(vec![
            json(GeneralSettings::KEY, &self.general)?,
            json(NotificationSettings::KEY, &self.notifications)?,
            json(FolderSettings::KEY, &self.downloads)?,
            json(QueueSettings::KEY, &self.queue)?,
            json(PerformanceSettings::KEY, &self.performance)?,
            json(EngineSettings::KEY, &self.engine)?,
            json(AdvancedSettings::KEY, &self.advanced)?,
            json(TorrentSettings::KEY, &self.torrent)?,
            json(NewTorrentSettings::KEY, &self.new_torrents)?,
            json(StorageSettings::KEY, &self.storage)?,
        ])
    }

    /// A fixed example for the contract fixtures: every section at its defaults, as the screen
    /// first shows them (`settings.json`).
    pub fn sample() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_v1s_and_pass_their_checks() {
        let s = Settings::default();
        assert_eq!(s.checked(), Ok(s.clone()));
        assert!(s.advanced.enable_bittorrent && !s.advanced.enable_metalink);
        assert_eq!(s.queue.max_concurrent_downloads, 3);
        assert_eq!(s.engine, EngineSettings::DEFAULT);
        assert_eq!(s.storage.history_retention_days, 90);
    }

    #[test]
    fn one_bad_section_refuses_them_all() {
        let mut s = Settings::default();
        s.advanced.user_agent = Some("bad\nagent".to_owned());
        assert_eq!(s.checked(), Err(ErrorCode::InvalidInput));
        let mut s = Settings::default();
        s.advanced.user_agent = Some("x".repeat(AdvancedSettings::MAX_USER_AGENT_BYTES + 1));
        assert_eq!(s.checked(), Err(ErrorCode::InvalidInput));
        let mut s = Settings::default();
        s.advanced.global_download_limit_kib = u64::MAX;
        assert_eq!(s.checked(), Err(ErrorCode::InvalidInput));
        let mut s = Settings::default();
        s.new_torrents.seeding = SeedPolicy::Ratio { ratio: 1000.0 };
        assert_eq!(s.checked(), Err(ErrorCode::InvalidInput));
        let mut s = Settings::default();
        s.engine.reset_after_secs = 5;
        s.engine.stalled_after_secs = 10;
        assert_eq!(s.checked(), Err(ErrorCode::InvalidInput));
    }

    #[test]
    fn every_section_has_its_own_key() {
        let keys: Vec<_> = Settings::default()
            .sections()
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "{keys:?}");
    }
}
