//! Categories and destinations in the manager (#31): the category commands, a download's
//! category, and where a new download is placed. The rules are in [`crate::destination`] and
//! [`crate::category`]; here they meet the database and the downloads in the list.

use std::path::{Path, PathBuf};

use super::Manager;
use crate::category::{for_file_name, Category, CategoryInput};
use crate::database::Database;
use crate::destination::{
    existing, resolve, system_downloads, Choice, FolderSettings, OnConflict, Place, Placement,
};
use crate::domain::{DestinationCheck, ErrorCode};
use crate::metalink::create_folders;
use crate::naming::name_for_download;
use crate::request::MAX_URL_BYTES;

impl Manager {
    /// Every category, in the order the automatic category is chosen in.
    pub fn categories(&self) -> Result<Vec<Category>, ErrorCode> {
        self.database.categories().map_err(|e| e.code)
    }

    /// Adds a user category. `InvalidInput`, `InvalidPath` or `NameTaken`.
    pub fn create_category(&mut self, input: &CategoryInput) -> Result<Category, ErrorCode> {
        self.database.create_category(input).map_err(|e| e.code)
    }

    /// Changes a category. Downloads keep it, and nothing is moved.
    pub fn update_category(
        &mut self,
        id: &str,
        input: &CategoryInput,
    ) -> Result<Category, ErrorCode> {
        self.database.update_category(id, input).map_err(|e| e.code)
    }

    /// Deletes a user category: its downloads are left without one, and no file is touched.
    /// `NotFound`, or `InvalidState` for a built-in one.
    pub fn delete_category(&mut self, id: &str) -> Result<(), ErrorCode> {
        self.database.delete_category(id).map_err(|e| e.code)?;
        // The database cleared it on the stored rows; the records in the list follow.
        for download in self.downloads.values_mut() {
            if download.record.category_id.as_deref() == Some(id) {
                download.record.category_id = None;
            }
        }
        Ok(())
    }

    /// Sets or clears a download's category (V1's `set_download_category`). Nothing is moved.
    pub fn set_download_category(
        &mut self,
        id: &str,
        category_id: Option<&str>,
    ) -> Result<(), ErrorCode> {
        let category_id = self.known_category(category_id)?;
        let download = self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?;
        let mut record = download.record.clone();
        record.category_id = category_id;
        self.database.update_download(&record).map_err(|e| e.code)?;
        download.record = record;
        Ok(())
    }

    /// What adding `url` with `choice` would do, before it is added: the folder, the category,
    /// the name and whether it is taken. A typed name keeps the link's file type (V1 #46); with
    /// none, the link names the file, or the server does once it starts.
    pub fn check_destination(
        &self,
        url: &str,
        choice: &Choice,
    ) -> Result<DestinationCheck, ErrorCode> {
        self.checked(url, choice).map(|(check, _)| check)
    }

    /// [`Manager::check_destination`], with the category subfolder at the end of the folder.
    fn checked(
        &self,
        url: &str,
        choice: &Choice,
    ) -> Result<(DestinationCheck, Option<String>), ErrorCode> {
        if url.len() > MAX_URL_BYTES {
            return Err(ErrorCode::InvalidUrl);
        }
        let categories = self.categories()?;
        let settings = self
            .database
            .section::<FolderSettings>()
            .map_err(|e| e.code)?;
        let file_name = name_for_download(url, choice.file_name.as_deref(), |ext| {
            categories
                .iter()
                .any(|c| c.extensions.iter().any(|e| e == ext))
        })?;
        let choice = Choice {
            file_name: file_name.clone(),
            ..choice.clone()
        };
        let resolved = resolve(&choice, &settings, &categories, system_downloads())?;
        let found = existing(&resolved.directory, file_name.as_deref(), |name| {
            self.waiting_at(&resolved.directory, name)
        });
        let check = DestinationCheck {
            directory: resolved.directory.to_string_lossy().into_owned(),
            directory_exists: found.directory_exists,
            automatic_directory: resolved.automatic_directory,
            category_id: resolved.category.map(|c| c.id),
            file_name,
            file_exists: found.file_exists,
            partial_download_exists: found.partial_download_exists,
            suggested_name: found.suggested_name,
        };
        Ok((check, resolved.subfolder))
    }

    /// Where to add `url` with `choice`. A taken name is asked about (`Conflict`) or replaced by
    /// the first free one, as `on_conflict` says; an existing file is never replaced (PO decision
    /// on #31). `FileExists` when no free name is left.
    ///
    /// A missing category subfolder is created then, inside its existing parent; anything at its
    /// name but a plain folder, such as a link, is refused (`InvalidPath`). Any other missing folder is `DestinationMissing`: the default
    /// folder, a category's own and a chosen one.
    pub fn place(
        &self,
        url: &str,
        choice: &Choice,
        on_conflict: OnConflict,
    ) -> Result<Placement, ErrorCode> {
        let (check, subfolder) = self.checked(url, choice)?;
        let file_name = if check.file_exists || check.partial_download_exists {
            match on_conflict {
                OnConflict::Ask => return Ok(Placement::Conflict(check)),
                OnConflict::Rename => {
                    Some(check.suggested_name.clone().ok_or(ErrorCode::FileExists)?)
                }
            }
        } else {
            check.file_name
        };
        let directory = PathBuf::from(check.directory);
        ready_folder(&directory, subfolder, check.directory_exists)?;
        Ok(Placement::Placed(Place {
            directory,
            file_name,
            category_id: check.category_id,
        }))
    }

    /// Where to add a torrent with `choice`, whose file name, if any, chooses the automatic
    /// category, as [`Manager::place`] does for a link. `name` is the torrent's own: its file, or
    /// its folder for several files, once known. A taken one is asked about whatever
    /// `on_conflict` says, because a torrent's names come from its metadata and aren't changed;
    /// the check then suggests none.
    pub(super) fn place_torrent(
        &self,
        choice: &Choice,
        name: Option<&str>,
    ) -> Result<Placement, ErrorCode> {
        let categories = self.categories()?;
        let settings = self
            .database
            .section::<FolderSettings>()
            .map_err(|e| e.code)?;
        let resolved = resolve(choice, &settings, &categories, system_downloads())?;
        let found = existing(&resolved.directory, name, |n| {
            self.waiting_at(&resolved.directory, n)
        });
        let category_id = resolved.category.map(|c| c.id);
        if found.file_exists || found.partial_download_exists {
            return Ok(Placement::Conflict(DestinationCheck {
                directory: resolved.directory.to_string_lossy().into_owned(),
                directory_exists: found.directory_exists,
                automatic_directory: resolved.automatic_directory,
                category_id,
                file_name: name.map(str::to_owned),
                file_exists: found.file_exists,
                partial_download_exists: found.partial_download_exists,
                suggested_name: None,
            }));
        }
        ready_folder(
            &resolved.directory,
            resolved.subfolder,
            found.directory_exists,
        )?;
        Ok(Placement::Placed(Place {
            directory: resolved.directory,
            file_name: None,
            category_id,
        }))
    }

    /// `id` when that category exists, `NotFound` when it doesn't.
    pub(super) fn known_category(&self, id: Option<&str>) -> Result<Option<String>, ErrorCode> {
        let Some(id) = id else { return Ok(None) };
        match self.database.category(id).map_err(|e| e.code)? {
            Some(category) => Ok(Some(category.id)),
            None => Err(ErrorCode::NotFound),
        }
    }

    /// A download in the list that hasn't finished is to be saved as `name` in `directory`.
    fn waiting_at(&self, directory: &Path, name: &str) -> bool {
        self.downloads.values().any(|d| {
            !d.record.status.is_terminal()
                && d.record.file_name.as_deref() == Some(name)
                && Path::new(&d.record.directory) == directory
        })
    }
}

/// Makes `directory` ready for a new download: a missing category `subfolder` at its end is created
/// inside its existing parent; any other missing folder is `DestinationMissing`.
fn ready_folder(
    directory: &Path,
    subfolder: Option<String>,
    directory_exists: bool,
) -> Result<(), ErrorCode> {
    match (subfolder, directory.parent()) {
        // Always through `create_folders`, even when the folder seems to exist: `is_dir`
        // follows a link, and a link in place of the subfolder must be refused, not used.
        (Some(subfolder), Some(parent)) => {
            create_folders(parent, &[subfolder])?;
        }
        _ if !directory_exists => return Err(ErrorCode::DestinationMissing),
        _ => {}
    }
    Ok(())
}

/// The automatic category for a name the server gave after the download started, when automatic
/// categories are on. Settings or categories that can't be read give none: adding and the
/// destination check report that error.
pub(super) fn automatic(database: &Database, name: Option<&str>) -> Option<String> {
    let name = name?;
    let on = database.section::<FolderSettings>().ok()?.auto_categorize;
    let categories = database.categories().ok()?;
    on.then(|| for_file_name(&categories, name).map(|c| c.id.clone()))
        .flatten()
}
