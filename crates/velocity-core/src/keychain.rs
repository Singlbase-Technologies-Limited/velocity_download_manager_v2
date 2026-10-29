//! Logins saved in the system keychain, with the user's consent (#70, ADR-0015): Windows
//! Credential Manager, and on Linux the Secret Service (GNOME Keyring, KWallet), through the
//! `keyring` crate (PO decision on #70). Only a download whose login the user asked to remember is
//! saved, as one entry under V2's own service name, never V1's; removing the download deletes it.
//! Nothing else about a login is ever stored (docs/SECURITY.md#secrets-70).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use crate::branding::branding;
use crate::domain::ErrorCode;
use crate::manager::SensitiveOptions;

/// Where a download's remembered login is kept, by its app id.
pub trait Keychain: Send {
    /// Saves `login` for the download `id`, replacing any saved before. `KeychainUnavailable`
    /// when there is no keychain, or it refused.
    fn save(&self, id: &str, login: &SensitiveOptions) -> Result<(), ErrorCode>;
    /// The login saved for `id`, or `None` when there is none.
    fn load(&self, id: &str) -> Result<Option<SensitiveOptions>, ErrorCode>;
    /// Deletes the login saved for `id`; one that isn't there is fine.
    fn delete(&self, id: &str) -> Result<(), ErrorCode>;
}

/// No keychain: nothing can be remembered.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoKeychain;

impl Keychain for NoKeychain {
    fn save(&self, _id: &str, _login: &SensitiveOptions) -> Result<(), ErrorCode> {
        Err(ErrorCode::KeychainUnavailable)
    }

    fn load(&self, _id: &str) -> Result<Option<SensitiveOptions>, ErrorCode> {
        Ok(None)
    }

    fn delete(&self, _id: &str) -> Result<(), ErrorCode> {
        Ok(())
    }
}

/// The system's keychain, each login a JSON string under V2's service name.
#[derive(Debug, Clone)]
pub struct SystemKeychain {
    service: String,
}

impl SystemKeychain {
    /// V2's: `<identifier>.logins`, so V1's and V2's, and development and release builds, never
    /// share an entry.
    pub fn new() -> Self {
        Self::with_service(&format!("{}.logins", branding().identifier))
    }

    /// Under another service name, for tests.
    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }

    fn entry(&self, id: &str) -> Result<keyring::Entry, ErrorCode> {
        keyring::Entry::new(&self.service, id).map_err(|_| ErrorCode::KeychainUnavailable)
    }
}

impl Default for SystemKeychain {
    fn default() -> Self {
        Self::new()
    }
}

impl Keychain for SystemKeychain {
    fn save(&self, id: &str, login: &SensitiveOptions) -> Result<(), ErrorCode> {
        let json = Zeroizing::new(serde_json::to_string(login).map_err(|_| ErrorCode::Internal)?);
        self.entry(id)?
            .set_password(&json)
            .map_err(|_| ErrorCode::KeychainUnavailable)
    }

    fn load(&self, id: &str) -> Result<Option<SensitiveOptions>, ErrorCode> {
        match self.entry(id)?.get_password() {
            Ok(json) => {
                let json = Zeroizing::new(json);
                // A saved login this build can't read is as good as none: the user is asked.
                Ok(serde_json::from_str(&json).ok())
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(ErrorCode::KeychainUnavailable),
        }
    }

    fn delete(&self, id: &str) -> Result<(), ErrorCode> {
        match self.entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(ErrorCode::KeychainUnavailable),
        }
    }
}

/// A keychain in memory, for tests: what was saved, as the system one would keep it.
#[derive(Debug, Default, Clone)]
pub struct MemoryKeychain {
    entries: Arc<Mutex<HashMap<String, Zeroizing<String>>>>,
    refuse: bool,
}

impl MemoryKeychain {
    /// One that refuses every save, as an unavailable keychain does.
    pub fn refusing() -> Self {
        Self {
            refuse: true,
            ..Self::default()
        }
    }

    /// The ids with a saved login.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.lock().keys().cloned().collect();
        ids.sort();
        ids
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Zeroizing<String>>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Keychain for MemoryKeychain {
    fn save(&self, id: &str, login: &SensitiveOptions) -> Result<(), ErrorCode> {
        if self.refuse {
            return Err(ErrorCode::KeychainUnavailable);
        }
        let json = serde_json::to_string(login).map_err(|_| ErrorCode::Internal)?;
        self.lock().insert(id.to_owned(), Zeroizing::new(json));
        Ok(())
    }

    fn load(&self, id: &str) -> Result<Option<SensitiveOptions>, ErrorCode> {
        Ok(self
            .lock()
            .get(id)
            .and_then(|json| serde_json::from_str(json).ok()))
    }

    fn delete(&self, id: &str) -> Result<(), ErrorCode> {
        self.lock().remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{Credentials, Header, SshKey};

    fn login() -> SensitiveOptions {
        SensitiveOptions {
            credentials: Some(Credentials {
                user_name: "alice".to_owned(),
                password: "KEYCHAIN-SECRET-70".into(),
            }),
            ssh_key: Some(SshKey {
                path: std::env::temp_dir().join("id_ed25519"),
                passphrase: Some("PHRASE-70".into()),
            }),
            headers: vec![Header {
                name: "Cookie".to_owned(),
                value: "session=COOKIE-70".into(),
            }],
        }
    }

    #[test]
    fn without_a_keychain_nothing_is_remembered() {
        assert_eq!(
            NoKeychain.save("d-1", &login()),
            Err(ErrorCode::KeychainUnavailable)
        );
        assert_eq!(NoKeychain.load("d-1"), Ok(None));
        assert_eq!(NoKeychain.delete("d-1"), Ok(()));
    }

    #[test]
    fn v2s_service_name_is_its_own() {
        let k = SystemKeychain::new();
        assert_eq!(k.service, format!("{}.logins", branding().identifier));
        assert!(!k.service.starts_with("com.velocity.downloadmanager."));
        assert_ne!(k.service, "com.velocity.downloadmanager.logins");
    }

    /// The real keychain of the machine the test runs on, under a service of the test's own. Where
    /// it is unavailable (a Linux session without a Secret Service), saving says so, and that is
    /// what is checked; otherwise the login goes in and comes back whole, and is deleted.
    #[test]
    fn the_system_keychain_keeps_a_login_whole_or_says_it_cant() {
        let k = SystemKeychain::with_service(&format!(
            "{}.tests.{}",
            branding().identifier,
            std::process::id()
        ));
        let id = "d-test-70";
        match k.save(id, &login()) {
            Ok(()) => {
                assert_eq!(k.load(id), Ok(Some(login())));
                k.delete(id).unwrap();
                assert_eq!(k.load(id), Ok(None));
                k.delete(id).unwrap();
            }
            Err(code) => {
                eprintln!("the system keychain is unavailable here: {code:?}");
                assert_eq!(code, ErrorCode::KeychainUnavailable);
            }
        }
    }

    #[test]
    fn the_memory_keychain_behaves_like_one() {
        let k = MemoryKeychain::default();
        k.save("d-1", &login()).unwrap();
        assert_eq!(k.load("d-1"), Ok(Some(login())));
        assert_eq!(k.ids(), ["d-1"]);
        k.delete("d-1").unwrap();
        assert_eq!(k.load("d-1"), Ok(None));
        assert_eq!(
            MemoryKeychain::refusing().save("d-1", &login()),
            Err(ErrorCode::KeychainUnavailable)
        );
    }
}
