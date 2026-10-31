//! Where V2 keeps its data, and its IPC endpoint name (#27). Both derive from V2's identifier
//! (`branding.json`), so V2 never shares a folder, database, checkpoint or pipe with V1.
//! The native messaging host (#65) resolves the same paths without Tauri, which is why this
//! lives in the core and uses `dirs` rather than Tauri's path resolver.

use crate::branding::branding;
use crate::domain::ErrorCode;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Overrides the data directory, for tests and portable setups. Must be an absolute path.
/// Distinct from V1's `VELOCITY_DATA_DIR`, so a V1 setting never redirects V2.
pub const DATA_DIR_ENV: &str = "VELOCITY_V2_DATA_DIR";

/// V2's data directory and the files inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    data_dir: PathBuf,
}

impl Paths {
    /// `$VELOCITY_V2_DATA_DIR` if set, otherwise `<platform data dir>/<identifier>`.
    pub fn resolve() -> Result<Self, ErrorCode> {
        Self::resolve_with(
            std::env::var_os(DATA_DIR_ENV),
            dirs::data_dir(),
            branding().app_identifier(),
        )
    }

    /// The pure form of [`Paths::resolve`], for tests.
    pub fn resolve_with(
        override_dir: Option<OsString>,
        platform_data_dir: Option<PathBuf>,
        identifier: &str,
    ) -> Result<Self, ErrorCode> {
        if let Some(dir) = override_dir {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                return Err(ErrorCode::InvalidPath);
            }
            return Ok(Self { data_dir: dir });
        }
        let base = platform_data_dir.ok_or(ErrorCode::Unsupported)?;
        Ok(Self {
            data_dir: base.join(identifier),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The SQLite database (#26).
    pub fn database(&self) -> PathBuf {
        self.data_dir.join("velocity.sqlite3")
    }

    /// Engine checkpoints (#43).
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.data_dir.join("checkpoints")
    }

    /// The `.torrent` files fetched from links, kept as their torrents' origin (#60).
    pub fn torrent_links_dir(&self) -> PathBuf {
        self.data_dir.join("torrent-links")
    }

    /// Crash logs (#28): what happened when the app panicked, redacted, never sent anywhere.
    pub fn crash_logs_dir(&self) -> PathBuf {
        self.data_dir.join("crash-logs")
    }

    /// The SFTP host keys the user trusted (#53): V2's own known_hosts file, never
    /// `~/.ssh/known_hosts`.
    pub fn known_hosts(&self) -> PathBuf {
        self.data_dir.join("known_hosts")
    }

    /// The browser bridge's per-launch token (#65): written by the app when it starts, read by the
    /// native host. Only this user can read it.
    pub fn ipc_token_file(&self) -> PathBuf {
        self.data_dir.join("ipc.token")
    }

    /// Local socket path (Unix) or named pipe name (Windows) for the browser bridge (#65).
    pub fn ipc_endpoint(&self) -> String {
        if cfg!(windows) {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
            windows_pipe_name(branding().app_identifier(), &user, &self.data_dir)
        } else {
            self.data_dir
                .join("ipc.sock")
                .to_string_lossy()
                .into_owned()
        }
    }

    /// Creates the data directory, owner-only where the platform supports it.
    pub fn ensure(&self) -> Result<(), ErrorCode> {
        std::fs::create_dir_all(&self.data_dir).map_err(|_| ErrorCode::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| ErrorCode::PermissionDenied)?;
        }
        Ok(())
    }
}

/// Windows pipe names are machine-global, so the name carries the identifier, the user and a hash
/// of the data directory: separate installs or test instances never collide, while the app and
/// the native host, which resolve the same directory, always agree (V1's scheme, V2 identifier).
/// FNV-1a is stable across builds; uniqueness matters here, not secrecy (a per-launch token
/// authenticates clients, #65).
pub fn windows_pipe_name(identifier: &str, user: &str, data_dir: &Path) -> String {
    let safe_user: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
    let dir = data_dir.to_string_lossy().to_lowercase();
    let hash = dir.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!(r"\\.\pipe\{identifier}.ipc.{safe_user}.{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const V1_IDENTIFIER: &str = "com.velocity.downloadmanager";

    fn abs(p: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:\\{}", p.replace('/', "\\")))
        } else {
            PathBuf::from(format!("/{p}"))
        }
    }

    #[test]
    fn the_data_dir_is_the_platform_data_dir_plus_the_v2_identifier() {
        let p = Paths::resolve_with(
            None,
            Some(abs("Users/u/AppData/Roaming")),
            "dev.velocity.downloadmanager.v2",
        )
        .unwrap();
        assert_eq!(
            p.data_dir(),
            abs("Users/u/AppData/Roaming").join("dev.velocity.downloadmanager.v2")
        );
    }

    #[test]
    fn an_absolute_override_wins() {
        let p = Paths::resolve_with(
            Some(abs("tmp/v2-test").into_os_string()),
            Some(abs("data")),
            "x.y.z",
        )
        .unwrap();
        assert_eq!(p.data_dir(), abs("tmp/v2-test"));
    }

    #[test]
    fn a_relative_or_empty_override_is_refused() {
        assert_eq!(
            Paths::resolve_with(Some("relative/dir".into()), Some(abs("data")), "x.y.z")
                .unwrap_err(),
            crate::domain::ErrorCode::InvalidPath
        );
        assert_eq!(
            Paths::resolve_with(Some("".into()), Some(abs("data")), "x.y.z").unwrap_err(),
            crate::domain::ErrorCode::InvalidPath
        );
    }

    #[test]
    fn no_platform_data_dir_is_an_error_not_a_guess() {
        assert_eq!(
            Paths::resolve_with(None, None, "x.y.z").unwrap_err(),
            crate::domain::ErrorCode::Unsupported
        );
    }

    #[test]
    fn the_override_variable_is_v2_specific() {
        assert_eq!(DATA_DIR_ENV, "VELOCITY_V2_DATA_DIR");
    }

    #[test]
    fn v2_and_v1_data_dirs_are_siblings_never_nested() {
        let base = abs("data");
        let v1 = base.join(V1_IDENTIFIER);
        for id in [
            "dev.velocity.downloadmanager.v2",
            "com.velocity.downloadmanager.v2",
        ] {
            let v2 = Paths::resolve_with(None, Some(base.clone()), id).unwrap();
            assert_ne!(v2.data_dir(), v1.as_path());
            assert!(
                !v2.data_dir().starts_with(&v1),
                "{} is inside V1's folder",
                v2.data_dir().display()
            );
            assert!(!v1.starts_with(v2.data_dir()));
        }
    }

    #[test]
    fn files_live_inside_the_data_dir() {
        let p = Paths::resolve_with(None, Some(abs("data")), "x.y.z").unwrap();
        for f in [
            p.database(),
            p.checkpoints_dir(),
            p.torrent_links_dir(),
            p.crash_logs_dir(),
        ] {
            assert!(f.starts_with(p.data_dir()), "{}", f.display());
        }
    }

    #[test]
    fn the_windows_pipe_name_embeds_the_identifier_user_and_data_dir() {
        let a = windows_pipe_name(
            "dev.velocity.downloadmanager.v2",
            "Jane Doe",
            Path::new("C:\\Data\\dev.velocity.downloadmanager.v2"),
        );
        assert!(
            a.starts_with(r"\\.\pipe\dev.velocity.downloadmanager.v2.ipc.JaneDoe."),
            "{a}"
        );
        // The same folder in another case gives the same pipe (Windows paths are case-insensitive).
        assert_eq!(
            a,
            windows_pipe_name(
                "dev.velocity.downloadmanager.v2",
                "Jane Doe",
                Path::new("c:\\data\\DEV.velocity.downloadmanager.v2")
            )
        );
        // Another data directory (a test instance) gets another pipe.
        assert_ne!(
            a,
            windows_pipe_name(
                "dev.velocity.downloadmanager.v2",
                "Jane Doe",
                Path::new("C:\\tmp\\t1")
            )
        );
    }

    #[test]
    fn the_v2_pipe_never_equals_v1s() {
        let dir = Path::new("C:\\Data\\x");
        let v1 = windows_pipe_name(V1_IDENTIFIER, "u", dir);
        for id in [
            "dev.velocity.downloadmanager.v2",
            "com.velocity.downloadmanager.v2",
        ] {
            assert_ne!(windows_pipe_name(id, "u", dir), v1);
        }
    }

    #[test]
    fn ensure_creates_the_data_dir() {
        let root = std::env::temp_dir().join(format!("velocity-v2-paths-{}", std::process::id()));
        let p =
            Paths::resolve_with(Some(root.join("data").into_os_string()), None, "x.y.z").unwrap();
        p.ensure().unwrap();
        assert!(p.data_dir().is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
