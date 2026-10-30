//! The window's settings (#29): how it looks, the first-run setup, and the sections the shell
//! reads. Their screen is #30's.

use super::Manager;
use crate::app_settings::{AppearanceSettings, OnboardingSettings, ShellSettings};
use crate::database::settings::Section;
use crate::destination::{check_folder, system_downloads, FolderSettings};
use crate::domain::ErrorCode;
use crate::links::is_link;

impl Manager {
    /// A section of the saved settings, or its defaults before it was saved.
    pub fn settings_section<S: Section>(&self) -> Result<S, ErrorCode> {
        self.database.section::<S>().map_err(|e| e.code)
    }

    /// What the window starts with: how it looks, whether the first-run setup is needed, and
    /// the folder downloads go to now.
    pub fn shell_settings(&self) -> Result<ShellSettings, ErrorCode> {
        let folders = self.settings_section::<FolderSettings>()?;
        let default_directory = match folders.default_directory.as_str() {
            "" => system_downloads().map(|p| p.to_string_lossy().into_owned()),
            dir => Some(dir.to_owned()),
        };
        Ok(ShellSettings {
            appearance: self.settings_section()?,
            onboarding_completed: self.settings_section::<OnboardingSettings>()?.completed,
            default_directory,
        })
    }

    /// Saves how the window looks, as checked.
    pub fn set_appearance(
        &mut self,
        appearance: &AppearanceSettings,
        now_ms: u64,
    ) -> Result<AppearanceSettings, ErrorCode> {
        self.database
            .save_section(appearance, now_ms)
            .map_err(|e| e.code)
    }

    /// Finishes the first-run setup with the folder the user chose for downloads, which must be
    /// an existing folder, not a link: `InvalidPath`, or `DestinationMissing` when it doesn't
    /// exist. Creating one is left to the user, as for every chosen folder (FILE_NAMING.md).
    pub fn complete_onboarding(
        &mut self,
        directory: &str,
        now_ms: u64,
    ) -> Result<ShellSettings, ErrorCode> {
        let folder = check_folder(directory.trim())?;
        // Never through a link: the folder is checked as itself.
        match std::fs::symlink_metadata(&folder) {
            Ok(meta) if meta.is_dir() && !is_link(&meta) => {}
            Ok(_) => return Err(ErrorCode::InvalidPath),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ErrorCode::DestinationMissing)
            }
            Err(_) => return Err(ErrorCode::InvalidPath),
        }
        let folders = FolderSettings {
            default_directory: folder.to_string_lossy().into_owned(),
            ..self.settings_section()?
        };
        self.database
            .save_section(&folders, now_ms)
            .map_err(|e| e.code)?;
        self.database
            .save_section(&OnboardingSettings { completed: true }, now_ms)
            .map_err(|e| e.code)?;
        self.shell_settings()
    }
}

/// The Settings screen's settings (#30): read, saved whole and applied at once.
impl Manager {
    /// Every setting the Settings screen edits, as saved, or the defaults of those never saved.
    pub fn settings(&self) -> Result<crate::settings::Settings, ErrorCode> {
        Ok(crate::settings::Settings {
            general: self.settings_section()?,
            notifications: self.settings_section()?,
            downloads: self.settings_section()?,
            queue: self.settings_section()?,
            performance: self.settings_section()?,
            engine: self.settings_section()?,
            advanced: self.settings_section()?,
            torrent: self.settings_section()?,
            new_torrents: self.settings_section()?,
            storage: self.settings_section()?,
            browser: self.settings_section()?,
            proxy: self.settings_section()?,
        })
    }

    /// Checks every section, saves them in one write, and applies them at once. Nothing is saved
    /// or applied when one is refused (`InvalidInput`). An engine that isn't running gets them when
    /// it next starts. Returns the settings as saved.
    pub fn set_settings(
        &mut self,
        settings: &crate::settings::Settings,
        now_ms: u64,
    ) -> Result<crate::settings::Settings, ErrorCode> {
        let settings = settings.checked()?;
        let proxy_before = self.settings_section::<crate::proxy::ProxySettings>()?;
        self.database
            .save_sections(settings.sections()?, now_ms)
            .map_err(|e| e.code)?;
        self.apply_settings(&settings, now_ms)?;
        // Saved and applied whatever the keychain says; its refusal is then the error (#192).
        self.proxy_remembering_changed(&proxy_before, &settings.proxy)?;
        Ok(settings)
    }

    /// Back to the defaults, keeping the folder downloads go to (V1's `reset_settings`).
    pub fn reset_settings(&mut self, now_ms: u64) -> Result<crate::settings::Settings, ErrorCode> {
        let mut defaults = crate::settings::Settings::default();
        defaults.downloads.default_directory = self.settings()?.downloads.default_directory;
        self.set_settings(&defaults, now_ms)
    }

    /// Applies the saved settings, when the app starts, with the proxy's remembered password.
    pub fn apply_saved_settings(&mut self, now_ms: u64) -> Result<(), ErrorCode> {
        let settings = self.settings()?;
        self.load_proxy_password(&settings.proxy);
        self.apply_settings(&settings, now_ms)
    }

    /// Gives each part of the app its settings. The engines may be unavailable: they get them when
    /// they start (the supervisors keep the last ones they were given).
    fn apply_settings(
        &mut self,
        s: &crate::settings::Settings,
        now_ms: u64,
    ) -> Result<(), ErrorCode> {
        self.set_queue_settings(s.queue.clone())?;
        self.metalink_enabled = s.advanced.enable_metalink;
        self.torrents_enabled = s.advanced.enable_bittorrent;
        self.user_agent = s.advanced.user_agent.clone();
        let global = crate::transfer::Limits::from_kib(s.advanced.global_download_limit_kib)?;
        let engine = [
            self.set_performance(s.performance),
            self.engine.set_engine_settings(s.engine),
            self.engine.set_global_limits(global),
            self.torrents.set_settings(s.torrent.clone()),
        ];
        for result in engine {
            match result {
                Ok(()) | Err(ErrorCode::EngineUnavailable) => {}
                Err(code) => return Err(code),
            }
        }
        self.apply_proxy(&s.proxy)?;
        self.apply_retention(now_ms)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::app_settings::{GeneralSettings, LayoutSettings, Theme};
    use crate::database::Database;
    use crate::transfer::fake::FakeTransfer;

    const T0: u64 = 1_792_600_000_000;

    fn manager() -> (Manager, Arc<Database>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let m = Manager::open(Box::new(FakeTransfer::new()), Arc::clone(&db)).unwrap();
        (m, db)
    }

    #[test]
    fn a_new_install_needs_the_first_run_setup_and_offers_the_systems_downloads_folder() {
        let (m, _) = manager();
        let s = m.shell_settings().unwrap();
        assert!(!s.onboarding_completed);
        assert_eq!(s.appearance, AppearanceSettings::default());
        assert_eq!(
            s.default_directory,
            system_downloads().map(|p| p.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn finishing_the_setup_saves_the_folder_and_is_remembered() {
        let (mut m, db) = manager();
        let dir = std::env::temp_dir();
        let dir = dir.to_string_lossy();
        let s = m.complete_onboarding(&format!(" {dir} "), T0).unwrap();
        assert!(s.onboarding_completed);
        let saved = check_folder(&dir).unwrap().to_string_lossy().into_owned();
        assert_eq!(s.default_directory.as_deref(), Some(saved.as_str()));
        assert_eq!(
            db.section::<FolderSettings>().unwrap().default_directory,
            saved
        );
        assert_eq!(m.shell_settings(), Ok(s), "read back");
    }

    #[test]
    fn the_setup_refuses_a_folder_that_isnt_one_and_saves_nothing() {
        let (mut m, db) = manager();
        let missing = std::env::temp_dir().join("velocity-no-such-folder-29");
        let file = std::env::temp_dir().join("velocity-onboarding-file-29.txt");
        std::fs::write(&file, b"x").unwrap();
        for (dir, code) in [
            ("relative".to_owned(), ErrorCode::InvalidPath),
            (String::new(), ErrorCode::InvalidPath),
            (
                missing.to_string_lossy().into_owned(),
                ErrorCode::DestinationMissing,
            ),
            (file.to_string_lossy().into_owned(), ErrorCode::InvalidPath),
        ] {
            assert_eq!(m.complete_onboarding(&dir, T0), Err(code), "{dir}");
        }
        std::fs::remove_file(&file).unwrap();
        assert_eq!(db.settings(), Ok(None), "nothing saved");
    }

    #[cfg(unix)]
    #[test]
    fn the_setup_refuses_a_link_to_a_folder() {
        let (mut m, _) = manager();
        let base = std::env::temp_dir().join(format!("velocity-29-link-{}", std::process::id()));
        std::fs::create_dir_all(base.join("real")).unwrap();
        let link = base.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(base.join("real"), &link).unwrap();
        let result = m.complete_onboarding(&link.to_string_lossy(), T0);
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(result, Err(ErrorCode::InvalidPath));
    }

    #[test]
    fn the_appearance_is_saved_as_checked_and_kept_apart_from_other_settings() {
        let (mut m, db) = manager();
        let quiet = GeneralSettings {
            confirm_exit_with_active: false,
            ..GeneralSettings::default()
        };
        db.save_section(&quiet, T0).unwrap();
        let dark = AppearanceSettings {
            theme: Theme::Dark,
            layout: LayoutSettings {
                sidebar_width: Some(250),
                ..LayoutSettings::default()
            },
            ..AppearanceSettings::default()
        };
        assert_eq!(m.set_appearance(&dark, T0), Ok(dark.clone()));
        assert_eq!(m.shell_settings().unwrap().appearance, dark);
        assert_eq!(m.settings_section::<GeneralSettings>(), Ok(quiet));
        let wide = AppearanceSettings {
            layout: LayoutSettings {
                sidebar_width: Some(5000),
                ..LayoutSettings::default()
            },
            ..AppearanceSettings::default()
        };
        assert_eq!(m.set_appearance(&wide, T0), Err(ErrorCode::InvalidInput));
        assert_eq!(m.shell_settings().unwrap().appearance, dark, "kept");
    }
}
