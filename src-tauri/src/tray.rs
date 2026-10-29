//! The tray icon (#29, V1's tray): show the window, pause or resume everything, and quit. Linux
//! needs an AppIndicator library when the app runs; without a tray the app keeps working, and
//! closing the window quits it.

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use velocity_core::domain::DownloadsSnapshot;
use velocity_core::notices::active_downloads;

use crate::downloads::Downloads;
use crate::shell::{request_quit, show_main_window};

const TRAY_ID: &str = "velocity-tray";

/// Creates the tray icon; an error means this desktop has no tray.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Velocity", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause_all", "Pause all", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume_all", "Resume all", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &pause, &resume, &separator, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(velocity_core::app_info().name)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "pause_all" => everything(app, true),
            "resume_all" => everything(app, false),
            "quit" => request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map(|_| ())
}

/// Pauses or resumes every download, as the window's buttons do.
fn everything(app: &AppHandle, pause: bool) {
    let downloads = Arc::clone(app.state::<Downloads>().inner());
    tauri::async_runtime::spawn_blocking(move || {
        // A failure shows in the window's next snapshot, as for the window's own buttons.
        let _ = downloads.call(move |m| if pause { m.pause_all() } else { m.resume_all() });
    });
}

/// Shows what is happening in the tray icon's tooltip.
pub fn show_activity(app: &AppHandle, snapshot: &DownloadsSnapshot) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip(velocity_core::app_info().name, snapshot)));
    }
}

/// "Name — 2 active, 1.5 MiB/s", or "Name" when nothing is active.
fn tooltip(name: &str, snapshot: &DownloadsSnapshot) -> String {
    let active = active_downloads(snapshot);
    if active == 0 {
        return name.to_owned();
    }
    let speed: u64 = snapshot
        .downloads
        .iter()
        .map(|r| r.progress.download_speed_bps)
        .sum();
    format!("{name} — {active} active, {}/s", human_bytes(speed))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use velocity_core::domain::{DownloadRecord, DownloadStatus};

    fn snapshot(rows: &[(DownloadStatus, u64)]) -> DownloadsSnapshot {
        let mut s = DownloadsSnapshot::sample();
        s.downloads = rows
            .iter()
            .enumerate()
            .map(|(i, (status, speed))| {
                let mut r = DownloadRecord::sample();
                r.id = format!("d{i}");
                r.status = *status;
                r.progress.download_speed_bps = *speed;
                r
            })
            .collect();
        s
    }

    #[test]
    fn the_tooltip_names_the_app_and_says_what_is_active() {
        assert_eq!(tooltip("Velocity", &snapshot(&[])), "Velocity");
        assert_eq!(
            tooltip("Velocity", &snapshot(&[(DownloadStatus::Paused, 0)])),
            "Velocity",
            "nothing active"
        );
        assert_eq!(
            tooltip(
                "Velocity",
                &snapshot(&[
                    (DownloadStatus::Downloading, 1024 * 1024),
                    (DownloadStatus::Downloading, 512 * 1024),
                    (DownloadStatus::Queued, 0),
                ])
            ),
            "Velocity — 2 active, 1.5 MiB/s"
        );
    }

    #[test]
    fn speeds_are_in_binary_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }
}
