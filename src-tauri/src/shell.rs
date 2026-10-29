//! The window around the downloads (#29): its settings, the first-run setup, the folder chooser,
//! closing to the tray, quitting with downloads active, and desktop notifications. Commands stay
//! thin: the manager checks every value.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use velocity_core::app_settings::{
    AppearanceSettings, GeneralSettings, NotificationSettings, ShellSettings,
};
use velocity_core::destination::check_folder;
use velocity_core::domain::{DownloadsSnapshot, ErrorCode};
use velocity_core::manager::now_ms;
use velocity_core::notices::{active_downloads, notices};
use velocity_core::settings::Settings;
use velocity_core::torrent_engine::NetworkStatus;

use crate::downloads::{on_manager, Downloads};
use crate::tray;

/// The event the window is sent when quitting would stop active downloads: it asks the user,
/// then calls `quit_app`.
pub const QUIT_REQUESTED_EVENT: &str = "quit-requested";

/// What `quit-requested` carries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitRequest {
    /// The downloads quitting would stop.
    pub active: usize,
}

/// What the shell keeps between snapshots and window events.
#[derive(Default)]
pub struct ShellState {
    /// Read when the app starts, and changed by the Settings screen (#30).
    pub general: Mutex<GeneralSettings>,
    pub notifications: Mutex<NotificationSettings>,
    /// Downloads moving data now, from the last snapshot.
    active: AtomicUsize,
    /// Whether the tray icon exists: without one, closing quits and the app never starts hidden.
    pub tray: AtomicBool,
}

impl ShellState {
    pub fn general(&self) -> GeneralSettings {
        self.general.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn notifications(&self) -> NotificationSettings {
        self.notifications
            .lock()
            .map(|n| n.clone())
            .unwrap_or_default()
    }

    pub fn has_tray(&self) -> bool {
        self.tray.load(Ordering::Relaxed)
    }

    /// Takes in a new snapshot of the downloads: the active count, the tray's tooltip, and a
    /// notification for each change since `before` (none for the first one, which is what was
    /// already there).
    pub fn observe(
        &self,
        app: &AppHandle,
        before: Option<&DownloadsSnapshot>,
        after: &DownloadsSnapshot,
    ) {
        self.active
            .store(active_downloads(after), Ordering::Relaxed);
        if self.has_tray() {
            tray::show_activity(app, after);
        }
        let Some(before) = before else { return };
        for notice in notices(before, after, &self.notifications()) {
            // A desktop without a notification service still runs every download.
            let _ = app
                .notification()
                .builder()
                .title(notice.title)
                .body(notice.body)
                .show();
        }
    }
}

/// Shows the window, from the tray or a second launch.
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Quits, or, when downloads are active and the user wants to be asked, shows the window and
/// asks it to confirm (V1's confirm exit).
pub fn request_quit(app: &AppHandle) {
    let shell = app.state::<ShellState>();
    let active = shell.active.load(Ordering::Relaxed);
    if active == 0 || !shell.general().confirm_exit_with_active {
        app.exit(0);
        return;
    }
    show_main_window(app);
    if app
        .emit(QUIT_REQUESTED_EVENT, QuitRequest { active })
        .is_err()
    {
        // No window to ask: quitting can't be confirmed, so it goes ahead as asked.
        app.exit(0);
    }
}

/// What closing the window does: hide it in the tray, or quit.
pub fn on_close_requested(app: &AppHandle) {
    let shell = app.state::<ShellState>();
    if shell.has_tray() && shell.general().close_to_tray {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
    } else {
        request_quit(app);
    }
}

#[tauri::command]
pub async fn get_shell_settings(
    downloads: State<'_, Downloads>,
) -> Result<ShellSettings, ErrorCode> {
    on_manager(&downloads, |m| m.shell_settings()).await?
}

#[tauri::command]
pub async fn set_appearance(
    downloads: State<'_, Downloads>,
    appearance: AppearanceSettings,
) -> Result<AppearanceSettings, ErrorCode> {
    on_manager(&downloads, move |m| m.set_appearance(&appearance, now_ms())).await?
}

/// Every setting the Settings screen edits (#30).
#[tauri::command]
pub async fn get_settings(downloads: State<'_, Downloads>) -> Result<Settings, ErrorCode> {
    on_manager(&downloads, |m| m.settings()).await?
}

/// Checks, saves and applies the settings (#30); closing and notifications follow at once.
#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    downloads: State<'_, Downloads>,
    settings: Settings,
) -> Result<Settings, ErrorCode> {
    let saved = on_manager(&downloads, move |m| m.set_settings(&settings, now_ms())).await??;
    follow(&app, &saved);
    Ok(saved)
}

/// Back to the defaults, keeping the download folder (#30, V1's `reset_settings`).
#[tauri::command]
pub async fn reset_settings(
    app: AppHandle,
    downloads: State<'_, Downloads>,
) -> Result<Settings, ErrorCode> {
    let saved = on_manager(&downloads, |m| m.reset_settings(now_ms())).await??;
    follow(&app, &saved);
    Ok(saved)
}

/// The torrent helper's listening port and port mapping (#30).
#[tauri::command]
pub async fn get_torrent_network(
    downloads: State<'_, Downloads>,
) -> Result<NetworkStatus, ErrorCode> {
    on_manager(&downloads, |m| m.torrent_network()).await?
}

/// What closing and notifications do, from the settings just saved.
fn follow(app: &AppHandle, settings: &Settings) {
    let shell = app.state::<ShellState>();
    if let Ok(mut general) = shell.general.lock() {
        *general = settings.general.clone();
    }
    if let Ok(mut notifications) = shell.notifications.lock() {
        *notifications = settings.notifications.clone();
    };
}

/// Finishes the first-run setup with the folder the user chose for downloads.
#[tauri::command]
pub async fn complete_onboarding(
    downloads: State<'_, Downloads>,
    directory: String,
) -> Result<ShellSettings, ErrorCode> {
    on_manager(&downloads, move |m| {
        m.complete_onboarding(&directory, now_ms())
    })
    .await?
}

/// Asks the user for a folder with the system's chooser, starting in `initial` when it is a
/// valid folder. `null` when they cancel.
#[tauri::command]
pub async fn choose_directory(
    app: AppHandle,
    initial: Option<String>,
) -> Result<Option<String>, ErrorCode> {
    let mut dialog = app.dialog().file().set_title("Choose a download folder");
    if let Some(start) = initial.as_deref().and_then(|i| check_folder(i.trim()).ok()) {
        dialog = dialog.set_directory(start);
    }
    // The chooser waits for the user: off the command runtime's threads.
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
        .await
        .map_err(|_| ErrorCode::Internal)?;
    picked
        .map(|p| {
            p.into_path()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|_| ErrorCode::InvalidPath)
        })
        .transpose()
}

/// Quits after the user confirmed it in the window.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
