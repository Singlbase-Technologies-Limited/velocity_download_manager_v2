//! A local OpenSSH server for V2's SFTP tests (#53): the `sshd` installed on the machine, started
//! without administrator rights on 127.0.0.1 and a free port, with throwaway host keys, for the
//! user running the tests, who logs in with a key. `internal-sftp` serves the files from a
//! temporary folder (PO decision on #53: a real OpenSSH server, not a Rust one).
//!
//! - **Where it can't run** ([`SshServer::start`] returns why): no `sshd` installed. The tests then
//!   report NOT RUN, never PASS.
//! - **Password logins** need an `sshd` running as root or an administrator, which a test can't
//!   start. [`PasswordServer::from_env`] reads one that is provisioned instead: by CI
//!   (`scripts/ci-sftp-server.sh`), or locally once (`scripts/local-sftp-server.sh`, #176).
//! - **Host keys:** Ed25519 by default, or ECDSA or RSA. [`SshServer::restart`] can come back with
//!   a new key, the case a client must always refuse.
//! - **A cut:** [`SshServer::stop`] kills `sshd` and every session it started.
//!
//! Test code: it trusts its callers and isn't hardened.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The passphrase of [`SshServer::client_key_with_passphrase`].
pub const KEY_PASSPHRASE: &str = "velocity test passphrase";

/// The host key algorithm `sshd` presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostKey {
    #[default]
    Ed25519,
    Ecdsa,
    Rsa,
}

impl HostKey {
    fn keygen_type(self) -> &'static str {
        match self {
            HostKey::Ed25519 => "ed25519",
            HostKey::Ecdsa => "ecdsa",
            HostKey::Rsa => "rsa",
        }
    }
}

/// A running `sshd` serving files over SFTP. It stops, and removes its folder, when dropped.
pub struct SshServer {
    dir: PathBuf,
    addr: SocketAddr,
    sshd: PathBuf,
    child: Option<Child>,
    host_key: HostKey,
    generation: u32,
    user: String,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

impl SshServer {
    /// Serves `files` (name, bytes) from a folder of its own, presenting a `host_key`. `Err` says
    /// why it can't run here, such as no `sshd` installed.
    ///
    /// Where `VELOCITY_REQUIRE_SSHD` is set, as on CI, a server that can't start panics instead:
    /// a missing sshd must fail there, never pass as NOT RUN.
    pub fn start(files: Vec<(String, Vec<u8>)>, host_key: HostKey) -> Result<Self, String> {
        let started = Self::try_start(files, host_key);
        if let (Err(why), true) = (
            &started,
            std::env::var_os("VELOCITY_REQUIRE_SSHD").is_some(),
        ) {
            panic!("VELOCITY_REQUIRE_SSHD is set, but sshd can't run: {why}");
        }
        started
    }

    fn try_start(files: Vec<(String, Vec<u8>)>, host_key: HostKey) -> Result<Self, String> {
        let sshd = find_sshd().ok_or_else(|| "no sshd is installed".to_owned())?;
        let keygen = find_keygen().ok_or_else(|| "no ssh-keygen is installed".to_owned())?;
        let user = current_user().ok_or_else(|| "the current user name is unknown".to_owned())?;
        let dir = std::env::temp_dir().join(format!(
            "v2-ssh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).map_err(|e| e.to_string())?;
        for (name, data) in files {
            std::fs::write(dir.join("files").join(name), data).map_err(|e| e.to_string())?;
        }
        let keys = [("client", ""), ("client_passphrase", KEY_PASSPHRASE)];
        for (name, passphrase) in keys {
            keygen_run(&keygen, "ed25519", passphrase, &dir.join(name))?;
        }
        let mut authorized = String::new();
        for (name, _) in keys {
            authorized += &std::fs::read_to_string(dir.join(format!("{name}.pub")))
                .map_err(|e| e.to_string())?;
        }
        std::fs::write(dir.join("authorized_keys"), authorized).map_err(|e| e.to_string())?;
        let mut server = SshServer {
            addr: free_addr().map_err(|e| e.to_string())?,
            dir,
            sshd,
            child: None,
            host_key,
            generation: 0,
            user,
        };
        server.new_host_key(&keygen)?;
        server.launch()?;
        Ok(server)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The user the tests log in as: the one running them.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// `sftp://127.0.0.1:<port>/<absolute path of the file>`, with no login in it.
    pub fn url(&self, name: &str) -> String {
        let path = self.dir.join("files").join(name);
        let path = path.to_string_lossy().replace('\\', "/");
        let path = if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        };
        format!("sftp://{}{path}", self.addr)
    }

    /// The path of a served file, to change it on the server.
    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join("files").join(name)
    }

    /// An authorised private key without a passphrase (OpenSSH format, Ed25519).
    pub fn client_key(&self) -> PathBuf {
        self.dir.join("client")
    }

    /// An authorised private key encrypted with [`KEY_PASSPHRASE`].
    pub fn client_key_with_passphrase(&self) -> PathBuf {
        self.dir.join("client_passphrase")
    }

    /// The host key `sshd` presents now, as an OpenSSH public key line (`ssh-ed25519 AAAA… c`).
    pub fn host_key(&self) -> String {
        std::fs::read_to_string(self.host_key_path().with_extension("pub"))
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    /// Kills `sshd` and every session it started: a transfer under way is cut.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            kill_tree(&mut child);
        }
    }

    /// Starts `sshd` again on the same port, with a new host key if `new_host_key`.
    pub fn restart(&mut self, new_host_key: bool) -> Result<(), String> {
        self.stop();
        if new_host_key {
            let keygen = find_keygen().ok_or_else(|| "no ssh-keygen is installed".to_owned())?;
            self.new_host_key(&keygen)?;
        }
        self.launch()
    }

    fn host_key_path(&self) -> PathBuf {
        self.dir.join(format!("host_key_{}", self.generation))
    }

    fn new_host_key(&mut self, keygen: &Path) -> Result<(), String> {
        self.generation += 1;
        keygen_run(
            keygen,
            self.host_key.keygen_type(),
            "",
            &self.host_key_path(),
        )
    }

    fn launch(&mut self) -> Result<(), String> {
        let path = |p: &Path| p.to_string_lossy().replace('\\', "/");
        let config = format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {host_key}\nPidFile {pid}\n\
             AuthorizedKeysFile {authorized}\nPubkeyAuthentication yes\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nStrictModes no\n\
             Subsystem sftp internal-sftp\n{pam}",
            port = self.addr.port(),
            host_key = path(&self.host_key_path()),
            pid = path(&self.dir.join("sshd.pid")),
            authorized = path(&self.dir.join("authorized_keys")),
            pam = if cfg!(windows) { "" } else { "UsePAM no\n" },
        );
        let config_path = self.dir.join("sshd_config");
        std::fs::write(&config_path, config).map_err(|e| e.to_string())?;
        let log = std::fs::File::create(self.dir.join("sshd.log")).map_err(|e| e.to_string())?;
        let mut command = Command::new(&self.sshd);
        command
            .args(["-D", "-e", "-f"])
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        let mut child = command
            .spawn()
            .map_err(|e| format!("sshd didn't start: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(self.addr).is_err() {
            if let Ok(Some(status)) = child.try_wait() {
                let log = std::fs::read_to_string(self.dir.join("sshd.log")).unwrap_or_default();
                return Err(format!("sshd exited ({status}): {log}"));
            }
            if Instant::now() > deadline {
                kill_tree(&mut child);
                return Err("sshd didn't listen within 10 s".to_owned());
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.child = Some(child);
        Ok(())
    }
}

impl Drop for SshServer {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A password-login SFTP server that CI or a local setup provisions, since only root or an administrator can run
/// one (`VELOCITY_SFTP_PASSWORD_SERVER`: `host:port|user|password|/path/of/a/file|its SHA-256`).
pub struct PasswordServer {
    pub addr: String,
    pub user: String,
    pub password: String,
    pub path: String,
    pub sha256: String,
}

impl PasswordServer {
    /// The provisioned server, or why there is none.
    /// `VELOCITY_REQUIRE_SFTP_PASSWORD`, set where one is provisioned, turns its absence into a
    /// panic.
    pub fn from_env() -> Result<Self, String> {
        let value = std::env::var("VELOCITY_SFTP_PASSWORD_SERVER").map_err(|_| {
            if std::env::var_os("VELOCITY_REQUIRE_SFTP_PASSWORD").is_some() {
                panic!("VELOCITY_REQUIRE_SFTP_PASSWORD is set, but no server is described");
            }
            "VELOCITY_SFTP_PASSWORD_SERVER isn't set".to_owned()
        })?;
        let parts: Vec<&str> = value.split('|').collect();
        let [addr, user, password, path, sha256] = parts[..] else {
            return Err("VELOCITY_SFTP_PASSWORD_SERVER needs 5 fields".to_owned());
        };
        Ok(PasswordServer {
            addr: addr.to_owned(),
            user: user.to_owned(),
            password: password.to_owned(),
            path: path.to_owned(),
            sha256: sha256.to_owned(),
        })
    }

    /// `sftp://<host:port><path>`, with no login in it.
    pub fn url(&self) -> String {
        format!("sftp://{}{}", self.addr, self.path)
    }
}

fn find_sshd() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match std::env::var_os("VELOCITY_SSHD") {
        Some(path) => vec![PathBuf::from(path)],
        None if cfg!(windows) => vec![PathBuf::from(r"C:\Windows\System32\OpenSSH\sshd.exe")],
        None => vec![
            PathBuf::from("/usr/sbin/sshd"),
            PathBuf::from("/usr/bin/sshd"),
        ],
    };
    candidates.into_iter().find(|p| p.is_file())
}

fn find_keygen() -> Option<PathBuf> {
    if cfg!(windows) {
        let p = PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe");
        return p.is_file().then_some(p);
    }
    ["/usr/bin/ssh-keygen", "/usr/local/bin/ssh-keygen"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn keygen_run(keygen: &Path, kind: &str, passphrase: &str, file: &Path) -> Result<(), String> {
    let status = Command::new(keygen)
        .args([
            "-q",
            "-t",
            kind,
            "-N",
            passphrase,
            "-C",
            "velocity-test",
            "-f",
        ])
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("ssh-keygen didn't start: {e}"))?;
    if !status.success() {
        return Err(format!("ssh-keygen -t {kind} failed: {status}"));
    }
    // ssh-keygen writes the private key readable by its owner only (mode 600; on Windows, an ACL
    // for the owner), as sshd and ssh insist.
    Ok(())
}

fn current_user() -> Option<String> {
    let name = if cfg!(windows) {
        std::env::var("USERNAME").ok()
    } else {
        std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("LOGNAME").ok())
    };
    name.filter(|n| !n.is_empty())
}

/// 127.0.0.1 and a port that was free a moment ago.
fn free_addr() -> io::Result<SocketAddr> {
    TcpListener::bind("127.0.0.1:0")?.local_addr()
}

/// Kills `child` and every process it started: `taskkill /T` on Windows; on Unix, every
/// descendant `ps` lists, since sshd moves each session into a session (and process group) of its
/// own, which a kill of sshd's group would miss.
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else {
        let pids: Vec<String> = descendants(pid)
            .into_iter()
            .chain([pid])
            .map(|p| p.to_string())
            .collect();
        let _ = Command::new("kill")
            .arg("-9")
            .args(&pids)
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Every process below `root`, from `ps -e -o pid=,ppid=`.
fn descendants(root: u32) -> Vec<u32> {
    let Ok(out) = Command::new("ps").args(["-e", "-o", "pid=,ppid="]).output() else {
        return Vec::new();
    };
    let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    let mut found = Vec::new();
    let mut parents = vec![root];
    while let Some(parent) = parents.pop() {
        for &(pid, ppid) in &pairs {
            if ppid == parent && !found.contains(&pid) {
                found.push(pid);
                parents.push(pid);
            }
        }
    }
    found
}
