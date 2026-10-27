//! Where a download is saved (#31): the folder from the user, the category or the settings, and
//! what is already there. Checks paths by their text only; following links and the other file
//! system checks are #72's.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::category::{for_file_name, Category};
use crate::domain::ErrorCode;
use crate::naming::{free_file_name, sanitize_file_name, PART_FILE_SUFFIX};

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

/// What the user chose for a new download; `None` leaves it to [`resolve`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    pub directory: Option<String>,
    /// The file's name, when it is known before the download starts.
    pub file_name: Option<String>,
    pub category_id: Option<String>,
}

/// Where a new download goes, as [`resolve`] chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub directory: PathBuf,
    /// The folder came from the category or the settings, not from the user.
    pub automatic_directory: bool,
    pub category: Option<Category>,
    /// The category's subfolder at the end of `directory`, when it is in one: the one folder
    /// adding may create.
    pub subfolder: Option<String>,
}

/// The category and folder for a new download, as V1 chose them (V1 #38):
/// - **The category** is the one chosen, else, with automatic categories on, the one for the
///   file name's extension. `NotFound` for a chosen category that doesn't exist.
/// - **The folder** is the one chosen, else the category's own folder, else the default folder
///   (the setting, or `system_downloads`), in a subfolder named after the category when
///   category subfolders are on. `InvalidPath` for a chosen folder that isn't valid, and
///   `DestinationMissing` when there is no default folder.
pub fn resolve(
    choice: &Choice,
    settings: &FolderSettings,
    categories: &[Category],
    system_downloads: Option<PathBuf>,
) -> Result<Resolved, ErrorCode> {
    let category = match choice.category_id.as_deref() {
        Some(id) => Some(
            categories
                .iter()
                .find(|c| c.id == id)
                .ok_or(ErrorCode::NotFound)?,
        ),
        None if settings.auto_categorize => choice
            .file_name
            .as_deref()
            .and_then(|name| for_file_name(categories, name)),
        None => None,
    };
    let chosen = choice.directory.as_deref().map(str::trim);
    let mut subfolder = None;
    let (directory, automatic_directory) = match chosen.filter(|d| !d.is_empty()) {
        Some(dir) => (check_folder(dir)?, false),
        None => match category.and_then(|c| c.directory.as_deref()) {
            Some(own) => (check_folder(own)?, true),
            None => {
                let default = match settings.default_directory.as_str() {
                    "" => system_downloads.ok_or(ErrorCode::DestinationMissing)?,
                    dir => check_folder(dir)?,
                };
                subfolder = category
                    .filter(|_| settings.category_subfolders)
                    .map(|c| sanitize_file_name(&c.name));
                (
                    subfolder
                        .as_ref()
                        .map_or_else(|| default.clone(), |s| default.join(s)),
                    true,
                )
            }
        },
    };
    Ok(Resolved {
        directory,
        automatic_directory,
        category: category.cloned(),
        subfolder,
    })
}

/// What adding does when the name is taken (PO decision on #31: an existing file is never
/// replaced, and V1's partial downloads are never resumed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnConflict {
    /// Add nothing, and say what is there and which name is free.
    #[default]
    Ask,
    /// Use the first free `name (n).ext`.
    Rename,
}

impl OnConflict {
    /// Every value, in contract order (the fixture shared with `src/contract.ts`).
    pub const ALL: [OnConflict; 2] = [OnConflict::Ask, OnConflict::Rename];
}

/// Where a new download goes, once its name is free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub directory: PathBuf,
    /// `None` when the server will name the file.
    pub file_name: Option<String>,
    pub category_id: Option<String>,
}

/// What placing a new download gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Placed(Place),
    /// The name is taken and [`OnConflict::Ask`] was chosen: nothing is added.
    Conflict(crate::domain::DestinationCheck),
}

/// The system's Downloads folder, or `Downloads` in the home folder when the system names none
/// (V1's fallback, without its last resort of the temporary folder).
pub fn system_downloads() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
}

/// The suffix of V1's control file beside a download it hasn't finished (aria2's). V2 never reads
/// one; it only counts the name as taken, so V1's download is never written over.
pub const V1_PART_SUFFIX: &str = ".aria2";

/// What is already in a folder under a download's name (V1's destination check).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Existing {
    pub directory_exists: bool,
    /// Something is at the name: a file, a folder or a link, even a broken one.
    pub file_exists: bool,
    /// A download, V2's or V1's, that hasn't finished is at the name.
    pub partial_download_exists: bool,
    /// When the name is taken, the first free `name (n).ext`.
    pub suggested_name: Option<String>,
}

/// What is in `directory` under `name`. A name is taken by anything at it, at its V2 part file or
/// at its V1 control file, or by a download `in_list` says has it and hasn't finished. Nothing
/// is created or changed.
pub fn existing(directory: &Path, name: Option<&str>, in_list: impl Fn(&str) -> bool) -> Existing {
    let directory_exists = directory.is_dir();
    let Some(name) = name.filter(|_| directory_exists) else {
        return Existing {
            directory_exists,
            ..Existing::default()
        };
    };
    // symlink_metadata doesn't follow a link: a broken link at the name counts.
    let at = |name: &str| std::fs::symlink_metadata(directory.join(name)).is_ok();
    let partial = |name: &str| {
        in_list(name)
            || at(&format!("{name}{PART_FILE_SUFFIX}"))
            || at(&format!("{name}{V1_PART_SUFFIX}"))
    };
    let file_exists = at(name);
    let partial_download_exists = partial(name);
    let suggested_name = (file_exists || partial_download_exists)
        .then(|| free_file_name(name, |n| at(n) || partial(n)))
        .flatten();
    Existing {
        directory_exists,
        file_exists,
        partial_download_exists,
        suggested_name,
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

    fn categories() -> Vec<Category> {
        let category =
            |id: &str, name: &str, extensions: &[&str], directory: Option<&str>| Category {
                id: id.to_owned(),
                name: name.to_owned(),
                color: "#112233".to_owned(),
                icon: "folder".to_owned(),
                extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
                directory: directory.map(absolute),
                builtin: false,
            };
        vec![
            category("documents", "Documents", &["pdf"], None),
            category("images", "Disk images", &["iso"], None),
            category("mine", "Mine", &["abc"], Some("Elsewhere")),
            category("odd", "A/B: c?", &["odd"], None),
        ]
    }

    fn choice(
        directory: Option<&str>,
        file_name: Option<&str>,
        category_id: Option<&str>,
    ) -> Choice {
        Choice {
            directory: directory.map(str::to_owned),
            file_name: file_name.map(str::to_owned),
            category_id: category_id.map(str::to_owned),
        }
    }

    /// `(folder, automatic, category id)` for `choice` with `settings`, the system's Downloads
    /// folder being `absolute("Downloads")`.
    fn where_with(
        settings: &FolderSettings,
        choice: &Choice,
    ) -> Result<(PathBuf, bool, Option<String>), ErrorCode> {
        let system = Some(PathBuf::from(absolute("Downloads")));
        resolve(choice, settings, &categories(), system)
            .map(|r| (r.directory, r.automatic_directory, r.category.map(|c| c.id)))
    }

    fn folder(rest: &str) -> PathBuf {
        PathBuf::from(absolute(rest))
    }

    /// A fresh folder for one test, removed afterwards.
    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new(test: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("v2-destination-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempFolder(p)
        }

        fn touch(&self, name: &str) {
            std::fs::write(self.0.join(name), b"x").unwrap();
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_free_name_or_a_missing_folder_is_reported_as_such() {
        let dir = TempFolder::new("free");
        assert_eq!(
            existing(&dir.0, Some("a.zip"), |_| false),
            Existing {
                directory_exists: true,
                ..Existing::default()
            }
        );
        assert_eq!(
            existing(&dir.0.join("missing"), Some("a.zip"), |_| false),
            Existing::default()
        );
        // No name yet: the server names it later, and the engine finds it a free one.
        dir.touch("a.zip");
        assert_eq!(
            existing(&dir.0, None, |_| false),
            Existing {
                directory_exists: true,
                ..Existing::default()
            }
        );
    }

    #[test]
    fn a_taken_name_gets_a_free_one_suggested() {
        let dir = TempFolder::new("taken");
        dir.touch("a.zip");
        dir.touch("a (1).zip.vdmpart");
        assert_eq!(
            existing(&dir.0, Some("a.zip"), |_| false),
            Existing {
                directory_exists: true,
                file_exists: true,
                partial_download_exists: false,
                suggested_name: Some("a (2).zip".to_owned()),
            }
        );
        // A V2 download that hasn't finished, with no file yet. The suggestion numbers the name
        // as given, as the engine does (`free_file_name`, #40).
        assert_eq!(
            existing(&dir.0, Some("a (1).zip"), |_| false),
            Existing {
                directory_exists: true,
                file_exists: false,
                partial_download_exists: true,
                suggested_name: Some("a (1) (1).zip".to_owned()),
            }
        );
        // A folder at the name takes it too.
        std::fs::create_dir(dir.0.join("b.zip")).unwrap();
        assert!(existing(&dir.0, Some("b.zip"), |_| false).file_exists);
    }

    #[test]
    fn a_v1_download_in_the_same_folder_is_never_written_over() {
        // V1 (aria2) writes the file at its name, with a control file beside it.
        let dir = TempFolder::new("v1");
        dir.touch("ubuntu.iso");
        dir.touch("ubuntu.iso.aria2");
        dir.touch("other.iso.aria2");
        assert_eq!(
            existing(&dir.0, Some("ubuntu.iso"), |_| false),
            Existing {
                directory_exists: true,
                file_exists: true,
                partial_download_exists: true,
                suggested_name: Some("ubuntu (1).iso".to_owned()),
            }
        );
        // Only V1's control file is there: the name is still taken.
        let other = existing(&dir.0, Some("other.iso"), |_| false);
        assert!(other.partial_download_exists && !other.file_exists);
        assert_eq!(other.suggested_name.as_deref(), Some("other (1).iso"));
    }

    #[test]
    fn an_automatic_category_also_chooses_the_folder() {
        let settings = FolderSettings::default();
        let at = |name| where_with(&settings, &choice(None, Some(name), None));
        assert_eq!(
            at("ubuntu.iso"),
            Ok((
                folder("Downloads").join("Disk images"),
                true,
                Some("images".to_owned())
            ))
        );
        // A file whose extension no category lists stays in the default folder.
        assert_eq!(at("blob.dat"), Ok((folder("Downloads"), true, None)));
        // The category's own folder, when it has one.
        assert_eq!(
            at("x.abc"),
            Ok((folder("Elsewhere"), true, Some("mine".to_owned())))
        );
        // The subfolder is a safe file name.
        assert_eq!(
            at("x.odd"),
            Ok((
                folder("Downloads").join("A_B_ c_"),
                true,
                Some("odd".to_owned())
            ))
        );
        // No name yet: the server will name the file, so no category yet.
        assert_eq!(
            where_with(&settings, &choice(None, None, None)),
            Ok((folder("Downloads"), true, None))
        );
    }

    #[test]
    fn a_chosen_folder_or_category_takes_precedence() {
        let settings = FolderSettings::default();
        let mine = absolute("mine");
        // A chosen folder wins; the category is still automatic.
        assert_eq!(
            where_with(&settings, &choice(Some(&mine), Some("disk.ISO"), None)),
            Ok((folder("mine"), false, Some("images".to_owned())))
        );
        // A chosen category wins over the file type, and decides the folder.
        assert_eq!(
            where_with(
                &settings,
                &choice(None, Some("notes.iso"), Some("documents"))
            ),
            Ok((
                folder("Downloads").join("Documents"),
                true,
                Some("documents".to_owned())
            ))
        );
        // A blank folder is no folder.
        assert_eq!(
            where_with(&settings, &choice(Some("  "), Some("a.pdf"), None)),
            Ok((
                folder("Downloads").join("Documents"),
                true,
                Some("documents".to_owned())
            ))
        );
    }

    #[test]
    fn the_settings_turn_the_automatic_choices_off() {
        let no_auto = FolderSettings {
            auto_categorize: false,
            ..FolderSettings::default()
        };
        assert_eq!(
            where_with(&no_auto, &choice(None, Some("ubuntu.iso"), None)),
            Ok((folder("Downloads"), true, None))
        );
        // A chosen category still has its folder.
        assert_eq!(
            where_with(&no_auto, &choice(None, Some("ubuntu.iso"), Some("images"))),
            Ok((
                folder("Downloads").join("Disk images"),
                true,
                Some("images".to_owned())
            ))
        );
        let no_subfolders = FolderSettings {
            category_subfolders: false,
            ..FolderSettings::default()
        };
        assert_eq!(
            where_with(&no_subfolders, &choice(None, Some("ubuntu.iso"), None)),
            Ok((folder("Downloads"), true, Some("images".to_owned())))
        );
        let own_default = FolderSettings {
            default_directory: absolute("Default"),
            ..FolderSettings::default()
        };
        assert_eq!(
            where_with(&own_default, &choice(None, Some("blob.dat"), None)),
            Ok((folder("Default"), true, None))
        );
    }

    #[test]
    fn what_cant_be_resolved_is_refused() {
        let settings = FolderSettings::default();
        assert_eq!(
            where_with(&settings, &choice(None, None, Some("c-404"))),
            Err(ErrorCode::NotFound)
        );
        assert_eq!(
            where_with(&settings, &choice(Some("relative"), None, None)),
            Err(ErrorCode::InvalidPath)
        );
        assert_eq!(
            resolve(&choice(None, None, None), &settings, &categories(), None),
            Err(ErrorCode::DestinationMissing),
            "no system Downloads folder and none set"
        );
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
