//! V2's identity, from the repository's `branding.json` (#27, ADR-0003). One source for the
//! product name, the Tauri identifier (which decides the data directory and single-instance
//! key), IPC names and the browser integration's names.
//!
//! Development builds use the plain fields; packaging (#81–#83) switches to the `release*`
//! fields. `src-tauri/tauri.conf.json` must carry the same identifier and product name, which a
//! test in `src-tauri` checks.

use serde::Deserialize;
use std::sync::OnceLock;

/// The contents of `branding.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Branding {
    pub product_name: String,
    pub release_product_name: String,
    /// Tauri identifier for development builds: decides the data directory and single instance.
    pub identifier: String,
    pub release_identifier: String,
    pub native_host_name: String,
    pub release_native_host_name: String,
    /// The browser extension's name in development builds and in release builds (#64): it says
    /// "V2", so that V1's and V2's extensions can be told apart when both are installed.
    pub extension_name: String,
    pub release_extension_name: String,
    /// The short name the extension uses in menus and badges ("Download with …").
    pub extension_short_name: String,
    pub firefox_extension_id: String,
    /// The public half of V2's Chrome extension key (base64 DER), generated in #64. It pins the
    /// extension ID; the private half is never committed.
    pub chrome_extension_key: String,
    /// The extension IDs the native host allows: the one derived from `chrome_extension_key`
    /// (checked by `scripts/extension-lib.test.mjs`).
    pub chrome_extension_ids: Vec<String>,
}

const RAW: &str = include_str!("../../../branding.json");

/// V2's identity, parsed once. `branding.json` is compiled in and covered by tests, so a parse
/// failure is a build defect, not a runtime condition.
pub fn branding() -> &'static Branding {
    static B: OnceLock<Branding> = OnceLock::new();
    B.get_or_init(|| serde_json::from_str(RAW).expect("branding.json is valid (checked by tests)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V1's identity at commit e9d717a8 (V1 `branding.json`). V2 must reuse none of it.
    const V1_IDENTIFIER: &str = "com.velocity.downloadmanager";
    const V1_NATIVE_HOST: &str = "com.velocity.download_manager";
    const V1_FIREFOX_ID: &str = "velocity-download-manager@velocity.local";
    const V1_CHROME_ID: &str = "eecopkgfiffjgmngfmjmdaejbkbbippo";
    /// V1's extension name and Chrome key (V1 `branding.json`, `chromeExtensionKey`).
    const V1_EXTENSION_NAME: &str = "Velocity Download Manager";
    const V1_CHROME_KEY: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAo15wOoNApvSfuGVevsk7fqI3qo1KYF24IG1NWijAmrFWo0FtL1fVRXq5LZsVyQN1bkuobZufFxPArf0B+CmXOfqkOzAeBAp/ZZPTRjFoIT5kOMtx/0MH63N1/+r5kpAYkmFqWTtukdjCiD3mKZGrinSpjbg2bOYV3RbvyIlH94MsQt8ANDWOx7ffQ+h8eGLYUNhrIWjtPT+tOnKnRPXxyQJjb+j3IiNINZ6PfSKpOiWTILdLdmOyg5QQdgXTFwPQ+YPMAsQ8Bf+TlvlaDgzGnEsvwKek0O2m2VdZ9dtBo/tP+t6VBGO4CkU+zi0SHGO2ZjM8yrPkGe81jbmFQ1/EnwIDAQAB";

    fn is_reverse_dns(s: &str) -> bool {
        let parts: Vec<_> = s.split('.').collect();
        parts.len() >= 3
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            })
    }

    /// Chrome's rule for native messaging host names: lowercase alphanumerics, `_` and `.`,
    /// not starting or ending with `.`, and no `..`.
    fn is_native_host_name(s: &str) -> bool {
        !s.is_empty()
            && !s.starts_with('.')
            && !s.ends_with('.')
            && !s.contains("..")
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
    }

    #[test]
    fn identifiers_are_well_formed() {
        let b = branding();
        assert!(is_reverse_dns(&b.identifier), "{}", b.identifier);
        assert!(
            is_reverse_dns(&b.release_identifier),
            "{}",
            b.release_identifier
        );
        assert!(
            is_native_host_name(&b.native_host_name),
            "{}",
            b.native_host_name
        );
        assert!(
            is_native_host_name(&b.release_native_host_name),
            "{}",
            b.release_native_host_name
        );
        let (local, domain) = b
            .firefox_extension_id
            .split_once('@')
            .expect("Firefox ID is email-like");
        assert!(!local.is_empty() && domain.contains('.'));
        for id in &b.chrome_extension_ids {
            assert!(
                id.len() == 32 && id.chars().all(|c| ('a'..='p').contains(&c)),
                "{id}"
            );
        }
    }

    #[test]
    fn nothing_is_reused_from_v1() {
        let b = branding();
        for id in [&b.identifier, &b.release_identifier] {
            assert_ne!(id, V1_IDENTIFIER);
        }
        for host in [&b.native_host_name, &b.release_native_host_name] {
            assert_ne!(host, V1_NATIVE_HOST);
        }
        assert_ne!(b.firefox_extension_id, V1_FIREFOX_ID);
        assert!(!b.chrome_extension_ids.iter().any(|i| i == V1_CHROME_ID));
        assert_ne!(b.chrome_extension_key, V1_CHROME_KEY);
        for name in [&b.extension_name, &b.release_extension_name] {
            assert_ne!(name, V1_EXTENSION_NAME);
            assert!(name.contains("V2"), "{name}");
        }
        assert!(b.extension_short_name.contains("V2"));
    }

    #[test]
    fn the_extension_has_a_chrome_id_and_key() {
        let b = branding();
        assert_eq!(b.chrome_extension_ids.len(), 1);
        assert!(
            b.chrome_extension_key.starts_with("MII"),
            "a base64 DER key"
        );
        assert_ne!(b.extension_name, b.release_extension_name);
    }

    #[test]
    fn development_and_release_identities_differ() {
        let b = branding();
        assert_ne!(b.identifier, b.release_identifier);
        assert_ne!(b.native_host_name, b.release_native_host_name);
        assert_ne!(b.product_name, b.release_product_name);
    }

    #[test]
    fn release_values_are_the_ones_adr_0003_records() {
        let b = branding();
        assert_eq!(b.release_identifier, "com.velocity.downloadmanager.v2");
        assert_eq!(
            b.release_native_host_name,
            "com.velocity.download_manager_v2"
        );
        assert_eq!(
            b.firefox_extension_id,
            "velocity-download-manager-v2@velocity.local"
        );
        assert_eq!(b.identifier, "dev.velocity.downloadmanager.v2");
        assert_eq!(b.release_product_name, "Velocity Download Manager");
    }
}
