//! URLs as libcurl reads them (#40): the same parser that sends a request decides which origin it
//! goes to, so a URL can't mean one host to Velocity's rules and another to libcurl. libcurl's URL
//! API (`curl_url`, curl 7.62+) isn't declared by curl-sys 0.4.91, so its few functions are
//! declared here, from `curl/urlapi.h` of curl 8.23.0.

use std::ffi::{c_char, c_int, c_uint, CStr, CString};
use std::ptr;

/// Where a request goes: scheme, host and port, compared to decide what a redirect may carry
/// (RFC 6454 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// Lower case.
    pub scheme: String,
    /// Lower case; an IPv6 address in brackets.
    pub host: String,
    /// The scheme's default port when the URL gives none.
    pub port: u16,
}

/// The origin of an absolute URL, or `None` when libcurl can't read it or it has no host.
pub fn origin(url: &str) -> Option<Origin> {
    let parsed = Parsed::new(url)?;
    let scheme = parsed.part(CURLUPART_SCHEME, 0)?.to_ascii_lowercase();
    let host = parsed.part(CURLUPART_HOST, 0)?.to_ascii_lowercase();
    let port = parsed
        .part(CURLUPART_PORT, CURLU_DEFAULT_PORT)?
        .parse()
        .ok()?;
    (!host.is_empty()).then_some(Origin { scheme, host, port })
}

/// libcurl's `CURLU`, opaque.
#[repr(C)]
struct CurlUrl {
    _private: [u8; 0],
}

// curl/urlapi.h of curl 8.23.0. `CURLUcode` and `CURLUPart` are C enums: `int`.
extern "C" {
    fn curl_url() -> *mut CurlUrl;
    fn curl_url_cleanup(handle: *mut CurlUrl);
    fn curl_url_set(handle: *mut CurlUrl, what: c_int, part: *const c_char, flags: c_uint)
        -> c_int;
    fn curl_url_get(
        handle: *const CurlUrl,
        what: c_int,
        part: *mut *mut c_char,
        flags: c_uint,
    ) -> c_int;
}

const CURLUE_OK: c_int = 0;
const CURLUPART_URL: c_int = 0;
const CURLUPART_SCHEME: c_int = 1;
const CURLUPART_HOST: c_int = 5;
const CURLUPART_PORT: c_int = 6;
/// Gives the scheme's default port when the URL has none.
const CURLU_DEFAULT_PORT: c_uint = 1 << 0;
/// Reads schemes libcurl wasn't built for too, such as `file`, so that their origin is refused by
/// Velocity's rules rather than lost as a parse error.
const CURLU_NON_SUPPORT_SCHEME: c_uint = 1 << 3;

/// A URL parsed by libcurl, freed when dropped.
struct Parsed(*mut CurlUrl);

impl Parsed {
    fn new(url: &str) -> Option<Self> {
        let url = CString::new(url).ok()?;
        // SAFETY: curl_url takes no arguments and returns a new handle or null.
        let handle = unsafe { curl_url() };
        if handle.is_null() {
            return None;
        }
        let parsed = Parsed(handle);
        // SAFETY: `parsed.0` is a live handle; `url` is a NUL-terminated string that libcurl
        // copies during the call.
        let code = unsafe {
            curl_url_set(
                parsed.0,
                CURLUPART_URL,
                url.as_ptr(),
                CURLU_NON_SUPPORT_SCHEME,
            )
        };
        (code == CURLUE_OK).then_some(parsed)
    }

    /// One part of the URL, or `None` when it has none or it isn't UTF-8.
    fn part(&self, what: c_int, flags: c_uint) -> Option<String> {
        let mut out: *mut c_char = ptr::null_mut();
        // SAFETY: `self.0` is a live handle and `out` a valid place for libcurl to store a
        // pointer to a string it allocates.
        let code = unsafe { curl_url_get(self.0, what, &mut out, flags) };
        if code != CURLUE_OK || out.is_null() {
            return None;
        }
        // SAFETY: libcurl returned a NUL-terminated string it allocated; it is copied here, then
        // freed with curl_free exactly once, as curl_url_get requires.
        let part = unsafe { CStr::from_ptr(out) }
            .to_str()
            .ok()
            .map(str::to_owned);
        unsafe { curl_sys::curl_free(out.cast()) };
        part
    }
}

impl Drop for Parsed {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the live handle from curl_url, cleaned up once, here.
        unsafe { curl_url_cleanup(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(scheme: &str, host: &str, port: u16) -> Option<Origin> {
        Some(Origin {
            scheme: scheme.to_owned(),
            host: host.to_owned(),
            port,
        })
    }

    #[test]
    fn an_origin_is_the_scheme_host_and_port() {
        assert_eq!(
            origin("https://example.com/a?b#c"),
            of("https", "example.com", 443)
        );
        assert_eq!(origin("http://example.com"), of("http", "example.com", 80));
        assert_eq!(
            origin("http://127.0.0.1:8080/x"),
            of("http", "127.0.0.1", 8080)
        );
        assert_eq!(
            origin("ftp://ftp.example.org/f"),
            of("ftp", "ftp.example.org", 21)
        );
    }

    #[test]
    fn an_origin_ignores_case_and_the_login() {
        assert_eq!(
            origin("HTTPS://User:Pw@EXAMPLE.Com:443/A"),
            of("https", "example.com", 443)
        );
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets() {
        assert_eq!(origin("http://[::1]:9000/"), of("http", "[::1]", 9000));
    }

    #[test]
    fn a_url_without_a_host_has_no_origin() {
        for url in [
            "file:///etc/passwd",
            "not a url",
            "",
            "https://",
            "/relative/path",
        ] {
            assert_eq!(origin(url), None, "{url:?}");
        }
    }

    #[test]
    fn an_ambiguous_url_gets_the_origin_libcurl_connects_to() {
        // To libcurl a backslash is no path separator, so `evil.example\` is a login and the host
        // is good.example: the origin is the host the request really goes to.
        assert_eq!(
            origin("http://evil.example\\@good.example/"),
            of("http", "good.example", 80)
        );
    }
}
