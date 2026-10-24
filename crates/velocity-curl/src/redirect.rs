//! Which redirects a fetch follows, and what it still sends after one (#40, #67). Velocity
//! follows redirects itself instead of letting libcurl do it, because libcurl would forward custom
//! headers to another host. Each hop is checked here, on libcurl's own reading of the URL.

use crate::fetch::CurlFailure;
use crate::url::{origin, Origin};

/// The most redirects one fetch follows.
pub const MAX_REDIRECTS: u32 = 10;

/// A hop Velocity follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// The absolute URL to request next, as libcurl resolved the `Location`.
    pub url: String,
    /// It goes to the first URL's origin: the login and the custom headers may go with it.
    pub same_origin: bool,
}

/// Why a redirect isn't followed. The message names no URL, which may hold a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub kind: CurlFailure,
    pub message: String,
}

/// Decides the redirect from `from` to `to` (absolute), after `followed` redirects of a fetch
/// whose first URL had the origin `first`: only to `http` or `https`, never from `https` to
/// `http` (no confirmation exists yet, #67), and at most [`MAX_REDIRECTS`].
pub fn next_hop(
    first: Option<&Origin>,
    from: &str,
    to: &str,
    followed: u32,
) -> Result<Hop, Refusal> {
    let refuse = |kind, message: String| Err(Refusal { kind, message });
    if followed >= MAX_REDIRECTS {
        return refuse(
            CurlFailure::TooManyRedirects,
            format!("more than {MAX_REDIRECTS} redirects"),
        );
    }
    let Some(target) = origin(to) else {
        return refuse(
            CurlFailure::Refused,
            "a redirect to an address that can't be read".to_owned(),
        );
    };
    if !matches!(target.scheme.as_str(), "http" | "https") {
        return refuse(
            CurlFailure::Refused,
            format!("a redirect to {}://, which isn't allowed", target.scheme),
        );
    }
    let from_https = origin(from).is_some_and(|o| o.scheme == "https");
    if from_https && target.scheme == "http" {
        return refuse(
            CurlFailure::InsecureRedirect,
            "a redirect from https:// to http://, which isn't allowed".to_owned(),
        );
    }
    Ok(Hop {
        url: to.to_owned(),
        same_origin: first == Some(&target),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first() -> Origin {
        origin("https://files.example.com/a.zip").expect("an origin")
    }

    fn hop(from: &str, to: &str, followed: u32) -> Result<Hop, Refusal> {
        next_hop(Some(&first()), from, to, followed)
    }

    const FROM: &str = "https://files.example.com/a.zip";

    #[test]
    fn a_redirect_within_the_origin_keeps_what_the_fetch_sends() {
        assert_eq!(
            hop(FROM, "https://files.example.com/b.zip?x=1", 0),
            Ok(Hop {
                url: "https://files.example.com/b.zip?x=1".to_owned(),
                same_origin: true,
            })
        );
        // The default port is the same origin.
        assert_eq!(
            hop(FROM, "https://FILES.example.com:443/c", 3).map(|h| h.same_origin),
            Ok(true)
        );
    }

    #[test]
    fn a_redirect_that_leaves_the_origin_drops_secrets() {
        for to in [
            "https://cdn.example.net/a.zip",
            "https://files.example.com:8443/a.zip",
            "https://u:p@other.example/a.zip",
        ] {
            assert_eq!(hop(FROM, to, 0).map(|h| h.same_origin), Ok(false), "{to}");
        }
        // Coming back to the first origin later is the same origin again.
        assert_eq!(
            hop(
                "https://cdn.example.net/x",
                "https://files.example.com/y",
                1
            )
            .map(|h| h.same_origin),
            Ok(true)
        );
        // Without a first origin, nothing is the same origin.
        assert_eq!(
            next_hop(None, FROM, "https://files.example.com/b", 0).map(|h| h.same_origin),
            Ok(false)
        );
    }

    #[test]
    fn http_may_redirect_to_https() {
        let from = "http://files.example.com/a.zip";
        let to = "https://files.example.com/a.zip";
        let hop = next_hop(origin(from).as_ref(), from, to, 0).expect("followed");
        // Another scheme is another origin.
        assert!(!hop.same_origin);
    }

    #[test]
    fn https_never_redirects_to_http() {
        let refused = hop(FROM, "http://files.example.com/a.zip", 0).unwrap_err();
        assert_eq!(refused.kind, CurlFailure::InsecureRedirect);
        assert!(
            !refused.message.contains("files.example.com"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn a_redirect_to_another_protocol_is_refused() {
        for to in [
            "ftp://files.example.com/a.zip",
            "file:///etc/passwd",
            "sftp://files.example.com/a.zip",
            "scp://files.example.com/a.zip",
            "gopher://files.example.com/",
            "not a url",
        ] {
            let refused = hop(FROM, to, 0).unwrap_err();
            assert_eq!(refused.kind, CurlFailure::Refused, "{to}");
            assert!(
                !refused.message.contains("files.example.com"),
                "{}",
                refused.message
            );
        }
    }

    #[test]
    fn at_most_ten_redirects_are_followed() {
        let to = "https://files.example.com/next";
        assert!(hop(FROM, to, MAX_REDIRECTS - 1).is_ok());
        assert_eq!(
            hop(FROM, to, MAX_REDIRECTS).unwrap_err().kind,
            CurlFailure::TooManyRedirects
        );
    }
}
