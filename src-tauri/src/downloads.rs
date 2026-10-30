//! The downloads: the manager on its own thread, its commands, and its snapshots as events (#58).
//! Commands stay thin: they check what the UI sent, ask the manager, and return what it answered.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager as _, State};
use velocity_core::database::Database;
use velocity_core::domain::{
    BatchResult, ConfirmKind, DownloadsSnapshot, EngineHealth, ErrorCode, KeptFile,
};
use velocity_core::history::{HistoryPage, HistoryQuery, StorageSettings};
use velocity_core::manager::{
    Manager, ManagerService, MoveDirection, SensitiveOptions, TorrentDetails, TorrentPreview,
};
use velocity_core::paths::Paths;
use velocity_core::seeding::SeedPolicy;
use velocity_core::torrent_engine::{
    MissingTorrentEngine, PeerStatus, TorrentEngine, TorrentSettings, TrackerStatus,
};
use velocity_core::transfer::{DirectTransfer, Limits, MissingEngine};
use velocity_torrent::supervisor::SupervisedTorrentEngine;
use velocity_transfer::supervisor::{SupervisedEngine, SupervisorOptions};

use crate::shell::ShellState;

/// The event each new snapshot is sent as.
pub const SNAPSHOT_EVENT: &str = "downloads-snapshot";

/// The manager, as the commands reach it.
pub type Downloads = Arc<ManagerService>;

/// Where the program `name` is: beside the app's own executable (PO decision on #58, until #81
/// packages the engines as sidecars). In development that is `target/debug`.
pub fn program_beside(app: &Path, name: &str) -> PathBuf {
    app.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// Opens the database, starts the transfer engine, the torrent helper and the manager. An engine
/// that can't start leaves the app running without it, saying why ([`MissingEngine`],
/// [`MissingTorrentEngine`]); a database that can't open stops it.
pub fn start(app: &AppHandle, paths: &Paths) -> Result<Downloads, String> {
    let database = Database::open(&paths.database()).map_err(|e| e.to_string())?;
    // What closing and notifications do (#29). A saved section this build can't read is left as
    // it was, and the defaults apply until it is fixed.
    let shell = app.state::<ShellState>();
    if let Ok(mut general) = shell.general.lock() {
        *general = database.section().unwrap_or_default();
    }
    if let Ok(mut notifications) = shell.notifications.lock() {
        *notifications = database.section().unwrap_or_default();
    }
    let exe = std::env::current_exe()
        .map_err(|e| format!("{} ({e})", ErrorCode::EngineMissing.user_message()))?;
    let program = program_beside(&exe, "velocity-transfer");
    let engine: Box<dyn DirectTransfer> =
        match SupervisedEngine::start(program.clone(), SupervisorOptions::default()) {
            Ok(engine) => Box::new(engine),
            Err(code) => {
                tracing::warn!("the transfer engine couldn't start: {code:?}");
                Box::new(MissingEngine::new(code, program))
            }
        };
    // The torrent helper starts with the saved settings (#30); a section that can't be read
    // leaves its defaults, which are V1's, and the Settings screen says why.
    let torrent_settings: TorrentSettings = database.section().unwrap_or_default();
    // Logins the user asked to remember are kept in the system keychain (#70), and so is the
    // proxy's password (#192), which the torrent helper gets before anything else.
    let keychain = velocity_core::keychain::SystemKeychain::new();
    let proxy =
        velocity_core::manager::saved_proxy(&database.section().unwrap_or_default(), &keychain);
    let torrents: Box<dyn TorrentEngine> = match SupervisedTorrentEngine::start_with_proxy(
        program_beside(&exe, "velocity-torrent"),
        SupervisorOptions::default(),
        torrent_settings,
        proxy,
    ) {
        Ok(torrents) => Box::new(torrents),
        Err(code) => {
            tracing::warn!("the torrent helper couldn't start: {code:?}");
            Box::new(MissingTorrentEngine::new(code))
        }
    };
    let mut manager = Manager::open_with_keychain(engine, Arc::new(database), Box::new(keychain))
        .map_err(|e| e.to_string())?
        .with_torrents(torrents)
        // `.torrent` links are fetched into the app's own folder (#60).
        .with_torrent_links(paths.torrent_links_dir())
        .map_err(|code| {
            format!(
                "{} ({})",
                code.user_message(),
                paths.torrent_links_dir().display()
            )
        })?;
    // The history's retention (#32, V1's), when the app starts. A failure is the snapshot's
    // storage error, which the window shows; the app runs on.
    let _ = manager.apply_retention(velocity_core::manager::now_ms());
    // The saved settings (#30): the queue, the engine, the limits and the switches.
    if let Err(code) = manager.apply_saved_settings(velocity_core::manager::now_ms()) {
        tracing::warn!("the saved settings couldn't be applied: {code:?}");
    }
    let app = app.clone();
    let mut last: Option<DownloadsSnapshot> = None;
    Ok(Arc::new(ManagerService::start(manager, move |snapshot| {
        // Emitting fails only once the app is closing, when no window is left to show it.
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
        if let Some(before) = &last {
            if before.engine != snapshot.engine {
                tracing::info!("transfer engine: {:?}", snapshot.engine);
            }
            if before.torrent_engine != snapshot.torrent_engine {
                tracing::info!("torrent helper: {:?}", snapshot.torrent_engine);
            }
            if before.storage_error != snapshot.storage_error {
                tracing::warn!("saving downloads: {:?}", snapshot.storage_error);
            }
        }
        app.state::<ShellState>()
            .observe(&app, last.as_ref(), snapshot);
        last = Some(snapshot.clone());
    })))
}

/// Runs `f` on the manager's thread, off the window's thread.
pub(crate) async fn on_manager<R: Send + 'static>(
    downloads: &Downloads,
    f: impl FnOnce(&mut Manager) -> R + Send + 'static,
) -> Result<R, ErrorCode> {
    let service = Arc::clone(downloads);
    tauri::async_runtime::spawn_blocking(move || service.call(f))
        .await
        .map_err(|_| ErrorCode::Internal)?
}

#[tauri::command]
pub async fn get_snapshot(downloads: State<'_, Downloads>) -> Result<DownloadsSnapshot, ErrorCode> {
    on_manager(&downloads, |m| m.snapshot()).await
}

#[tauri::command]
pub async fn pause_downloads(
    downloads: State<'_, Downloads>,
    ids: Vec<String>,
) -> Result<BatchResult, ErrorCode> {
    on_manager(&downloads, move |m| m.batch(&ids, Manager::pause)).await?
}

#[tauri::command]
pub async fn resume_downloads(
    downloads: State<'_, Downloads>,
    ids: Vec<String>,
) -> Result<BatchResult, ErrorCode> {
    on_manager(&downloads, move |m| m.batch(&ids, Manager::resume)).await?
}

#[tauri::command]
pub async fn cancel_downloads(
    downloads: State<'_, Downloads>,
    ids: Vec<String>,
) -> Result<BatchResult, ErrorCode> {
    on_manager(&downloads, move |m| m.batch(&ids, Manager::cancel)).await?
}

#[tauri::command]
pub async fn retry_downloads(
    downloads: State<'_, Downloads>,
    ids: Vec<String>,
) -> Result<BatchResult, ErrorCode> {
    on_manager(&downloads, move |m| m.batch(&ids, Manager::retry)).await?
}

#[tauri::command]
pub async fn retry_failed(downloads: State<'_, Downloads>) -> Result<(), ErrorCode> {
    on_manager(&downloads, Manager::retry_failed).await?
}

#[tauri::command]
pub async fn remove_downloads(
    downloads: State<'_, Downloads>,
    ids: Vec<String>,
    delete_files: bool,
) -> Result<BatchResult, ErrorCode> {
    on_manager(&downloads, move |m| m.remove_many(&ids, delete_files)).await?
}

#[tauri::command]
pub async fn pause_all(downloads: State<'_, Downloads>) -> Result<(), ErrorCode> {
    on_manager(&downloads, Manager::pause_all).await?
}

#[tauri::command]
pub async fn resume_all(downloads: State<'_, Downloads>) -> Result<(), ErrorCode> {
    on_manager(&downloads, Manager::resume_all).await?
}

#[tauri::command]
pub async fn clear_completed(downloads: State<'_, Downloads>) -> Result<usize, ErrorCode> {
    on_manager(&downloads, Manager::clear_completed).await?
}

/// Gives a download that came back after a restart without its login, custom headers or SSH key
/// those again (#59). The core checks them as when the download was added; the window then
/// resumes or retries it.
#[tauri::command]
pub async fn set_download_login(
    downloads: State<'_, Downloads>,
    id: String,
    login: SensitiveOptions,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.set_login(&id, login)).await?
}

#[tauri::command]
pub async fn move_download(
    downloads: State<'_, Downloads>,
    id: String,
    direction: MoveDirection,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.move_download(&id, direction)).await?
}

#[tauri::command]
pub async fn set_download_connections(
    downloads: State<'_, Downloads>,
    id: String,
    connections: u32,
) -> Result<u32, ErrorCode> {
    on_manager(&downloads, move |m| {
        m.set_download_connections(&id, connections)
    })
    .await?
}

#[tauri::command]
pub async fn set_download_speed_limit(
    downloads: State<'_, Downloads>,
    id: String,
    kib: u64,
) -> Result<(), ErrorCode> {
    let limits = Limits::from_kib(kib)?;
    on_manager(&downloads, move |m| m.set_download_limits(&id, limits)).await?
}

/// "Restart transfer engine": returns the health after asking.
#[tauri::command]
pub async fn restart_engine(downloads: State<'_, Downloads>) -> Result<EngineHealth, ErrorCode> {
    on_manager(&downloads, |m| {
        m.restart_engine()?;
        Ok(m.engine_health())
    })
    .await?
}

/// Downloads exactly `files` of a torrent, by index (V1's `start_torrent_files` and
/// `change_torrent_files`); with `delete_dropped`, deletes the torrent's own files no longer
/// chosen (#63). Returns the files kept, with why.
#[tauri::command]
pub async fn choose_torrent_files(
    downloads: State<'_, Downloads>,
    id: String,
    files: Vec<u32>,
    delete_dropped: bool,
) -> Result<Vec<KeptFile>, ErrorCode> {
    if files.len() > velocity_core::manager::MAX_BATCH {
        return Err(ErrorCode::InvalidInput);
    }
    on_manager(&downloads, move |m| {
        m.change_files(&id, files, delete_dropped)
    })
    .await?
}

/// A torrent's seeding policy, upload limit, files and the files chosen (#63).
#[tauri::command]
pub async fn get_torrent_details(
    downloads: State<'_, Downloads>,
    id: String,
) -> Result<TorrentDetails, ErrorCode> {
    on_manager(&downloads, move |m| m.torrent_details(&id)).await?
}

/// What adding the `.torrent` file at `path` would add (V1's `inspect_torrent_file`, #63).
#[tauri::command]
pub async fn preview_torrent_file(
    downloads: State<'_, Downloads>,
    path: String,
) -> Result<TorrentPreview, ErrorCode> {
    on_manager(&downloads, move |m| m.preview_torrent(Path::new(&path))).await?
}

/// A torrent's trackers, with passkeys redacted (#63).
#[tauri::command]
pub async fn get_torrent_trackers(
    downloads: State<'_, Downloads>,
    id: String,
) -> Result<Vec<TrackerStatus>, ErrorCode> {
    on_manager(&downloads, move |m| m.torrent_trackers(&id)).await?
}

/// A torrent's connected peers (#63).
#[tauri::command]
pub async fn get_torrent_peers(
    downloads: State<'_, Downloads>,
    id: String,
) -> Result<Vec<PeerStatus>, ErrorCode> {
    on_manager(&downloads, move |m| m.torrent_peers(&id)).await?
}

/// Announces a torrent to its trackers now (V1's `reannounce`, #63).
#[tauri::command]
pub async fn reannounce_torrent(
    downloads: State<'_, Downloads>,
    id: String,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.reannounce(&id)).await?
}

/// Sets a torrent's upload limit in KiB/s; 0 for none (V1's `set_upload_limit`, #63).
#[tauri::command]
pub async fn set_torrent_upload_limit(
    downloads: State<'_, Downloads>,
    id: String,
    kib: u64,
) -> Result<(), ErrorCode> {
    let bps = kib.checked_mul(1024).ok_or(ErrorCode::InvalidInput)?;
    on_manager(&downloads, move |m| m.set_upload_limit(&id, bps)).await?
}

/// Sets when a torrent stops seeding; `{"mode":"never"}` stops it now (V1's `stop_seeding`).
#[tauri::command]
pub async fn set_torrent_seeding(
    downloads: State<'_, Downloads>,
    id: String,
    seeding: SeedPolicy,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.set_seeding(&id, seeding)).await?
}

/// Starts a new torrent helper in place of the running one; returns its health after asking.
#[tauri::command]
pub async fn restart_torrent_helper(
    downloads: State<'_, Downloads>,
) -> Result<EngineHealth, ErrorCode> {
    on_manager(&downloads, |m| {
        m.restart_torrent_helper()?;
        Ok(m.torrent_helper_health())
    })
    .await?
}

/// A page of the history, newest first (V1's `get_history`, #32).
#[tauri::command]
pub async fn get_history(
    downloads: State<'_, Downloads>,
    query: HistoryQuery,
) -> Result<HistoryPage, ErrorCode> {
    on_manager(&downloads, move |m| m.history(&query)).await?
}

/// Clears the history; returns how many entries left it. Files are never touched (#32).
#[tauri::command]
pub async fn clear_history(downloads: State<'_, Downloads>) -> Result<usize, ErrorCode> {
    on_manager(&downloads, |m| m.clear_history()).await?
}

/// Takes finished downloads out of the history (V1's `delete_history_entries`, #32).
#[tauri::command]
pub async fn delete_history_entries(
    downloads: State<'_, Downloads>,
    ids: Vec<String>,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.delete_history(&ids)).await?
}

/// How long the history keeps entries (#32); #30's Settings screen shows them.
#[tauri::command]
pub async fn get_storage_settings(
    downloads: State<'_, Downloads>,
) -> Result<StorageSettings, ErrorCode> {
    on_manager(&downloads, |m| m.settings_section::<StorageSettings>()).await?
}

/// Saves the history's retention, as checked, and applies it at once (#32).
#[tauri::command]
pub async fn set_storage_settings(
    downloads: State<'_, Downloads>,
    settings: StorageSettings,
) -> Result<StorageSettings, ErrorCode> {
    on_manager(&downloads, move |m| {
        m.set_storage_settings(&settings, velocity_core::manager::now_ms())
    })
    .await?
}

/// Saves a download's login, custom headers and SSH key in the system keychain, at the user's
/// request (#70): after a restart they come back from there instead of being asked for again.
#[tauri::command]
pub async fn remember_login(downloads: State<'_, Downloads>, id: String) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.remember_login(&id)).await?
}

/// Trusts the SFTP host key the user confirmed (#53); the UI then retries the download.
#[tauri::command]
pub async fn trust_host_key(
    downloads: State<'_, Downloads>,
    host: String,
    port: u16,
    fingerprint: String,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| {
        m.trust_host_key(&host, port, &fingerprint)
    })
    .await?
}

/// Allows what a download stopped at for the user's confirmation, and retries it (#67): a
/// redirect from https to http, or a link from the browser to a local address (ADR-0006).
#[tauri::command]
pub async fn allow_download(
    downloads: State<'_, Downloads>,
    id: String,
    kind: ConfirmKind,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.allow_download(&id, kind)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engines_are_looked_for_beside_the_app() {
        let app = Path::new("/apps/velocity/velocity-download-manager-v2");
        for name in ["velocity-transfer", "velocity-torrent"] {
            let expected = format!("/apps/velocity/{name}{}", std::env::consts::EXE_SUFFIX);
            assert_eq!(program_beside(app, name), PathBuf::from(expected));
        }
    }
}
