//! The proxy every download goes through, when the user sets one (#192, ADR-0008): HTTP, HTTPS or
//! SOCKS5, with an optional login. The settings document holds its type, host, port and user name
//! ([`ProxySettings`]), never its password. The password stays in memory, or in the system keychain
//! when the user ticks "Remember" (PO decision on #192, as for logins, ADR-0015). The engines are
//! given the whole [`Proxy`] through their pipes, like a download's login, and use it for every
//! transfer that starts after it is set: the libcurl layer for HTTP, HTTPS and FTP, the SFTP layer
//! through a tunnel of its own, and the torrent helper for peers and trackers. Proxy variables in
//! the environment (`http_proxy` and the like) are always ignored.

use serde::{Deserialize, Serialize};

use crate::database::settings::Section;
use crate::domain::ErrorCode;
use crate::request::check_credentials;
use crate::transfer::Credentials;

/// The longest host name kept: a DNS name's limit.
const MAX_HOST_BYTES: usize = 253;

/// What kind of proxy, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyKind {
    /// No proxy: every connection goes straight to its server.
    #[default]
    None,
    /// An HTTP proxy. Connections go through it as `CONNECT` tunnels, so it never reads a
    /// request, its headers or a site's login, even for `http://` links.
    Http,
    /// An HTTP proxy reached over TLS, whose certificate is verified as a server's is. The SFTP
    /// layer and the torrent helper can't use one (PO decision on #192).
    Https,
    /// A SOCKS5 proxy, which also resolves the names V2 connects to.
    Socks5,
}

/// The Settings screen's Proxy section (#192): saved in the settings document, without the
/// password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxySettings {
    pub kind: ProxyKind,
    /// A DNS name or an IP address (an IPv6 one in brackets), without a scheme or a port.
    pub host: String,
    pub port: u16,
    /// The proxy's login name; empty for none.
    pub user_name: String,
    /// Keep the password in the system keychain (unticked by default, ADR-0015).
    pub remember_password: bool,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            kind: ProxyKind::None,
            host: String::new(),
            port: 8080,
            user_name: String::new(),
            remember_password: false,
        }
    }
}

impl Section for ProxySettings {
    const KEY: &'static str = "proxy";

    /// With a proxy: a host that is a DNS name or an IP address (an international name is kept
    /// in its ASCII form, as links are, #67), a port from 1 to 65535, and a user name a login
    /// may have. Without one, the fields are kept as typed, for when it is turned on again.
    fn checked(&self) -> Result<Self, ErrorCode> {
        let user_name = self.user_name.trim().to_owned();
        if !user_name.is_empty() {
            check_credentials(&Credentials {
                user_name: user_name.clone(),
                password: "".into(),
            })?;
        }
        if self.kind == ProxyKind::None {
            if self.host.len() > MAX_HOST_BYTES || self.host.chars().any(char::is_control) {
                return Err(ErrorCode::InvalidInput);
            }
            return Ok(Self {
                user_name,
                ..self.clone()
            });
        }
        Ok(Self {
            host: check_host(&self.host)?,
            port: check_port(self.port)?,
            user_name,
            ..self.clone()
        })
    }
}

impl ProxySettings {
    /// The proxy the engines are given, with `password` when the settings name a user; `None`
    /// without a proxy.
    pub fn proxy(&self, password: Option<&crate::secret::Secret>) -> Option<Proxy> {
        if self.kind == ProxyKind::None {
            return None;
        }
        let login = (!self.user_name.is_empty()).then(|| Credentials {
            user_name: self.user_name.clone(),
            password: password.cloned().unwrap_or_default(),
        });
        Some(Proxy {
            kind: self.kind,
            host: self.host.clone(),
            port: self.port,
            login,
        })
    }
}

/// The proxy as the engines get it: never `ProxyKind::None`, and with its login. `Debug` never
/// prints the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Proxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub login: Option<Credentials>,
}

impl Proxy {
    /// Checked again where it arrives, as everything crossing a pipe is: a kind, a host and a
    /// port as [`ProxySettings`] allows, and a login as a download's.
    pub fn check(&self) -> Result<(), ErrorCode> {
        if self.kind == ProxyKind::None || check_host(&self.host)? != self.host {
            return Err(ErrorCode::InvalidInput);
        }
        check_port(self.port)?;
        if let Some(login) = &self.login {
            check_credentials(login)?;
        }
        Ok(())
    }
}

/// `host` as kept: a DNS name in its ASCII form, an IPv4 address, or an IPv6 address in brackets.
fn check_host(host: &str) -> Result<String, ErrorCode> {
    let host = host.trim();
    if host.is_empty() || host.len() > MAX_HOST_BYTES {
        return Err(ErrorCode::InvalidInput);
    }
    match url::Host::parse(host) {
        Ok(url::Host::Domain(name)) if !name.is_empty() && name.len() <= MAX_HOST_BYTES => Ok(name),
        Ok(url::Host::Ipv4(ip)) => Ok(ip.to_string()),
        Ok(url::Host::Ipv6(ip)) => Ok(format!("[{ip}]")),
        _ => Err(ErrorCode::InvalidInput),
    }
}

fn check_port(port: u16) -> Result<u16, ErrorCode> {
    if port == 0 {
        Err(ErrorCode::InvalidInput)
    } else {
        Ok(port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(host: &str, port: u16, user: &str) -> ProxySettings {
        ProxySettings {
            kind: ProxyKind::Http,
            host: host.to_owned(),
            port,
            user_name: user.to_owned(),
            remember_password: false,
        }
    }

    #[test]
    fn a_proxy_needs_a_host_and_a_port() {
        assert_eq!(http("", 8080, "").checked(), Err(ErrorCode::InvalidInput));
        assert_eq!(
            http("proxy.test", 0, "").checked(),
            Err(ErrorCode::InvalidInput)
        );
        let ok = http(" Proxy.Test ", 3128, " alice ").checked().unwrap();
        assert_eq!(ok.host, "proxy.test");
        assert_eq!(ok.user_name, "alice");
    }

    #[test]
    fn a_host_is_a_name_or_an_address_and_nothing_more() {
        for bad in [
            "http://proxy.test",
            "proxy.test:8080",
            "user@proxy.test",
            "proxy.test/path",
            "proxy test",
            "proxy\u{7}.test",
            "::1",
        ] {
            assert_eq!(
                http(bad, 8080, "").checked(),
                Err(ErrorCode::InvalidInput),
                "{bad:?}"
            );
        }
        assert_eq!(
            http("10.0.0.5", 8080, "").checked().unwrap().host,
            "10.0.0.5"
        );
        assert_eq!(http("[::1]", 8080, "").checked().unwrap().host, "[::1]");
        assert_eq!(
            http("bücher.example", 8080, "").checked().unwrap().host,
            "xn--bcher-kva.example",
            "an international name in its ASCII form, as links (#67)"
        );
    }

    #[test]
    fn a_user_name_is_one_a_login_may_have() {
        assert_eq!(
            http("proxy.test", 8080, "al:ice").checked(),
            Err(ErrorCode::InvalidInput)
        );
        assert_eq!(
            http("proxy.test", 8080, "al\nice").checked(),
            Err(ErrorCode::InvalidInput)
        );
    }

    #[test]
    fn without_a_proxy_the_fields_are_kept_for_later() {
        let off = ProxySettings {
            kind: ProxyKind::None,
            ..http("half typed", 0, "")
        };
        assert_eq!(off.checked().unwrap().host, "half typed");
        assert_eq!(off.proxy(None), None);
    }

    #[test]
    fn the_engines_get_the_password_only_with_a_user_name() {
        let password: crate::secret::Secret = "hunter2".into();
        let with_user = http("proxy.test", 8080, "alice")
            .proxy(Some(&password))
            .unwrap();
        assert_eq!(
            with_user.login.as_ref().map(|l| l.password.expose()),
            Some("hunter2")
        );
        assert_eq!(with_user.check(), Ok(()));
        let without = http("proxy.test", 8080, "").proxy(Some(&password)).unwrap();
        assert_eq!(without.login, None);
    }

    #[test]
    fn a_proxy_from_the_pipe_is_checked_again() {
        let mut proxy = http("proxy.test", 8080, "").proxy(None).unwrap();
        assert_eq!(proxy.check(), Ok(()));
        proxy.host = "Proxy.Test".to_owned();
        assert_eq!(proxy.check(), Err(ErrorCode::InvalidInput), "not as kept");
        proxy.host = "proxy.test".to_owned();
        proxy.kind = ProxyKind::None;
        assert_eq!(proxy.check(), Err(ErrorCode::InvalidInput));
    }

    #[test]
    fn debug_never_prints_the_password() {
        let password: crate::secret::Secret = "hunter2-secret".into();
        let proxy = http("proxy.test", 8080, "alice").proxy(Some(&password));
        assert!(!format!("{proxy:?}").contains("hunter2-secret"));
    }
}
