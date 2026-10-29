//! The app's log, crash logs and diagnostics (#28). Every log line and crash log is redacted, and
//! nothing is sent anywhere: diagnostics are exported only to a file the user chooses
//! (DIAGNOSTICS.md, PRIVACY.md).

use std::path::PathBuf;

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tracing_subscriber::layer::SubscriberExt;
use velocity_core::crash::write_crash_log;
use velocity_core::diagnostics::DiagnosticReport;
use velocity_core::domain::ErrorCode;
use velocity_core::logs::AppLog;
use velocity_core::manager::now_ms;

use crate::downloads::{on_manager, Downloads};

/// Keeps the app's events in `log`, redacted, for the life of the process. Failing to install it
/// (only if something installed another first) leaves the app without a log, never without
/// redaction.
pub fn install_log(log: &AppLog) {
    let subscriber = tracing_subscriber::registry().with(log.layer());
    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// On a panic: the app's log says so, a redacted crash log is written in `crash_dir`, and the
/// default hook then reports it as usual. Nothing is uploaded (#28's crash policy).
pub fn install_crash_logs(crash_dir: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = format!(
            "Velocity {} panicked\n{info}\n\n{}",
            velocity_core::app_info().version,
            std::backtrace::Backtrace::force_capture()
        );
        tracing::error!("{info}");
        if let Err(e) = write_crash_log(&crash_dir, &report, now_ms()) {
            tracing::error!("the crash log couldn't be written: {e}");
        }
        default(info);
    }));
}

/// What the diagnostics show now (V1's `get_diagnostics`).
#[tauri::command]
pub async fn get_diagnostics(
    downloads: State<'_, Downloads>,
    log: State<'_, AppLog>,
) -> Result<DiagnosticReport, ErrorCode> {
    let lines = log.lines();
    on_manager(&downloads, move |m| m.diagnostics(lines, now_ms())).await
}

/// Saves the diagnostics, redacted, to a file the user chooses (V1's `export_diagnostics`).
/// Returns where; `null` when they cancel.
#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    downloads: State<'_, Downloads>,
    log: State<'_, AppLog>,
) -> Result<Option<String>, ErrorCode> {
    let lines = log.lines();
    let report = on_manager(&downloads, move |m| m.diagnostics(lines, now_ms())).await?;
    let text = report.export()?;
    let dialog = app
        .dialog()
        .file()
        .set_title("Export diagnostics")
        .set_file_name("velocity-diagnostics.json")
        .add_filter("JSON", &["json"]);
    // The chooser waits for the user: off the command runtime's threads.
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_save_file())
        .await
        .map_err(|_| ErrorCode::Internal)?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&path, text).map_err(|e| {
        tracing::warn!("the diagnostics couldn't be saved: {e}");
        ErrorCode::Io
    })?;
    tracing::info!("diagnostics exported");
    Ok(Some(path.to_string_lossy().into_owned()))
}
