//! Core domain logic for Velocity Download Manager V2.
//!
//! The application identity and the engine-neutral domain types shared with the UI
//! (`domain`, docs/CONTRACTS.md).

pub mod app_settings;
pub mod branding;
pub mod browser;
pub mod category;
pub mod crash;
pub mod database;
pub mod destination;
pub mod diagnostics;
pub mod domain;
pub mod fs_safety;
pub mod history;
pub mod keychain;
pub mod logs;
pub mod manager;
pub mod metalink;
pub mod naming;
pub mod notices;
pub mod paths;
pub mod performance;
pub mod proxy;
pub mod queue;
pub mod redact;
pub mod request;
pub mod secret;
pub mod seeding;
pub mod settings;
pub mod torrent;
pub mod torrent_engine;
pub mod torrent_files;
pub mod transfer;

use serde::Serialize;

/// Version of the command contract between the UI and the native side (docs/CONTRACTS.md).
/// Raise it whenever a command or a shared type changes incompatibly; the UI refuses to start
/// against a native side with a different version (`COMMAND_API_VERSION` in `src/contract.ts`).
/// Version 2 (#47) added [`domain::Progress::connection_detail`].
/// Version 3 (#46) added [`domain::DownloadRecord::verification`].
/// Version 4 (#57) gave [`domain::Capabilities`] what each protocol does
/// ([`domain::ProtocolCapabilities`]), in place of a list of kinds and one `validatorResume`.
/// Version 5 (#51) added [`domain::ProtocolCapabilities::encryption`].
/// Version 6 (#53) added [`domain::TransferError::host_key`].
/// Version 7 (#58) added the downloads commands and their types: [`domain::DownloadsSnapshot`],
/// [`domain::EngineHealth`] and [`domain::BatchResult`].
/// Version 8 (#58, torrents) added [`domain::DownloadsSnapshot::torrent_engine`].
/// Version 9 (#31) added the category commands, `set_download_category` and
/// `check_destination`, their types ([`category::Category`], [`domain::DestinationCheck`]) and
/// [`domain::ErrorCode::NameTaken`].
/// Version 10 (#29) added the window's commands (`get_shell_settings`, `set_appearance`,
/// `complete_onboarding`, `choose_directory`, `quit_app`), [`app_settings::ShellSettings`]
/// and the `quit-requested` event.
/// Version 11 (#60) added the Add dialog's commands (`add_downloads`, `get_capabilities`,
/// `choose_torrents`, `choose_ssh_key`), their types ([`manager::AddRequest`],
/// [`manager::AddResult`]) and the `torrents-dropped` event.
/// Version 12 (#59) added [`domain::DownloadRecord::retry_at_ms`].
pub const COMMAND_API_VERSION: u32 = 21;

/// Name, version and contract version of the running application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub command_api_version: u32,
}

/// Returns the application's identity.
pub fn app_info() -> AppInfo {
    AppInfo {
        // V2 replaces V1 under the name "Velocity Download Manager" (#11); development builds
        // add "(dev)" (branding.json, ADR-0003).
        name: branding::branding().product_name.as_str(),
        version: env!("CARGO_PKG_VERSION"),
        command_api_version: COMMAND_API_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_the_command_api_version_in_camel_case() {
        let json = serde_json::to_value(app_info()).unwrap();
        assert_eq!(json["commandApiVersion"], COMMAND_API_VERSION);
    }

    #[test]
    fn app_info_reports_the_crate_version() {
        assert_eq!(app_info().version, env!("CARGO_PKG_VERSION"));
    }

    /// PO decision on #11: V2 replaces V1 under the name "Velocity Download Manager".
    /// Development builds carry a "(dev)" suffix so they can be told apart from an
    /// installed V1. Separation from V1 comes from distinct identifiers, not the name.
    #[test]
    fn development_builds_use_the_product_name_with_a_dev_suffix() {
        assert_eq!(app_info().name, "Velocity Download Manager (dev)");
    }
}
