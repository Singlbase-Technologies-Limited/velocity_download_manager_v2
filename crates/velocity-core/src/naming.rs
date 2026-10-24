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
fn percent_decode(s: &str) -> String {
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
    String::from_utf8_lossy(&out).into_owned()
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
}
