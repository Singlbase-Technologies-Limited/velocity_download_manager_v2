//! The proxy (#192, ADR-0008): its settings come from the settings document, and its password
//! from the user, held in memory, and in the system keychain only when the user ticked
//! "Remember" (PO decision on #192, as for logins, ADR-0015). Both engines are given the whole
//! proxy whenever either changes; while an HTTPS proxy is set, which libtorrent can't use, no
//! torrent is started.

use super::{Manager, SensitiveOptions};
use crate::domain::ErrorCode;
use crate::keychain::Keychain;
use crate::proxy::{Proxy, ProxyKind, ProxySettings};
use crate::request::check_credentials;
use crate::secret::Secret;
use crate::transfer::Credentials;

/// The keychain entry the proxy's password is kept under: no download's id looks like it.
const KEYCHAIN_ID: &str = "proxy";

/// The proxy as the app starts (#192): the saved settings, with the password remembered in
/// `keychain` when they ask for one. The torrent helper gets it before anything else, so no peer
/// or tracker connection goes around it.
pub fn saved_proxy(settings: &ProxySettings, keychain: &dyn Keychain) -> Option<Proxy> {
    settings.proxy(remembered_password(settings, keychain).as_ref())
}

/// The password remembered for the proxy, when the settings ask to remember it. A keychain that
/// can't be read leaves none: the user is asked again.
fn remembered_password(settings: &ProxySettings, keychain: &dyn Keychain) -> Option<Secret> {
    if !settings.remember_password {
        return None;
    }
    match keychain.load(KEYCHAIN_ID) {
        Ok(saved) => saved.and_then(|s| s.credentials).map(|c| c.password),
        Err(code) => {
            tracing::warn!("the proxy's saved password couldn't be read: {code:?}");
            None
        }
    }
}

impl Manager {
    /// Whether the proxy has a password, typed this session or remembered.
    pub fn proxy_password_set(&self) -> bool {
        self.proxy_password.is_some()
    }

    /// Sets the proxy's password, or clears it with `None`, and gives both engines the proxy
    /// with it at once. When the settings ask to remember it, it is saved in the keychain, or
    /// deleted from it. `InvalidInput` for a password a login can't have. `KeychainUnavailable`
    /// when the keychain refused: the password is still used until the app closes.
    pub fn set_proxy_password(&mut self, password: Option<String>) -> Result<(), ErrorCode> {
        let password = password.map(Secret::from);
        if let Some(password) = &password {
            check_credentials(&Credentials {
                user_name: "proxy".to_owned(),
                password: password.clone(),
            })?;
        }
        self.proxy_password = password;
        let settings = self.settings_section::<ProxySettings>()?;
        self.apply_proxy(&settings)?;
        if settings.remember_password {
            self.keep_proxy_password()?;
        }
        Ok(())
    }

    /// When the app starts: the remembered password, if the settings ask for one.
    pub(super) fn load_proxy_password(&mut self, settings: &ProxySettings) {
        if self.proxy_password.is_none() {
            self.proxy_password = remembered_password(settings, self.keychain.as_ref());
        }
    }

    /// After the settings changed: saves the password in the keychain when "Remember" was
    /// ticked, and deletes it when it was unticked.
    pub(super) fn proxy_remembering_changed(
        &mut self,
        before: &ProxySettings,
        now: &ProxySettings,
    ) -> Result<(), ErrorCode> {
        match (before.remember_password, now.remember_password) {
            (false, true) => self.keep_proxy_password(),
            (true, false) => self.keychain.delete(KEYCHAIN_ID),
            _ => Ok(()),
        }
    }

    /// Gives both engines the proxy of `settings`, with the password held, or none. An engine
    /// that isn't running gets it when it starts.
    pub(super) fn apply_proxy(&mut self, settings: &ProxySettings) -> Result<(), ErrorCode> {
        self.proxy_kind = settings.kind;
        let proxy = settings.proxy(self.proxy_password.as_ref());
        for result in [
            self.engine.set_proxy(proxy.clone()),
            self.torrents.set_proxy(proxy),
        ] {
            match result {
                Ok(()) | Err(ErrorCode::EngineUnavailable) => {}
                Err(code) => return Err(code),
            }
        }
        Ok(())
    }

    /// `Err(ProxyUnsupported)` while an HTTPS proxy is set: torrents can't use one.
    pub(super) fn torrents_can_use_the_proxy(&self) -> Result<(), ErrorCode> {
        if self.proxy_kind == ProxyKind::Https {
            Err(ErrorCode::ProxyUnsupported)
        } else {
            Ok(())
        }
    }

    fn keep_proxy_password(&mut self) -> Result<(), ErrorCode> {
        match &self.proxy_password {
            Some(password) => self.keychain.save(
                KEYCHAIN_ID,
                &SensitiveOptions {
                    credentials: Some(Credentials {
                        user_name: "proxy".to_owned(),
                        password: password.clone(),
                    }),
                    ..SensitiveOptions::default()
                },
            ),
            None => self.keychain.delete(KEYCHAIN_ID),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::database::Database;
    use crate::database::TorrentOrigin;
    use crate::keychain::MemoryKeychain;
    use crate::manager::{AddOptions, TorrentRequest};
    use crate::seeding::SeedPolicy;
    use crate::settings::Settings;
    use crate::torrent_engine::fake::{FakeTorrentControl, FakeTorrentEngine};
    use crate::transfer::fake::{FakeControl, FakeTransfer};

    const T0: u64 = 1_792_600_000_000;
    const PASSWORD: &str = "PROXY-PASSWORD-192";

    fn manager(
        db: &Arc<Database>,
        keychain: &MemoryKeychain,
    ) -> (Manager, FakeControl, FakeTorrentControl) {
        let engine = FakeTransfer::new();
        let control = engine.control();
        let torrents = FakeTorrentEngine::new();
        let torrent_control = torrents.control();
        let m = Manager::open_with_keychain(
            Box::new(engine),
            Arc::clone(db),
            Box::new(keychain.clone()),
        )
        .unwrap()
        .with_torrents(Box::new(torrents));
        (m, control, torrent_control)
    }

    fn with_proxy(kind: ProxyKind, remember: bool) -> Settings {
        Settings {
            proxy: ProxySettings {
                kind,
                host: "proxy.test".to_owned(),
                port: 3128,
                user_name: "alice".to_owned(),
                remember_password: remember,
            },
            ..Settings::default()
        }
    }

    fn engine_password(control: &FakeControl) -> Option<String> {
        control
            .proxy()
            .and_then(|p| p.login)
            .map(|l| l.password.expose().to_owned())
    }

    #[test]
    fn both_engines_get_the_proxy_with_its_password_which_isnt_saved() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let keychain = MemoryKeychain::default();
        let (mut m, control, torrents) = manager(&db, &keychain);
        m.set_settings(&with_proxy(ProxyKind::Http, false), T0)
            .unwrap();
        assert_eq!(
            control.proxy().map(|p| p.host),
            Some("proxy.test".to_owned())
        );
        assert_eq!(
            engine_password(&control),
            Some(String::new()),
            "no password yet"
        );
        assert!(!m.proxy_password_set());

        m.set_proxy_password(Some(PASSWORD.to_owned())).unwrap();
        assert!(m.proxy_password_set());
        assert_eq!(engine_password(&control).as_deref(), Some(PASSWORD));
        assert!(torrents
            .log()
            .contains(&"set_proxy proxy.test:3128".to_owned()));
        assert!(keychain.ids().is_empty(), "not remembered without consent");
        let saved = serde_json::to_string(&m.settings().unwrap()).unwrap();
        assert!(!saved.contains(PASSWORD), "{saved}");

        m.set_settings(&Settings::default(), T0).unwrap();
        assert_eq!(control.proxy(), None, "no proxy is set explicitly");
        assert!(torrents.log().contains(&"set_proxy none".to_owned()));
    }

    #[test]
    fn the_password_is_remembered_only_with_consent_and_comes_back_after_a_restart() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let keychain = MemoryKeychain::default();
        {
            let (mut m, _, _) = manager(&db, &keychain);
            m.set_settings(&with_proxy(ProxyKind::Socks5, true), T0)
                .unwrap();
            m.set_proxy_password(Some(PASSWORD.to_owned())).unwrap();
            assert_eq!(keychain.ids(), ["proxy"]);
        }
        let (mut m, control, _) = manager(&db, &keychain);
        m.apply_saved_settings(T0).unwrap();
        assert!(m.proxy_password_set());
        assert_eq!(engine_password(&control).as_deref(), Some(PASSWORD));
        assert_eq!(
            saved_proxy(&m.settings().unwrap().proxy, &keychain)
                .and_then(|p| p.login)
                .map(|l| l.password.expose().to_owned())
                .as_deref(),
            Some(PASSWORD),
            "what the torrent helper starts with"
        );

        m.set_settings(&with_proxy(ProxyKind::Socks5, false), T0)
            .unwrap();
        assert!(
            keychain.ids().is_empty(),
            "unticked: deleted from the keychain"
        );
        assert!(m.proxy_password_set(), "still held for this session");
    }

    #[test]
    fn a_refused_keychain_is_said_and_the_password_is_used_all_the_same() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let (mut m, control, _) = manager(&db, &MemoryKeychain::refusing());
        m.set_settings(&with_proxy(ProxyKind::Http, true), T0)
            .unwrap();
        assert_eq!(
            m.set_proxy_password(Some(PASSWORD.to_owned())),
            Err(ErrorCode::KeychainUnavailable)
        );
        assert_eq!(engine_password(&control).as_deref(), Some(PASSWORD));
    }

    #[test]
    fn a_password_a_login_cant_have_is_refused() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let (mut m, control, _) = manager(&db, &MemoryKeychain::default());
        m.set_settings(&with_proxy(ProxyKind::Http, false), T0)
            .unwrap();
        assert_eq!(
            m.set_proxy_password(Some("line\nbreak".to_owned())),
            Err(ErrorCode::InvalidInput)
        );
        assert!(!m.proxy_password_set());
        assert_eq!(engine_password(&control), Some(String::new()));
    }

    #[test]
    fn an_https_proxy_starts_no_torrent() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let (mut m, _, torrents) = manager(&db, &MemoryKeychain::default());
        m.set_settings(&with_proxy(ProxyKind::Https, false), T0)
            .unwrap();
        let dir = std::env::temp_dir();
        let added = m.add_torrent(
            TorrentRequest {
                origin: TorrentOrigin::Magnet {
                    uri: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned(),
                },
                directory: dir,
                trackers: Vec::new(),
                choose_files: false,
                files: None,
                seeding: SeedPolicy::Never {},
                upload_limit: 0,
                download_limit: 0,
            },
            AddOptions::default(),
            T0,
        );
        assert_eq!(added, Err(ErrorCode::ProxyUnsupported));
        assert!(
            !torrents.log().iter().any(|c| c == "add"),
            "{:?}",
            torrents.log()
        );
    }
}
