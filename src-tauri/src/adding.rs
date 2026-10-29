//! Adding downloads (#60): the Add dialog's command, what the engine can do, the `.torrent` and
//! SSH key choosers, and `.torrent` files dropped on the window. Commands stay thin: the manager
//! checks every value.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::{DialogExt, FilePath};
use velocity_core::domain::{Capabilities, ErrorCode};
use velocity_core::manager::{AddRequest, AddResult, FETCH_WAIT, MAX_ADD};

use crate::downloads::{on_manager, Downloads};

/// The event `.torrent` files dropped on the window are sent as: the window opens the Add dialog
/// with them.
pub const TORRENTS_DROPPED_EVENT: &str = "torrents-dropped";

/// What `torrents-dropped` carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroppedTorrents {
    /// The `.torrent` files, in the order dropped.
    pub paths: Vec<String>,
    /// Dropped things that aren't `.torrent` files, which nothing is done with.
    pub ignored: usize,
}

/// The `.torrent` files among `paths`: plain files named `.torrent`, never through a link, at
/// most [`MAX_ADD`]. The manager reads and checks each one again when it is added.
pub fn dropped_torrents(paths: &[PathBuf]) -> DroppedTorrents {
    let torrent = |path: &Path| {
        let named = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("torrent"));
        // symlink_metadata doesn't follow a link, so a link is never a plain file here.
        named
            && std::fs::symlink_metadata(path)
                .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
    };
    let mut dropped = DroppedTorrents::default();
    for path in paths {
        if torrent(path) && dropped.paths.len() < MAX_ADD {
            dropped.paths.push(path.to_string_lossy().into_owned());
        } else {
            dropped.ignored += 1;
        }
    }
    dropped
}

/// Sends the window the `.torrent` files among `paths`, when there is something to say.
pub fn on_dropped(app: &AppHandle, paths: &[PathBuf]) {
    let dropped = dropped_torrents(paths);
    if dropped != DroppedTorrents::default() {
        // Emitting fails only once the app is closing, when no window is left to show it.
        let _ = app.emit(TORRENTS_DROPPED_EVENT, dropped);
    }
}

/// Adds the Add dialog's links and `.torrent` files, fetching `.torrent` links first, and returns
/// a result for each, in order.
#[tauri::command]
pub async fn add_downloads(
    downloads: State<'_, Downloads>,
    request: AddRequest,
) -> Result<Vec<AddResult>, ErrorCode> {
    let service = Arc::clone(&downloads);
    // Fetching waits, off the command runtime's threads.
    tauri::async_runtime::spawn_blocking(move || service.add(request, FETCH_WAIT))
        .await
        .map_err(|_| ErrorCode::Internal)?
}

/// What the transfer engine can do, so the window offers only that.
#[tauri::command]
pub async fn get_capabilities(downloads: State<'_, Downloads>) -> Result<Capabilities, ErrorCode> {
    on_manager(&downloads, |m| m.capabilities()).await
}

/// A chooser's pick as paths; one that isn't a path on disk is `InvalidPath`.
fn picked_paths(picked: Vec<FilePath>) -> Result<Vec<String>, ErrorCode> {
    picked
        .into_iter()
        .map(|p| {
            p.into_path()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|_| ErrorCode::InvalidPath)
        })
        .collect()
}

/// Asks the user for `.torrent` files with the system's chooser. Empty when they cancel.
#[tauri::command]
pub async fn choose_torrents(app: AppHandle) -> Result<Vec<String>, ErrorCode> {
    let dialog = app
        .dialog()
        .file()
        .set_title("Choose .torrent files")
        .add_filter("Torrent files", &["torrent"]);
    // The chooser waits for the user: off the command runtime's threads.
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_files())
        .await
        .map_err(|_| ErrorCode::Internal)?;
    let paths = picked_paths(picked.unwrap_or_default())?;
    if paths.len() > MAX_ADD {
        return Err(ErrorCode::InvalidInput);
    }
    Ok(paths)
}

/// Asks the user for an SSH private key file, starting in `~/.ssh` when there is one. `null` when
/// they cancel. The engine reads the key when the download connects (#53).
#[tauri::command]
pub async fn choose_ssh_key(app: AppHandle) -> Result<Option<String>, ErrorCode> {
    let mut dialog = app.dialog().file().set_title("Choose an SSH private key");
    if let Some(ssh) = dirs_ssh().filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(ssh);
    }
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_file())
        .await
        .map_err(|_| ErrorCode::Internal)?;
    Ok(picked_paths(picked.into_iter().collect())?.pop())
}

/// `~/.ssh`, where SSH keeps keys by default.
fn dirs_ssh() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".ssh"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_plain_torrent_files_are_taken_from_a_drop() {
        let dir = std::env::temp_dir().join(format!("v2-dropped-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("folder.torrent")).unwrap();
        fs::write(dir.join("album.TORRENT"), b"d4:infoe").unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        let dropped = dropped_torrents(&[
            dir.join("album.TORRENT"),
            dir.join("notes.txt"),
            dir.join("folder.torrent"),
            dir.join("missing.torrent"),
        ]);
        assert_eq!(
            dropped,
            DroppedTorrents {
                paths: vec![dir.join("album.TORRENT").to_string_lossy().into_owned()],
                ignored: 3,
            }
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_dropped_link_to_a_torrent_is_not_followed() {
        let dir = std::env::temp_dir().join(format!("v2-dropped-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.torrent"), b"d4:infoe").unwrap();
        std::os::unix::fs::symlink(dir.join("real.torrent"), dir.join("link.torrent")).unwrap();
        let dropped = dropped_torrents(&[dir.join("link.torrent")]);
        assert_eq!(dropped.paths, Vec::<String>::new());
        assert_eq!(dropped.ignored, 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_drop_takes_at_most_one_requests_worth() {
        let dir = std::env::temp_dir().join(format!("v2-dropped-many-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.torrent"), b"d4:infoe").unwrap();
        let many = vec![dir.join("a.torrent"); MAX_ADD + 2];
        let dropped = dropped_torrents(&many);
        assert_eq!(dropped.paths.len(), MAX_ADD);
        assert_eq!(dropped.ignored, 2);
        fs::remove_dir_all(&dir).unwrap();
    }
}
