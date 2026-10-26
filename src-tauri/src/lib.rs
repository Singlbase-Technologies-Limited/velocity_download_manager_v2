//! Tauri desktop shell. Commands stay thin: they delegate to `velocity-core`.

mod downloads;

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
    tauri::Builder::default()
        // Registered first, as the plugin requires. It is keyed on the app identifier, so a
        // second launch of V2 focuses this window, and never interferes with V1 (ADR-0003).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            // V2's own data directory (velocity_core::paths). Failing to create it stops the
            // app with the reason, instead of running without a place for its data.
            let paths = Paths::resolve().map_err(|code| code.user_message().to_owned())?;
            paths.ensure().map_err(|code| {
                format!("{} ({})", code.user_message(), paths.data_dir().display())
            })?;
            // The downloads (#58): the database, the engine and the manager.
            app.manage(downloads::start(app.handle(), &paths)?);
            Ok(())
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
            downloads::set_download_connections,
            downloads::set_download_speed_limit,
            downloads::restart_engine,
            downloads::trust_host_key,
            downloads::choose_torrent_files,
            downloads::set_torrent_seeding,
            downloads::restart_torrent_helper,
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
}
