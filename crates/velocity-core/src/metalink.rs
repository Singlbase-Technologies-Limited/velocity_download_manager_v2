//! Metalink documents (#54): Metalink 4 (RFC 5854) and Metalink 3 (metalinker.org 3.0), read into
//! the files they describe. A document is untrusted input (AGENTS.md):
//!
//! - **No entity can expand:** a document with a `<!DOCTYPE>` is refused, so it can declare no
//!   entity, internal or external. Only XML's five predefined entities and character references
//!   are read (quick-xml, PO decision on #54).
//! - **Bounded:** at most [`MAX_DOCUMENT_BYTES`], [`MAX_FILES`] files, [`MAX_URLS_PER_FILE`]
//!   sources per file and [`MAX_DEPTH`] levels of nesting, read as a stream.
//! - **Safe names:** a file's name may hold subfolders (`docs/manual.pdf`, kept as V1 did: PO
//!   decision on #54), each a safe name ([`check_file_name`]), at most [`MAX_FOLDERS`] deep. One
//!   unsafe name, or two files at one path in any case, refuses the whole document.
//! - **Sources:** only `http`, `https`, `ftp` and `ftps` URLs without a login, best first: Metalink 4's
//!   lower `priority`, Metalink 3's higher `preference`, then unranked ones in document order.
//!   Torrents (`metaurl`, a Metalink 3 `bittorrent` resource) are a non-goal of #54.
//! - **Hashes:** the strongest supported whole-file hash, and the strongest piece list, which must
//!   fit the file's size when it has one. Elements in other namespaces are ignored.

use crate::domain::{DownloadKind, ErrorCode};
use crate::fs_safety::{is_link, plain_dir};
use crate::naming::check_file_name;
use crate::request::{split_login, MAX_URL_BYTES};
use crate::transfer::{Checksum, HashAlgorithm, PieceHashes};
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::XmlVersion;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// The largest document read (as V1's torrent parser's limit).
pub const MAX_DOCUMENT_BYTES: usize = 16 << 20;
/// The most `<file>` elements a document may have.
pub const MAX_FILES: usize = 10_000;
/// The most `<url>` elements one file may have.
pub const MAX_URLS_PER_FILE: usize = 256;
/// The deepest elements may nest; Metalink itself needs 5.
pub const MAX_DEPTH: usize = 32;
/// The most subfolders a file's name may have.
pub const MAX_FOLDERS: usize = 32;

/// One file a Metalink document describes, with what V2 can use of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalinkFile {
    /// The subfolders the file is saved in, outermost first; empty for none.
    pub folders: Vec<String>,
    pub name: String,
    pub size: Option<u64>,
    pub checksum: Option<Checksum>,
    pub pieces: Option<PieceHashes>,
    /// The `http`, `https`, `ftp` and `ftps` sources, best first. Never empty.
    pub urls: Vec<String>,
}

/// What a Metalink document describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metalink {
    pub files: Vec<MetalinkFile>,
    /// Files without a source V2 can use, such as only a torrent; not in [`Metalink::files`].
    pub skipped: usize,
}

/// Why a document can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetalinkError {
    TooLarge,
    NotUtf8,
    Malformed(String),
    DocType,
    NotMetalink,
    UnsafeName(String),
    DuplicateName(String),
    BadHash(String),
    BadPieces(String),
    NoFiles,
    /// A Metalink file on disk that can't be read: why, as the app's code.
    Unreadable(ErrorCode),
}

impl MetalinkError {
    pub fn code(&self) -> ErrorCode {
        match self {
            MetalinkError::UnsafeName(_) | MetalinkError::DuplicateName(_) => {
                ErrorCode::InvalidFilename
            }
            MetalinkError::Unreadable(code) => *code,
            _ => ErrorCode::InvalidInput,
        }
    }
}

impl fmt::Display for MetalinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetalinkError::TooLarge => write!(
                f,
                "The Metalink file is too large: Velocity reads at most 16 MiB, {MAX_FILES} files, \
                 {MAX_URLS_PER_FILE} sources per file and {MAX_DEPTH} levels of nesting."
            ),
            MetalinkError::NotUtf8 => f.write_str("The Metalink file isn't UTF-8 text."),
            MetalinkError::Malformed(detail) => {
                write!(f, "The Metalink file isn't valid: {detail}.")
            }
            MetalinkError::DocType => f.write_str(
                "The Metalink file declares a document type, which Velocity refuses: it could \
                 define entities that expand without limit or read other files.",
            ),
            MetalinkError::NotMetalink => {
                f.write_str("The file isn't a Metalink 3 or Metalink 4 document.")
            }
            MetalinkError::UnsafeName(name) => {
                write!(
                    f,
                    "The Metalink file names a file Velocity won't create: {name:?}."
                )
            }
            MetalinkError::DuplicateName(name) => write!(
                f,
                "The Metalink file names {name:?} twice, or as both a file and a folder."
            ),
            MetalinkError::BadHash(name) => {
                write!(f, "The Metalink file's hash of {name:?} isn't valid.")
            }
            MetalinkError::BadPieces(name) => write!(
                f,
                "The Metalink file's piece hashes of {name:?} aren't valid or don't fit the file."
            ),
            MetalinkError::Unreadable(code) => write!(
                f,
                "The Metalink file can't be read: {}",
                code.user_message()
            ),
            MetalinkError::NoFiles => f.write_str(
                "The Metalink file has no file Velocity can download: it uses only http, https, \
                 ftp and ftps sources.",
            ),
        }
    }
}

impl std::error::Error for MetalinkError {}

const V4_NAMESPACE: &str = "urn:ietf:params:xml:ns:metalink";
const V3_NAMESPACE: &str = "http://www.metalinker.org/";
/// The schemes a source may have. SFTP needs a login and a host key the user confirms, which a
/// document can't supply; anything else isn't a direct download.
const SCHEMES: [&str; 4] = ["http", "https", "ftp", "ftps"];
/// The longest text an element may hold: a URL's limit, more than any size or digest needs.
const MAX_TEXT_BYTES: usize = MAX_URL_BYTES;
/// The rank of a source without a priority or preference: after every ranked one.
const UNRANKED: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    V3,
    V4,
}

/// The elements read; [`Tag::Ignored`] for the rest and everything inside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Root,
    Files,
    File,
    Verification,
    Resources,
    Size,
    Hash,
    Pieces,
    PieceHash,
    Url,
    Ignored,
}

impl Tag {
    fn holds_text(self) -> bool {
        matches!(self, Tag::Size | Tag::Hash | Tag::PieceHash | Tag::Url)
    }

    /// What a child element called `local`, in the document's namespace, is inside `self`.
    fn child(self, version: Version, local: &str) -> Tag {
        use Version::*;
        match (version, self, local) {
            (V4, Tag::Root, "file") | (V3, Tag::Files, "file") => Tag::File,
            (V3, Tag::Root, "files") => Tag::Files,
            (_, Tag::File, "size") => Tag::Size,
            (V4, Tag::File, "hash") | (V3, Tag::Verification, "hash") => Tag::Hash,
            (V4, Tag::File, "pieces") | (V3, Tag::Verification, "pieces") => Tag::Pieces,
            (V4, Tag::File, "url") | (V3, Tag::Resources, "url") => Tag::Url,
            (V3, Tag::File, "verification") => Tag::Verification,
            (V3, Tag::File, "resources") => Tag::Resources,
            (_, Tag::Pieces, "hash") => Tag::PieceHash,
            _ => Tag::Ignored,
        }
    }
}

/// A hash type's name in Metalink 4 (the IANA names, `sha-256`) or Metalink 3 (`sha256`).
fn algorithm(name: &str) -> Option<HashAlgorithm> {
    Some(match name.to_ascii_lowercase().as_str() {
        "md5" => HashAlgorithm::Md5,
        "sha-1" | "sha1" => HashAlgorithm::Sha1,
        "sha-224" | "sha224" => HashAlgorithm::Sha224,
        "sha-256" | "sha256" => HashAlgorithm::Sha256,
        "sha-384" | "sha384" => HashAlgorithm::Sha384,
        "sha-512" | "sha512" => HashAlgorithm::Sha512,
        _ => return None,
    })
}

/// Which of two supported algorithms to trust: the stronger.
fn strength(algorithm: HashAlgorithm) -> u8 {
    match algorithm {
        HashAlgorithm::Md5 => 0,
        HashAlgorithm::Sha1 => 1,
        HashAlgorithm::Sha224 => 2,
        HashAlgorithm::Sha256 => 3,
        HashAlgorithm::Sha384 => 4,
        HashAlgorithm::Sha512 => 5,
    }
}

/// `url` when V2 may fetch it: a supported scheme with something after it, no login (a document
/// mustn't send credentials), no spaces or control characters, within [`MAX_URL_BYTES`].
fn source(url: &str) -> Option<String> {
    let well_formed = !url.is_empty()
        && url.len() <= MAX_URL_BYTES
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    let (scheme, rest) = url.split_once("://").filter(|_| well_formed)?;
    let supported = SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme));
    (supported && !rest.is_empty() && split_login(url).1.is_none()).then(|| url.to_owned())
}

/// The protocol of a source [`MetalinkFile::urls`] holds: HTTP for `http` and `https`, FTP for
/// `ftp` and `ftps`.
pub fn source_kind(url: &str) -> Option<DownloadKind> {
    let (scheme, _) = url.split_once("://")?;
    let is = |s: &str| scheme.eq_ignore_ascii_case(s);
    if is("http") || is("https") {
        Some(DownloadKind::Http)
    } else if is("ftp") || is("ftps") {
        Some(DownloadKind::Ftp)
    } else {
        None
    }
}

/// A number made of ASCII digits only (`u64::from_str` would take a `+`).
fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

struct PiecesBuilder {
    algorithm: Option<HashAlgorithm>,
    length: Option<String>,
    hex: Vec<String>,
}

#[derive(Default)]
struct FileBuilder {
    name: Option<String>,
    size: Option<u64>,
    hashes: Vec<(HashAlgorithm, String)>,
    pieces: Vec<PiecesBuilder>,
    /// Usable sources with their rank, lower first.
    urls: Vec<(u32, String)>,
    url_elements: usize,
}

impl FileBuilder {
    /// The file, `None` when it has no usable source, or why it can't be used.
    fn build(self) -> Result<Option<MetalinkFile>, MetalinkError> {
        let name = self
            .name
            .ok_or_else(|| MetalinkError::Malformed("a file has no name".to_owned()))?;
        let mut parts: Vec<String> = name.split('/').map(str::to_owned).collect();
        if parts.len() > MAX_FOLDERS + 1 || parts.iter().any(|p| check_file_name(p).is_err()) {
            return Err(MetalinkError::UnsafeName(name));
        }
        let mut urls = self.urls;
        urls.sort_by_key(|(rank, _)| *rank);
        let mut seen = BTreeSet::new();
        let urls: Vec<String> = urls
            .into_iter()
            .map(|(_, url)| url)
            .filter(|url| seen.insert(url.clone()))
            .collect();
        if urls.is_empty() {
            return Ok(None);
        }
        let mut best: Option<&(HashAlgorithm, String)> = None;
        for hash in &self.hashes {
            if best.is_none_or(|b| strength(hash.0) > strength(b.0)) {
                best = Some(hash);
            }
        }
        let checksum = best
            .map(|(algorithm, hex)| {
                Checksum {
                    algorithm: *algorithm,
                    hex: hex.clone(),
                }
                .normalized()
                .map_err(|_| MetalinkError::BadHash(name.clone()))
            })
            .transpose()?;
        let mut best: Option<(HashAlgorithm, &PiecesBuilder)> = None;
        for pieces in &self.pieces {
            if let Some(algorithm) = pieces.algorithm {
                if best.is_none_or(|(b, _)| strength(algorithm) > strength(b)) {
                    best = Some((algorithm, pieces));
                }
            }
        }
        let pieces = best
            .map(|(algorithm, pieces)| {
                let bad = || MetalinkError::BadPieces(name.clone());
                let length = pieces.length.as_deref().and_then(number).ok_or_else(bad)?;
                let list = PieceHashes {
                    algorithm,
                    length,
                    hex: pieces.hex.clone(),
                }
                .normalized()
                .map_err(|_| bad())?;
                match self.size {
                    Some(size) if size.div_ceil(length) != list.hex.len() as u64 => Err(bad()),
                    _ => Ok(list),
                }
            })
            .transpose()?;
        let file_name = parts.pop().unwrap_or_default();
        Ok(Some(MetalinkFile {
            folders: parts,
            name: file_name,
            size: self.size,
            checksum,
            pieces,
            urls,
        }))
    }
}

fn malformed(e: impl fmt::Display) -> MetalinkError {
    MetalinkError::Malformed(e.to_string())
}

#[derive(Default)]
struct Parser {
    version: Option<Version>,
    stack: Vec<Tag>,
    /// The text of the open [`Tag::holds_text`] element.
    text: String,
    file: Option<FileBuilder>,
    /// The open `<hash>`'s algorithm, `None` for one V2 doesn't support.
    hash_type: Option<HashAlgorithm>,
    /// The open `<url>`'s rank, and whether its Metalink 3 `type` is a direct download.
    url_rank: u32,
    url_direct: bool,
    files: Vec<MetalinkFile>,
    file_elements: usize,
    skipped: usize,
}

impl Parser {
    fn open(
        &mut self,
        namespace: &ResolveResult,
        element: &BytesStart,
    ) -> Result<(), MetalinkError> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(MetalinkError::TooLarge);
        }
        let local = element.local_name();
        let local: &str = local.as_ref();
        let bound = match namespace {
            ResolveResult::Bound(Namespace(ns)) => Some(*ns),
            _ => None,
        };
        let tag = match (self.stack.last(), self.version) {
            (None, Some(_)) => {
                return Err(MetalinkError::Malformed(
                    "it has more than one root element".to_owned(),
                ))
            }
            (None, None) => {
                self.version = Some(match (bound, local) {
                    (Some(V4_NAMESPACE), "metalink") => Version::V4,
                    (Some(V3_NAMESPACE), "metalink") => Version::V3,
                    _ => return Err(MetalinkError::NotMetalink),
                });
                Tag::Root
            }
            (Some(&parent), Some(version)) => {
                let ours = bound
                    == Some(match version {
                        Version::V4 => V4_NAMESPACE,
                        Version::V3 => V3_NAMESPACE,
                    });
                if ours {
                    parent.child(version, local)
                } else {
                    Tag::Ignored
                }
            }
            // The root sets the version, so this can't happen; an error, never a panic, on input.
            (Some(_), None) => {
                return Err(MetalinkError::Malformed(
                    "it has no Metalink root".to_owned(),
                ))
            }
        };
        let attributes = attributes(element)?;
        let attribute = |name: &str| {
            attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let file = self.file.as_mut();
        match (tag, file) {
            (Tag::File, _) => {
                self.file_elements += 1;
                if self.file_elements > MAX_FILES {
                    return Err(MetalinkError::TooLarge);
                }
                self.file = Some(FileBuilder {
                    name: attribute("name").map(str::to_owned),
                    ..FileBuilder::default()
                });
            }
            (Tag::Hash, _) => self.hash_type = attribute("type").and_then(algorithm),
            (Tag::Pieces, Some(file)) => file.pieces.push(PiecesBuilder {
                algorithm: attribute("type").and_then(algorithm),
                length: attribute("length").map(str::to_owned),
                hex: Vec::new(),
            }),
            (Tag::PieceHash, Some(file)) => {
                // Metalink 3 numbers its pieces; they must come in order.
                if let (Some(index), Some(pieces)) = (attribute("piece"), file.pieces.last()) {
                    if number(index) != Some(pieces.hex.len() as u64) {
                        let name = file.name.clone().unwrap_or_default();
                        return Err(MetalinkError::BadPieces(name));
                    }
                }
            }
            (Tag::Url, Some(file)) => {
                file.url_elements += 1;
                if file.url_elements > MAX_URLS_PER_FILE {
                    return Err(MetalinkError::TooLarge);
                }
                let version = self.version.unwrap_or(Version::V4);
                self.url_rank = match version {
                    Version::V4 => attribute("priority")
                        .and_then(number)
                        .filter(|p| (1..=999_999).contains(p))
                        .map_or(UNRANKED, |p| p as u32),
                    Version::V3 => attribute("preference")
                        .and_then(number)
                        .filter(|p| *p <= 100)
                        .map_or(UNRANKED, |p| 100 - p as u32),
                };
                self.url_direct = version == Version::V4
                    || attribute("type")
                        .is_none_or(|t| SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(t)));
            }
            _ => {}
        }
        if tag.holds_text() {
            self.text.clear();
        }
        self.stack.push(tag);
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), MetalinkError> {
        if self.stack.last().is_some_and(|t| t.holds_text()) {
            if self.text.len() + text.len() > MAX_TEXT_BYTES {
                return Err(MetalinkError::TooLarge);
            }
            self.text.push_str(text);
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), MetalinkError> {
        let tag = self.stack.pop().ok_or_else(|| {
            MetalinkError::Malformed("an element closes that never opened".to_owned())
        })?;
        let text = if tag.holds_text() {
            std::mem::take(&mut self.text)
        } else {
            String::new()
        };
        let value = text.trim();
        if tag == Tag::File {
            let file = self.file.take().unwrap_or_default();
            match file.build()? {
                Some(file) => self.files.push(file),
                None => self.skipped += 1,
            }
            return Ok(());
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        match tag {
            Tag::Size => {
                file.size = Some(number(value).ok_or_else(|| {
                    MetalinkError::Malformed(format!("a file's size isn't a number: {value:?}"))
                })?);
            }
            Tag::Hash => {
                if let Some(algorithm) = self.hash_type {
                    file.hashes.push((algorithm, value.to_owned()));
                }
            }
            Tag::PieceHash => {
                if let Some(pieces) = file.pieces.last_mut() {
                    pieces.hex.push(value.to_owned());
                }
            }
            Tag::Url if self.url_direct => {
                if let Some(url) = source(value) {
                    file.urls.push((self.url_rank, url));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<Metalink, MetalinkError> {
        if self.version.is_none() {
            return Err(MetalinkError::Malformed(
                "it has no root element".to_owned(),
            ));
        }
        if !self.stack.is_empty() {
            return Err(MetalinkError::Malformed(
                "it ends inside an element".to_owned(),
            ));
        }
        // One path per file, whatever its case (Windows and macOS), and no file where another
        // file's folder is.
        let mut paths = BTreeSet::new();
        let mut folders = BTreeSet::new();
        for file in &self.files {
            let parts: Vec<String> = file
                .folders
                .iter()
                .chain([&file.name])
                .map(|p| p.to_lowercase())
                .collect();
            for end in 1..parts.len() {
                folders.insert(parts[..end].join("/"));
            }
            let path = parts.join("/");
            if !paths.insert(path.clone()) {
                return Err(MetalinkError::DuplicateName(path));
            }
        }
        if let Some(clash) = paths.intersection(&folders).next() {
            return Err(MetalinkError::DuplicateName(clash.clone()));
        }
        if self.files.is_empty() {
            return Err(MetalinkError::NoFiles);
        }
        Ok(Metalink {
            files: self.files,
            skipped: self.skipped,
        })
    }
}

/// An element's attributes without a prefix, their values normalised and unescaped. A duplicate
/// attribute, or a reference to an entity XML doesn't predefine, is malformed.
fn attributes(element: &BytesStart) -> Result<Vec<(String, String)>, MetalinkError> {
    let mut out = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(malformed)?;
        if attribute.key.prefix().is_some() {
            continue;
        }
        let key = attribute.key.local_name();
        let key: &str = key.as_ref();
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(malformed)?;
        out.push((key.to_owned(), value.into_owned()));
    }
    Ok(out)
}

/// The text a reference stands for: a character, or one of XML's five predefined entities. With
/// no document type there is nothing else it could be.
fn reference(reference: &BytesRef) -> Result<String, MetalinkError> {
    if reference.is_char_ref() {
        return match reference.resolve_char_ref() {
            Ok(Some(c)) => Ok(c.to_string()),
            _ => Err(MetalinkError::Malformed(
                "a character reference isn't valid".to_owned(),
            )),
        };
    }
    let name = reference.xml10_content();
    Ok(match name.as_ref() {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "apos" => "'",
        "quot" => "\"",
        other => {
            return Err(MetalinkError::Malformed(format!(
                "it refers to an entity it doesn't declare: {other:?}"
            )))
        }
    }
    .to_owned())
}

/// Reads a Metalink 4 or Metalink 3 document: UTF-8, at most [`MAX_DOCUMENT_BYTES`], with no
/// document type declaration (so no entity can expand). Files without a usable source are skipped
/// and counted; a file Velocity won't create refuses the whole document.
pub fn parse(document: &[u8]) -> Result<Metalink, MetalinkError> {
    if document.len() > MAX_DOCUMENT_BYTES {
        return Err(MetalinkError::TooLarge);
    }
    let text = std::str::from_utf8(document).map_err(|_| MetalinkError::NotUtf8)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = NsReader::from_str(text);
    let mut parser = Parser::default();
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(malformed)?;
        match event {
            Event::Decl(declaration) => {
                if let Some(encoding) = declaration.encoding() {
                    let encoding = encoding.map_err(malformed)?;
                    let utf8 = ["utf-8", "utf8", "us-ascii"]
                        .iter()
                        .any(|e| e.eq_ignore_ascii_case(&encoding));
                    if !utf8 {
                        return Err(MetalinkError::NotUtf8);
                    }
                }
            }
            Event::DocType(_) => return Err(MetalinkError::DocType),
            Event::Start(element) => parser.open(&namespace, &element)?,
            Event::Empty(element) => {
                parser.open(&namespace, &element)?;
                parser.close()?;
            }
            Event::End(_) => parser.close()?,
            Event::Text(text) => parser.text(&text.xml10_content())?,
            Event::CData(text) => parser.text(&text.xml10_content())?,
            Event::GeneralRef(r) => {
                let text = reference(&r)?;
                parser.text(&text)?;
            }
            Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    parser.finish()
}

/// Reads and parses a Metalink file the user chose (as V1's `validate_local_metadata_file`): an
/// absolute path to a plain file named `.metalink` or `.meta4`, never a link, at most
/// [`MAX_DOCUMENT_BYTES`].
pub fn read_file(path: &Path) -> Result<Metalink, MetalinkError> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if path.is_absolute() && !matches!(extension.as_deref(), Some("metalink" | "meta4")) {
        return Err(MetalinkError::Unreadable(ErrorCode::InvalidInput));
    }
    read_document(path)
}

/// Reads and parses the Metalink document at `path`, whatever its name, such as a download the
/// server called `application/metalink4+xml`: an absolute path to a plain file, never a link, at
/// most [`MAX_DOCUMENT_BYTES`].
pub fn read_document(path: &Path) -> Result<Metalink, MetalinkError> {
    use std::io::Read;
    let unreadable = |e: std::io::Error| {
        MetalinkError::Unreadable(match e.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::Io,
        })
    };
    if !path.is_absolute() {
        return Err(MetalinkError::Unreadable(ErrorCode::InvalidPath));
    }
    let meta = std::fs::symlink_metadata(path).map_err(unreadable)?;
    if is_link(&meta) || !meta.is_file() {
        return Err(MetalinkError::Unreadable(ErrorCode::InvalidPath));
    }
    if meta.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err(MetalinkError::TooLarge);
    }
    let mut document = Vec::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        // One byte over the limit tells a file that grew since the check.
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut document)
        .map_err(unreadable)?;
    parse(&document)
}

/// Creates a file's `folders` inside `directory`, one level at a time, and returns the innermost
/// (PO decision on #54: subfolders are kept). A folder already there is used; anything else at a
/// level is refused with `InvalidPath` and never followed: a file, or a link such as a symbolic
/// link or a junction (`fs_safety`). So the result is always inside `directory`, which is the
/// user's choice and must be an existing folder (`DestinationMissing` otherwise).
pub fn create_folders(directory: &Path, folders: &[String]) -> Result<PathBuf, ErrorCode> {
    if folders.iter().any(|f| check_file_name(f).is_err()) {
        return Err(ErrorCode::InvalidFilename);
    }
    if !std::fs::metadata(directory).is_ok_and(|m| m.is_dir()) {
        return Err(ErrorCode::DestinationMissing);
    }
    let mut path = directory.to_path_buf();
    for folder in folders {
        path.push(folder);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            // Made by an earlier file, or by another program: used only if it's a real folder.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                plain_dir(&path).map_err(|_| ErrorCode::InvalidPath)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(ErrorCode::PermissionDenied)
            }
            Err(_) => return Err(ErrorCode::Io),
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC5854: &str = include_str!("../tests/fixtures/metalink/rfc5854-example.meta4");
    const V4_FILES: &str = include_str!("../tests/fixtures/metalink/v4-files.meta4");
    const V3: &str = include_str!("../tests/fixtures/metalink/v3-example.metalink");

    const V4_NS: &str = "urn:ietf:params:xml:ns:metalink";

    fn hex(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    /// A Metalink 4 document with these `<file>` elements.
    fn v4(files: &str) -> String {
        format!("<?xml version=\"1.0\"?><metalink xmlns=\"{V4_NS}\">{files}</metalink>")
    }

    fn file(name: &str) -> String {
        format!("<file name=\"{name}\"><url>https://example.com/f</url></file>")
    }

    fn err(document: &str) -> MetalinkError {
        parse(document.as_bytes()).expect_err(document)
    }

    #[test]
    fn the_rfc_5854_example_parses() {
        let m = parse(RFC5854.as_bytes()).unwrap();
        assert_eq!(m.skipped, 0);
        assert_eq!(
            m.files,
            [MetalinkFile {
                folders: vec![],
                name: "example.ext".to_owned(),
                size: Some(14471447),
                checksum: Some(Checksum {
                    algorithm: HashAlgorithm::Sha256,
                    hex: "f0ad929cd259957e160ea442eb80986b5f01f0ad929cd259957e160ea442eb80"
                        .to_owned(),
                }),
                pieces: None,
                // Equal priorities keep the document's order; the torrent `metaurl` isn't a source.
                urls: vec![
                    "ftp://ftp.example.com/example.ext".to_owned(),
                    "http://example.com/example.ext".to_owned(),
                ],
            }]
        );
    }

    #[test]
    fn a_metalink_4_file_keeps_its_folders_best_hash_pieces_and_usable_sources_in_priority_order() {
        let m = parse(V4_FILES.as_bytes()).unwrap();
        let iso = &m.files[0];
        assert_eq!(iso.folders, ["disc"]);
        assert_eq!(iso.name, "image.iso");
        assert_eq!(iso.size, Some(2560));
        // The strongest of MD5, SHA-1 and SHA-256, in lowercase; Whirlpool isn't supported.
        assert_eq!(
            iso.checksum,
            Some(Checksum {
                algorithm: HashAlgorithm::Sha256,
                hex: hex('a', 64),
            })
        );
        assert_eq!(
            iso.pieces,
            Some(PieceHashes {
                algorithm: HashAlgorithm::Sha1,
                length: 1024,
                hex: vec![hex('2', 40), hex('3', 40), hex('4', 40)],
            })
        );
        // Lower priority first, unranked last, duplicates once. Dropped: SFTP (it needs a login
        // V2 can't take from a document), `file:`, a login in the URL, a torrent `metaurl` and an
        // element from another namespace.
        assert_eq!(
            iso.urls,
            [
                "https://mirror-a.example.com/image.iso?x=1&y=2",
                "https://mirror-b.example.com/image.iso",
                "FTP://ftp.example.com/image.iso",
                "http://unranked.example.com/image.iso",
            ]
        );
    }

    #[test]
    fn character_references_and_cdata_are_read() {
        let m = parse(V4_FILES.as_bytes()).unwrap();
        let readme = &m.files[1];
        assert_eq!(readme.name, "readme \u{2014} notes.txt");
        assert_eq!(readme.size, None);
        assert_eq!(readme.urls, ["https://example.com/readme.txt"]);
    }

    #[test]
    fn a_file_with_no_usable_source_is_skipped_and_counted() {
        let m = parse(V4_FILES.as_bytes()).unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.skipped, 1);
    }

    #[test]
    fn a_metalink_3_file_parses_with_its_preferences() {
        let m = parse(V3.as_bytes()).unwrap();
        assert_eq!(
            m.files,
            [MetalinkFile {
                folders: vec!["tools".to_owned()],
                name: "archive.tar.gz".to_owned(),
                size: Some(4096),
                checksum: Some(Checksum {
                    algorithm: HashAlgorithm::Sha1,
                    hex: hex('5', 40),
                }),
                pieces: Some(PieceHashes {
                    algorithm: HashAlgorithm::Sha1,
                    length: 2048,
                    hex: vec![hex('6', 40), hex('7', 40)],
                }),
                // Higher preference first, none last; the `bittorrent` resource isn't a source.
                urls: vec![
                    "ftp://ftp.example.com/archive.tar.gz".to_owned(),
                    "http://us.example.com/archive.tar.gz".to_owned(),
                    "https://unranked.example.com/archive.tar.gz".to_owned(),
                ],
            }]
        );
        assert_eq!(m.skipped, 0);
    }

    #[test]
    fn any_document_type_declaration_is_refused_so_no_entity_can_expand() {
        let laughs = format!(
            "<?xml version=\"1.0\"?><!DOCTYPE m [<!ENTITY a \"lol\"><!ENTITY b \"&a;&a;&a;&a;\">\
             <!ENTITY c \"&b;&b;&b;&b;\">]><metalink xmlns=\"{V4_NS}\"><file name=\"&c;\">\
             <url>https://example.com/f</url></file></metalink>"
        );
        let external = format!(
            "<?xml version=\"1.0\"?><!DOCTYPE m [<!ENTITY x SYSTEM \"file:///etc/passwd\">]>\
             <metalink xmlns=\"{V4_NS}\"><file name=\"f\"><url>https://example.com/&x;</url>\
             </file></metalink>"
        );
        let plain = format!(
            "<!DOCTYPE metalink><metalink xmlns=\"{V4_NS}\">{}</metalink>",
            file("f")
        );
        for document in [laughs, external, plain] {
            assert_eq!(err(&document), MetalinkError::DocType, "{document}");
        }
    }

    #[test]
    fn an_undeclared_entity_is_malformed() {
        let document = v4("<file name=\"f\"><url>https://example.com/&x;</url></file>");
        assert!(matches!(err(&document), MetalinkError::Malformed(_)));
    }

    #[test]
    fn unsafe_names_are_refused() {
        for name in [
            "../evil.exe",
            "a/../../evil",
            "/etc/passwd",
            "C:\\Windows\\evil.dll",
            "C:evil",
            "a\\b",
            "CON",
            "dir/NUL.txt",
            "a//b",
            "a/",
            ".",
            "tab&#9;name",
            "nul&#x1;byte",
            "trailing. ",
            // A part file's name (`naming::PART_FILE_SUFFIX`): another download's work in progress.
            "x.vdmpart",
        ] {
            let document = v4(&file(name));
            assert!(
                matches!(err(&document), MetalinkError::UnsafeName(_)),
                "{name}: {:?}",
                parse(document.as_bytes())
            );
        }
        let deep = vec!["d"; 40].join("/");
        assert!(matches!(
            err(&v4(&file(&deep))),
            MetalinkError::UnsafeName(_)
        ));
        assert!(matches!(
            err(&v4("<file><url>https://example.com/f</url></file>")),
            MetalinkError::Malformed(_)
        ));
    }

    #[test]
    fn two_files_at_one_path_are_refused_whatever_their_case() {
        for pair in [
            ["a.iso", "a.iso"],
            ["A.iso", "a.ISO"],
            ["d/f", "D/F"],
            ["a", "a/b"],
            ["x/y/z", "x/Y"],
        ] {
            let document = v4(&format!("{}{}", file(pair[0]), file(pair[1])));
            assert!(
                matches!(err(&document), MetalinkError::DuplicateName(_)),
                "{pair:?}"
            );
        }
        // A folder holding several files is no clash.
        let m = parse(v4(&format!("{}{}", file("d/a"), file("d/b"))).as_bytes()).unwrap();
        assert_eq!(m.files.len(), 2);
    }

    #[test]
    fn oversized_documents_are_refused() {
        let big = v4(&format!(
            "<!--{}-->{}",
            "x".repeat(MAX_DOCUMENT_BYTES),
            file("f")
        ));
        assert_eq!(err(&big), MetalinkError::TooLarge);
        let deep = v4(&format!(
            "<file name=\"f\"><url>https://example.com/f</url>{}{}</file>",
            "<x>".repeat(100),
            "</x>".repeat(100)
        ));
        assert_eq!(err(&deep), MetalinkError::TooLarge);
        let many: String = (0..=MAX_FILES).map(|i| file(&format!("f{i}"))).collect();
        assert_eq!(err(&v4(&many)), MetalinkError::TooLarge);
        let urls: String = (0..=MAX_URLS_PER_FILE)
            .map(|i| format!("<url>https://example.com/{i}</url>"))
            .collect();
        assert_eq!(
            err(&v4(&format!("<file name=\"f\">{urls}</file>"))),
            MetalinkError::TooLarge
        );
    }

    #[test]
    fn other_documents_are_not_metalink() {
        for document in [
            "<rss version=\"2.0\"><channel/></rss>".to_owned(),
            "<metalink xmlns=\"urn:example:other\"><file name=\"f\"/></metalink>".to_owned(),
            "<metalink><file name=\"f\"/></metalink>".to_owned(),
        ] {
            assert_eq!(err(&document), MetalinkError::NotMetalink, "{document}");
        }
    }

    #[test]
    fn malformed_xml_is_refused() {
        for document in [
            "".to_owned(),
            format!("<metalink xmlns=\"{V4_NS}\"><file name=\"f\">"),
            format!("<metalink xmlns=\"{V4_NS}\"><file name=\"f\"></url></metalink>"),
            format!("{}<metalink xmlns=\"{V4_NS}\"/>", v4(&file("f"))),
            v4("<file name=\"f\"><size>12x</size><url>https://example.com/f</url></file>"),
            v4("<file name=\"f\"><size>-1</size><url>https://example.com/f</url></file>"),
            v4("<file name=\"f\" name=\"g\"><url>https://example.com/f</url></file>"),
        ] {
            assert!(
                matches!(err(&document), MetalinkError::Malformed(_)),
                "{document}: {:?}",
                parse(document.as_bytes())
            );
        }
    }

    #[test]
    fn only_utf_8_is_read() {
        assert_eq!(
            parse(b"<metalink xmlns=\"urn:ietf:params:xml:ns:metalink\">\xff</metalink>"),
            Err(MetalinkError::NotUtf8)
        );
        let latin1 = format!(
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><metalink xmlns=\"{V4_NS}\">{}</metalink>",
            file("f")
        );
        assert_eq!(err(&latin1), MetalinkError::NotUtf8);
        // A byte-order mark is fine.
        let bom = format!("\u{feff}{}", v4(&file("f")));
        assert_eq!(parse(bom.as_bytes()).unwrap().files.len(), 1);
    }

    #[test]
    fn a_bad_hash_or_piece_list_is_refused() {
        let short = v4(&format!(
            "<file name=\"f\"><hash type=\"sha-256\">{}</hash><url>https://example.com/f</url></file>",
            hex('a', 63)
        ));
        assert!(matches!(err(&short), MetalinkError::BadHash(_)));
        let not_hex = v4(&format!(
            "<file name=\"f\"><hash type=\"md5\">{}</hash><url>https://example.com/f</url></file>",
            hex('g', 32)
        ));
        assert!(matches!(err(&not_hex), MetalinkError::BadHash(_)));
        // 3 pieces of 1 KiB can't describe 4 KiB.
        let pieces: String = (0..3)
            .map(|_| format!("<hash>{}</hash>", hex('1', 40)))
            .collect();
        let wrong_count = v4(&format!(
            "<file name=\"f\"><size>4096</size><pieces length=\"1024\" type=\"sha-1\">{pieces}</pieces>\
             <url>https://example.com/f</url></file>"
        ));
        assert!(matches!(err(&wrong_count), MetalinkError::BadPieces(_)));
        let tiny = v4(&format!(
            "<file name=\"f\"><pieces length=\"16\" type=\"sha-1\">{pieces}</pieces>\
             <url>https://example.com/f</url></file>"
        ));
        assert!(matches!(err(&tiny), MetalinkError::BadPieces(_)));
    }

    #[test]
    fn a_document_with_nothing_v2_can_download_is_refused() {
        assert_eq!(err(&v4("")), MetalinkError::NoFiles);
        let torrent_only = v4(
            "<file name=\"f\"><metaurl mediatype=\"torrent\">https://example.com/f.torrent</metaurl></file>",
        );
        assert_eq!(err(&torrent_only), MetalinkError::NoFiles);
    }

    /// A fresh folder for one test.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("v2-metalink-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn folders_are_created_inside_the_chosen_folder_and_reused() {
        let base = scratch("create");
        let inner = create_folders(&base, &names(&["disc", "extras"])).unwrap();
        assert_eq!(inner, base.join("disc").join("extras"));
        assert!(inner.is_dir());
        // Again, and with a sibling: what is there is used.
        assert_eq!(
            create_folders(&base, &names(&["disc", "extras"])),
            Ok(inner)
        );
        assert!(create_folders(&base, &names(&["disc", "more"]))
            .unwrap()
            .is_dir());
        // No folders: the chosen folder itself.
        assert_eq!(create_folders(&base, &[]), Ok(base.clone()));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_file_where_a_folder_should_be_is_refused() {
        let base = scratch("file");
        std::fs::write(base.join("disc"), b"x").unwrap();
        assert_eq!(
            create_folders(&base, &names(&["disc", "inner"])),
            Err(ErrorCode::InvalidPath)
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_link_where_a_folder_should_be_is_never_followed() {
        let base = scratch("link");
        let outside = scratch("link-outside");
        let link = base.join("disc");
        crate::fs_safety::test_links::folder_link(&outside, &link);
        assert_eq!(
            create_folders(&base, &names(&["disc", "inner"])),
            Err(ErrorCode::InvalidPath)
        );
        assert!(!outside.join("inner").exists());
        assert_eq!(
            create_folders(&base, &names(&["disc"])),
            Err(ErrorCode::InvalidPath)
        );
        #[cfg(windows)]
        std::fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&link).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn the_chosen_folder_must_exist_and_names_must_be_safe() {
        let base = scratch("missing");
        assert_eq!(
            create_folders(&base.join("absent"), &names(&["a"])),
            Err(ErrorCode::DestinationMissing)
        );
        for bad in ["..", "a/b", "", "CON"] {
            assert_eq!(
                create_folders(&base, &names(&[bad])),
                Err(ErrorCode::InvalidFilename),
                "{bad}"
            );
        }
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_metalink_file_on_disk_is_read() {
        let dir = scratch("read");
        let path = dir.join("files.META4");
        std::fs::write(&path, RFC5854).unwrap();
        assert_eq!(read_file(&path), parse(RFC5854.as_bytes()));
        let v3 = dir.join("old.metalink");
        std::fs::write(&v3, V3).unwrap();
        assert_eq!(read_file(&v3).unwrap().files.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_plain_metalink_file_within_the_limit_is_read() {
        let dir = scratch("read-refused");
        let unreadable = |path: &Path| read_file(path).unwrap_err();
        assert_eq!(
            unreadable(Path::new("relative.meta4")),
            MetalinkError::Unreadable(ErrorCode::InvalidPath)
        );
        let xml = dir.join("files.xml");
        std::fs::write(&xml, RFC5854).unwrap();
        assert_eq!(
            unreadable(&xml),
            MetalinkError::Unreadable(ErrorCode::InvalidInput)
        );
        assert_eq!(
            unreadable(&dir.join("absent.meta4")),
            MetalinkError::Unreadable(ErrorCode::NotFound)
        );
        let folder = dir.join("folder.meta4");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(
            unreadable(&folder),
            MetalinkError::Unreadable(ErrorCode::InvalidPath)
        );
        let big = dir.join("big.meta4");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_DOCUMENT_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(unreadable(&big), MetalinkError::TooLarge);
        #[cfg(unix)]
        {
            let real = dir.join("real.meta4");
            std::fs::write(&real, RFC5854).unwrap();
            let link = dir.join("link.meta4");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(
                unreadable(&link),
                MetalinkError::Unreadable(ErrorCode::InvalidPath)
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn errors_map_to_the_app_codes() {
        assert_eq!(
            MetalinkError::UnsafeName("x".into()).code(),
            ErrorCode::InvalidFilename
        );
        assert_eq!(
            MetalinkError::DuplicateName("x".into()).code(),
            ErrorCode::InvalidFilename
        );
        assert_eq!(MetalinkError::DocType.code(), ErrorCode::InvalidInput);
    }
}
