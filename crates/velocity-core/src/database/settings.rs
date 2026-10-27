//! The settings document in the database. Its fields are defined and validated by #30; here it
//! is one JSON object. #31's folder settings are its `downloads` section.

use rusqlite::OptionalExtension;

use super::{to_sql_int, Database, DatabaseError};
use crate::destination::FolderSettings;
use crate::domain::ErrorCode;

/// The settings document's section for [`FolderSettings`].
const FOLDERS: &str = "downloads";

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

    /// The saved folder settings, or the defaults before any were saved. A saved section this
    /// build can't read, or one that fails its checks, is a `Database` error, never replaced.
    pub fn folder_settings(&self) -> Result<FolderSettings, DatabaseError> {
        let unreadable = |detail: String| DatabaseError::new(ErrorCode::Database, detail);
        let Some(section) = self
            .settings()?
            .and_then(|mut d| d.get_mut(FOLDERS).map(|s| s.take()))
        else {
            return Ok(FolderSettings::default());
        };
        // serde would read a struct from an array too.
        if !section.is_object() {
            return Err(unreadable(
                "the folder settings aren't an object".to_owned(),
            ));
        }
        let settings: FolderSettings = serde_json::from_value(section)
            .map_err(|e| unreadable(format!("the folder settings: {e}")))?;
        settings
            .checked()
            .map_err(|code| unreadable(format!("the folder settings: {code:?}")))
    }

    /// Checks and saves the folder settings, keeping the rest of the document.
    pub fn save_folder_settings(
        &self,
        settings: &FolderSettings,
        now_ms: u64,
    ) -> Result<FolderSettings, DatabaseError> {
        let settings = settings
            .checked()
            .map_err(|code| DatabaseError::new(code, "the folder settings"))?;
        let mut document = self
            .settings()?
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        let section = serde_json::to_value(&settings)
            .map_err(|e| DatabaseError::new(ErrorCode::Internal, e.to_string()))?;
        if let Some(object) = document.as_object_mut() {
            object.insert(FOLDERS.to_owned(), section);
        }
        self.save_settings(&document, now_ms)?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(rest: &str) -> String {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        format!("{root}{rest}")
    }

    #[test]
    fn folder_settings_are_the_defaults_until_saved() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.folder_settings(), Ok(FolderSettings::default()));
        db.save_settings(&serde_json::json!({ "other": 1 }), 1)
            .unwrap();
        assert_eq!(db.folder_settings(), Ok(FolderSettings::default()));
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
        let saved = db.save_folder_settings(&mine, 2).unwrap();
        assert_eq!(saved.default_directory, absolute("Mine"), "as checked");
        assert_eq!(db.folder_settings(), Ok(saved));
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
            db.save_folder_settings(&bad, 1).err().map(|e| e.code),
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
                db.folder_settings().err().map(|e| e.code),
                Some(ErrorCode::Database),
                "{stored}"
            );
            assert_eq!(db.settings(), Ok(Some(stored)), "left as it was");
        }
    }
}
