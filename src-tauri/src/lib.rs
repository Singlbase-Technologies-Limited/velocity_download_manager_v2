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
        .expect("error while running the Velocity Download Manager desktop shell");
}

#[cfg(test)]
mod tests {
    /// V1 identity values (V1 `branding.json` and `vite.config.ts` at commit e9d717a8).
    const V1_IDENTIFIER: &str = "com.velocity.downloadmanager";
    const V1_DEV_URL: &str = "http://localhost:1420";

    fn config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json is valid JSON")
    }

    /// The identifier decides the data directory and single-instance lock, so V2 must
    /// never share V1's, even though both carry the "Velocity Download Manager" name.
    #[test]
    fn identifier_differs_from_v1() {
        let id = config()["identifier"].as_str().unwrap().to_owned();
        assert_ne!(id, V1_IDENTIFIER);
        assert!(
            !id.starts_with(&format!("{V1_IDENTIFIER}.")),
            "{id} would nest under V1's identifier"
        );
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
