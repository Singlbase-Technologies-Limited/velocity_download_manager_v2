//! Secrets in memory (#70): a password, an SSH key's passphrase, a custom header's value. A
//! [`Secret`] is overwritten when it is dropped (`zeroize`, PO decision on #70), `Debug` never
//! shows it, and it is never written to the database, a checkpoint, a log or a process's
//! arguments (docs/SECURITY.md#secrets-70). It crosses a boundary as its plain string only where
//! it must: the engine's pipe and the window's commands, both in memory.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

/// A secret string, wiped from memory when dropped.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(Zeroizing::new(value.into()))
    }

    /// The secret itself, for the one place that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Secret::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Secret::new(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// As its plain string: the contract's JSON doesn't change.
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Secret::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_shows_it() {
        let s = Secret::new("hunter2-70");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{:?}", Some(s.clone())), "Some(<redacted>)");
        assert_eq!(s.expose(), "hunter2-70");
    }

    #[test]
    fn it_crosses_the_contract_as_its_plain_string() {
        let s = Secret::new("p@ss");
        assert_eq!(serde_json::to_string(&s).unwrap(), r#""p@ss""#);
        let back: Secret = serde_json::from_str(r#""p@ss""#).unwrap();
        assert_eq!(back, s);
        assert!(serde_json::from_str::<Secret>("1").is_err());
    }

    #[test]
    fn it_is_wiped_when_dropped() {
        // zeroize overwrites the String's buffer in place before freeing it; reading freed memory
        // can't be tested soundly, so this checks the type that does the wiping is the one held.
        fn wipes<T: zeroize::ZeroizeOnDrop>() {}
        wipes::<Zeroizing<String>>();
        let s = Secret::new("x");
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
    }
}
