//! Opening a finished download's file, and showing it in its folder (#61; V1's
//! `open_download_file` and `open_download_folder`). The manager says what to open; the app opens
//! it with the system's own program. #72 hardens the file checks further.
//!
//! - **Only the download's own file:** its name inside its folder, a plain file or a torrent's
//!   folder, never anything reached through a link.
//! - **Programs ask first** (PO decision on #61): a file the system would run, by its extension or,
//!   on Unix, its permissions, opens only once the user confirmed it.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use super::Manager;
use crate::domain::{DownloadStatus, ErrorCode};
use crate::fs_safety::is_link;
use crate::naming::check_file_name_in_folder;

/// What "Open folder" shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderView {
    /// The download's file, or a torrent's folder, selected in the folder that holds it.
    Select(PathBuf),
    /// The folder alone: the download's file isn't there.
    Folder(PathBuf),
}

/// Extensions of files Windows or a Linux desktop would run or install rather than show, in
/// lowercase.
const PROGRAM_EXTENSIONS: &[&str] = &[
    "exe", "msi", "msix", "appx", "bat", "cmd", "com", "scr", "pif", "cpl", "msc", "hta", "lnk",
    "reg", "ps1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "jar", "sh", "run", "bin", "appimage",
    "desktop", "deb", "rpm",
];

impl Manager {
    /// The finished download's file to open: a plain file or a torrent's folder, inside the
    /// download's folder and not reached through a link. `InvalidState` before it finished,
    /// `NotFound` when the file is gone, `InvalidPath` for a link, and `ConfirmationRequired` for a
    /// program until `allow_program`.
    pub fn file_to_open(&self, id: &str, allow_program: bool) -> Result<PathBuf, ErrorCode> {
        let record = &self.downloads.get(id).ok_or(ErrorCode::NotFound)?.record;
        if !matches!(
            record.status,
            DownloadStatus::Completed | DownloadStatus::Seeding
        ) {
            return Err(ErrorCode::InvalidState);
        }
        let path = own_path(&record.directory, record.file_name.as_deref())
            .ok_or(ErrorCode::InvalidState)?;
        let meta = fs::symlink_metadata(&path).map_err(|_| ErrorCode::NotFound)?;
        if is_link(&meta) || !(meta.is_file() || meta.is_dir()) {
            return Err(ErrorCode::InvalidPath);
        }
        if meta.is_file() && is_program(&path, &meta) && !allow_program {
            return Err(ErrorCode::ConfirmationRequired);
        }
        Ok(path)
    }

    /// What "Open folder" shows for the download: its file selected, or the folder alone when the
    /// file isn't there or is a link. `DestinationMissing` when the folder is gone.
    pub fn folder_to_open(&self, id: &str) -> Result<FolderView, ErrorCode> {
        let record = &self.downloads.get(id).ok_or(ErrorCode::NotFound)?.record;
        let directory = PathBuf::from(&record.directory);
        if !directory.is_dir() {
            return Err(ErrorCode::DestinationMissing);
        }
        let file = own_path(&record.directory, record.file_name.as_deref()).filter(|path| {
            fs::symlink_metadata(path)
                .is_ok_and(|meta| !is_link(&meta) && (meta.is_file() || meta.is_dir()))
        });
        Ok(match file {
            Some(path) => FolderView::Select(path),
            None => FolderView::Folder(directory),
        })
    }
}

/// The download's file: its name, which must be one name and not a part file, in its folder.
fn own_path(directory: &str, file_name: Option<&str>) -> Option<PathBuf> {
    let name = file_name?;
    check_file_name_in_folder(name).ok()?;
    Some(Path::new(directory).join(name))
}

/// Whether the system would run the file rather than show it.
fn is_program(path: &Path, meta: &Metadata) -> bool {
    let by_name = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PROGRAM_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
    by_name || executable(meta)
}

#[cfg(unix)]
fn executable(meta: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

/// Windows runs files by their extension only.
#[cfg(not(unix))]
fn executable(_meta: &Metadata) -> bool {
    false
}
