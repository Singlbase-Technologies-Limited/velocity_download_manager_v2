//! Starting libcurl (#35): the only way to get a [`Libcurl`] handle, so the engine can't run on a
//! libcurl that failed the capability check.

use std::ffi::{c_char, CStr};
use std::fmt;
use std::sync::OnceLock;

use crate::capabilities::{check, IncompatibleCurl, LinkedCurl};

/// `curl_version_info_data` has `feature_names` from this age on (libcurl 7.87.0).
const AGE_WITH_FEATURE_NAMES: curl_sys::CURLversion = curl_sys::CURLVERSION_ELEVENTH;

/// A C string owned by libcurl, or empty for a null pointer.
///
/// # Safety
/// `ptr` is null or points to a NUL-terminated string that stays valid while it's read.
unsafe fn text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        // SAFETY: non-null, and NUL-terminated per the caller's guarantee.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

/// A NULL-terminated array of C strings owned by libcurl.
///
/// # Safety
/// `list` is null or points to an array of valid C strings ending in a null pointer.
unsafe fn text_list(list: *const *const c_char) -> Vec<String> {
    let mut out = Vec::new();
    if list.is_null() {
        return out;
    }
    let mut i = 0;
    loop {
        // SAFETY: the array ends with a null pointer, and `i` never passes it.
        let item = unsafe { *list.add(i) };
        if item.is_null() {
            return out;
        }
        // SAFETY: each entry before the terminator is a valid C string.
        out.push(unsafe { text(item) });
        i += 1;
    }
}

/// What `curl_version_info(CURLVERSION_NOW)` reports about the linked libcurl.
fn read_linked() -> LinkedCurl {
    // SAFETY: after curl_global_init, curl_version_info returns a pointer to static data that
    // libcurl owns and never frees; the strings and arrays it points to are static too.
    unsafe {
        let info = curl_sys::curl_version_info(curl_sys::CURLVERSION_NOW);
        if info.is_null() {
            return LinkedCurl {
                version: String::new(),
                tls: String::new(),
                protocols: Vec::new(),
                features: Vec::new(),
            };
        }
        let info = &*info;
        LinkedCurl {
            version: text(info.version),
            tls: text(info.ssl_version),
            protocols: text_list(info.protocols),
            // An older struct has no feature names; an empty list then fails the check.
            features: if info.age >= AGE_WITH_FEATURE_NAMES {
                text_list(info.feature_names)
            } else {
                Vec::new()
            },
        }
    }
}

/// Proof that libcurl is initialised and passed the capability check.
#[derive(Debug)]
pub struct Libcurl {
    linked: LinkedCurl,
}

/// Why libcurl couldn't start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    /// `curl_global_init` returned this `CURLcode`.
    GlobalInitFailed { code: i32 },
    /// The linked libcurl failed the capability check.
    Incompatible(IncompatibleCurl),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GlobalInitFailed { code } => write!(
                f,
                "the transfer engine can't start: libcurl failed to initialise (CURLcode {code})"
            ),
            Self::Incompatible(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for StartError {}

impl Libcurl {
    /// Initialises libcurl once per process and checks what is linked. Every call repeats the
    /// check, so no caller can get a handle to a libcurl that failed it.
    pub fn start() -> Result<Self, StartError> {
        Self::start_with(read_linked)
    }

    /// `start`, with what libcurl reports supplied by `read` (the real `curl_version_info`, or a
    /// stand-in in tests).
    fn start_with(read: impl FnOnce() -> LinkedCurl) -> Result<Self, StartError> {
        static INIT: OnceLock<i32> = OnceLock::new();
        // SAFETY: curl_global_init runs once per process (OnceLock), before any other libcurl call
        // made through this crate. libcurl is never cleaned up, so the initialisation stays valid
        // for the process lifetime.
        let code = *INIT.get_or_init(|| unsafe {
            curl_sys::curl_global_init(curl_sys::CURL_GLOBAL_DEFAULT)
        } as i32);
        if code != 0 {
            return Err(StartError::GlobalInitFailed { code });
        }
        let linked = read();
        check(&linked).map_err(StartError::Incompatible)?;
        Ok(Self { linked })
    }

    /// What the linked libcurl reports.
    pub fn linked(&self) -> &LinkedCurl {
        &self.linked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::Problem;

    #[test]
    fn refuses_to_start_on_a_libcurl_below_the_floor() {
        let old = || LinkedCurl {
            version: "8.22.0".into(),
            ..read_linked()
        };
        let err = Libcurl::start_with(old).expect_err("8.22.0 must not start");
        match &err {
            StartError::Incompatible(e) => assert_eq!(
                e.problems,
                [Problem::BelowFloor {
                    linked: "8.22.0".into()
                }]
            ),
            other => panic!("expected Incompatible, got {other:?}"),
        }
        assert!(err.to_string().starts_with("the transfer engine can't start: libcurl 8.22.0 is linked, but V2 needs 8.23.0 or newer"), "{err}");
    }
}
