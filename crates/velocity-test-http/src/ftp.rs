//! A local FTP server for V2's engine tests (#51), ported from V1's `tests/common/ftp.rs` to the
//! standard library: passive mode only (`EPSV`, `PASV`), binary `RETR` from a `REST` offset,
//! `SIZE`, `MDTM`, and a required login when asked. FTPS uses a [`TestPki`]: explicit
//! (`AUTH TLS`, then `PBSZ 0` and `PROT P` for the data connections) or implicit (TLS from the
//! first byte). It misbehaves on demand, the way real servers do: no `SIZE`, no `REST`, no `MDTM`,
//! or a transfer cut part way. It records the logins, `RETR`s, `REST` offsets and which
//! connections were encrypted, so a test can prove what went over the wire.
//!
//! Listens on 127.0.0.1 only. Test code: it trusts its callers and isn't hardened.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::tls::{Leaf, TestPki};

/// How long the server waits for a client on either connection before it gives up.
const IDLE: Duration = Duration::from_secs(30);
/// The bytes the server writes to a data connection at a time.
const CHUNK: usize = 16 * 1024;

/// Whether, and how, the server speaks TLS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FtpTls {
    /// Plain FTP. `AUTH TLS` is refused.
    #[default]
    None,
    /// Explicit FTPS: plain until the client asks for `AUTH TLS`, then the control connection is
    /// TLS, and the data connections too after `PROT P`. It presents this leaf.
    Explicit(Leaf),
    /// Implicit FTPS (`ftps://`): TLS from the first byte, on both connections.
    Implicit(Leaf),
}

/// What the server does, beyond serving its files.
#[derive(Clone, Debug, Default)]
pub struct FtpOptions {
    /// The only user name and password accepted; `None` accepts any login.
    pub login: Option<(String, String)>,
    /// Answers `SIZE` with `502`, as servers without it do.
    pub no_size: bool,
    /// Answers `REST` with `502`, so a transfer can only start at byte 0.
    pub no_rest: bool,
    /// Answers `MDTM` with `502`, so the file has no modification time.
    pub no_mdtm: bool,
    /// The first `RETR` closes its data connection after this many bytes, and its reply says the
    /// transfer was aborted (`426`).
    pub cut_first: Option<u64>,
    /// Limits every data connection to this many bytes per second.
    pub throttle_bytes_per_sec: Option<u64>,
    pub tls: FtpTls,
}

/// A file the server serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FtpFile {
    pub data: Vec<u8>,
    /// What `MDTM` answers: `YYYYMMDDHHMMSS`, in UTC.
    pub modified: String,
}

/// What the server has seen and done so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FtpRecord {
    /// Control connections accepted.
    pub connections: u32,
    /// Every `USER` and `PASS` pair, in order.
    pub logins: Vec<(String, String)>,
    /// Every command's verb, in order (`PASS`'s argument is never kept here).
    pub commands: Vec<String>,
    /// `RETR`s that started a transfer.
    pub retrs: u32,
    /// The offset of each `REST` that was accepted, in order.
    pub rests: Vec<u64>,
    /// Control connections that became TLS (`AUTH TLS`) or started as TLS (implicit).
    pub tls_controls: u32,
    /// Data connections, encrypted or not, in order.
    pub data_connections: Vec<bool>,
    /// Whether [`FtpOptions::cut_first`] cut a transfer.
    pub cut: bool,
}

/// The modification time every file has unless [`FtpServer::replace`] gives another.
pub const MODIFIED: &str = "20200102030405";

struct Shared {
    files: Mutex<HashMap<String, FtpFile>>,
    options: FtpOptions,
    tls: Option<Arc<ServerConfig>>,
    record: Mutex<FtpRecord>,
}

/// A running FTP server on 127.0.0.1 and a free port. It stops when dropped.
pub struct FtpServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    pki: Option<TestPki>,
}

impl FtpServer {
    /// Serves `files` (path, bytes), at absolute paths such as `/pub/a.bin`, each with
    /// [`MODIFIED`] as its modification time.
    pub fn start(files: Vec<(String, Vec<u8>)>, options: FtpOptions) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local address");
        let pki = match options.tls {
            FtpTls::None => None,
            FtpTls::Explicit(leaf) | FtpTls::Implicit(leaf) => Some(TestPki::start(leaf)),
        };
        let files = files
            .into_iter()
            .map(|(path, data)| {
                let file = FtpFile {
                    data,
                    modified: MODIFIED.to_owned(),
                };
                (path, file)
            })
            .collect();
        let shared = Arc::new(Shared {
            files: Mutex::new(files),
            tls: pki.as_ref().map(|p| p.server_config(None)),
            options,
            record: Mutex::default(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let acceptor = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    shared.record.lock().unwrap().connections += 1;
                    let shared = Arc::clone(&shared);
                    thread::spawn(move || {
                        let _ = session(stream, &shared);
                    });
                }
            })
        };
        FtpServer {
            addr,
            shared,
            stop,
            acceptor: Some(acceptor),
            pki,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `ftp://localhost:<port>/<path>`, or `ftps://` for implicit FTPS. `localhost`, which the test
    /// certificate names, though the server listens on 127.0.0.1 only.
    pub fn url(&self, path: &str) -> String {
        let scheme = match self.shared.options.tls {
            FtpTls::Implicit(_) => "ftps",
            _ => "ftp",
        };
        format!(
            "{scheme}://localhost:{}/{}",
            self.addr.port(),
            path.trim_start_matches('/')
        )
    }

    /// The test CA's PEM file, for the client to trust, when the server speaks TLS.
    pub fn ca_file(&self) -> Option<&std::path::Path> {
        self.pki.as_ref().map(TestPki::ca_file)
    }

    /// Replaces the file at `path`, or adds it: the file changed on the server.
    pub fn replace(&self, path: &str, data: Vec<u8>, modified: &str) {
        let file = FtpFile {
            data,
            modified: modified.to_owned(),
        };
        self.shared
            .files
            .lock()
            .unwrap()
            .insert(path.to_owned(), file);
    }

    pub fn record(&self) -> FtpRecord {
        self.shared.record.lock().unwrap().clone()
    }
}

impl Drop for FtpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so the acceptor thread sees the flag and ends.
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.acceptor.take() {
            let _ = h.join();
        }
    }
}

/// A connection, plain or TLS.
enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
}

impl Stream {
    /// The same connection as TLS, once the handshake is done.
    fn into_tls(self, config: &Arc<ServerConfig>) -> io::Result<Stream> {
        let Stream::Plain(sock) = self else {
            return Ok(self);
        };
        let conn = ServerConnection::new(Arc::clone(config)).map_err(io::Error::other)?;
        let mut tls = StreamOwned::new(conn, sock);
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock)?;
        }
        Ok(Stream::Tls(Box::new(tls)))
    }

    fn is_tls(&self) -> bool {
        matches!(self, Stream::Tls(_))
    }

    /// Ends the connection: TLS's `close_notify` first, then the socket's write side.
    fn close(self) {
        match self {
            Stream::Plain(sock) => {
                let _ = sock.shutdown(std::net::Shutdown::Write);
            }
            Stream::Tls(mut tls) => {
                tls.conn.send_close_notify();
                let _ = tls.conn.complete_io(&mut tls.sock);
                let _ = tls.sock.shutdown(std::net::Shutdown::Write);
            }
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// One line from the control connection, without its line end; `None` once the client closed it.
/// Byte by byte, so nothing past the line is read before a switch to TLS.
fn read_line(stream: &mut Stream) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Ok((!line.is_empty()).then(|| text(&line))),
            Ok(_) if byte[0] == b'\n' => return Ok(Some(text(&line))),
            Ok(_) => line.push(byte[0]),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

fn text(line: &[u8]) -> String {
    String::from_utf8_lossy(line)
        .trim_end_matches('\r')
        .to_owned()
}

fn reply(stream: &mut Stream, line: &str) -> io::Result<()> {
    stream.write_all(format!("{line}\r\n").as_bytes())?;
    stream.flush()
}

/// What one control connection has set up so far.
struct Session {
    user: String,
    logged_in: bool,
    cwd: String,
    offset: u64,
    /// `PROT P`: the data connections are TLS.
    protect: bool,
    /// The data connection a passive command opened, accepted (and TLS set up) in the background.
    data: Option<Receiver<io::Result<Stream>>>,
}

fn session(sock: TcpStream, shared: &Shared) -> io::Result<()> {
    sock.set_read_timeout(Some(IDLE))?;
    let mut control = Stream::Plain(sock);
    let implicit = matches!(shared.options.tls, FtpTls::Implicit(_));
    if let (true, Some(config)) = (implicit, &shared.tls) {
        control = control.into_tls(config)?;
        shared.record.lock().unwrap().tls_controls += 1;
    }
    reply(&mut control, "220 Velocity test FTP")?;
    let mut s = Session {
        user: String::new(),
        logged_in: false,
        cwd: "/".to_owned(),
        offset: 0,
        protect: implicit,
        data: None,
    };
    while let Some(line) = read_line(&mut control)? {
        let (cmd, arg) = match line.split_once(' ') {
            Some((c, a)) => (c.to_ascii_uppercase(), a.trim().to_owned()),
            None => (line.trim().to_ascii_uppercase(), String::new()),
        };
        shared.record.lock().unwrap().commands.push(cmd.clone());
        let answer = match cmd.as_str() {
            "AUTH" => match (&shared.options.tls, &shared.tls, control.is_tls()) {
                (FtpTls::Explicit(_), Some(config), false)
                    if arg.eq_ignore_ascii_case("TLS") || arg.eq_ignore_ascii_case("TLS-C") =>
                {
                    reply(&mut control, "234 AUTH TLS OK")?;
                    control = control.into_tls(config)?;
                    shared.record.lock().unwrap().tls_controls += 1;
                    continue;
                }
                _ => "504 AUTH not supported".to_owned(),
            },
            "PBSZ" if control.is_tls() => "200 PBSZ=0".to_owned(),
            "PROT" if control.is_tls() => match arg.to_ascii_uppercase().as_str() {
                "P" => {
                    s.protect = true;
                    "200 PROT P OK".to_owned()
                }
                "C" => {
                    s.protect = false;
                    "200 PROT C OK".to_owned()
                }
                _ => "504 PROT level not supported".to_owned(),
            },
            "PBSZ" | "PROT" => "503 AUTH TLS first".to_owned(),
            "USER" => {
                s.user = arg;
                "331 Password required".to_owned()
            }
            "PASS" => {
                // The command list keeps the verb only; the password is recorded as a login.
                shared
                    .record
                    .lock()
                    .unwrap()
                    .logins
                    .push((s.user.clone(), arg.clone()));
                s.logged_in = match &shared.options.login {
                    Some((u, p)) => *u == s.user && *p == arg,
                    None => true,
                };
                if s.logged_in {
                    "230 Logged in".to_owned()
                } else {
                    "530 Login incorrect".to_owned()
                }
            }
            "QUIT" => {
                reply(&mut control, "221 Bye")?;
                control.close();
                return Ok(());
            }
            "FEAT" => {
                for line in [
                    "211-Features:",
                    " EPSV",
                    " MDTM",
                    " PASV",
                    " REST STREAM",
                    " SIZE",
                    " AUTH TLS",
                    " PBSZ",
                    " PROT",
                ] {
                    reply(&mut control, line)?;
                }
                "211 End".to_owned()
            }
            _ if !s.logged_in => "530 Please log in".to_owned(),
            "SYST" => "215 UNIX Type: L8".to_owned(),
            "TYPE" | "MODE" | "STRU" | "NOOP" | "OPTS" => "200 OK".to_owned(),
            "PWD" => format!("257 \"{}\" is the current directory", s.cwd),
            "CWD" => {
                s.cwd = resolve(&s.cwd, &arg);
                "250 OK".to_owned()
            }
            "SIZE" if shared.options.no_size => "502 SIZE not implemented".to_owned(),
            "SIZE" => match shared.file(&resolve(&s.cwd, &arg)) {
                Some(file) => format!("213 {}", file.data.len()),
                None => "550 No such file".to_owned(),
            },
            "MDTM" if shared.options.no_mdtm => "502 MDTM not implemented".to_owned(),
            "MDTM" => match shared.file(&resolve(&s.cwd, &arg)) {
                Some(file) => format!("213 {}", file.modified),
                None => "550 No such file".to_owned(),
            },
            "REST" if shared.options.no_rest => "502 REST not implemented".to_owned(),
            "REST" => match arg.parse() {
                Ok(n) => {
                    s.offset = n;
                    shared.record.lock().unwrap().rests.push(n);
                    format!("350 Restarting at {n}")
                }
                Err(_) => "501 Bad offset".to_owned(),
            },
            "EPSV" | "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                s.data = Some(accept_data(listener, s.protect, shared.tls.clone()));
                if cmd == "EPSV" {
                    format!("229 Entering Extended Passive Mode (|||{port}|)")
                } else {
                    format!(
                        "227 Entering Passive Mode (127,0,0,1,{},{})",
                        port >> 8,
                        port & 0xff
                    )
                }
            }
            "RETR" => retr(&mut control, &mut s, shared, &arg)?,
            "ABOR" => "226 Aborted".to_owned(),
            _ => "502 Not implemented".to_owned(),
        };
        reply(&mut control, &answer)?;
    }
    Ok(())
}

impl Shared {
    fn file(&self, path: &str) -> Option<FtpFile> {
        self.files.lock().unwrap().get(path).cloned()
    }
}

/// Accepts the data connection a passive command announced, and sets up TLS on it if `protect`,
/// on a thread of its own: a client may connect, and start its handshake, before it sends `RETR`.
fn accept_data(
    listener: TcpListener,
    protect: bool,
    tls: Option<Arc<ServerConfig>>,
) -> Receiver<io::Result<Stream>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let accepted = (|| {
            listener.set_nonblocking(true)?;
            let deadline = Instant::now() + IDLE;
            let sock = loop {
                match listener.accept() {
                    Ok((sock, _)) => break sock,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        if Instant::now() > deadline {
                            return Err(io::Error::from(io::ErrorKind::TimedOut));
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => return Err(e),
                }
            };
            sock.set_nonblocking(false)?;
            sock.set_read_timeout(Some(IDLE))?;
            let stream = Stream::Plain(sock);
            match (protect, &tls) {
                (true, Some(config)) => stream.into_tls(config),
                _ => Ok(stream),
            }
        })();
        let _ = tx.send(accepted);
    });
    rx
}

/// Sends the file at `arg` from the session's offset over the data connection, and returns the
/// final reply. The `150` goes out first.
fn retr(control: &mut Stream, s: &mut Session, shared: &Shared, arg: &str) -> io::Result<String> {
    let offset = std::mem::take(&mut s.offset);
    let Some(file) = shared.file(&resolve(&s.cwd, arg)) else {
        return Ok("550 No such file".to_owned());
    };
    let Some(data) = s.data.take() else {
        return Ok("425 Use PASV or EPSV first".to_owned());
    };
    reply(control, "150 Opening BINARY mode data connection")?;
    let mut stream = match data.recv_timeout(IDLE) {
        Ok(Ok(stream)) => stream,
        _ => return Ok("425 Can't open data connection".to_owned()),
    };
    let cut = {
        let mut record = shared.record.lock().unwrap();
        record.retrs += 1;
        record.data_connections.push(stream.is_tls());
        let cut = shared.options.cut_first.filter(|_| !record.cut);
        record.cut |= cut.is_some();
        cut
    };
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(file.data.len());
    let end = match cut {
        Some(after) => start
            .saturating_add(usize::try_from(after).unwrap_or(usize::MAX))
            .min(file.data.len()),
        None => file.data.len(),
    };
    let mut sent = true;
    for chunk in file.data[start..end].chunks(CHUNK) {
        if stream.write_all(chunk).is_err() || stream.flush().is_err() {
            // The client closed the data connection: it has what it wanted, or gave up.
            sent = false;
            break;
        }
        if let Some(rate) = shared.options.throttle_bytes_per_sec {
            thread::sleep(Duration::from_secs_f64(
                chunk.len() as f64 / rate.max(1) as f64,
            ));
        }
    }
    stream.close();
    Ok(if sent && cut.is_none() {
        "226 Transfer complete".to_owned()
    } else {
        "426 Connection closed; transfer aborted".to_owned()
    })
}

/// `arg` resolved against `cwd`, as an absolute path without `.` or `..`.
fn resolve(cwd: &str, arg: &str) -> String {
    let joined = if arg.starts_with('/') {
        arg.to_owned()
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), arg)
    };
    let mut parts: Vec<&str> = Vec::new();
    for p in joined.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn paths_resolve_against_the_working_directory() {
        assert_eq!(resolve("/", "a.bin"), "/a.bin");
        assert_eq!(resolve("/pub", "a.bin"), "/pub/a.bin");
        assert_eq!(resolve("/pub", "/b/c.bin"), "/b/c.bin");
        assert_eq!(resolve("/pub/x", "../a.bin"), "/pub/a.bin");
        assert_eq!(resolve("/", "./pub/./a.bin"), "/pub/a.bin");
    }
}
