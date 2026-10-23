//! Core domain logic for Velocity Download Manager V2.
//!
//! The application identity and the engine-neutral domain types shared with the UI
//! (`domain`, docs/CONTRACTS.md).

pub mod branding;
pub mod database;
pub mod domain;
pub mod manager;
pub mod paths;
pub mod transfer;

use serde::Serialize;

/// Version of the command contract between the UI and the native side (docs/CONTRACTS.md).
/// Raise it whenever a command or a shared type changes incompatibly; the UI refuses to start
/// against a native side with a different version (`COMMAND_API_VERSION` in `src/contract.ts`).
/// Version 2 (#47) added [`domain::Progress::connection_detail`].
pub const COMMAND_API_VERSION: u32 = 2;

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
