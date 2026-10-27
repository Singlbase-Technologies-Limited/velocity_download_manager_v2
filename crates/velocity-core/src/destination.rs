//! Where a download is saved (#31): the folder from the user, the category or the settings, and
//! what is already there. Checks paths by their text only; following links and the other file
//! system checks are #72's.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::ErrorCode;

/// The longest folder path, in bytes, accepted from the user.
pub const MAX_FOLDER_BYTES: usize = 4096;

/// `path` as a folder to save into, or `InvalidPath` when it is empty, too long, relative, has a
/// control character or a `..` (V1's `validate_directory_syntax`). The folder needn't exist.
pub fn check_folder(path: &str) -> Result<PathBuf, ErrorCode> {
    // Both separators on every platform: `a\..\b` is refused on Linux too.
    let parent = path.split(['/', '\\']).any(|part| part == "..");
    let folder = PathBuf::from(path);
    if path.is_empty()
        || path.len() > MAX_FOLDER_BYTES
        || path.chars().any(char::is_control)
        || parent
        || !folder.is_absolute()
    {
        return Err(ErrorCode::InvalidPath);
    }
    Ok(folder)
}

/// Where downloads go when the user names no folder (#31, V1's `DownloadSettings`). Saved as
/// the settings document's `downloads` section; #30 puts it on the Settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct FolderSettings {
    /// The default folder; empty for the system's Downloads folder.
    pub default_directory: String,
    /// Give a download the category its file name's extension belongs to.
    pub auto_categorize: bool,
    /// Save a categorised download in a subfolder named after its category, when the category
    /// has no folder of its own.
    pub category_subfolders: bool,
}

impl Default for FolderSettings {
    /// V1's defaults since its #38: an automatic category also chooses the folder.
    fn default() -> Self {
        FolderSettings {
            default_directory: String::new(),
            auto_categorize: true,
            category_subfolders: true,
        }
    }
}

impl FolderSettings {
    /// The settings as they are saved, with the folder trimmed, or `InvalidPath`.
    pub fn checked(&self) -> Result<FolderSettings, ErrorCode> {
        let default_directory = match self.default_directory.trim() {
            "" => String::new(),
            dir => check_folder(dir)?.to_string_lossy().into_owned(),
        };
        Ok(FolderSettings {
            default_directory,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute folder on this platform.
    fn absolute(rest: &str) -> String {
        if cfg!(windows) {
            format!("C:\\Users\\me\\{rest}")
        } else {
            format!("/home/me/{rest}")
        }
    }

    #[test]
    fn folder_settings_default_to_v1s() {
        let s: FolderSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, FolderSettings::default());
        assert_eq!(s.default_directory, "");
        assert!(s.auto_categorize && s.category_subfolders);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "defaultDirectory": "",
                "autoCategorize": true,
                "categorySubfolders": true
            })
        );
    }

    #[test]
    fn folder_settings_are_checked() {
        let with = |dir: &str| FolderSettings {
            default_directory: dir.to_owned(),
            ..FolderSettings::default()
        };
        assert_eq!(with("").checked(), Ok(with("")));
        assert_eq!(with("  ").checked(), Ok(with("")));
        let ok = absolute("Downloads");
        assert_eq!(with(&format!(" {ok} ")).checked(), Ok(with(&ok)));
        assert_eq!(with("Downloads").checked(), Err(ErrorCode::InvalidPath));
        assert_eq!(
            with(&absolute("a/../b")).checked(),
            Err(ErrorCode::InvalidPath)
        );
    }

    #[test]
    fn a_folder_must_be_a_plain_absolute_path() {
        let ok = absolute("Downloads");
        assert_eq!(check_folder(&ok), Ok(PathBuf::from(&ok)));
        assert_eq!(
            check_folder(&absolute("Unicode ✓ résumé")),
            Ok(PathBuf::from(absolute("Unicode ✓ résumé")))
        );
        let too_long = absolute(&"x".repeat(MAX_FOLDER_BYTES));
        for bad in [
            String::new(),
            "relative/path".to_owned(),
            "Downloads".to_owned(),
            absolute("a/../b"),
            absolute("a\\..\\b"),
            absolute("tab\there"),
            absolute("nul\0"),
            too_long,
        ] {
            assert_eq!(check_folder(&bad), Err(ErrorCode::InvalidPath), "{bad:?}");
        }
    }
}
