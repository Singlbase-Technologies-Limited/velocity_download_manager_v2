//! Tauri desktop shell. Commands stay thin: they delegate to `velocity-core`.

mod adding;
mod browser;
mod categories;
mod diagnostics;
mod downloads;
mod opening;
mod shell;
mod tray;

use tauri::Manager;
use velocity_core::paths::Paths;
use velocity_core::AppInfo;

/// Returns the application's name, version and contract version.
#[tauri::command]
fn app_info() -> AppInfo {
    velocity_core::app_info()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The app's log, kept in memory and redacted (#28).
    let log = velocity_core::logs::AppLog::new();
    diagnostics::install_log(&log);
    tracing::info!("Velocity {} starting", velocity_core::app_info().version);
    tauri::Builder::default()
        // Registered first, as the plugin requires. It is keyed on the app identifier, so a
        // second launch of V2 focuses this window, and never interferes with V1 (ADR-0003).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            shell::show_main_window(app);
        }))
        // Used by native code only: the web layer is granted neither (capabilities/default.json).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(shell::ShellState::default())
        .manage(log)
        .setup(|app| {
            // V2's own data directory (velocity_core::paths). Failing to create it stops the
            // app with the reason, instead of running without a place for its data.
            let paths = Paths::resolve().map_err(|code| code.user_message().to_owned())?;
            paths.ensure().map_err(|code| {
                format!("{} ({})", code.user_message(), paths.data_dir().display())
            })?;
            // Crash logs in the app's own folder, never sent anywhere (#28).
            diagnostics::install_crash_logs(paths.crash_logs_dir());
            // The downloads (#58): the database, the engine and the manager.
            let downloads = downloads::start(app.handle(), &paths)?;
            // The browser bridge (#65): the native host reaches the app through it.
            browser::start(app.handle(), &paths, std::sync::Arc::clone(&downloads));
            app.manage(downloads);
            // The tray (#29). Without one, closing quits and the app never starts hidden.
            let shell = app.state::<shell::ShellState>();
            let has_tray = tray::create(app.handle()).is_ok();
            shell
                .tray
                .store(has_tray, std::sync::atomic::Ordering::Relaxed);
            // The window starts hidden (tauri.conf.json), so starting minimised never flashes it.
            if !(has_tray && shell.general().start_minimized) {
                shell::show_main_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                shell::on_close_requested(window.app_handle());
            }
            // `.torrent` files dropped on the window open the Add dialog (#60).
            tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                adding::on_dropped(window.app_handle(), paths);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            downloads::get_snapshot,
            downloads::pause_downloads,
            downloads::resume_downloads,
            downloads::cancel_downloads,
            downloads::retry_downloads,
            downloads::retry_failed,
            downloads::remove_downloads,
            downloads::pause_all,
            downloads::resume_all,
            downloads::clear_completed,
            downloads::move_download,
            downloads::set_download_login,
            downloads::set_download_connections,
            downloads::set_download_speed_limit,
            downloads::restart_engine,
            downloads::trust_host_key,
            downloads::allow_download,
            browser::get_browser_integration,
            browser::register_browser,
            browser::unregister_browser,
            downloads::remember_login,
            downloads::choose_torrent_files,
            downloads::set_torrent_seeding,
            downloads::restart_torrent_helper,
            diagnostics::get_diagnostics,
            diagnostics::export_diagnostics,
            downloads::get_history,
            downloads::clear_history,
            downloads::delete_history_entries,
            downloads::get_storage_settings,
            downloads::set_storage_settings,
            downloads::get_torrent_details,
            downloads::preview_torrent_file,
            downloads::get_torrent_trackers,
            downloads::get_torrent_peers,
            downloads::reannounce_torrent,
            downloads::set_torrent_upload_limit,
            opening::open_download_file,
            opening::open_download_folder,
            adding::add_downloads,
            adding::get_capabilities,
            adding::choose_torrents,
            adding::choose_ssh_key,
            categories::list_categories,
            categories::create_category,
            categories::update_category,
            categories::delete_category,
            categories::set_download_category,
            categories::check_destination,
            shell::get_shell_settings,
            shell::set_appearance,
            shell::complete_onboarding,
            shell::get_settings,
            shell::set_settings,
            shell::reset_settings,
            shell::get_proxy_password_set,
            shell::set_proxy_password,
            shell::get_torrent_network,
            shell::choose_directory,
            shell::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Velocity Download Manager desktop shell");
}

#[cfg(test)]
mod tests {
    use velocity_core::branding::branding;
    use velocity_core::paths::{windows_pipe_name, Paths};

    /// V1 identity values (V1 `branding.json` and `vite.config.ts` at commit e9d717a8).
    const V1_IDENTIFIER: &str = "com.velocity.downloadmanager";
    const V1_NATIVE_HOST: &str = "com.velocity.download_manager";
    const V1_DEV_URL: &str = "http://localhost:1420";

    fn config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json is valid JSON")
    }

    /// The Tauri identifier decides the data directory and the single-instance key, so it must
    /// be V2's identifier from branding.json, never V1's.
    #[test]
    fn tauri_identifier_is_the_v2_identifier_from_branding() {
        let id = config()["identifier"].as_str().unwrap().to_owned();
        assert_eq!(id, branding().identifier);
        assert_ne!(id, V1_IDENTIFIER);
    }

    /// PO decision on #27 (2026-10-22): the release identifier may start with V1's identifier
    /// (ADR-0003: com.velocity.downloadmanager.v2), because V1 uses its identifier exactly, never
    /// as a prefix. So every derived resource is checked for exact non-collision instead.
    #[test]
    fn no_derived_resource_collides_with_v1() {
        let b = branding();
        let base = std::path::PathBuf::from(if cfg!(windows) { "C:\\data" } else { "/data" });
        let v1_dir = base.join(V1_IDENTIFIER);
        for id in [&b.identifier, &b.release_identifier] {
            let dir = Paths::resolve_with(None, Some(base.clone()), id).unwrap();
            assert_ne!(dir.data_dir(), v1_dir.as_path(), "data directory");
            assert!(
                !dir.data_dir().starts_with(&v1_dir),
                "data directory inside V1's"
            );
            assert_ne!(
                windows_pipe_name(id, "u", dir.data_dir()),
                windows_pipe_name(V1_IDENTIFIER, "u", &v1_dir),
                "IPC pipe"
            );
            // The single-instance plugin keys on the identifier itself.
            assert_ne!(id.as_str(), V1_IDENTIFIER, "single-instance key");
        }
        for host in [&b.native_host_name, &b.release_native_host_name] {
            assert_ne!(host, V1_NATIVE_HOST, "native host name");
        }
    }

    /// The dialog and notification plugins are used by native code only (#29): the web layer is
    /// granted V1's core defaults and nothing else.
    #[test]
    fn the_web_layer_gets_only_the_core_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(
            capability["permissions"],
            serde_json::json!([
                "core:app:default",
                "core:event:default",
                "core:window:default"
            ])
        );
        assert_eq!(capability["windows"], serde_json::json!(["main"]));
    }

    #[test]
    fn dev_server_port_differs_from_v1() {
        assert_ne!(config()["build"]["devUrl"].as_str().unwrap(), V1_DEV_URL);
    }

    #[test]
    fn window_and_product_name_match_the_core() {
        let c = config();
        let name = velocity_core::app_info().name;
        assert_eq!(c["productName"].as_str().unwrap(), name);
        assert_eq!(c["app"]["windows"][0]["title"].as_str().unwrap(), name);
    }

    /// What `tauri build --config src-tauri/tauri.release.conf.json` merges over tauri.conf.json
    /// for the Windows installer (#81) and the Linux packages (#82).
    fn release_config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.release.conf.json"))
            .expect("tauri.release.conf.json is valid JSON")
    }

    /// The installer runs as the release identity, which the app's code gets from the
    /// release-identity feature: the two must agree, or the app's data folder and single-instance
    /// key would differ.
    #[test]
    fn the_release_config_carries_the_release_identity() {
        let (b, r) = (branding(), release_config());
        assert_eq!(r["identifier"].as_str().unwrap(), b.release_identifier);
        assert_eq!(r["productName"].as_str().unwrap(), b.release_product_name);
        assert_eq!(
            r["app"]["windows"][0]["title"].as_str().unwrap(),
            b.release_product_name
        );
    }

    /// A merge patch replaces arrays whole, so the release window repeats the development one:
    /// it may differ only in its title.
    #[test]
    fn the_release_window_is_the_development_window() {
        let mut dev = config()["app"]["windows"].clone();
        let mut release = release_config()["app"]["windows"].clone();
        dev[0].as_object_mut().unwrap().remove("title");
        release[0].as_object_mut().unwrap().remove("title");
        assert_eq!(release, dev);
    }

    /// One installer for this user, no MSI (PO decisions on #81), a deb and an rpm, no AppImage
    /// (PO decision on #82), with the programs the app starts from beside its own executable.
    #[test]
    fn the_installer_is_a_per_user_setup_with_every_program() {
        let bundle = &release_config()["bundle"];
        assert_eq!(bundle["targets"], serde_json::json!(["nsis", "deb", "rpm"]));
        assert!(bundle["linux"].get("appimage").is_none());
        assert_eq!(bundle["windows"]["nsis"]["installMode"], "currentUser");
        let programs: Vec<&str> = bundle["externalBin"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().trim_start_matches("binaries/"))
            .collect();
        assert_eq!(
            programs,
            [
                crate::downloads::ENGINE_PROGRAM,
                crate::downloads::TORRENT_PROGRAM,
                crate::browser::HOST_PROGRAM
            ]
        );
        assert!(
            !programs.iter().any(|p| p.contains("aria2")),
            "no aria2 (ADR-0001)"
        );
    }

    /// Uninstalling removes V2's browser registrations, by the release host's name, for every
    /// browser the app registers with, and never V1's.
    #[test]
    fn the_uninstaller_removes_v2s_browser_keys_only() {
        let hooks = include_str!("../windows/installer-hooks.nsh");
        let host = &branding().release_native_host_name;
        for browser in velocity_core::browser::registration::Browser::ALL {
            let key = format!(
                r#"DeleteRegKey HKCU "Software\{}\{host}""#,
                browser.registry_path()
            );
            assert!(hooks.contains(&key), "{key}");
        }
        assert_eq!(hooks.matches("DeleteRegKey").count(), 5);
        assert!(!hooks.contains(&format!(r#"\{V1_NATIVE_HOST}""#)));
        // Not on an upgrade, which runs the old uninstaller with /UPDATE.
        assert!(hooks.contains("${If} $UpdateMode <> 1"));
    }
}
