//! What a user or the browser may add to a download's requests (#40): custom headers, a referrer,
//! a user agent and a login. All of it is untrusted input, checked here before it reaches an
//! engine. Ported from V1's `validation.rs` (`validate_headers`, `validate_referrer`, credential
//! extraction), with the headers V2's engine manages itself added to the refused ones.

use crate::domain::ErrorCode;
use crate::naming::percent_decode;
use crate::transfer::{Credentials, Header};

/// The most custom headers one download may have.
pub const MAX_HEADERS: usize = 32;
/// The longest header name, and the longest value, in bytes.
pub const MAX_HEADER_NAME_BYTES: usize = 128;
pub const MAX_HEADER_VALUE_BYTES: usize = 8192;
/// The longest URL, referrer included, in bytes.
pub const MAX_URL_BYTES: usize = 8192;
/// The longest user agent, in bytes (V1's limit).
pub const MAX_USER_AGENT_BYTES: usize = 256;
/// The longest user name, and the longest password, in bytes.
pub const MAX_LOGIN_PART_BYTES: usize = 1024;

/// The user agent sent when the user hasn't set one (PO decision on #40).
pub fn default_user_agent() -> String {
    format!("VelocityDownloadManager/{}", env!("CARGO_PKG_VERSION"))
}

/// Headers the engine sets itself, or that have a field of their own (`Referer`, `User-Agent`):
/// a custom one would contradict what the engine asks for (ranges, `If-Range`, no encoding) or
/// the connection it manages. `Proxy-Authorization` is refused because V2 sends it to no proxy,
/// so it would only reach the server.
const MANAGED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "range",
    "if-range",
    "accept-encoding",
    "connection",
    "transfer-encoding",
    "te",
    "upgrade",
    "expect",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "referer",
    "user-agent",
];

/// `InvalidInput` unless every header has a valid name (an RFC 9110 token) and a value without
/// control characters but tab (so no CR or LF to start another header), within the limits, and
/// none is one the engine manages itself.
pub fn check_headers(headers: &[Header]) -> Result<(), ErrorCode> {
    if headers.len() > MAX_HEADERS {
        return Err(ErrorCode::InvalidInput);
    }
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let valid = |h: &Header| {
        !h.name.is_empty()
            && h.name.len() <= MAX_HEADER_NAME_BYTES
            && h.name.chars().all(token)
            && !MANAGED_HEADERS.contains(&h.name.to_ascii_lowercase().as_str())
            && h.value.len() <= MAX_HEADER_VALUE_BYTES
            && !h.value.chars().any(|c| c.is_control() && c != '\t')
    };
    if headers.iter().all(valid) {
        Ok(())
    } else {
        Err(ErrorCode::InvalidInput)
    }
}

/// `InvalidUrl` unless `referrer` is an `http` or `https` URL with a host and without a login,
/// spaces or control characters, within [`MAX_URL_BYTES`].
pub fn check_referrer(referrer: &str) -> Result<(), ErrorCode> {
    let well_formed = referrer.len() <= MAX_URL_BYTES
        && !referrer
            .chars()
            .any(|c| c.is_whitespace() || c.is_control());
    let Some((scheme, rest)) = referrer.split_once("://").filter(|_| well_formed) else {
        return Err(ErrorCode::InvalidUrl);
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let http = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    if http && !authority.is_empty() && !authority.contains('@') {
        Ok(())
    } else {
        Err(ErrorCode::InvalidUrl)
    }
}

/// `InvalidInput` unless `user_agent` is non-empty, without control characters, within
/// [`MAX_USER_AGENT_BYTES`].
pub fn check_user_agent(user_agent: &str) -> Result<(), ErrorCode> {
    if user_agent.is_empty()
        || user_agent.len() > MAX_USER_AGENT_BYTES
        || user_agent.chars().any(char::is_control)
    {
        Err(ErrorCode::InvalidInput)
    } else {
        Ok(())
    }
}

/// `InvalidInput` unless the user name is non-empty without a colon (RFC 7617 §2), and neither
/// part has a control character or exceeds [`MAX_LOGIN_PART_BYTES`].
pub fn check_credentials(credentials: &Credentials) -> Result<(), ErrorCode> {
    let Credentials {
        user_name,
        password,
    } = credentials;
    let part = |s: &str| s.len() <= MAX_LOGIN_PART_BYTES && !s.chars().any(char::is_control);
    if !user_name.is_empty() && !user_name.contains(':') && part(user_name) && part(password) {
        Ok(())
    } else {
        Err(ErrorCode::InvalidInput)
    }
}

/// `url` without its login, and the login, percent-decoded: `https://al%40ice:pw@host/f` is
/// `https://host/f` and `al@ice` with `pw`. A URL without one comes back as it is. The login ends
/// at the last `@` before the path, since a password may contain one.
pub fn split_login(url: &str) -> (String, Option<Credentials>) {
    let Some(at) = url.find("://") else {
        return (url.to_owned(), None);
    };
    let (scheme, rest) = url.split_at(at + 3);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some(login_end) = rest[..end].rfind('@') else {
        return (url.to_owned(), None);
    };
    let login = &rest[..login_end];
    let (user_name, password) = login.split_once(':').unwrap_or((login, ""));
    let credentials = Credentials {
        user_name: percent_decode(user_name),
        password: percent_decode(password),
    };
    (
        format!("{scheme}{}", &rest[login_end + 1..]),
        Some(credentials),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> Header {
        Header {
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn ordinary_headers_are_accepted() {
        let headers = [
            header("Authorization", "Bearer abc"),
            header("Cookie", "a=1; b=2"),
            header("X-Custom_Thing.1", "any value: with colons"),
            header("Accept", ""),
            header("X-Tab", "a\tb"),
            header("X-Unicode", "résumé"),
        ];
        assert_eq!(check_headers(&headers), Ok(()));
    }

    #[test]
    fn a_header_value_cannot_inject_another_header() {
        for value in ["a\r\nInjected: 1", "a\nb", "a\rb", "a\0b"] {
            assert_eq!(
                check_headers(&[header("X-A", value)]),
                Err(ErrorCode::InvalidInput),
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_header_value_has_no_control_character_but_tab() {
        // RFC 9110 §5.5: field values are visible characters, spaces and tabs.
        for value in ["a\u{1}b", "a\u{1b}[31m", "a\u{7f}b", "a\u{85}b"] {
            assert_eq!(
                check_headers(&[header("X-A", value)]),
                Err(ErrorCode::InvalidInput),
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_header_name_must_be_a_token() {
        for name in ["", "Bad Name", "X:A", "X\nA", "Ünicode", &"x".repeat(129)] {
            assert_eq!(
                check_headers(&[header(name, "v")]),
                Err(ErrorCode::InvalidInput),
                "{name:?}"
            );
        }
    }

    #[test]
    fn headers_the_engine_manages_are_refused_in_any_case() {
        for name in [
            "Host",
            "content-length",
            "RANGE",
            "If-Range",
            "Accept-Encoding",
            "Connection",
            "Transfer-Encoding",
            "TE",
            "Upgrade",
            "Expect",
            "Keep-Alive",
            "Proxy-Connection",
            "Proxy-Authorization",
            "Referer",
            "User-Agent",
        ] {
            assert_eq!(
                check_headers(&[header(name, "x")]),
                Err(ErrorCode::InvalidInput),
                "{name}"
            );
        }
    }

    #[test]
    fn headers_are_limited_in_number_and_size() {
        let many: Vec<Header> = (0..=MAX_HEADERS)
            .map(|i| header(&format!("X-{i}"), "v"))
            .collect();
        assert_eq!(check_headers(&many), Err(ErrorCode::InvalidInput));
        assert_eq!(check_headers(&many[..MAX_HEADERS]), Ok(()));
        let long = "v".repeat(MAX_HEADER_VALUE_BYTES + 1);
        assert_eq!(
            check_headers(&[header("X-A", &long)]),
            Err(ErrorCode::InvalidInput)
        );
    }

    #[test]
    fn a_referrer_is_an_http_url_without_a_login() {
        assert_eq!(check_referrer("https://example.com/page?q=1"), Ok(()));
        assert_eq!(check_referrer("HTTP://example.com/"), Ok(()));
        for bad in [
            "ftp://example.com/",
            "javascript:alert(1)",
            "https://u:p@example.com/",
            "https://example.com/a b",
            "https://example.com/\r\nX: 1",
            "https://",
            "",
        ] {
            assert!(check_referrer(bad).is_err(), "{bad:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!(check_referrer(&long), Err(ErrorCode::InvalidUrl));
    }

    #[test]
    fn the_default_user_agent_names_velocity_and_its_version() {
        let agent = default_user_agent();
        assert_eq!(
            agent,
            format!("VelocityDownloadManager/{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(check_user_agent(&agent), Ok(()));
    }

    #[test]
    fn a_user_agent_is_short_and_without_control_characters() {
        assert_eq!(check_user_agent("Mozilla/5.0 (X11; Linux x86_64)"), Ok(()));
        for bad in [
            "",
            "a\r\nX: 1",
            "tab\there",
            &"a".repeat(MAX_USER_AGENT_BYTES + 1),
        ] {
            assert_eq!(
                check_user_agent(bad),
                Err(ErrorCode::InvalidInput),
                "{bad:?}"
            );
        }
    }

    fn login(user_name: &str, password: &str) -> Credentials {
        Credentials {
            user_name: user_name.to_owned(),
            password: password.to_owned(),
        }
    }

    #[test]
    fn a_login_needs_a_user_name_without_a_colon() {
        assert_eq!(check_credentials(&login("alice", "p:a@s s")), Ok(()));
        assert_eq!(check_credentials(&login("alice", "")), Ok(()));
        for bad in [
            login("", "pw"),
            login("a:b", "pw"),
            login("a\r\nb", "pw"),
            login("alice", "p\nw"),
            login("alice", "p\0w"),
            login(&"a".repeat(MAX_LOGIN_PART_BYTES + 1), "pw"),
            login("alice", &"p".repeat(MAX_LOGIN_PART_BYTES + 1)),
        ] {
            assert_eq!(
                check_credentials(&bad),
                Err(ErrorCode::InvalidInput),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_login_is_moved_out_of_the_url_and_decoded() {
        let (url, found) = split_login("https://al%40ice:s3cr%3At@example.com:8443/f?q=a@b#x");
        assert_eq!(url, "https://example.com:8443/f?q=a@b#x");
        assert_eq!(found, Some(login("al@ice", "s3cr:t")));
        // A password may contain an @: the login ends at the last @ before the path.
        let (url, found) = split_login("http://u:p@ss@host/x");
        assert_eq!(url, "http://host/x");
        assert_eq!(found, Some(login("u", "p@ss")));
        // A user name alone.
        let (url, found) = split_login("ftp://anon@ftp.example.org/f");
        assert_eq!(url, "ftp://ftp.example.org/f");
        assert_eq!(found, Some(login("anon", "")));
    }

    #[test]
    fn a_url_without_a_login_is_kept_as_it_is() {
        for url in [
            "https://example.com/path@not-a-login?q=a@b",
            "https://example.com",
            "not a url",
        ] {
            assert_eq!(split_login(url), (url.to_owned(), None), "{url}");
        }
    }

    #[test]
    fn a_login_never_shows_in_debug_output() {
        let shown = format!("{:?}", login("alice", "hunter2"));
        assert!(
            !shown.contains("alice") && !shown.contains("hunter2"),
            "{shown}"
        );
    }
}
