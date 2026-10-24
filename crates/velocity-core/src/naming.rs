//! File names from untrusted sources: a link's last path segment, a server's
//! `Content-Disposition`, a browser's suggestion (#40). Ported from V1's `validation.rs`; the
//! category, conflict and rename settings built on them are #31's.

/// The longest file name, in bytes, kept on every platform.
pub const MAX_FILE_NAME_BYTES: usize = 240;
/// The name used when nothing usable is left.
pub const FALLBACK_FILE_NAME: &str = "download";
/// What the engine appends to a file name while downloading it, so no download is ever named so.
pub const PART_FILE_SUFFIX: &str = ".vdmpart";

/// Names Windows reserves for devices, with or without an extension (`CON`, `con.txt`). Windows 11
/// also reserves `COM` and `LPT` with a superscript digit, and `CONIN$` and `CONOUT$`.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9", "LPT¹", "LPT²", "LPT³", "CONIN$", "CONOUT$",
];

/// Extensions of server-side scripts. A link ending in one names the program that serves the file,
/// not the file, so the server's name is used instead.
const SCRIPT_EXTENSIONS: &[&str] = &["php", "asp", "aspx", "jsp", "cgi"];

/// An extension longer than this isn't kept when a name is shortened: it is likely not one.
const MAX_KEPT_EXTENSION_BYTES: usize = 15;

/// Turns any string into a safe file name: one plain path component on every supported platform.
/// Separators, control characters and characters Windows forbids become `_`; leading dots and
/// trailing dots and spaces go; a reserved device name or a part file's name gets a `_`; a long
/// name is shortened, keeping its extension; nothing usable left gives [`FALLBACK_FILE_NAME`].
pub fn sanitize_file_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| if is_forbidden(c) { '_' } else { c })
        .collect();
    let mut out = mapped
        .trim()
        .trim_start_matches('.')
        .trim()
        .trim_end_matches(['.', ' '])
        .to_owned();
    if out.is_empty() {
        return FALLBACK_FILE_NAME.to_owned();
    }
    if is_reserved(&out) {
        out.insert(0, '_');
    }
    if out.to_ascii_lowercase().ends_with(PART_FILE_SUFFIX) {
        out.push('_');
    }
    shorten(&out, "")
}

/// The decoded, sanitised last path segment of `url`, or `None` when it has none.
pub fn file_name_from_url(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let path_start = rest.find(['/', '?', '#'])?;
    let path = &rest[path_start..];
    if !path.starts_with('/') {
        return None;
    }
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let segment = path.split('/').rfind(|s| !s.is_empty())?;
    let decoded = percent_decode(segment);
    (!decoded.trim().is_empty()).then(|| sanitize_file_name(&decoded))
}

/// The file name a `Content-Disposition` header value gives (RFC 6266), sanitised, or `None` when
/// it gives none. `filename*` (RFC 8187: UTF-8 or ISO-8859-1, percent-encoded) wins over
/// `filename`, wherever each comes; one it can't decode is ignored. Any path in the name is
/// dropped, keeping its last component (RFC 6266 §4.3), so `../../x.sh` gives `x.sh`.
pub fn file_name_from_content_disposition(value: &str) -> Option<String> {
    let (mut plain, mut encoded) = (None, None);
    // The disposition type comes first; only its parameters can name the file.
    let mut rest = value.split_once(';')?.1;
    loop {
        rest = rest.trim_start_matches([' ', '\t', ';']);
        let Some(eq) = rest.find(['=', ';']) else {
            break;
        };
        if rest[eq..].starts_with(';') {
            rest = &rest[eq..]; // a parameter without a value
            continue;
        }
        let name = rest[..eq].trim().to_ascii_lowercase();
        let after_eq = rest[eq + 1..].trim_start_matches([' ', '\t']);
        let (parameter, after) = match after_eq.strip_prefix('"') {
            Some(quoted) => match unquote(quoted) {
                Some(parsed) => parsed,
                // An unterminated quoted string: nothing after it is reliable.
                None => break,
            },
            None => {
                let end = after_eq.find(';').unwrap_or(after_eq.len());
                (after_eq[..end].trim().to_owned(), &after_eq[end..])
            }
        };
        match name.as_str() {
            "filename" if plain.is_none() => plain = Some(parameter),
            "filename*" if encoded.is_none() => encoded = Some(parameter),
            _ => {}
        }
        rest = after;
    }
    let raw = encoded.and_then(|e| decode_ext_value(&e)).or(plain)?;
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let usable = !last.trim().trim_matches('.').trim().is_empty();
    usable.then(|| sanitize_file_name(last))
}

/// The content of a quoted string whose opening quote is already gone, and what follows its closing
/// quote; a backslash escapes the next character (RFC 9110 §5.6.4). `None` without a closing quote.
fn unquote(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => out.push(chars.next()?.1),
            c => out.push(c),
        }
    }
    None
}

/// An RFC 8187 `charset'language'value`, decoded; `None` for another charset or bad UTF-8.
fn decode_ext_value(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let (charset, _language, encoded) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = percent_decode_bytes(encoded);
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.into_iter().map(char::from).collect())
    } else {
        None
    }
}

/// The lowercase extension of a file name: `zip` for `a.ZIP`.
pub fn extension_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then(|| ext.to_ascii_lowercase())
}

/// The name to save a link under before asking the server: the link's last path segment, but only
/// when it has an extension that isn't a server script's. `/files/12345` and `/download.php?id=3`
/// give `None`, so the server's name is used (`Content-Disposition`, then the final URL).
pub fn file_name_for_link(url: &str) -> Option<String> {
    let name = file_name_from_url(url)?;
    let ext = extension_of(&name)?;
    (!SCRIPT_EXTENSIONS.contains(&ext.as_str())).then_some(name)
}

/// `name` if it is free, or else the first free `stem (n).ext`, n from 1 to 9999, shortened to fit
/// [`MAX_FILE_NAME_BYTES`]. `None` when every one is taken.
pub fn free_file_name(name: &str, mut is_taken: impl FnMut(&str) -> bool) -> Option<String> {
    if !is_taken(name) {
        return Some(name.to_owned());
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    (1..=9999).find_map(|n| {
        let candidate = match ext {
            Some(ext) => shorten(&format!("{stem}.{ext}"), &format!(" ({n})")),
            None => shorten(stem, &format!(" ({n})")),
        };
        (!is_taken(&candidate)).then_some(candidate)
    })
}

fn is_forbidden(c: char) -> bool {
    c.is_control() || matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

/// Windows ignores the extension and trailing spaces: `con.txt` and `CON .txt` are `CON`.
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end();
    WINDOWS_RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
}

/// `name` with `tag` before its extension, cut to [`MAX_FILE_NAME_BYTES`] by shortening the stem
/// at a character boundary. An extension too long to be one isn't kept apart.
fn shorten(name: &str, tag: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= MAX_KEPT_EXTENSION_BYTES => {
            (stem, format!(".{ext}"))
        }
        _ => (name, String::new()),
    };
    let room = MAX_FILE_NAME_BYTES.saturating_sub(tag.len() + ext.len());
    let mut cut = stem.len().min(room);
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    let stem = if cut < stem.len() {
        stem[..cut].trim_end_matches(['.', ' '])
    } else {
        stem
    };
    format!("{stem}{tag}{ext}")
}

/// Decodes `%XX` escapes; bytes that aren't UTF-8 become U+FFFD.
pub(crate) fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(s)).into_owned()
}

/// Decodes `%XX` escapes; a `%` without two hexadecimal digits stays as it is.
fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing_leaves_one_plain_path_component() {
        assert_eq!(sanitize_file_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("a\\b:c*?\"<>|d"), "a_b_c______d");
        assert_eq!(sanitize_file_name("tab\there\nnew\u{0}"), "tab_here_new_");
        assert_eq!(sanitize_file_name(".hidden"), "hidden");
        assert_eq!(sanitize_file_name("trailing. . "), "trailing");
    }

    #[test]
    fn sanitizing_avoids_names_windows_reserves() {
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("con"), "_con");
        assert_eq!(sanitize_file_name("lpt1.tar.gz"), "_lpt1.tar.gz");
        assert_eq!(sanitize_file_name("CON .txt"), "_CON .txt");
        assert_eq!(sanitize_file_name("CONSOLE.txt"), "CONSOLE.txt");
    }

    #[test]
    fn sanitizing_never_gives_a_part_file_name() {
        assert_eq!(sanitize_file_name("a.zip.vdmpart"), "a.zip.vdmpart_");
        assert_eq!(sanitize_file_name("A.VDMPART"), "A.VDMPART_");
    }

    #[test]
    fn sanitizing_an_empty_name_gives_the_fallback() {
        for empty in ["", "   ", "...", ". ."] {
            assert_eq!(sanitize_file_name(empty), FALLBACK_FILE_NAME, "{empty:?}");
        }
    }

    #[test]
    fn sanitizing_shortens_a_long_name_and_keeps_its_extension() {
        let long = format!("{}.zip", "x".repeat(400));
        let s = sanitize_file_name(&long);
        assert_eq!(s.len(), MAX_FILE_NAME_BYTES);
        assert!(s.ends_with(".zip"));
        // Never cut inside a character.
        let s = sanitize_file_name(&format!("{}.zip", "é".repeat(200)));
        assert!(s.len() <= MAX_FILE_NAME_BYTES && s.ends_with(".zip"), "{s}");
    }

    #[test]
    fn a_url_names_its_last_decoded_path_segment() {
        let cases = [
            (
                "https://e.com/a/My%20File.iso?x=1#part",
                Some("My File.iso"),
            ),
            ("https://e.com/a/b%2Fc.zip", Some("b_c.zip")),
            (
                "https://e.com/%E2%9C%93-r%C3%A9sum%C3%A9.pdf",
                Some("✓-résumé.pdf"),
            ),
            ("https://u:p@e.com/dir/", Some("dir")),
            ("ftp://e.com/pub/file.ISO", Some("file.ISO")),
            ("https://e.com/", None),
            ("https://e.com", None),
            ("https://e.com?name=a.zip", None),
            ("not a url", None),
        ];
        for (url, want) in cases {
            assert_eq!(file_name_from_url(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn extensions_are_lowercase_and_need_a_stem() {
        assert_eq!(extension_of("a.ZIP").as_deref(), Some("zip"));
        assert_eq!(extension_of("a.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension_of(".hidden"), None);
        assert_eq!(extension_of("noext"), None);
        assert_eq!(extension_of("a."), None);
        assert_eq!(extension_of("a.b c"), None);
    }

    #[test]
    fn a_link_names_the_file_only_with_a_real_extension() {
        let cases = [
            (
                "https://e.com/a/My%20File.iso?x=1#part",
                Some("My File.iso"),
            ),
            ("https://e.com/CON.txt", Some("_CON.txt")),
            (
                "https://e.com/archive.tar.gz?token=abc",
                Some("archive.tar.gz"),
            ),
            // Extensionless endpoints and scripts: the server names the file.
            ("https://e.com/files/12345", None),
            ("https://e.com/download.php?id=3", None),
            ("https://e.com/Get.ASPX?f=1", None),
            ("https://e.com/run.cgi", None),
            ("https://e.com/.hidden", None),
            ("https://e.com/", None),
        ];
        for (url, want) in cases {
            assert_eq!(file_name_for_link(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn a_taken_name_gets_the_first_free_number() {
        let taken = ["a.zip", "a (1).zip"];
        let free = free_file_name("a.zip", |n| taken.contains(&n));
        assert_eq!(free.as_deref(), Some("a (2).zip"));
        assert_eq!(
            free_file_name("b.zip", |n| taken.contains(&n)).as_deref(),
            Some("b.zip")
        );
        assert_eq!(
            free_file_name("README", |n| n == "README").as_deref(),
            Some("README (1)")
        );
        assert_eq!(
            free_file_name("a.tar.gz", |n| n == "a.tar.gz").as_deref(),
            Some("a.tar (1).gz")
        );
    }

    #[test]
    fn a_numbered_name_stays_within_the_length_limit() {
        let long = format!("{}.zip", "x".repeat(MAX_FILE_NAME_BYTES - 4));
        let free = free_file_name(&long, |n| n == long).expect("a free name");
        assert!(free.len() <= MAX_FILE_NAME_BYTES, "{}", free.len());
        assert!(free.ends_with(" (1).zip"), "{free}");
    }

    #[test]
    fn no_free_name_when_every_number_is_taken() {
        assert_eq!(free_file_name("a.zip", |_| true), None);
    }

    fn disposition(value: &str) -> Option<String> {
        file_name_from_content_disposition(value)
    }

    #[test]
    fn a_disposition_names_its_plain_or_quoted_filename() {
        assert_eq!(
            disposition("attachment; filename=report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            disposition(r#"attachment; filename="Annual report.pdf""#).as_deref(),
            Some("Annual report.pdf")
        );
        // Any disposition type, parameter names in any case, spaces around `=` and `;`.
        assert_eq!(
            disposition("INLINE ; FileName = a.txt").as_deref(),
            Some("a.txt")
        );
        // A `;` inside quotes is part of the name; a backslash escapes the next character.
        assert_eq!(
            disposition(r#"attachment; filename="a;b \"c\".txt"; size=3"#).as_deref(),
            Some("a;b _c_.txt")
        );
        // A server that sends the name's UTF-8 bytes as they are (header lines arrive as UTF-8).
        assert_eq!(
            disposition("attachment; filename=\"résumé.pdf\"").as_deref(),
            Some("résumé.pdf")
        );
    }

    #[test]
    fn a_disposition_prefers_the_encoded_filename_star() {
        let both = r#"attachment; filename="fallback.pdf"; filename*=UTF-8''%E2%82%AC%20rates.pdf"#;
        assert_eq!(disposition(both).as_deref(), Some("€ rates.pdf"));
        // Whichever comes first.
        let star_first = r#"attachment; filename*=utf-8'en'%C3%A9t%C3%A9.txt; filename="ete.txt""#;
        assert_eq!(disposition(star_first).as_deref(), Some("été.txt"));
        // ISO-8859-1 is the other charset RFC 8187 names.
        assert_eq!(
            disposition("attachment; filename*=iso-8859-1''%E9t%E9.txt").as_deref(),
            Some("été.txt")
        );
    }

    #[test]
    fn a_filename_star_it_cannot_read_falls_back_to_filename() {
        for value in [
            r#"attachment; filename*=x-unknown''abc.txt; filename="plain.txt""#,
            r#"attachment; filename*=UTF-8''%FF%FE.txt; filename="plain.txt""#,
            r#"attachment; filename*=no-quotes.txt; filename="plain.txt""#,
        ] {
            assert_eq!(disposition(value).as_deref(), Some("plain.txt"), "{value}");
        }
    }

    #[test]
    fn a_disposition_name_loses_any_path_and_is_sanitised() {
        let cases = [
            (r#"attachment; filename="../../etc/passwd""#, Some("passwd")),
            (
                r#"attachment; filename="C:\\Windows\\evil.exe""#,
                Some("evil.exe"),
            ),
            // Unescaped, `\W` is a quoted pair for `W` (RFC 9110 §5.6.4): still one component.
            (
                r#"attachment; filename="C:\Windows\evil.exe""#,
                Some("C_Windowsevil.exe"),
            ),
            ("attachment; filename*=UTF-8''..%2F..%2Fx.sh", Some("x.sh")),
            (r#"attachment; filename="CON.txt""#, Some("_CON.txt")),
            (
                r#"attachment; filename="a.zip.vdmpart""#,
                Some("a.zip.vdmpart_"),
            ),
            (
                "attachment; filename*=UTF-8''new%0Aline%00.txt",
                Some("new_line_.txt"),
            ),
            (r#"attachment; filename=".bashrc""#, Some("bashrc")),
        ];
        for (value, want) in cases {
            assert_eq!(disposition(value).as_deref(), want, "{value}");
        }
    }

    #[test]
    fn a_disposition_without_a_usable_name_gives_none() {
        for value in [
            "attachment",
            "inline",
            "",
            "attachment; filename=",
            r#"attachment; filename="""#,
            r#"attachment; filename="   ""#,
            r#"attachment; filename="../""#,
            r#"attachment; filename="unterminated"#,
            "attachment; name=field",
        ] {
            assert_eq!(disposition(value), None, "{value:?}");
        }
    }
}
