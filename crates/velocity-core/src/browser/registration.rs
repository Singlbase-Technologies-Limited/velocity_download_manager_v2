//! Registering the native host with each browser, for this user only (#65), ported from V1's
//! `native_registration.rs` with V2's host name (ADR-0003). Chromium browsers let only the
//! extension IDs in `allowed_origins` start the host, and Firefox only those in
//! `allowed_extensions`; both lists come from `branding.json`. The places follow each browser's
//! documentation:
//!
//! - Chrome: <https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging#native-messaging-host-location>
//! - Edge: <https://learn.microsoft.com/microsoft-edge/extensions/developer-guide/native-messaging>
//! - Firefox: <https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_manifests#manifest_location>
//!
//! **V1 is never touched:** every file and registry key is named after V2's host, which differs
//! from V1's, and unregistering removes a manifest only when it names V2's host.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::branding::branding;
use crate::domain::ErrorCode;
use crate::paths::Paths;

/// The browsers the host can be registered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Browser {
    Chrome,
    Chromium,
    Edge,
    Brave,
    Firefox,
}

impl Browser {
    pub const ALL: [Browser; 5] = [
        Browser::Chrome,
        Browser::Chromium,
        Browser::Edge,
        Browser::Brave,
        Browser::Firefox,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
            Self::Brave => "brave",
            Self::Firefox => "firefox",
        }
    }

    /// Where the browser looks for hosts in the registry, under `HKEY_CURRENT_USER\Software`.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn registry_path(self) -> &'static str {
        match self {
            Self::Chrome => r"Google\Chrome\NativeMessagingHosts",
            Self::Chromium => r"Chromium\NativeMessagingHosts",
            Self::Edge => r"Microsoft\Edge\NativeMessagingHosts",
            Self::Brave => r"BraveSoftware\Brave-Browser\NativeMessagingHosts",
            Self::Firefox => r"Mozilla\NativeMessagingHosts",
        }
    }
}

/// Whether the host is registered with a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registration {
    pub browser: Browser,
    /// V2's manifest is there (and, on Windows, the registry points to it).
    pub registered: bool,
    /// Where V2's manifest goes for this browser.
    pub manifest_path: String,
    /// The registered manifest starts this build's host; false when it names another copy, such
    /// as one moved or reinstalled since, which registering again fixes.
    pub host_matches: bool,
}

/// What the Settings screen's Browser section shows (#65): the host, V2's names, and where the
/// host is registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserIntegration {
    /// The native host beside the app, or `None` when it isn't there, which leaves nothing to
    /// register.
    pub host_path: Option<String>,
    pub native_host_name: String,
    pub chrome_extension_ids: Vec<String>,
    pub firefox_extension_id: String,
    pub registrations: Vec<Registration>,
}

impl BrowserIntegration {
    /// The integration as `locations` show it, with the host at `host`.
    pub fn of(locations: &Locations, host: Option<&Path>) -> Self {
        let b = branding();
        Self {
            host_path: host.map(|h| h.to_string_lossy().into_owned()),
            native_host_name: b.native_host_name.clone(),
            chrome_extension_ids: b.chrome_extension_ids.clone(),
            firefox_extension_id: b.firefox_extension_id.clone(),
            registrations: locations.status(host),
        }
    }

    /// A fixed example for the contract fixtures: registered with Chrome only.
    pub fn sample() -> Self {
        let b = branding();
        Self {
            host_path: Some("/opt/velocity/velocity-native-host-v2".to_owned()),
            native_host_name: b.native_host_name.clone(),
            chrome_extension_ids: b.chrome_extension_ids.clone(),
            firefox_extension_id: b.firefox_extension_id.clone(),
            registrations: Browser::ALL
                .iter()
                .map(|&browser| Registration {
                    browser,
                    registered: browser == Browser::Chrome,
                    manifest_path: format!(
                        "/home/user/.config/{}/NativeMessagingHosts/{}.json",
                        browser.key(),
                        b.native_host_name
                    ),
                    host_matches: browser == Browser::Chrome,
                })
                .collect(),
        }
    }
}

/// The manifest that lets `browser` start the host at `host`.
pub fn manifest(browser: Browser, host: &Path) -> Value {
    let b = branding();
    let mut m = json!({
        "name": b.native_host_name,
        "description": format!("{} browser integration", b.product_name),
        "path": host.to_string_lossy(),
        "type": "stdio",
    });
    if browser == Browser::Firefox {
        m["allowed_extensions"] = json!([b.firefox_extension_id]);
    } else {
        m["allowed_origins"] = json!(chrome_origins());
    }
    m
}

/// The origins of V2's Chrome extension: `chrome-extension://<id>/`.
pub fn chrome_origins() -> Vec<String> {
    branding()
        .chrome_extension_ids
        .iter()
        .map(|id| format!("chrome-extension://{id}/"))
        .collect()
}

/// Where this user's browsers look for native hosts. Tests point it at a folder and a registry
/// key of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    /// The home folder: Firefox's manifests on Linux, and the macOS ones.
    home: PathBuf,
    /// The configuration folder (`$XDG_CONFIG_HOME`): the Chromium browsers' on Linux.
    config: PathBuf,
    /// V2's data folder: the manifests on Windows, which the registry points to.
    data_dir: PathBuf,
    /// The registry key the browsers' keys are under, below `HKEY_CURRENT_USER`: `Software`.
    #[cfg_attr(not(windows), allow(dead_code))]
    registry_root: String,
}

impl Locations {
    /// This user's.
    pub fn for_user(paths: &Paths) -> Result<Self, ErrorCode> {
        let home = dirs::home_dir().ok_or(ErrorCode::Unsupported)?;
        let config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
        Ok(Self::new(
            home,
            config,
            paths.data_dir().to_owned(),
            "Software",
        ))
    }

    /// Explicit ones, for tests.
    pub fn new(home: PathBuf, config: PathBuf, data_dir: PathBuf, registry_root: &str) -> Self {
        Self {
            home,
            config,
            data_dir,
            registry_root: registry_root.to_owned(),
        }
    }

    /// The folder `browser` reads manifests from (Linux and macOS), or V2's own (Windows).
    fn manifest_dir(&self, browser: Browser) -> PathBuf {
        if cfg!(windows) {
            return self.data_dir.join("native-messaging");
        }
        if cfg!(target_os = "macos") {
            let base = self.home.join("Library/Application Support");
            return base.join(match browser {
                Browser::Chrome => "Google/Chrome/NativeMessagingHosts",
                Browser::Chromium => "Chromium/NativeMessagingHosts",
                Browser::Edge => "Microsoft Edge/NativeMessagingHosts",
                Browser::Brave => "BraveSoftware/Brave-Browser/NativeMessagingHosts",
                Browser::Firefox => "Mozilla/NativeMessagingHosts",
            });
        }
        match browser {
            Browser::Firefox => self.home.join(".mozilla/native-messaging-hosts"),
            Browser::Chrome => self.config.join("google-chrome/NativeMessagingHosts"),
            Browser::Chromium => self.config.join("chromium/NativeMessagingHosts"),
            Browser::Edge => self.config.join("microsoft-edge/NativeMessagingHosts"),
            Browser::Brave => self
                .config
                .join("BraveSoftware/Brave-Browser/NativeMessagingHosts"),
        }
    }

    /// Where V2's manifest for `browser` goes. On Windows each browser gets its own file, since
    /// they all live in V2's folder.
    pub fn manifest_path(&self, browser: Browser) -> PathBuf {
        let name = &branding().native_host_name;
        let file = if cfg!(windows) {
            format!("{name}.{}.json", browser.key())
        } else {
            format!("{name}.json")
        };
        self.manifest_dir(browser).join(file)
    }

    /// V2's registry key for `browser`, below `HKEY_CURRENT_USER`.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn registry_key(&self, browser: Browser) -> String {
        format!(
            r"{}\{}\{}",
            self.registry_root,
            browser.registry_path(),
            branding().native_host_name
        )
    }

    /// Registers the host at `host` with `browser`, for this user. `NotFound` when `host` isn't
    /// an absolute path to a file; `Io` when the manifest or the registry can't be written.
    pub fn register(&self, browser: Browser, host: &Path) -> Result<(), ErrorCode> {
        if !host.is_absolute() || !host.is_file() {
            return Err(ErrorCode::NotFound);
        }
        let path = self.manifest_path(browser);
        let dir = path.parent().ok_or(ErrorCode::InvalidPath)?;
        std::fs::create_dir_all(dir).map_err(|_| ErrorCode::Io)?;
        let body =
            serde_json::to_vec_pretty(&manifest(browser, host)).map_err(|_| ErrorCode::Internal)?;
        // Written beside it and renamed into place: a reader never sees half of it, and a link
        // already there is replaced, never followed.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, body).map_err(|_| ErrorCode::Io)?;
        std::fs::rename(&temp, &path).map_err(|_| ErrorCode::Io)?;
        #[cfg(windows)]
        registry::set_default(&self.registry_key(browser), &path.to_string_lossy())?;
        Ok(())
    }

    /// Unregisters the host from `browser`: removes V2's registry key and V2's manifest, and
    /// nothing else. A file in the manifest's place that doesn't name V2's host, or isn't a plain
    /// file, is left alone.
    pub fn unregister(&self, browser: Browser) -> Result<(), ErrorCode> {
        #[cfg(windows)]
        registry::delete(&self.registry_key(browser))?;
        let path = self.manifest_path(browser);
        if self.manifest(browser).is_some() {
            std::fs::remove_file(&path).map_err(|_| ErrorCode::Io)?;
        }
        Ok(())
    }

    /// V2's manifest for `browser`: a plain file there that names V2's host.
    fn manifest(&self, browser: Browser) -> Option<Value> {
        let path = self.manifest_path(browser);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let value: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        (value["name"].as_str() == Some(branding().native_host_name.as_str())).then_some(value)
    }

    /// Whether the host is registered with each browser, and whether as `host`.
    pub fn status(&self, host: Option<&Path>) -> Vec<Registration> {
        Browser::ALL
            .iter()
            .map(|&browser| {
                let path = self.manifest_path(browser);
                let manifest = self.manifest(browser);
                #[cfg(windows)]
                let registered = manifest.is_some()
                    && registry::get_default(&self.registry_key(browser)).as_deref()
                        == Some(&*path.to_string_lossy());
                #[cfg(not(windows))]
                let registered = manifest.is_some();
                let host_matches = registered
                    && host.is_some_and(|h| {
                        manifest.as_ref().and_then(|m| m["path"].as_str())
                            == Some(&*h.to_string_lossy())
                    });
                Registration {
                    browser,
                    registered,
                    manifest_path: path.to_string_lossy().into_owned(),
                    host_matches,
                }
            })
            .collect()
    }
}

/// `HKEY_CURRENT_USER` keys with one default string value: what Windows browsers read
/// (`windows-sys`, PO decision on #65).
#[cfg(windows)]
pub(crate) mod registry {
    use std::ptr;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegGetValueW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };

    use crate::domain::ErrorCode;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Creates `key` if needed and sets its default value to `value`.
    pub(crate) fn set_default(key: &str, value: &str) -> Result<(), ErrorCode> {
        let (key, value) = (wide(key), wide(value));
        let mut handle: HKEY = ptr::null_mut();
        // SAFETY: `key` is NUL-terminated and outlives the call; `handle` receives a key that is
        // closed below; the other pointers may be null.
        let created = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        };
        if created != ERROR_SUCCESS {
            return Err(ErrorCode::Io);
        }
        let bytes = u32::try_from(value.len() * 2).map_err(|_| ErrorCode::InvalidInput)?;
        // SAFETY: `handle` is the open key from above; a null name sets the default value;
        // `value` is a NUL-terminated UTF-16 string of `bytes` bytes, read during the call.
        let set =
            unsafe { RegSetValueExW(handle, ptr::null(), 0, REG_SZ, value.as_ptr().cast(), bytes) };
        // SAFETY: `handle` is open, and closed only here.
        unsafe { RegCloseKey(handle) };
        if set == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(ErrorCode::Io)
        }
    }

    /// The default value of `key`, if it exists and is a string.
    pub(crate) fn get_default(key: &str) -> Option<String> {
        let key = wide(key);
        let mut size = 0u32;
        // SAFETY: `key` is NUL-terminated; a null buffer asks for the size only.
        let sized = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                ptr::null(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        if sized != ERROR_SUCCESS {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: `buffer` holds `size` bytes, and RegGetValueW writes at most that many.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                ptr::null(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16(&buffer[..end]).ok()
    }

    /// Deletes `key`, which has no subkeys. A key that isn't there is fine.
    pub(crate) fn delete(key: &str) -> Result<(), ErrorCode> {
        let key = wide(key);
        // SAFETY: `key` is NUL-terminated and outlives the call.
        match unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            _ => Err(ErrorCode::Io),
        }
    }

    /// Deletes `key` and everything under it: the tests' own keys only.
    #[cfg(test)]
    pub(crate) fn delete_tree(key: &str) {
        let key = wide(key);
        // SAFETY: `key` is NUL-terminated and outlives the call.
        unsafe {
            windows_sys::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr())
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V1's host name and Firefox ID (V1 `branding.json` at e9d717a8).
    const V1_HOST: &str = "com.velocity.download_manager";

    struct Fixture {
        dir: PathBuf,
        locations: Locations,
        host: PathBuf,
        #[cfg_attr(not(windows), allow(dead_code))]
        registry_root: String,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("v2-reg-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let host = dir.join("velocity-native-host-v2");
            std::fs::write(&host, b"host").unwrap();
            // A key of the test's own, never the browsers' real ones.
            let registry_root = format!(r"Software\VelocityV2Tests\{name}-{}", std::process::id());
            let locations = Locations::new(
                dir.join("home"),
                dir.join("config"),
                dir.join("data"),
                &registry_root,
            );
            Fixture {
                dir,
                locations,
                host,
                registry_root,
            }
        }

        /// V1's manifest for `browser` where V1 puts it, and on Windows V1's registry key.
        fn install_v1(&self, browser: Browser) -> (PathBuf, Vec<u8>) {
            let v2 = self.locations.manifest_path(browser);
            let v1 = v2.with_file_name(
                v2.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .replace(&branding().native_host_name, V1_HOST),
            );
            std::fs::create_dir_all(v1.parent().unwrap()).unwrap();
            let body =
                serde_json::to_vec(&json!({"name": V1_HOST, "path": "/v1/host", "type": "stdio"}))
                    .unwrap();
            std::fs::write(&v1, &body).unwrap();
            #[cfg(windows)]
            registry::set_default(&self.v1_key(browser), &v1.to_string_lossy()).unwrap();
            (v1, body)
        }

        #[cfg(windows)]
        fn v1_key(&self, browser: Browser) -> String {
            format!(
                r"{}\{}\{V1_HOST}",
                self.registry_root,
                browser.registry_path()
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            #[cfg(windows)]
            {
                registry::delete_tree(&self.registry_root);
                // Removed once no other test's key is left in it.
                let _ = registry::delete(r"Software\VelocityV2Tests");
            }
        }
    }

    #[test]
    fn manifests_let_only_v2s_extensions_start_the_host() {
        let c = manifest(Browser::Chrome, Path::new("/opt/v/host"));
        assert_eq!(c["name"], branding().native_host_name);
        assert_eq!(c["type"], "stdio");
        assert_eq!(c["allowed_origins"], json!(chrome_origins()));
        assert_eq!(
            chrome_origins(),
            [format!(
                "chrome-extension://{}/",
                branding().chrome_extension_ids[0]
            )]
        );
        assert!(c.get("allowed_extensions").is_none());
        let f = manifest(Browser::Firefox, Path::new("/opt/v/host"));
        assert_eq!(
            f["allowed_extensions"],
            json!([branding().firefox_extension_id])
        );
        assert!(f.get("allowed_origins").is_none());
        assert_ne!(branding().native_host_name, V1_HOST);
    }

    #[test]
    fn registering_needs_the_host_to_exist() {
        let f = Fixture::new("missing");
        let missing = f.dir.join("nope");
        assert_eq!(
            f.locations.register(Browser::Chrome, &missing),
            Err(ErrorCode::NotFound)
        );
        assert_eq!(
            f.locations
                .register(Browser::Chrome, Path::new("relative-host")),
            Err(ErrorCode::NotFound)
        );
        assert!(!f.locations.status(None)[0].registered);
    }

    #[test]
    fn register_and_unregister_each_browser() {
        let f = Fixture::new("each");
        for browser in Browser::ALL {
            f.locations.register(browser, &f.host).unwrap();
            let s = f.locations.status(Some(&f.host));
            let mine = s.iter().find(|r| r.browser == browser).unwrap();
            assert!(
                mine.registered && mine.host_matches,
                "{browser:?}: {mine:?}"
            );
            let written: Value =
                serde_json::from_slice(&std::fs::read(f.locations.manifest_path(browser)).unwrap())
                    .unwrap();
            assert_eq!(written, manifest(browser, &f.host), "{browser:?}");
            #[cfg(windows)]
            assert_eq!(
                registry::get_default(&f.locations.registry_key(browser)).as_deref(),
                Some(&*f.locations.manifest_path(browser).to_string_lossy())
            );
        }
        // Another copy of the host is registered, but not as this one.
        let other = f.dir.join("other-host");
        std::fs::write(&other, b"x").unwrap();
        assert!(f
            .locations
            .status(Some(&other))
            .iter()
            .all(|r| r.registered && !r.host_matches));
        for browser in Browser::ALL {
            f.locations.unregister(browser).unwrap();
            assert!(!f.locations.manifest_path(browser).exists(), "{browser:?}");
            #[cfg(windows)]
            assert_eq!(
                registry::get_default(&f.locations.registry_key(browser)),
                None
            );
        }
        assert!(f
            .locations
            .status(Some(&f.host))
            .iter()
            .all(|r| !r.registered));
        // Unregistering again is fine.
        f.locations.unregister(Browser::Chrome).unwrap();
    }

    /// The acceptance test of #65: V2 registers with V1's manifests (and registry values) in
    /// place, and unregisters, without changing any of V1's.
    #[test]
    fn v1s_registration_is_never_touched() {
        let f = Fixture::new("v1");
        let v1: Vec<_> = Browser::ALL.iter().map(|&b| (b, f.install_v1(b))).collect();
        let unchanged = |after: &str| {
            for (browser, (path, body)) in &v1 {
                assert_eq!(
                    &std::fs::read(path).unwrap(),
                    body,
                    "{browser:?}: V1's manifest {after}"
                );
                #[cfg(windows)]
                assert_eq!(
                    registry::get_default(&f.v1_key(*browser)).as_deref(),
                    Some(&*path.to_string_lossy()),
                    "{browser:?}: V1's registry value {after}"
                );
            }
        };
        for browser in Browser::ALL {
            f.locations.register(browser, &f.host).unwrap();
        }
        unchanged("after registering V2");
        for browser in Browser::ALL {
            f.locations.unregister(browser).unwrap();
        }
        unchanged("after unregistering V2");
    }

    #[test]
    fn a_file_in_v2s_place_that_isnt_v2s_manifest_is_left_alone() {
        let f = Fixture::new("foreign");
        let path = f.locations.manifest_path(Browser::Firefox);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"name":"someone.else"}"#).unwrap();
        f.locations.unregister(Browser::Firefox).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"name":"someone.else"}"#);
        assert!(!f.locations.status(None)[4].registered);
    }

    #[cfg(not(windows))]
    #[test]
    fn manifests_go_where_each_browser_reads_them_on_linux() {
        let f = Fixture::new("places");
        let name = format!("{}.json", branding().native_host_name);
        let expect = [
            (Browser::Chrome, "config/google-chrome/NativeMessagingHosts"),
            (Browser::Chromium, "config/chromium/NativeMessagingHosts"),
            (Browser::Edge, "config/microsoft-edge/NativeMessagingHosts"),
            (
                Browser::Brave,
                "config/BraveSoftware/Brave-Browser/NativeMessagingHosts",
            ),
            (Browser::Firefox, "home/.mozilla/native-messaging-hosts"),
        ];
        for (browser, dir) in expect {
            assert_eq!(
                f.locations.manifest_path(browser),
                f.dir.join(dir).join(&name)
            );
        }
    }
}
