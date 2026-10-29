//! The settings document in the database: one JSON object, whose sections are typed settings
//! ([`Section`]). Its model as a whole and its screen are #30's. #31's folder settings are its
//! `downloads` section, and #29's are `general`, `notifications`, `appearance` and
//! `onboarding`.

use rusqlite::OptionalExtension;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{to_sql_int, Database, DatabaseError};
use crate::domain::ErrorCode;

/// One section of the settings document: its key, and how a value is checked before it is saved
/// or used.
pub trait Section: Serialize + DeserializeOwned + Default + Clone {
    /// The section's key in the document.
    const KEY: &'static str;

    /// The value as it is saved, or why it can't be. Every value is valid unless a section says
    /// otherwise.
    fn checked(&self) -> Result<Self, ErrorCode> {
        Ok(self.clone())
    }
}

impl Database {
    /// The saved settings, or `None` before they were first saved.
    pub fn settings(&self) -> Result<Option<serde_json::Value>, DatabaseError> {
        let text: Option<String> = self
            .conn()
            .query_row("SELECT document FROM settings WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        text.map(|t| {
            serde_json::from_str(&t)
                .map_err(|e| DatabaseError::new(ErrorCode::Database, format!("the settings: {e}")))
        })
        .transpose()
    }

    /// Saves the settings document, which must be a JSON object.
    pub fn save_settings(
        &self,
        document: &serde_json::Value,
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        if !document.is_object() {
            return Err(DatabaseError::new(
                ErrorCode::InvalidInput,
                "the settings must be a JSON object",
            ));
        }
        self.conn().execute(
            "INSERT INTO settings (id, document, updated_at_ms) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET document = excluded.document,
                                            updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![document.to_string(), to_sql_int(now_ms)?],
        )?;
        Ok(())
    }

    /// The saved section, or its defaults before it was saved. A saved section this build can't
    /// read, or one that fails its checks, is a `Database` error, never replaced.
    pub fn section<S: Section>(&self) -> Result<S, DatabaseError> {
        let unreadable = |detail: String| {
            DatabaseError::new(
                ErrorCode::Database,
                format!("the {} settings: {detail}", S::KEY),
            )
        };
        let Some(section) = self
            .settings()?
            .and_then(|mut d| d.get_mut(S::KEY).map(|s| s.take()))
        else {
            return Ok(S::default());
        };
        // serde would read a struct from an array too.
        if !section.is_object() {
            return Err(unreadable("not an object".to_owned()));
        }
        // Over its defaults: a section an older V2 saved gets the fields added since (V1's
        // `migrate_settings_json`, #30). A field this build doesn't know is still refused.
        let mut merged = serde_json::to_value(S::default())
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        merge(&mut merged, section);
        let settings: S = serde_json::from_value(merged).map_err(|e| unreadable(e.to_string()))?;
        settings
            .checked()
            .map_err(|code| unreadable(format!("{code:?}")))
    }

    /// Saves several sections in one write, keeping the rest of the document (#30). Each must
    /// have been checked.
    pub fn save_sections(
        &self,
        sections: Vec<(&'static str, serde_json::Value)>,
        now_ms: u64,
    ) -> Result<(), DatabaseError> {
        let mut document = self
            .settings()?
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(object) = document.as_object_mut() {
            for (key, value) in sections {
                object.insert(key.to_owned(), value);
            }
        }
        self.save_settings(&document, now_ms)
    }

    /// Checks and saves a section, keeping the rest of the document, and returns it as saved.
    pub fn save_section<S: Section>(&self, settings: &S, now_ms: u64) -> Result<S, DatabaseError> {
        let settings = settings
            .checked()
            .map_err(|code| DatabaseError::new(code, format!("the {} settings", S::KEY)))?;
        let mut document = self
            .settings()?
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        let section = serde_json::to_value(&settings)
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        if let Some(object) = document.as_object_mut() {
            object.insert(S::KEY.to_owned(), section);
        }
        self.save_settings(&document, now_ms)?;
        Ok(settings)
    }
}

/// Puts `saved` over `base`: objects field by field, anything else whole.
fn merge(base: &mut serde_json::Value, saved: serde_json::Value) {
    match (base, saved) {
        (serde_json::Value::Object(base), serde_json::Value::Object(saved)) => {
            for (key, value) in saved {
                match base.get_mut(&key) {
                    Some(slot) => merge(slot, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_settings::{AppearanceSettings, GeneralSettings, LayoutSettings, Theme};
    use crate::destination::FolderSettings;

    fn absolute(rest: &str) -> String {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        format!("{root}{rest}")
    }

    /// #30: settings an older V2 saved get the fields added since, at their defaults.
    #[test]
    fn a_section_saved_without_newer_fields_reads_with_their_defaults() {
        use crate::queue::QueueSettings;
        let db = Database::open_in_memory().unwrap();
        db.save_settings(
            &serde_json::json!({"queue": {"maxConcurrentDownloads": 5, "schedule": {"enabled": true}}}),
            1,
        )
        .unwrap();
        let queue: QueueSettings = db.section().unwrap();
        assert_eq!(queue.max_concurrent_downloads, 5);
        assert!(queue.schedule.enabled);
        assert_eq!(queue.schedule.days, QueueSettings::default().schedule.days);
        assert_eq!(
            queue.start_immediately,
            QueueSettings::default().start_immediately
        );
        // A field this build doesn't know is still refused, never dropped.
        db.save_settings(&serde_json::json!({"queue": {"later": 1}}), 2)
            .unwrap();
        assert_eq!(
            db.section::<QueueSettings>().unwrap_err().code,
            ErrorCode::Database
        );
    }

    #[test]
    fn folder_settings_are_the_defaults_until_saved() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(
            db.section::<FolderSettings>(),
            Ok(FolderSettings::default())
        );
        db.save_settings(&serde_json::json!({ "other": 1 }), 1)
            .unwrap();
        assert_eq!(
            db.section::<FolderSettings>(),
            Ok(FolderSettings::default())
        );
    }

    #[test]
    fn folder_settings_are_saved_beside_the_rest_of_the_document() {
        let db = Database::open_in_memory().unwrap();
        db.save_settings(&serde_json::json!({ "other": 1 }), 1)
            .unwrap();
        let mine = FolderSettings {
            default_directory: format!(" {} ", absolute("Mine")),
            auto_categorize: false,
            category_subfolders: false,
        };
        let saved = db.save_section(&mine, 2).unwrap();
        assert_eq!(saved.default_directory, absolute("Mine"), "as checked");
        assert_eq!(db.section::<FolderSettings>(), Ok(saved));
        assert_eq!(db.settings().unwrap().unwrap()["other"], 1);
    }

    #[test]
    fn invalid_folder_settings_are_refused_not_replaced() {
        let db = Database::open_in_memory().unwrap();
        let bad = FolderSettings {
            default_directory: "relative".to_owned(),
            ..FolderSettings::default()
        };
        assert_eq!(
            db.save_section(&bad, 1).err().map(|e| e.code),
            Some(ErrorCode::InvalidPath)
        );
        assert_eq!(db.settings(), Ok(None), "nothing saved");
        for stored in [
            serde_json::json!({ "downloads": { "defaultDirectory": "relative" } }),
            serde_json::json!({ "downloads": { "autoCategorize": "yes" } }),
            serde_json::json!({ "downloads": [] }),
        ] {
            db.save_settings(&stored, 2).unwrap();
            assert_eq!(
                db.section::<FolderSettings>().err().map(|e| e.code),
                Some(ErrorCode::Database),
                "{stored}"
            );
            assert_eq!(db.settings(), Ok(Some(stored)), "left as it was");
        }
    }

    #[test]
    fn each_section_is_its_defaults_until_saved_and_is_saved_on_its_own() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(
            db.section::<GeneralSettings>(),
            Ok(GeneralSettings::default())
        );
        let quiet = GeneralSettings {
            close_to_tray: false,
            ..GeneralSettings::default()
        };
        db.save_section(&quiet, 1).unwrap();
        let dark = AppearanceSettings {
            theme: Theme::Dark,
            ..AppearanceSettings::default()
        };
        db.save_section(&dark, 2).unwrap();
        assert_eq!(db.section::<GeneralSettings>(), Ok(quiet));
        assert_eq!(db.section::<AppearanceSettings>(), Ok(dark));
        assert_eq!(
            db.section::<FolderSettings>(),
            Ok(FolderSettings::default())
        );
        let document = db.settings().unwrap().unwrap();
        assert_eq!(document["general"]["closeToTray"], false);
        assert_eq!(document["appearance"]["theme"], "dark");
    }

    #[test]
    fn a_section_that_fails_its_checks_is_refused_not_replaced() {
        let db = Database::open_in_memory().unwrap();
        let wide = AppearanceSettings {
            layout: LayoutSettings {
                sidebar_width: Some(10_000),
                ..LayoutSettings::default()
            },
            ..AppearanceSettings::default()
        };
        assert_eq!(
            db.save_section(&wide, 1).err().map(|e| e.code),
            Some(ErrorCode::InvalidInput)
        );
        assert_eq!(db.settings(), Ok(None), "nothing saved");
        let stored = serde_json::json!({ "appearance": { "layout": { "sidebarWidth": 10000 } } });
        db.save_settings(&stored, 2).unwrap();
        assert_eq!(
            db.section::<AppearanceSettings>().err().map(|e| e.code),
            Some(ErrorCode::Database)
        );
        assert_eq!(db.settings(), Ok(Some(stored)), "left as it was");
    }
}
