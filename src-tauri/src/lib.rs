//! Tauri desktop shell. Commands stay thin: they delegate to `velocity-core`.

use velocity_core::AppInfo;

/// Returns the application's name and version for the window and About screens.
#[tauri::command]
fn app_info() -> AppInfo {
    velocity_core::app_info()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("error while running the Velocity Download Manager V2 shell");
}
