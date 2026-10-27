//! Categories and destinations in the manager (#31): the category commands, a download's
//! category, and where a new download is placed. The rules are in [`crate::destination`] and
//! [`crate::category`]; here they meet the database and the downloads in the list.

use std::path::{Path, PathBuf};

use super::Manager;
use crate::category::{for_file_name, Category, CategoryInput};
use crate::database::Database;
use crate::destination::{
    existing, resolve, system_downloads, Choice, OnConflict, Place, Placement,
};
use crate::domain::{DestinationCheck, ErrorCode};
use crate::naming::{file_name_for_link, with_link_extension};

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
        let categories = self.categories()?;
        let settings = self.database.folder_settings().map_err(|e| e.code)?;
        let link_name = file_name_for_link(url);
        let file_name = match choice.file_name.as_deref().map(str::trim) {
            Some(typed) if !typed.is_empty() => {
                Some(with_link_extension(typed, link_name.as_deref(), |ext| {
                    categories
                        .iter()
                        .any(|c| c.extensions.iter().any(|e| e == ext))
                })?)
            }
            _ => link_name,
        };
        let choice = Choice {
            file_name: file_name.clone(),
            ..choice.clone()
        };
        let resolved = resolve(&choice, &settings, &categories, system_downloads())?;
        let found = existing(&resolved.directory, file_name.as_deref(), |name| {
            self.waiting_at(&resolved.directory, name)
        });
        Ok(DestinationCheck {
            directory: resolved.directory.to_string_lossy().into_owned(),
            directory_exists: found.directory_exists,
            automatic_directory: resolved.automatic_directory,
            category_id: resolved.category.map(|c| c.id),
            file_name,
            file_exists: found.file_exists,
            partial_download_exists: found.partial_download_exists,
            suggested_name: found.suggested_name,
        })
    }

    /// Where to add `url` with `choice`. A taken name is asked about (`Conflict`) or replaced by
    /// the first free one, as `on_conflict` says; an existing file is never replaced (PO decision
    /// on #31). `FileExists` when no free name is left.
    pub fn place(
        &self,
        url: &str,
        choice: &Choice,
        on_conflict: OnConflict,
    ) -> Result<Placement, ErrorCode> {
        let check = self.check_destination(url, choice)?;
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
        Ok(Placement::Placed(Place {
            directory: PathBuf::from(check.directory),
            file_name,
            category_id: check.category_id,
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

/// The automatic category for a name the server gave after the download started, when automatic
/// categories are on. Settings or categories that can't be read give none: adding and the
/// destination check report that error.
pub(super) fn automatic(database: &Database, name: Option<&str>) -> Option<String> {
    let name = name?;
    let on = database.folder_settings().ok()?.auto_categorize;
    let categories = database.categories().ok()?;
    on.then(|| for_file_name(&categories, name).map(|c| c.id.clone()))
        .flatten()
}
