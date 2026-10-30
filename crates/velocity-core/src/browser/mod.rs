//! The browser integration (#65): the extension's messages ([`protocol`]), the local connection
//! between the native host and the app ([`ipc`]), what the app does with each message
//! ([`bridge`]), and registering the native host with each browser ([`registration`]). Ported from
//! V1 with V2's own names, so V1's extension and host never reach V2, nor V2's reach V1
//! (ADR-0003, docs/BROWSER_EXTENSION.md).

pub mod bridge;
pub mod ipc;
pub mod protocol;
pub mod registration;

use serde::{Deserialize, Serialize};

use crate::database::settings::Section;

/// What the app does with links from the browser (V1's `browser` settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserSettings {
    /// Take links from the browser extension at all.
    pub accept_requests: bool,
    /// Open the Add dialog for a link from the browser instead of adding it at once. Torrents and
    /// magnet links always open it.
    pub confirm_before_adding: bool,
}

impl Default for BrowserSettings {
    /// V1's: links are taken, and the Add dialog opens for each.
    fn default() -> Self {
        Self {
            accept_requests: true,
            confirm_before_adding: true,
        }
    }
}

impl Section for BrowserSettings {
    const KEY: &'static str = "browser";
}
