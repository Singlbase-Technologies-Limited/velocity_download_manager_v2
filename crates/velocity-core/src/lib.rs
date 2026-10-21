//! Core domain logic for Velocity Download Manager V2.
//!
//! At this bootstrap stage the crate only exposes the application's identity,
//! which the desktop shell shows and which must never be confused with the
//! original Velocity Download Manager (V1).

use serde::Serialize;

/// Product name shown in the window title and About screen.
///
/// Placeholder until the V2 identity decision (issue #11) is recorded; it must
/// stay distinct from V1's "Velocity Download Manager".
pub const PRODUCT_NAME: &str = "Velocity Download Manager V2 (dev)";

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

    /// V1's product name (`branding.json` at V1 commit e9d717a8). V2 must differ.
    const V1_PRODUCT_NAME: &str = "Velocity Download Manager";

    #[test]
    fn app_info_reports_the_crate_version() {
        assert_eq!(app_info().version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn product_name_is_distinct_from_v1() {
        let name = app_info().name;
        assert_ne!(name, V1_PRODUCT_NAME);
        assert!(name.contains("V2"), "{name} should identify itself as V2");
    }
}
