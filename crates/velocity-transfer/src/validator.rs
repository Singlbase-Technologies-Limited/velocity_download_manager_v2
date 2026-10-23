//! Which validator proves that the file on the server is still the one a download started (#45,
//! report section 5.3, RFC 9110 §8.8 and §13.1.5). Only a strong validator goes in `If-Range`.

use std::time::Duration;

use velocity_curl::ResponseHead;

/// How much later than `Last-Modified` the answer's `Date` must be for the date to be strong. RFC
/// 9110 §8.8.2.2 sets 1 s for a client; the report leaves a larger margin as a hypothesis to test.
pub const STRONG_DATE_MARGIN: Duration = Duration::from_secs(1);

/// A strong validator, as the server sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Validator {
    /// A strong entity tag, quotes included: `"v1"`.
    ETag(String),
    /// A `Last-Modified` date that the answer's own `Date` shows to be strong.
    LastModified(String),
}

/// The strong validator an answer gives, if any: a strong ETag; otherwise, when there is no entity
/// tag at all, a `Last-Modified` at least [`STRONG_DATE_MARGIN`] before the answer's `Date`;
/// otherwise none (ADR-0010 applies). A weak ETag rules out both: RFC 9110 §13.1.5 allows a date in
/// `If-Range` only when the client has no entity tag.
pub fn choose(head: &ResponseHead) -> Option<Validator> {
    match head.header("etag").and_then(EntityTag::parse) {
        Some(EntityTag { weak: false, tag }) => return Some(Validator::ETag(tag.to_owned())),
        Some(EntityTag { weak: true, .. }) => return None,
        None => {}
    }
    let modified = head.header("last-modified")?.trim();
    let at = httpdate::parse_http_date(modified).ok()?;
    let date = httpdate::parse_http_date(head.header("date")?.trim()).ok()?;
    let strong = date
        .duration_since(at)
        .is_ok_and(|d| d >= STRONG_DATE_MARGIN);
    strong.then(|| Validator::LastModified(modified.to_owned()))
}

/// What the first answer said about the file, which every later answer of the download must agree
/// with (report section 5.3). Validators belong to the file, not to the URL, so an answer that a
/// redirect brought from elsewhere is held to the same.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Representation {
    /// The entity tag, strong or weak, as sent.
    pub etag: Option<String>,
    /// The strong validator, sent as `If-Range`.
    pub validator: Option<Validator>,
}

impl Representation {
    /// What `head`, the first answer, says.
    pub fn of(head: &ResponseHead) -> Self {
        Representation {
            etag: head
                .header("etag")
                .and_then(EntityTag::parse)
                .map(|t| t.as_sent()),
            validator: choose(head),
        }
    }

    /// What a checkpoint recorded: its ETag, and its `Last-Modified`, which a checkpoint holds only
    /// when it was the strong validator.
    pub fn recorded(etag: Option<&str>, last_modified: Option<&str>) -> Self {
        let tag = etag.and_then(EntityTag::parse);
        let validator = match &tag {
            Some(EntityTag { weak: false, tag }) => Some(Validator::ETag((*tag).to_owned())),
            Some(EntityTag { weak: true, .. }) => None,
            None => last_modified.map(|m| Validator::LastModified(m.to_owned())),
        };
        Representation {
            etag: tag.map(|t| t.as_sent()),
            validator,
        }
    }

    /// The `If-Range` value: the strong validator, if there is one.
    pub fn if_range(&self) -> Option<String> {
        self.validator.as_ref().map(|v| match v {
            Validator::ETag(tag) => tag.clone(),
            Validator::LastModified(date) => date.clone(),
        })
    }

    /// The `Last-Modified` date a checkpoint keeps: only a strong one.
    pub fn last_modified(&self) -> Option<&str> {
        match &self.validator {
            Some(Validator::LastModified(date)) => Some(date),
            _ => None,
        }
    }

    /// Whether `head`, a later answer, is about the same file: `Err` says what changed.
    pub fn check(&self, head: &ResponseHead) -> Result<(), String> {
        if let Some(etag) = &self.etag {
            // Compared as sent, so a weak tag never matches a strong one (RFC 9110 §8.8.3.2).
            match head.header("etag").map(str::trim) {
                Some(now) if now == etag => {}
                Some(now) => return Err(format!("the file's ETag changed from {etag} to {now}")),
                // Report section 5.3: conservative, as the server can no longer show it's the same.
                None => return Err(format!("the server stopped sending the file's ETag {etag}")),
            }
        }
        if let Some(date) = self.last_modified() {
            match head.header("last-modified").map(str::trim) {
                Some(now) if now == date => {}
                Some(now) => return Err(format!("the file's date changed from {date} to {now}")),
                None => return Err(format!("the server stopped sending the file's date {date}")),
            }
        }
        Ok(())
    }
}

/// An `ETag` value (RFC 9110 §8.8.3): `"opaque"` or `W/"opaque"`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EntityTag<'a> {
    pub weak: bool,
    /// The quoted part, quotes included.
    pub tag: &'a str,
}

impl<'a> EntityTag<'a> {
    /// The entity tag in `value`, or `None` if it isn't one.
    pub fn parse(value: &'a str) -> Option<Self> {
        let value = value.trim();
        let (weak, tag) = match value.strip_prefix("W/") {
            Some(rest) => (true, rest),
            None => (false, value),
        };
        let inner = tag.strip_prefix('"')?.strip_suffix('"')?;
        // etagc: %x21 / %x23-7E / obs-text, so no quote, space or control character inside.
        let valid = inner
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x7E).contains(&b) || b >= 0x80);
        valid.then_some(EntityTag { weak, tag })
    }

    /// The tag as a server sends it, `W/` included when weak.
    fn as_sent(&self) -> String {
        if self.weak {
            format!("W/{}", self.tag)
        } else {
            self.tag.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(headers: &[(&str, &str)]) -> ResponseHead {
        ResponseHead {
            status: 206,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            http2: false,
        }
    }

    const MODIFIED: &str = "Sun, 06 Nov 1994 08:49:37 GMT";

    #[test]
    fn a_strong_etag_is_chosen_over_last_modified() {
        let h = head(&[
            ("ETag", "\"v1\""),
            ("Last-Modified", MODIFIED),
            ("Date", "Sun, 06 Nov 1994 09:00:00 GMT"),
        ]);
        assert_eq!(choose(&h), Some(Validator::ETag("\"v1\"".to_owned())));
    }

    #[test]
    fn last_modified_is_chosen_only_when_date_is_at_least_a_second_later() {
        let with_date = |date: &str| choose(&head(&[("Last-Modified", MODIFIED), ("Date", date)]));
        assert_eq!(
            with_date("Sun, 06 Nov 1994 08:49:38 GMT"),
            Some(Validator::LastModified(MODIFIED.to_owned()))
        );
        // RFC 9110 §8.8.2.2: in the same second, the file may have changed again unseen.
        assert_eq!(with_date(MODIFIED), None);
        assert_eq!(with_date("Sun, 06 Nov 1994 08:49:00 GMT"), None);
        assert_eq!(with_date("not a date"), None);
        assert_eq!(choose(&head(&[("Last-Modified", MODIFIED)])), None);
        let unparsable = head(&[
            ("Last-Modified", "yesterday"),
            ("Date", "Sun, 06 Nov 1994 09:00:00 GMT"),
        ]);
        assert_eq!(choose(&unparsable), None);
    }

    #[test]
    fn a_weak_etag_rules_out_both_validators() {
        // RFC 9110 §13.1.5: no weak tag in If-Range, and no date while an entity tag exists.
        let h = head(&[
            ("ETag", "W/\"v1\""),
            ("Last-Modified", MODIFIED),
            ("Date", "Sun, 06 Nov 1994 09:00:00 GMT"),
        ]);
        assert_eq!(choose(&h), None);
    }

    #[test]
    fn a_malformed_etag_is_not_an_entity_tag() {
        // An ETag must be quoted (RFC 9110 §8.8.3); anything else is ignored, so Last-Modified
        // may still serve.
        for bad in ["v1", "\"v1", "\"v\"1\"", "W/v1", ""] {
            let h = head(&[
                ("ETag", bad),
                ("Last-Modified", MODIFIED),
                ("Date", "Sun, 06 Nov 1994 09:00:00 GMT"),
            ]);
            assert_eq!(
                choose(&h),
                Some(Validator::LastModified(MODIFIED.to_owned())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn no_validator_without_either_header() {
        assert_eq!(choose(&head(&[])), None);
    }

    const LATER: &str = "Sun, 06 Nov 1994 09:00:00 GMT";

    #[test]
    fn a_later_answer_with_another_etag_is_another_file() {
        let first = Representation::of(&head(&[("ETag", "\"v1\"")]));
        assert_eq!(first.if_range().as_deref(), Some("\"v1\""));
        assert_eq!(first.check(&head(&[("ETag", "\"v1\"")])), Ok(()));
        assert_eq!(first.check(&head(&[("ETag", " \"v1\" ")])), Ok(()));
        for other in ["\"v2\"", "W/\"v1\"", "garbage"] {
            let e = first.check(&head(&[("ETag", other)])).expect_err(other);
            assert!(e.contains("ETag"), "{other}: {e}");
        }
    }

    #[test]
    fn a_dropped_etag_counts_as_another_file() {
        // Report section 5.3: conservative, as the server can no longer show it's the same.
        let first = Representation::of(&head(&[("ETag", "\"v1\"")]));
        let e = first.check(&head(&[])).expect_err("dropped");
        assert!(e.contains("ETag"), "{e}");
        // A weak tag can't go in If-Range, but a change or a drop still shows another file.
        let weak = Representation::of(&head(&[("ETag", "W/\"v1\"")]));
        assert_eq!(weak.if_range(), None);
        assert_eq!(weak.check(&head(&[("ETag", "W/\"v1\"")])), Ok(()));
        assert!(weak.check(&head(&[("ETag", "W/\"v2\"")])).is_err());
        assert!(weak.check(&head(&[])).is_err());
    }

    #[test]
    fn a_later_answer_with_another_strong_date_is_another_file() {
        let first = Representation::of(&head(&[("Last-Modified", MODIFIED), ("Date", LATER)]));
        assert_eq!(first.if_range().as_deref(), Some(MODIFIED));
        assert_eq!(first.last_modified(), Some(MODIFIED));
        assert_eq!(first.check(&head(&[("Last-Modified", MODIFIED)])), Ok(()));
        let other = "Mon, 07 Nov 1994 08:49:37 GMT";
        assert!(first.check(&head(&[("Last-Modified", other)])).is_err());
        assert!(first.check(&head(&[])).is_err());
    }

    #[test]
    fn without_a_validator_any_later_answer_agrees() {
        // ADR-0010: only the total is left to compare, which the segment checks do.
        let weak_date = Representation::of(&head(&[("Last-Modified", MODIFIED)]));
        assert_eq!(weak_date, Representation::default());
        assert_eq!(weak_date.last_modified(), None);
        let later = head(&[("ETag", "\"new\""), ("Last-Modified", LATER)]);
        assert_eq!(weak_date.check(&later), Ok(()));
    }

    #[test]
    fn a_checkpoint_restores_what_the_first_answer_said() {
        let strong = Representation::of(&head(&[("ETag", "\"v1\"")]));
        assert_eq!(Representation::recorded(Some("\"v1\""), None), strong);
        let weak = Representation::of(&head(&[("ETag", "W/\"v1\"")]));
        assert_eq!(Representation::recorded(Some("W/\"v1\""), None), weak);
        let dated = Representation::of(&head(&[("Last-Modified", MODIFIED), ("Date", LATER)]));
        assert_eq!(Representation::recorded(None, Some(MODIFIED)), dated);
        assert_eq!(
            Representation::recorded(None, None),
            Representation::default()
        );
    }
}
