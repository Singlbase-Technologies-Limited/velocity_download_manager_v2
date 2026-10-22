//! What the linked libcurl can do, and whether V2 can use it (#35). The engine starts only after
//! [`check`] accepts the libcurl that is actually linked, so an old or reduced libcurl is refused
//! at run time even if a build check was skipped.

use std::fmt;

/// The libcurl version floor (research report section 8.1, ADR-0004). The native build checks the
/// same value (`LIBCURL_FLOOR` in scripts/native/native-lib.mjs); each bump is its own reviewed change.
pub const LIBCURL_FLOOR: (u32, u32, u32) = (8, 23, 0);
/// Protocols the engine needs. SFTP joins when its backend is decided and built (#52, #53).
pub const REQUIRED_PROTOCOLS: &[&str] = &["ftp", "ftps", "http", "https"];
/// Protocols the linked libcurl may contain; anything else must be compiled out.
pub const ALLOWED_PROTOCOLS: &[&str] = &["ftp", "ftps", "http", "https", "sftp"];
/// libcurl features the engine needs: HTTP/2, TLS, and compressed responses.
pub const REQUIRED_FEATURES: &[&str] = &["HTTP2", "SSL", "libz"];
/// The TLS backend each platform's build uses (docs/BUILDING_NATIVE.md).
pub const EXPECTED_TLS: &str = if cfg!(windows) { "Schannel" } else { "OpenSSL" };

/// What `curl_version_info` reports about the linked libcurl.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedCurl {
    /// For example `8.23.0`.
    pub version: String,
    /// The TLS backend, for example `Schannel` or `OpenSSL/3.5.9`.
    pub tls: String,
    pub protocols: Vec<String>,
    pub features: Vec<String>,
}

/// One reason the linked libcurl can't be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    BelowFloor {
        linked: String,
    },
    UnreadableVersion {
        linked: String,
    },
    WrongTls {
        linked: String,
        expected: &'static str,
    },
    MissingProtocols(Vec<String>),
    MissingFeatures(Vec<String>),
    UnexpectedProtocols(Vec<String>),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (major, minor, patch) = LIBCURL_FLOOR;
        match self {
            Self::BelowFloor { linked } => write!(
                f,
                "libcurl {linked} is linked, but V2 needs {major}.{minor}.{patch} or newer"
            ),
            Self::UnreadableVersion { linked } => {
                write!(f, "libcurl reports a version V2 can't read ({linked:?})")
            }
            Self::WrongTls { linked, expected } => write!(
                f,
                "libcurl uses the TLS backend {linked}, but this build needs {expected}"
            ),
            Self::MissingProtocols(p) => {
                write!(f, "libcurl lacks required protocols: {}", p.join(", "))
            }
            Self::MissingFeatures(p) => {
                write!(f, "libcurl lacks required features: {}", p.join(", "))
            }
            Self::UnexpectedProtocols(p) => write!(
                f,
                "libcurl includes protocols V2 doesn't use, which should be compiled out: {}",
                p.join(", ")
            ),
        }
    }
}

/// The linked libcurl failed the capability check; every problem found is listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncompatibleCurl {
    pub linked: LinkedCurl,
    pub problems: Vec<Problem>,
}

impl fmt::Display for IncompatibleCurl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let problems: Vec<String> = self.problems.iter().map(ToString::to_string).collect();
        write!(
            f,
            "the transfer engine can't start: {}. Rebuild the native libraries with `pnpm native:build` (docs/BUILDING_NATIVE.md)",
            problems.join("; ")
        )
    }
}

impl std::error::Error for IncompatibleCurl {}

/// Accepts the linked libcurl only if it meets the floor and has exactly what the engine needs.
pub fn check(linked: &LinkedCurl) -> Result<(), IncompatibleCurl> {
    let mut problems = Vec::new();
    match parse_version(&linked.version) {
        Some(v) if v < LIBCURL_FLOOR => problems.push(Problem::BelowFloor {
            linked: linked.version.clone(),
        }),
        Some(_) => {}
        None => problems.push(Problem::UnreadableVersion {
            linked: linked.version.clone(),
        }),
    }
    let backend = linked.tls.split('/').next().unwrap_or_default();
    if backend != EXPECTED_TLS {
        problems.push(Problem::WrongTls {
            linked: linked.tls.clone(),
            expected: EXPECTED_TLS,
        });
    }
    let missing = |required: &[&str], present: &[String]| -> Vec<String> {
        required
            .iter()
            .filter(|r| !present.iter().any(|p| p == *r))
            .map(|r| (*r).to_owned())
            .collect()
    };
    let missing_protocols = missing(REQUIRED_PROTOCOLS, &linked.protocols);
    if !missing_protocols.is_empty() {
        problems.push(Problem::MissingProtocols(missing_protocols));
    }
    let missing_features = missing(REQUIRED_FEATURES, &linked.features);
    if !missing_features.is_empty() {
        problems.push(Problem::MissingFeatures(missing_features));
    }
    let unexpected: Vec<String> = linked
        .protocols
        .iter()
        .filter(|p| !ALLOWED_PROTOCOLS.contains(&p.as_str()))
        .cloned()
        .collect();
    if !unexpected.is_empty() {
        problems.push(Problem::UnexpectedProtocols(unexpected));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(IncompatibleCurl {
            linked: linked.clone(),
            problems,
        })
    }
}

/// `major.minor.patch` as numbers, so 8.100.0 sorts after 8.23.0. Anything else is unreadable.
fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) => {
            Some((major, minor, patch))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the Velocity-built libcurl reports (docs/BUILDING_NATIVE.md, 2026-10-22).
    fn velocity_build() -> LinkedCurl {
        LinkedCurl {
            version: "8.23.0".into(),
            tls: if cfg!(windows) {
                "Schannel".into()
            } else {
                "OpenSSL/3.5.9".into()
            },
            protocols: ["ftp", "ftps", "http", "https"].map(String::from).to_vec(),
            features: [
                "alt-svc",
                "AsynchDNS",
                "HSTS",
                "HTTP2",
                "HTTPS-proxy",
                "IPv6",
                "Largefile",
                "libz",
                "SSL",
                "threadsafe",
                "UnixSockets",
            ]
            .map(String::from)
            .to_vec(),
        }
    }

    fn problems(linked: LinkedCurl) -> Vec<Problem> {
        check(&linked)
            .expect_err("the check should refuse this libcurl")
            .problems
    }

    #[test]
    fn accepts_the_velocity_built_libcurl() {
        assert_eq!(check(&velocity_build()), Ok(()));
    }

    #[test]
    fn accepts_newer_versions_compared_numerically() {
        for version in ["8.23.1", "8.24.0", "9.0.0", "8.100.0"] {
            assert_eq!(
                check(&LinkedCurl {
                    version: version.into(),
                    ..velocity_build()
                }),
                Ok(()),
                "{version}"
            );
        }
    }

    #[test]
    fn refuses_a_libcurl_below_the_floor() {
        for version in ["8.22.0", "8.9.9", "7.88.1"] {
            assert_eq!(
                problems(LinkedCurl {
                    version: version.into(),
                    ..velocity_build()
                }),
                [Problem::BelowFloor {
                    linked: version.into()
                }],
                "{version}"
            );
        }
    }

    #[test]
    fn refuses_a_version_it_cannot_read() {
        assert_eq!(
            problems(LinkedCurl {
                version: "8.23.0-DEV".into(),
                ..velocity_build()
            }),
            [Problem::UnreadableVersion {
                linked: "8.23.0-DEV".into()
            }]
        );
    }

    #[test]
    fn refuses_another_tls_backend() {
        let other = if cfg!(windows) {
            "OpenSSL/3.5.9"
        } else {
            "GnuTLS/3.8.3"
        };
        assert_eq!(
            problems(LinkedCurl {
                tls: other.into(),
                ..velocity_build()
            }),
            [Problem::WrongTls {
                linked: other.into(),
                expected: EXPECTED_TLS
            }]
        );
    }

    #[test]
    fn refuses_missing_protocols_and_features() {
        let mut linked = velocity_build();
        linked.protocols.retain(|p| p.starts_with("http"));
        linked.features.retain(|f| f != "HTTP2");
        assert_eq!(
            problems(linked),
            [
                Problem::MissingProtocols(vec!["ftp".into(), "ftps".into()]),
                Problem::MissingFeatures(vec!["HTTP2".into()])
            ]
        );
    }

    #[test]
    fn refuses_protocols_that_should_be_compiled_out() {
        let mut linked = velocity_build();
        linked.protocols.extend(["file", "ldap"].map(String::from));
        assert_eq!(
            problems(linked),
            [Problem::UnexpectedProtocols(vec![
                "file".into(),
                "ldap".into()
            ])]
        );
    }

    #[test]
    fn reports_every_problem_with_an_actionable_message() {
        let err = check(&LinkedCurl {
            version: "8.22.0".into(),
            protocols: vec!["http".into(), "https".into(), "file".into()],
            ..velocity_build()
        })
        .expect_err("8.22.0 is below the floor");
        assert_eq!(err.problems.len(), 3);
        let message = err.to_string();
        assert!(
            message.contains("libcurl 8.22.0 is linked, but V2 needs 8.23.0 or newer"),
            "{message}"
        );
        assert!(message.contains("ftp, ftps"), "{message}");
        assert!(message.contains("`pnpm native:build`"), "{message}");
    }
}
