//! Velocity's own known_hosts file (#53, research report section 8). SSH has no certificate
//! authority to trust, so a host key is trusted only once the user has confirmed its fingerprint,
//! and it is pinned from then on.
//!
//! - **The file is V2's,** never `~/.ssh/known_hosts`, and unhashed: one OpenSSH line per host,
//!   `host key-type base64-key` (`[host]:port` when the port isn't 22), readable by people and by
//!   OpenSSH.
//! - **A pin per host:** a host is known by the whole public key, and shown by its SHA-256
//!   fingerprint (`SHA256:…`, as OpenSSH prints it).
//! - **A changed key is always refused,** and [`KnownHosts::trust`] never replaces one: trusting a
//!   host that already has a key is an error.
//! - **Writes are atomic:** a temporary file beside it, then a rename.

use std::io;
use std::path::{Path, PathBuf};

use russh::keys::{HashAlg, PublicKey};

/// What the file says about a server's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Its key is the one pinned for this host and port.
    Known,
    /// No key is pinned for this host and port: the user must confirm the fingerprint first.
    Unknown,
    /// Another key is pinned for this host and port: always refused. The pinned key's
    /// fingerprint, to show.
    Changed { pinned: String },
}

/// The SHA-256 fingerprint of `key`, as OpenSSH prints it: `SHA256:` and unpadded base64.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// The known_hosts file at a path.
#[derive(Clone, Debug)]
pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        KnownHosts { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file says about `key` for `host` and `port`. A missing file knows no host.
    pub fn check(&self, host: &str, port: u16, key: &PublicKey) -> io::Result<Verdict> {
        let name = host_name(host, port);
        let pinned: Vec<PublicKey> = self
            .entries()?
            .into_iter()
            .filter(|(h, _)| *h == name)
            .map(|(_, k)| k)
            .collect();
        Ok(match pinned.first() {
            None => Verdict::Unknown,
            Some(_) if pinned.iter().any(|p| same_key(p, key)) => Verdict::Known,
            Some(first) => Verdict::Changed {
                pinned: fingerprint(first),
            },
        })
    }

    /// Pins `key` for `host` and `port`, after the user confirmed its fingerprint. Refused when
    /// the host already has another key: a changed key is never trusted by replacing the old one.
    pub fn trust(&self, host: &str, port: u16, key: &PublicKey) -> io::Result<()> {
        match self.check(host, port, key)? {
            Verdict::Known => return Ok(()),
            Verdict::Changed { pinned } => {
                return Err(io::Error::other(format!(
                    "{} already has another key ({pinned}); a changed key is never trusted",
                    host_name(host, port)
                )))
            }
            Verdict::Unknown => {}
        }
        let mut text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        // Without the key's comment: the server chose it, and it says nothing about the key.
        let mut bare = key.clone();
        bare.set_comment("");
        let line = bare.to_openssh().map_err(io::Error::other)?;
        text.push_str(&format!("{} {}\n", host_name(host, port), line.trim()));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.path)
    }

    /// Every entry the file holds: host name and key. Lines that aren't one (comments, hashed or
    /// malformed entries) are skipped.
    fn entries(&self) -> io::Result<Vec<(String, PublicKey)>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let (host, key) = line.split_once(char::is_whitespace)?;
                let key = PublicKey::from_openssh(key.trim()).ok()?;
                Some((host.to_ascii_lowercase(), key))
            })
            .collect())
    }
}

/// A host as known_hosts names it: `host` for port 22, `[host]:port` otherwise, in lower case.
fn host_name(host: &str, port: u16) -> String {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// The same key: its algorithm and key data, whatever the comment.
fn same_key(a: &PublicKey, b: &PublicKey) -> bool {
    a.key_data() == b.key_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two Ed25519 keys, from ssh-keygen.
    const ED_A: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAION1Z9n3NBrwQZbuazFoc3SR7aF6wvQqjMTB3MRmrZ1l a";
    const ED_B: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICyquc+qWZ3KgGvYiqDLXVHKlCRTX9BUgxFJ6g6BNL29 b";

    fn key(line: &str) -> PublicKey {
        PublicKey::from_openssh(line).unwrap()
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("v2-kh-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_host_is_unknown_until_trusted_then_known() {
        let dir = Dir::new("trust");
        let hosts = KnownHosts::new(dir.0.join("known_hosts"));
        assert_eq!(
            hosts.check("example.org", 22, &key(ED_A)).unwrap(),
            Verdict::Unknown
        );
        hosts.trust("example.org", 22, &key(ED_A)).unwrap();
        assert_eq!(
            hosts.check("example.org", 22, &key(ED_A)).unwrap(),
            Verdict::Known
        );
        // Hosts are compared without case; ports are part of the host.
        assert_eq!(
            hosts.check("EXAMPLE.org", 22, &key(ED_A)).unwrap(),
            Verdict::Known
        );
        assert_eq!(
            hosts.check("example.org", 2222, &key(ED_A)).unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn a_changed_key_is_refused_and_never_replaced() {
        let dir = Dir::new("changed");
        let hosts = KnownHosts::new(dir.0.join("known_hosts"));
        hosts.trust("127.0.0.1", 2222, &key(ED_A)).unwrap();
        let pinned = fingerprint(&key(ED_A));
        assert_eq!(
            hosts.check("127.0.0.1", 2222, &key(ED_B)).unwrap(),
            Verdict::Changed { pinned }
        );
        assert!(hosts.trust("127.0.0.1", 2222, &key(ED_B)).is_err());
        assert_eq!(
            hosts.check("127.0.0.1", 2222, &key(ED_A)).unwrap(),
            Verdict::Known
        );
    }

    #[test]
    fn the_file_is_unhashed_openssh_lines() {
        let dir = Dir::new("format");
        let path = dir.0.join("known_hosts");
        let hosts = KnownHosts::new(&path);
        hosts.trust("example.org", 22, &key(ED_A)).unwrap();
        hosts.trust("127.0.0.1", 2222, &key(ED_B)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].starts_with("example.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAION1"));
        assert!(lines[1].starts_with("[127.0.0.1]:2222 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICyq"));
        // Trusting the same key again changes nothing.
        hosts.trust("example.org", 22, &key(ED_A)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn comments_blank_and_foreign_lines_are_ignored() {
        let dir = Dir::new("lines");
        let path = dir.0.join("known_hosts");
        std::fs::write(
            &path,
            format!("# a comment\n\n|1|hashed+entry= ssh-ed25519 AAAA\nnot a line\n{ED_A}\n"),
        )
        .unwrap();
        let hosts = KnownHosts::new(&path);
        assert_eq!(
            hosts.check("example.org", 22, &key(ED_A)).unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn fingerprints_are_sha256_as_openssh_prints_them() {
        let f = fingerprint(&key(ED_A));
        assert!(f.starts_with("SHA256:"), "{f}");
        assert_eq!(f.len(), "SHA256:".len() + 43, "{f}");
    }
}
