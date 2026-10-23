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
}
