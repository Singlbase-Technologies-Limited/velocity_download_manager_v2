//! Redaction of secrets from text bound for logs and diagnostics (#28, V1's `redact.rs`). Every log
//! line, a child process's included, passes through [`redact`] before it is kept (DIAGNOSTICS.md).
//!
//! Hidden: URL logins (proxies' too), authorization, cookie and API-key headers (libcurl's debug
//! lines included), token-like query values, passwords and passphrases, private keys, magnet links,
//! the path and query of tracker URLs, and the secrets registered this session. Kept: hosts, ordinary
//! paths and queries, and SSH key fingerprints, which identify public keys.

use std::sync::{OnceLock, RwLock};

use regex::Regex;

/// Registered secrets shorter than this aren't replaced: they would hide ordinary words.
const MIN_SECRET_LEN: usize = 8;
/// The most secrets kept; the oldest goes first.
const MAX_SECRETS: usize = 64;

fn registered() -> &'static RwLock<Vec<String>> {
    static CELL: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(Vec::new()))
}

/// Makes every later [`redact`] replace `secret`: a password or passphrase given this session.
/// Kept in memory only, at most [`MAX_SECRETS`] of them.
pub fn register_secret(secret: &str) {
    if secret.len() < MIN_SECRET_LEN {
        return;
    }
    let mut list = registered().write().unwrap_or_else(|e| e.into_inner());
    if !list.iter().any(|s| s == secret) {
        if list.len() >= MAX_SECRETS {
            list.remove(0);
        }
        list.push(secret.to_owned());
    }
}

struct Patterns {
    private_key: Regex,
    magnet: Regex,
    tracker: Regex,
    header: Regex,
    userinfo: Regex,
    assignment: Regex,
    query: Regex,
}

fn patterns() -> &'static Patterns {
    static CELL: OnceLock<Patterns> = OnceLock::new();
    // The patterns are constants, checked by the tests; they can't fail at run time.
    let re = |p: &str| Regex::new(p).expect("a valid pattern");
    CELL.get_or_init(|| Patterns {
        private_key: re(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----"),
        // A whole magnet link: its hash, name and trackers identify what is downloaded.
        magnet: re(r"(?i)\bmagnet:\?[^\s\x22'<>]*"),
        // Tracker announce and scrape URLs: everything after the host may hold a passkey.
        tracker: re(r"(?i)\b((?:https?|udp)://[^/\s\x22'?#]+)/[^\s\x22']*(?:announce|scrape)[^\s\x22']*"),
        header: re(r"(?im)\b(authorization|proxy-authorization|cookie|set-cookie|x-api-key|x-auth-token)(\s*[:=]\s*)[^\r\n]*"),
        // Up to the last @ of the host part: a password may contain an @ itself.
        userinfo: re(r"(?i)\b([a-z][a-z0-9+.\-]*://)[^/?#\s\x22'<>]*@"),
        // `password=…`, `passphrase: …` and JSON's `"password":"…"`.
        assignment: re(r#"(?i)\b(password|passwd|passphrase|secret)("?\s*[=:]\s*)("(?:[^"\\]|\\.)*"|\S+)"#),
        // A token-like name at a word boundary, in a query or on its own.
        query: re(r"(?i)(\b(?:access_token|token|signature|sig|key|apikey|api_key|password|auth|passkey|authkey|torrent_pass|pid|uk|X-Amz-Signature|X-Amz-Credential|X-Goog-Signature)=)[^&\s#\x22]+"),
    })
}

/// Removes logins, cookies, authorization headers, tokens, private keys and registered secrets.
pub fn redact(input: &str) -> String {
    let p = patterns();
    let mut out = p
        .private_key
        .replace_all(input, "[redacted private key]")
        .into_owned();
    {
        let list = registered().read().unwrap_or_else(|e| e.into_inner());
        for secret in list.iter() {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[redacted]");
            }
        }
    }
    out = p.magnet.replace_all(&out, "magnet:[redacted]").into_owned();
    out = p.tracker.replace_all(&out, "$1/[redacted]").into_owned();
    out = p.header.replace_all(&out, "$1$2[redacted]").into_owned();
    out = p.userinfo.replace_all(&out, "$1[redacted]@").into_owned();
    out = p
        .assignment
        .replace_all(&out, "$1$2[redacted]")
        .into_owned();
    p.query.replace_all(&out, "$1[redacted]").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V1's `redacts_headers_tokens_and_userinfo`.
    #[test]
    fn redacts_headers_tokens_and_userinfo() {
        let s = "Authorization: Bearer abc.def\nCookie: a=b; c=d\n";
        let r = redact(s);
        assert!(!r.contains("abc.def") && !r.contains("a=b"), "{r}");
        let u = redact("https://user:pa55@example.com/file?id=1&signature=XYZ");
        assert!(
            !u.contains("pa55") && !u.contains("XYZ") && !u.contains("user:"),
            "{u}"
        );
        assert!(
            u.contains("id=1") && u.contains("example.com"),
            "the host and plain query stay: {u}"
        );
        assert!(redact("password=0123456789").contains("password=[redacted]"));
    }

    /// velocity-child's `hides_the_login_in_every_url` and `leaves_text_without_a_login_alone` (#39),
    /// now the shared redaction's.
    #[test]
    fn hides_the_login_in_every_url_up_to_the_last_at_sign() {
        assert_eq!(
            redact("ftp://anon@ftp.example.org and sftp://me:x@h:22/p"),
            "ftp://[redacted]@ftp.example.org and sftp://[redacted]@h:22/p"
        );
        // A password may contain an @ itself; everything up to the last @ of the host part goes.
        assert_eq!(redact("http://u:p@ss@host/"), "http://[redacted]@host/");
        for text in [
            "no url here",
            "https://example.com/path@not-a-login?q=a@b#c@d",
            "mail me@example.com",
            "://@",
            "",
        ] {
            assert_eq!(redact(text), text);
        }
    }

    /// V1's `redacts_magnet_links_and_tracker_urls`.
    #[test]
    fn redacts_magnet_links_and_tracker_urls() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let line = format!(
            "Adding magnet:?xt=urn:btih:{hash}&dn=secret.name&tr=http%3A%2F%2Ft.test%2Fannounce now"
        );
        let r = redact(&line);
        assert!(
            !r.contains(hash) && !r.contains("secret.name") && !r.contains("t.test"),
            "{r}"
        );
        assert!(r.contains("magnet:[redacted] now"), "{r}");
        // Private trackers put the passkey in the path or the query.
        for (url, secret) in [
            (
                "http://tracker.test:2710/a1b2c3d4e5f6/announce",
                "a1b2c3d4e5f6",
            ),
            (
                "https://tracker.test/announce.php?passkey=deadbeef99",
                "deadbeef99",
            ),
            (
                "udp://tracker.test:6969/announce?authkey=feedface77",
                "feedface77",
            ),
            (
                "http://tracker.test/scrape?info_hash=%12%34&pid=abc123",
                "abc123",
            ),
        ] {
            let r = redact(&format!("Tracker {url} failed"));
            assert!(!r.contains(secret), "{r}");
            assert!(r.contains("tracker.test"), "the host is kept: {r}");
        }
        // Ordinary download links are unaffected.
        assert_eq!(
            redact("https://cdn.test/files/a.zip?id=1"),
            "https://cdn.test/files/a.zip?id=1"
        );
    }

    /// V1's `redacts_registered_secrets`.
    #[test]
    fn redacts_registered_secrets() {
        register_secret("supersecretvalue123");
        assert_eq!(redact("x supersecretvalue123 y"), "x [redacted] y");
        register_secret("short");
        assert_eq!(redact("short"), "short", "too short to register");
    }

    #[test]
    fn redacts_private_keys_but_keeps_ssh_fingerprints() {
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQUJDREVGR0g=\n-----END OPENSSH PRIVATE KEY-----";
        let r = redact(&format!("key {key} read"));
        assert!(
            !r.contains("b3BlbnNzaC1rZXktdjEAAAAA") && !r.contains("QUJDREVGR0g="),
            "{r}"
        );
        assert!(r.contains("[redacted private key]"), "{r}");
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow==\n-----END RSA PRIVATE KEY-----";
        assert!(!redact(pem).contains("MIIEow=="));
        // A fingerprint identifies a public key, so it stays (the host-key question shows it, #53).
        let fp = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";
        assert_eq!(redact(&format!("host key {fp}")), format!("host key {fp}"));
    }

    #[test]
    fn redacts_proxy_logins_and_curl_debug_lines() {
        let r = redact("using proxy socks5h://alice:hunter2pass@proxy.test:1080");
        assert!(!r.contains("hunter2pass") && !r.contains("alice"), "{r}");
        assert!(r.contains("proxy.test:1080"), "{r}");
        // libcurl's verbose output marks sent headers with `> ` and received ones with `< `.
        for line in [
            "> Proxy-Authorization: Basic YWxpY2U6aHVudGVyMnBhc3M=",
            "> authorization: Digest username=\"me\", response=\"6629fae49393a053\"",
            "< Set-Cookie: session=0f1e2d3c; HttpOnly",
            "> X-Api-Key: sk-live-12345678",
        ] {
            let r = redact(line);
            for secret in [
                "YWxpY2U6aHVudGVyMnBhc3M=",
                "6629fae49393a053",
                "0f1e2d3c",
                "sk-live-12345678",
            ] {
                assert!(!r.contains(secret), "{line} -> {r}");
            }
        }
        assert!(!redact("access_token=eyJhbGciOi.payload.sig&x=1").contains("eyJhbGciOi"));
    }

    #[test]
    fn redacts_a_basic_login_in_an_error_detail() {
        let r = redact(r#"{"credentials":{"userName":"me","password":"p@ss w0rd!"}}"#);
        assert!(!r.contains("p@ss w0rd!"), "{r}");
        let r = redact("passphrase: correct horse battery staple");
        assert!(!r.contains("correct horse"), "{r}");
    }
}
