//! Reaching an SFTP server through the user's proxy (#192, PO decision): russh has no proxy
//! support, so the connection is opened here and russh runs over it. An HTTP proxy is asked for a
//! `CONNECT` tunnel (RFC 9110 §9.3.6), with its login as `Proxy-Authorization: Basic`; a SOCKS5
//! proxy for a `CONNECT` (RFC 1928), with its login as RFC 1929's. Either way the proxy resolves
//! the server's name, and its login goes to the proxy alone. An HTTPS proxy, which would need TLS
//! here, is refused. The host key is then checked against the server's name, as without a proxy.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use velocity_curl::{CurlFailure, FetchError, ProxyConfig, ProxyKind};

use crate::client::failure;

/// The most an HTTP proxy's answer to `CONNECT` may hold before its blank line.
const MAX_CONNECT_ANSWER: usize = 8 * 1024;

/// A connection to `host:port` through `proxy`, ready for SSH.
pub(crate) async fn tunnel(
    proxy: &ProxyConfig,
    host: &str,
    port: u16,
) -> Result<TcpStream, FetchError> {
    if proxy.kind == ProxyKind::Https {
        return Err(failure(
            CurlFailure::Refused,
            "SFTP can't go through an HTTPS proxy; choose an HTTP or SOCKS5 proxy in Settings, Proxy",
        ));
    }
    let proxy_host = proxy.host.trim_start_matches('[').trim_end_matches(']');
    let mut stream = TcpStream::connect((proxy_host, proxy.port))
        .await
        .map_err(|e| {
            failure(
                CurlFailure::Connect,
                format!("the proxy {}: {e}", proxy.host),
            )
        })?;
    match proxy.kind {
        ProxyKind::Socks5 => socks5(&mut stream, proxy, host, port).await?,
        ProxyKind::Http | ProxyKind::Https => http_connect(&mut stream, proxy, host, port).await?,
    }
    Ok(stream)
}

fn broken(e: std::io::Error) -> FetchError {
    failure(CurlFailure::Interrupted, format!("the proxy: {e}"))
}

fn refused(why: impl std::fmt::Display) -> FetchError {
    failure(
        CurlFailure::ProxyRefused,
        format!("the proxy refused: {why}"),
    )
}

async fn http_connect(
    stream: &mut TcpStream,
    proxy: &ProxyConfig,
    host: &str,
    port: u16,
) -> Result<(), FetchError> {
    let target = format!("{host}:{port}");
    let mut ask = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some((user, password)) = &proxy.login {
        ask.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64(format!("{user}:{password}").as_bytes())
        ));
    }
    ask.push_str("\r\n");
    stream.write_all(ask.as_bytes()).await.map_err(broken)?;
    // Byte by byte up to the blank line, so nothing of the tunnel's own data is read here.
    let mut answer = Vec::new();
    while !answer.ends_with(b"\r\n\r\n") {
        if answer.len() >= MAX_CONNECT_ANSWER {
            return Err(refused("its answer to CONNECT was too long"));
        }
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).await.map_err(broken)? == 0 {
            return Err(failure(
                CurlFailure::Interrupted,
                "the proxy closed the connection",
            ));
        }
        answer.push(byte[0]);
    }
    let status_line = String::from_utf8_lossy(&answer);
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(200..=299) => Ok(()),
        Some(407) => Err(refused("its login was refused (407)")),
        Some(code) => Err(refused(format!("CONNECT answered {code}"))),
        None => Err(failure(
            CurlFailure::Protocol,
            "the proxy's answer to CONNECT wasn't HTTP",
        )),
    }
}

async fn socks5(
    stream: &mut TcpStream,
    proxy: &ProxyConfig,
    host: &str,
    port: u16,
) -> Result<(), FetchError> {
    // Greeting: no login, or user name and password when there is one (RFC 1928 §3).
    let methods: &[u8] = if proxy.login.is_some() {
        &[0x00, 0x02]
    } else {
        &[0x00]
    };
    let mut hello = vec![0x05, methods.len() as u8];
    hello.extend_from_slice(methods);
    stream.write_all(&hello).await.map_err(broken)?;
    let mut chosen = [0u8; 2];
    stream.read_exact(&mut chosen).await.map_err(broken)?;
    match chosen {
        [0x05, 0x00] => {}
        [0x05, 0x02] => {
            let Some((user, password)) = &proxy.login else {
                return Err(refused("it asks for a login, and none is set"));
            };
            let (user, password) = (user.as_bytes(), password.as_bytes());
            if user.len() > 255 || password.len() > 255 {
                return Err(refused("its login is longer than SOCKS5 allows"));
            }
            // RFC 1929 §2.
            let mut login = vec![0x01, user.len() as u8];
            login.extend_from_slice(user);
            login.push(password.len() as u8);
            login.extend_from_slice(password);
            stream.write_all(&login).await.map_err(broken)?;
            let mut answer = [0u8; 2];
            stream.read_exact(&mut answer).await.map_err(broken)?;
            if answer[1] != 0x00 {
                return Err(refused("its login was refused"));
            }
        }
        [0x05, _] => return Err(refused("no way to log in that V2 offers")),
        _ => {
            return Err(failure(
                CurlFailure::Protocol,
                "the proxy doesn't speak SOCKS5",
            ))
        }
    }
    // CONNECT to the server by name, so the proxy resolves it (RFC 1928 §4, §5).
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let mut ask = vec![0x05, 0x01, 0x00];
    match bare.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            ask.push(0x01);
            ask.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            ask.push(0x04);
            ask.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            if bare.len() > 255 {
                return Err(refused("the server's name is longer than SOCKS5 allows"));
            }
            ask.push(0x03);
            ask.push(bare.len() as u8);
            ask.extend_from_slice(bare.as_bytes());
        }
    }
    ask.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&ask).await.map_err(broken)?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.map_err(broken)?;
    if head[0] != 0x05 {
        return Err(failure(
            CurlFailure::Protocol,
            "the proxy doesn't speak SOCKS5",
        ));
    }
    if head[1] != 0x00 {
        return Err(refused(format!("SOCKS5 reply {}", head[1])));
    }
    // The address the proxy bound, which is of no use here, then the port.
    let rest = match head[3] {
        0x01 => 4 + 2,
        0x04 => 16 + 2,
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(broken)?;
            usize::from(len[0]) + 2
        }
        _ => {
            return Err(failure(
                CurlFailure::Protocol,
                "the proxy's SOCKS5 reply has an unknown address type",
            ))
        }
    };
    let mut skipped = vec![0u8; rest];
    stream.read_exact(&mut skipped).await.map_err(broken)?;
    Ok(())
}

/// Standard base64 with padding (RFC 4648 §4), for the proxy's Basic login.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_rfc_4648s() {
        assert_eq!(
            base64(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }
}
