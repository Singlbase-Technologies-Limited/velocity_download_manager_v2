//! Core domain logic for Velocity Download Manager V2.
//!
//! At this bootstrap stage the crate only exposes the application's name and
//! version, which the desktop shell shows.

use serde::Serialize;

/// Product name shown in the window title and About screen.
///
/// V2 replaces V1 under the name "Velocity Download Manager" (PO decision, #11).
/// Development builds add "(dev)" so they can be told apart from an installed V1;
/// release naming is settled with packaging (#81).
pub const PRODUCT_NAME: &str = "Velocity Download Manager (dev)";

/// Name and version of the running application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// Returns the application's identity.
pub fn app_info() -> AppInfo {
    AppInfo {
        name: PRODUCT_NAME,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

// CI demo for #14 (not merged).
// Second push, to supersede the first run (#14).
