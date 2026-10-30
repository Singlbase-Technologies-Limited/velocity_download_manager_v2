//! Local proxies for the proxy tests (#192): an HTTP proxy that only tunnels (`CONNECT`, RFC 9110
//! §9.3.6) and refuses any other request, and a SOCKS5 proxy (RFC 1928, with RFC 1929's login),
//! each on 127.0.0.1 with an optional required login. Each records what it was asked, so a test
//! can prove a connection went through it, which login it got, and that it never saw a request
//! itself. The SOCKS5 proxy resolves names, and can be told a test name's address. Standard
//! library only; test code that trusts its callers.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::sync::{Arc, Mutex};
use std::thread;

/// What a proxy was asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyRecord {
    /// Every `host:port` a client asked to reach, as asked.
    pub targets: Vec<String>,
    /// The login each ask carried, if any.
    pub logins: Vec<Option<(String, String)>>,
    /// Asks refused for a missing or wrong login.
    pub refused: usize,
    /// Requests an HTTP proxy got that weren't `CONNECT` (it refuses them).
    pub plain_requests: usize,
}

type Login = Option<(String, String)>;

/// An HTTP proxy that only tunnels.
pub struct HttpProxy {
    addr: SocketAddr,
    record: Arc<Mutex<ProxyRecord>>,
}

impl HttpProxy {
    /// Starts one that requires `login`, or none.
    pub fn start(login: Option<(&str, &str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("address");
        let record = Arc::new(Mutex::new(ProxyRecord::default()));
        let required: Login = login.map(|(u, p)| (u.to_owned(), p.to_owned()));
        let shared = Arc::clone(&record);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (record, required) = (Arc::clone(&shared), required.clone());
                thread::spawn(move || {
                    let _ = serve_http(stream, &record, required.as_ref());
                });
            }
        });
        HttpProxy { addr, record }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn record(&self) -> ProxyRecord {
        self.record.lock().unwrap().clone()
    }
}

fn serve_http(
    mut client: TcpStream,
    record: &Mutex<ProxyRecord>,
    required: Option<&(String, String)>,
) -> io::Result<()> {
    let head = read_head(&mut client)?;
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    if !method.eq_ignore_ascii_case("CONNECT") {
        record.lock().unwrap().plain_requests += 1;
        return client.write_all(
            b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }
    let login = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authorization"))
        .and_then(|(_, value)| value.trim().strip_prefix("Basic ").map(str::to_owned))
        .and_then(|b64| base64_decode(b64.trim()))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|pair| {
            pair.split_once(':')
                .map(|(u, p)| (u.to_owned(), p.to_owned()))
        });
    {
        let mut r = record.lock().unwrap();
        r.targets.push(target.to_owned());
        r.logins.push(login.clone());
    }
    if required.is_some() && login.as_ref() != required {
        record.lock().unwrap().refused += 1;
        return client.write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }
    let server = match target.to_socket_addrs().ok().and_then(|mut a| a.next()) {
        Some(addr) => TcpStream::connect(addr),
        None => Err(io::Error::new(io::ErrorKind::NotFound, "unknown target")),
    };
    match server {
        Ok(server) => {
            client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
            relay(client, server);
            Ok(())
        }
        Err(_) => client.write_all(
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ),
    }
}

/// A SOCKS5 proxy, which resolves the names it is asked for.
pub struct Socks5Proxy {
    addr: SocketAddr,
    record: Arc<Mutex<ProxyRecord>>,
}

impl Socks5Proxy {
    /// Starts one that requires `login`, or none, and resolves each of `names` to its address
    /// (other names as the system does).
    pub fn start(login: Option<(&str, &str)>, names: &[(&str, IpAddr)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("address");
        let record = Arc::new(Mutex::new(ProxyRecord::default()));
        let required: Login = login.map(|(u, p)| (u.to_owned(), p.to_owned()));
        let names: Arc<HashMap<String, IpAddr>> =
            Arc::new(names.iter().map(|(n, ip)| ((*n).to_owned(), *ip)).collect());
        let shared = Arc::clone(&record);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (record, required, names) =
                    (Arc::clone(&shared), required.clone(), Arc::clone(&names));
                thread::spawn(move || {
                    let _ = serve_socks5(stream, &record, required.as_ref(), &names);
                });
            }
        });
        Socks5Proxy { addr, record }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn record(&self) -> ProxyRecord {
        self.record.lock().unwrap().clone()
    }
}

fn serve_socks5(
    mut client: TcpStream,
    record: &Mutex<ProxyRecord>,
    required: Option<&(String, String)>,
    names: &HashMap<String, IpAddr>,
) -> io::Result<()> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head)?;
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods)?;
    let want = if required.is_some() { 0x02 } else { 0x00 };
    if head[0] != 0x05 || !methods.contains(&want) {
        record.lock().unwrap().refused += 1;
        return client.write_all(&[0x05, 0xFF]);
    }
    client.write_all(&[0x05, want])?;
    let mut login = None;
    if want == 0x02 {
        let mut version_and_len = [0u8; 2];
        client.read_exact(&mut version_and_len)?;
        let mut user = vec![0u8; usize::from(version_and_len[1])];
        client.read_exact(&mut user)?;
        let mut len = [0u8; 1];
        client.read_exact(&mut len)?;
        let mut password = vec![0u8; usize::from(len[0])];
        client.read_exact(&mut password)?;
        let given = (
            String::from_utf8_lossy(&user).into_owned(),
            String::from_utf8_lossy(&password).into_owned(),
        );
        let ok = Some(&given) == required;
        login = Some(given);
        if !ok {
            let mut r = record.lock().unwrap();
            r.logins.push(login);
            r.refused += 1;
            return client.write_all(&[0x01, 0x01]);
        }
        client.write_all(&[0x01, 0x00])?;
    }
    let mut ask = [0u8; 4];
    client.read_exact(&mut ask)?;
    let host = match ask[3] {
        0x01 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip)?;
            Ipv4Addr::from(ip).to_string()
        }
        0x04 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip)?;
            Ipv6Addr::from(ip).to_string()
        }
        _ => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len)?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name)?;
            String::from_utf8_lossy(&name).into_owned()
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port)?;
    let port = u16::from_be_bytes(port);
    {
        let mut r = record.lock().unwrap();
        r.targets.push(format!("{host}:{port}"));
        r.logins.push(login);
    }
    let address = match names.get(&host) {
        Some(ip) => Some(SocketAddr::new(*ip, port)),
        None => (host.as_str(), port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next()),
    };
    match address.map(TcpStream::connect) {
        Some(Ok(server)) => {
            client.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
            relay(client, server);
            Ok(())
        }
        // General failure (RFC 1928 §6).
        _ => client.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0]),
    }
}

/// Reads up to the blank line ending a request's head, byte by byte, so nothing after it is lost.
fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    Ok(head)
}

/// Copies both ways until either side closes.
fn relay(client: TcpStream, server: TcpStream) {
    let (Ok(client_in), Ok(server_out)) = (client.try_clone(), server.try_clone()) else {
        return;
    };
    let upstream = thread::spawn(move || copy(client_in, server_out));
    copy(server, client);
    let _ = upstream.join();
}

fn copy(mut from: TcpStream, mut to: TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut bits, mut count) = (0u32, 0);
    for c in text.bytes().filter(|&c| c != b'=') {
        let v = ALPHABET.iter().position(|&a| a == c)? as u32;
        bits = (bits << 6) | v;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_a_basic_login() {
        assert_eq!(
            base64_decode("QWxhZGRpbjpvcGVuIHNlc2FtZQ==").unwrap(),
            b"Aladdin:open sesame"
        );
    }
}
