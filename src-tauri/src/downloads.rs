//! The downloads: the manager on its own thread, its commands, and its snapshots as events (#58).
//! Commands stay thin: they check what the UI sent, ask the manager, and return what it answered.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use velocity_core::database::Database;
use velocity_core::domain::{BatchResult, DownloadsSnapshot, EngineHealth, ErrorCode};
use velocity_core::manager::{Manager, ManagerService, MoveDirection};
use velocity_core::paths::Paths;
use velocity_core::seeding::SeedPolicy;
use velocity_core::torrent_engine::{MissingTorrentEngine, TorrentEngine, TorrentSettings};
use velocity_core::transfer::{DirectTransfer, Limits, MissingEngine};
use velocity_torrent::supervisor::SupervisedTorrentEngine;
use velocity_transfer::supervisor::{SupervisedEngine, SupervisorOptions};

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
    let exe = std::env::current_exe()
        .map_err(|e| format!("{} ({e})", ErrorCode::EngineMissing.user_message()))?;
    let program = program_beside(&exe, "velocity-transfer");
    let engine: Box<dyn DirectTransfer> =
        match SupervisedEngine::start(program.clone(), SupervisorOptions::default()) {
            Ok(engine) => Box::new(engine),
            Err(code) => Box::new(MissingEngine::new(code, program)),
        };
    // The torrent settings are #30's; until then the defaults, which are V1's.
    let torrents: Box<dyn TorrentEngine> = match SupervisedTorrentEngine::start(
        program_beside(&exe, "velocity-torrent"),
        SupervisorOptions::default(),
        TorrentSettings::default(),
    ) {
        Ok(torrents) => Box::new(torrents),
        Err(code) => Box::new(MissingTorrentEngine::new(code)),
    };
    let manager = Manager::open(engine, Arc::new(database))
        .map_err(|e| e.to_string())?
        .with_torrents(torrents);
    let app = app.clone();
    Ok(Arc::new(ManagerService::start(manager, move |snapshot| {
        // Emitting fails only once the app is closing, when no window is left to show it.
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
    })))
}

/// Runs `f` on the manager's thread, off the window's thread.
async fn on_manager<R: Send + 'static>(
    downloads: &Downloads,
    f: impl FnOnce(&mut Manager) -> R + Send + 'static,
) -> Result<R, ErrorCode> {
    let service = Arc::clone(downloads);
    tauri::async_runtime::spawn_blocking(move || service.call(f))
        .await
        .map_err(|_| ErrorCode::Internal)?
}

/// A speed limit in KiB/s as V1's command takes it: 0 for none.
pub fn limit_from_kib(kib: u64) -> Result<Limits, ErrorCode> {
    let limits = Limits {
        max_download_bps: match kib {
            0 => None,
            kib => Some(kib.checked_mul(1024).ok_or(ErrorCode::InvalidInput)?),
        },
    };
    limits.check()?;
    Ok(limits)
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
    let limits = limit_from_kib(kib)?;
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
/// `change_torrent_files`).
#[tauri::command]
pub async fn choose_torrent_files(
    downloads: State<'_, Downloads>,
    id: String,
    files: Vec<u32>,
) -> Result<(), ErrorCode> {
    if files.len() > velocity_core::manager::MAX_BATCH {
        return Err(ErrorCode::InvalidInput);
    }
    on_manager(&downloads, move |m| m.choose_files(&id, files)).await?
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

    #[test]
    fn a_speed_limit_is_in_kib_per_second_and_0_is_none() {
        assert_eq!(limit_from_kib(0), Ok(Limits::default()));
        assert_eq!(
            limit_from_kib(64),
            Ok(Limits {
                max_download_bps: Some(64 * 1024)
            })
        );
        assert_eq!(limit_from_kib(u64::MAX), Err(ErrorCode::InvalidInput));
    }
}
