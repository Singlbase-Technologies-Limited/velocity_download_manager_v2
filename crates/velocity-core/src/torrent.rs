//! Parsing of untrusted BitTorrent metadata (`.torrent` files) and magnet links (#62).
//!
//! libtorrent does the BitTorrent work, in the torrent helper (ADR-0002, `crates/velocity-lt`).
//! The core parses metadata itself so that, before anything reaches libtorrent, it can:
//! * compute the info hashes (v1 SHA-1, v2 SHA-256) for duplicate detection,
//! * verify BitTorrent v2 piece layers against their Merkle roots, and check that a hybrid
//!   torrent's v1 and v2 file lists agree,
//! * show the file list for file selection,
//! * refuse metadata whose names aren't safe file names (`naming::check_file_name_in_folder`),
//!   so nothing can escape the download folder and the names on disk are the torrent's,
//! * know exactly which files a torrent owns when the user deletes it.
//!
//! The parser is bounded (input size, nesting depth, entry count, file count, path length) and
//! never panics on malformed input.

use std::fmt;

use crate::domain::ErrorCode;

/// Largest `.torrent` file accepted.
pub const MAX_TORRENT_BYTES: usize = 16 * 1024 * 1024;
/// Largest number of files accepted in one torrent.
pub const MAX_FILES: usize = 100_000;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 2_000_000;
const MAX_PATH_COMPONENTS: usize = 64;
/// The most trackers a torrent, a magnet link or the settings may name.
pub const MAX_TRACKERS: usize = 64;
const MAX_TRACKER_LEN: usize = 2048;

/// Which BitTorrent metadata a torrent carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TorrentVersion {
    /// BitTorrent v1 (SHA-1 pieces) only.
    V1,
    /// BitTorrent v2 (BEP 52, SHA-256 Merkle trees) only.
    V2,
    /// Both, describing the same files (BEP 52 hybrid).
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    /// libtorrent's file index plus one. Padding files have indexes too.
    pub index: u32,
    /// Path components relative to the download folder. For a multi-file torrent the first
    /// component is the torrent's name (libtorrent creates `folder/name/...`).
    pub path: Vec<String>,
    pub length: u64,
    /// A BEP 47 padding file: alignment only, never real data, never offered for selection.
    pub pad: bool,
}

impl TorrentFile {
    pub fn relative_path(&self) -> std::path::PathBuf {
        self.path.iter().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentMeta {
    /// Lowercase hexadecimal SHA-1 of the bencoded `info` dictionary (BitTorrent v1). `None` for
    /// a v2-only torrent: no v1 hash is ever made up for one.
    pub info_hash: Option<String>,
    /// Lowercase hexadecimal SHA-256 of the `info` dictionary (BitTorrent v2).
    pub info_hash_v2: Option<String>,
    pub version: TorrentVersion,
    pub name: String,
    /// Every file in libtorrent's order, padding files included.
    pub files: Vec<TorrentFile>,
    /// Bytes of real data (padding excluded).
    pub total_length: u64,
    pub piece_length: u64,
    pub num_pieces: u64,
    pub private: bool,
    pub multi_file: bool,
    pub trackers: Vec<String>,
}

impl TorrentMeta {
    /// The torrent's identity: the v1 hash when there is one, otherwise the v2 hash.
    pub fn key(&self) -> &str {
        self.info_hash
            .as_deref()
            .or(self.info_hash_v2.as_deref())
            .unwrap_or_default()
    }

    /// The files a user can choose (padding files left out).
    pub fn user_files(&self) -> impl Iterator<Item = &TorrentFile> {
        self.files.iter().filter(|f| !f.pad)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagnetInfo {
    /// v1 info hash (`urn:btih:`), lowercase hex.
    pub info_hash: Option<String>,
    /// v2 info hash (`urn:btmh:1220…`), lowercase hex of the SHA-256.
    pub info_hash_v2: Option<String>,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

impl MagnetInfo {
    /// A normalized magnet URI containing only the info hashes, the display name and trackers.
    /// Other parameters (for example `x.pe` peer addresses) are dropped.
    pub fn canonical_uri(&self) -> String {
        let mut xt = Vec::new();
        if let Some(h) = &self.info_hash {
            xt.push(format!("xt=urn:btih:{h}"));
        }
        if let Some(h) = &self.info_hash_v2 {
            xt.push(format!("xt=urn:btmh:1220{h}"));
        }
        let mut out = format!("magnet:?{}", xt.join("&"));
        if let Some(dn) = &self.display_name {
            out.push_str("&dn=");
            out.push_str(&percent_encode(dn));
        }
        for tr in &self.trackers {
            out.push_str("&tr=");
            out.push_str(&percent_encode(tr));
        }
        out
    }

    /// The torrent's identity, as for [`TorrentMeta::key`].
    pub fn key(&self) -> &str {
        self.info_hash
            .as_deref()
            .or(self.info_hash_v2.as_deref())
            .unwrap_or_default()
    }

    /// What the link says about the torrent. A link with only a v1 hash may still be a hybrid;
    /// that is known once the metadata arrives.
    pub fn version(&self) -> TorrentVersion {
        match (&self.info_hash, &self.info_hash_v2) {
            (Some(_), Some(_)) => TorrentVersion::Hybrid,
            (None, Some(_)) => TorrentVersion::V2,
            _ => TorrentVersion::V1,
        }
    }

    /// Whether `meta` is the torrent this link names: every hash in the link must be the
    /// metadata's.
    pub fn matches(&self, meta: &TorrentMeta) -> bool {
        let ok = |want: &Option<String>, have: &Option<String>| match want {
            Some(w) => have.as_ref() == Some(w),
            None => true,
        };
        (self.info_hash.is_some() || self.info_hash_v2.is_some())
            && ok(&self.info_hash, &meta.info_hash)
            && ok(&self.info_hash_v2, &meta.info_hash_v2)
    }

    /// Whether two links name the same torrent (they share a hash).
    pub fn same_torrent(&self, other: &MagnetInfo) -> bool {
        let eq = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
        eq(&self.info_hash, &other.info_hash) || eq(&self.info_hash_v2, &other.info_hash_v2)
    }
}

/// Why metadata, a magnet link or a tracker URL was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentError {
    /// Metadata that isn't a valid torrent: why, for the user.
    Invalid(&'static str),
    /// A file or folder name that isn't a safe file name on every platform (shown escaped).
    UnsafeName(String),
    /// The same path twice, or one name for both a file and a folder: which.
    DuplicatePath(&'static str),
    /// A magnet link that can't be used: why.
    Magnet(&'static str),
    /// A tracker URL that isn't `http`, `https` or `udp` with a host (shown shortened).
    BadTracker(String),
}

impl TorrentError {
    pub fn code(&self) -> ErrorCode {
        match self {
            TorrentError::Invalid(_) => ErrorCode::InvalidInput,
            TorrentError::UnsafeName(_) | TorrentError::DuplicatePath(_) => {
                ErrorCode::InvalidFilename
            }
            TorrentError::Magnet(_) | TorrentError::BadTracker(_) => ErrorCode::InvalidUrl,
        }
    }
}

impl fmt::Display for TorrentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TorrentError::Invalid(why)
            | TorrentError::DuplicatePath(why)
            | TorrentError::Magnet(why) => f.write_str(why),
            TorrentError::UnsafeName(name) => {
                write!(f, "The torrent contains an unsafe file name: \"{name}\".")
            }
            TorrentError::BadTracker(url) => {
                write!(
                    f,
                    "\"{url}\" is not a valid tracker URL (http, https or udp)."
                )
            }
        }
    }
}

impl std::error::Error for TorrentError {}

pub type TorrentResult<T> = Result<T, TorrentError>;

/// Parse and validate a `.torrent` file's contents (BitTorrent v1, v2 or hybrid).
pub fn parse_torrent(bytes: &[u8]) -> TorrentResult<TorrentMeta> {
    parse(bytes, true)
}

/// Parse metadata that may have come from peers (a magnet link's metadata, or the stored copy of
/// it). Peers send only the info dictionary, so v2 piece layers may be absent; when they are
/// present they are still verified.
pub fn parse_metadata(bytes: &[u8]) -> TorrentResult<TorrentMeta> {
    parse(bytes, false)
}

fn parse(bytes: &[u8], require_layers: bool) -> TorrentResult<TorrentMeta> {
    if bytes.len() > MAX_TORRENT_BYTES {
        return Err(bad("The torrent file is larger than 16 MiB."));
    }
    let mut p = Parser {
        b: bytes,
        pos: 0,
        nodes: 0,
    };
    let root = p.value(0)?;
    if p.pos != bytes.len() {
        return Err(bad("The torrent file has unexpected data after its end."));
    }
    let B::Dict(top) = &root else {
        return Err(bad("The torrent file is not a dictionary."));
    };
    let (info, (start, end)) =
        get_span(top, "info").ok_or_else(|| bad("The torrent file has no info section."))?;
    let B::Dict(info) = info else {
        return Err(bad("The torrent's info section is not a dictionary."));
    };
    let info_bytes = &bytes[start..end];
    let has_v1 = get(info, "pieces").is_some();
    let has_v2 = match get(info, "meta version") {
        None => {
            if get(info, "file tree").is_some() {
                return Err(bad(
                    "The torrent has a v2 file tree but no metadata version.",
                ));
            }
            false
        }
        Some(B::Int(2)) => true,
        Some(_) => return Err(bad("The torrent uses an unknown metadata version.")),
    };
    if !has_v1 && !has_v2 {
        return Err(bad("The torrent's piece hashes are missing or malformed."));
    }
    let name = text(info, "name").ok_or_else(|| bad("The torrent has no valid name."))?;
    check_component(&name)?;
    let piece_length = int(get(info, "piece length"))
        .filter(|l| *l > 0 && *l <= 1 << 30)
        .ok_or_else(|| bad("The torrent has an invalid piece length."))?
        as u64;
    let private = int(get(info, "private")) == Some(1);

    let v1 = if has_v1 {
        Some(v1_files(info, &name, piece_length)?)
    } else {
        None
    };
    let v2 = if has_v2 {
        Some(v2_files(top, info, &name, piece_length, require_layers)?)
    } else {
        None
    };
    let (files, multi_file, num_pieces, version) = match (v1, v2) {
        (Some(v1), None) => (v1.files, v1.multi_file, v1.pieces, TorrentVersion::V1),
        (None, Some(v2)) => (v2.files, v2.multi_file, v2.pieces, TorrentVersion::V2),
        (Some(v1), Some(v2)) => {
            // The v1 list (with its padding) must describe exactly the v2 files.
            let data = |f: &[TorrentFile]| -> Vec<(Vec<String>, u64)> {
                f.iter()
                    .filter(|f| !f.pad)
                    .map(|f| (f.path.clone(), f.length))
                    .collect()
            };
            if data(&v1.files) != data(&v2.files)
                || v1.multi_file != v2.multi_file
                || v1.pieces != v2.pieces
            {
                return Err(bad(
                    "The torrent's v1 and v2 file lists do not match, so it is not a valid hybrid torrent.",
                ));
            }
            (v1.files, v1.multi_file, v1.pieces, TorrentVersion::Hybrid)
        }
        (None, None) => return Err(bad("The torrent's piece hashes are missing or malformed.")),
    };
    // libtorrent's padding entries for v2-only torrents are virtual (it inserts them itself), so
    // only listed paths must be unique; a v1 or hybrid torrent lists its padding files.
    let listed: Vec<TorrentFile> = files
        .iter()
        .filter(|f| !f.pad || version != TorrentVersion::V2)
        .cloned()
        .collect();
    check_unique_paths(&listed)?;
    let total_length = files
        .iter()
        .filter(|f| !f.pad)
        .try_fold(0u64, |a, f| a.checked_add(f.length))
        .ok_or_else(|| bad("The torrent's total size is too large."))?;
    if total_length == 0 {
        return Err(bad("The torrent contains no data."));
    }
    let mut trackers = Vec::new();
    if let Some(B::Bytes(a)) = get(top, "announce") {
        push_tracker(&mut trackers, a);
    }
    if let Some(B::List(tiers)) = get(top, "announce-list") {
        for tier in tiers {
            if let B::List(t) = tier {
                for u in t {
                    if let B::Bytes(u) = u {
                        push_tracker(&mut trackers, u);
                    }
                }
            }
        }
    }
    Ok(TorrentMeta {
        info_hash: has_v1.then(|| {
            use sha1::{Digest, Sha1};
            hex(&Sha1::digest(info_bytes))
        }),
        info_hash_v2: has_v2.then(|| hex(&sha256(info_bytes))),
        version,
        name,
        files,
        total_length,
        piece_length,
        num_pieces,
        private,
        multi_file,
        trackers,
    })
}

struct FileList {
    files: Vec<TorrentFile>,
    multi_file: bool,
    pieces: u64,
}

type Dict<'b, 'a> = &'b [(&'a [u8], B<'a>, (usize, usize))];

/// The v1 `files`/`length` description, with any BEP 47 padding files.
fn v1_files(info: Dict, name: &str, piece_length: u64) -> TorrentResult<FileList> {
    let pieces = match get(info, "pieces") {
        Some(B::Bytes(p)) if !p.is_empty() && p.len() % 20 == 0 => p.len() as u64 / 20,
        _ => return Err(bad("The torrent's piece hashes are missing or malformed.")),
    };
    let mut files = Vec::new();
    let multi_file = match get(info, "files") {
        Some(B::List(list)) => {
            if list.is_empty() || list.len() > MAX_FILES {
                return Err(bad("The torrent's file list is empty or too large."));
            }
            for (i, f) in list.iter().enumerate() {
                let B::Dict(f) = f else {
                    return Err(bad("The torrent's file list is malformed."));
                };
                let length = length_of(f)?;
                let comps = match get(f, "path.utf-8").or_else(|| get(f, "path")) {
                    Some(B::List(c)) => c,
                    _ => return Err(bad("A file in the torrent has no path.")),
                };
                if comps.is_empty() || comps.len() + 1 > MAX_PATH_COMPONENTS {
                    return Err(bad("A file in the torrent has an invalid path."));
                }
                let mut path = vec![name.to_string()];
                for c in comps {
                    let B::Bytes(c) = c else {
                        return Err(bad("A file in the torrent has an invalid path."));
                    };
                    let c = std::str::from_utf8(c)
                        .map_err(|_| bad("A file name in the torrent is not valid UTF-8."))?;
                    check_component(c)?;
                    path.push(c.to_string());
                }
                let pad = matches!(get(f, "attr"), Some(B::Bytes(a)) if a.contains(&b'p'));
                files.push(TorrentFile {
                    index: i as u32 + 1,
                    path,
                    length,
                    pad,
                });
            }
            true
        }
        Some(_) => return Err(bad("The torrent's file list is malformed.")),
        None => {
            files.push(TorrentFile {
                index: 1,
                path: vec![name.to_string()],
                length: length_of(info)?,
                pad: false,
            });
            false
        }
    };
    // Pieces cover the files back to back, padding included.
    let span = files
        .iter()
        .try_fold(0u64, |a, f| a.checked_add(f.length))
        .ok_or_else(|| bad("The torrent's total size is too large."))?;
    if span.div_ceil(piece_length) != pieces {
        return Err(bad("The torrent's piece count does not match its size."));
    }
    Ok(FileList {
        files,
        multi_file,
        pieces,
    })
}

const V2_BLOCK: u64 = 16 * 1024;

/// The BEP 52 `file tree`, checked against the `piece layers`. Files are listed in tree order
/// with the padding libtorrent inserts after every file that does not end on a piece boundary
/// (v2 files each start on a new piece).
fn v2_files(
    top: Dict,
    info: Dict,
    name: &str,
    piece_length: u64,
    require_layers: bool,
) -> TorrentResult<FileList> {
    if piece_length < V2_BLOCK || !piece_length.is_power_of_two() {
        return Err(bad("The torrent has an invalid piece length."));
    }
    let Some(B::Dict(tree)) = get(info, "file tree") else {
        return Err(bad("The torrent's v2 file tree is missing or malformed."));
    };
    let mut leaves = Vec::new();
    walk_tree(tree, &mut Vec::new(), &mut leaves, 0)?;
    if leaves.is_empty() || leaves.len() > MAX_FILES {
        return Err(bad("The torrent's file list is empty or too large."));
    }
    // A single-file torrent's tree holds one file named like the torrent.
    let multi_file = !(leaves.len() == 1 && leaves[0].0.len() == 1 && leaves[0].0[0] == name);
    let (layers, check_layers) = match get(top, "piece layers") {
        Some(B::Dict(d)) => (d.as_slice(), true),
        None => (&[][..], require_layers),
        Some(_) => return Err(bad("The torrent's piece layers are malformed.")),
    };
    let mut files = Vec::new();
    let mut pieces = 0u64;
    for (comps, length, root) in leaves {
        let path = if multi_file {
            std::iter::once(name.to_string()).chain(comps).collect()
        } else {
            comps
        };
        if length > 0 {
            let root = root.ok_or_else(|| bad("A v2 file entry has no valid root hash."))?;
            let n = length.div_ceil(piece_length);
            if length > piece_length && check_layers {
                let layer = layers
                    .iter()
                    .find(|(k, _, _)| *k == root.as_slice())
                    .map(|(_, v, _)| v);
                let Some(B::Bytes(layer)) = layer else {
                    return Err(bad("A file's piece layer is missing from the torrent."));
                };
                if layer.len() as u64 != n * 32 {
                    return Err(bad("A file's piece layer has the wrong size."));
                }
                if merkle_root(layer, piece_length) != root {
                    return Err(bad(
                        "A file's piece layer does not match its root hash, so the torrent is corrupt.",
                    ));
                }
            }
            pieces = pieces
                .checked_add(n)
                .ok_or_else(|| bad("The torrent's total size is too large."))?;
        }
        let tail = length % piece_length;
        let pad_path = (tail != 0).then(|| {
            vec![
                if multi_file {
                    name.to_string()
                } else {
                    path[0].clone()
                },
                ".pad".to_string(),
                (piece_length - tail).to_string(),
            ]
        });
        files.push(TorrentFile {
            index: files.len() as u32 + 1,
            path,
            length,
            pad: false,
        });
        if let Some(path) = pad_path {
            files.push(TorrentFile {
                index: files.len() as u32 + 1,
                path,
                length: piece_length - tail,
                pad: true,
            });
        }
    }
    Ok(FileList {
        files,
        multi_file,
        pieces,
    })
}

type Leaf = (Vec<String>, u64, Option<[u8; 32]>);

fn walk_tree(
    node: Dict,
    prefix: &mut Vec<String>,
    out: &mut Vec<Leaf>,
    depth: usize,
) -> TorrentResult<()> {
    if depth >= MAX_PATH_COMPONENTS {
        return Err(bad("A file in the torrent has an invalid path."));
    }
    let mut last: Option<&[u8]> = None;
    for (key, value, _) in node {
        // Keys are sorted, which fixes the file order every client agrees on.
        if last.is_some_and(|l| l >= *key) {
            return Err(bad("The torrent's v2 file tree is not sorted."));
        }
        last = Some(key);
        let B::Dict(child) = value else {
            return Err(bad("The torrent's v2 file tree is malformed."));
        };
        if key.is_empty() {
            return Err(bad("The torrent's v2 file tree is malformed."));
        }
        let c = std::str::from_utf8(key)
            .map_err(|_| bad("A file name in the torrent is not valid UTF-8."))?;
        check_component(c)?;
        prefix.push(c.to_string());
        match get(child, "") {
            Some(B::Dict(leaf)) => {
                if child.len() != 1 {
                    return Err(TorrentError::DuplicatePath(
                        "The torrent uses the same name for a file and a folder.",
                    ));
                }
                let length = length_of(leaf)?;
                let root = match get(leaf, "pieces root") {
                    Some(B::Bytes(r)) => Some(
                        <[u8; 32]>::try_from(*r)
                            .map_err(|_| bad("A v2 file entry has no valid root hash."))?,
                    ),
                    Some(_) => return Err(bad("A v2 file entry has no valid root hash.")),
                    None => None,
                };
                out.push((prefix.clone(), length, root));
            }
            Some(_) => return Err(bad("The torrent's v2 file tree is malformed.")),
            None => {
                if child.is_empty() {
                    return Err(bad("The torrent's v2 file tree has an empty folder."));
                }
                walk_tree(child, prefix, out, depth + 1)?;
            }
        }
        prefix.pop();
    }
    Ok(())
}

fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
}

/// The Merkle root over a BEP 52 piece layer. The layer is padded to a power of two with the
/// hash of an all-zero piece-sized subtree (leaves are 16 KiB blocks whose padding hash is 32
/// zero bytes).
fn merkle_root(layer: &[u8], piece_length: u64) -> [u8; 32] {
    let mut pad = [0u8; 32];
    let mut size = V2_BLOCK;
    while size < piece_length {
        pad = sha256(&[pad, pad].concat());
        size *= 2;
    }
    let mut level: Vec<[u8; 32]> = layer.as_chunks::<32>().0.to_vec();
    level.resize(level.len().next_power_of_two(), pad);
    while level.len() > 1 {
        level = level
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[a, b]| sha256(&[*a, *b].concat()))
            .collect();
        pad = sha256(&[pad, pad].concat());
    }
    level[0]
}

/// Parse and validate a magnet URI. v1 hashes (`urn:btih:`, 40 hexadecimal or 32 base32
/// characters) and v2 hashes (`urn:btmh:1220` followed by a 64-character SHA-256 multihash) are
/// accepted. Parameters other than the hashes, `dn` and `tr` (for example `x.pe` peer addresses)
/// are ignored.
pub fn parse_magnet(raw: &str) -> TorrentResult<MagnetInfo> {
    let raw = raw.trim();
    let query = raw
        .get(..8)
        .filter(|s| s.eq_ignore_ascii_case("magnet:?"))
        .map(|_| &raw[8..])
        .ok_or(TorrentError::Magnet("This is not a magnet link."))?;
    let query = query.split('#').next().unwrap_or_default();
    let mut hash: Option<String> = None;
    let mut hash_v2: Option<String> = None;
    let mut display_name = None;
    let mut trackers = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = query_value(v);
        match query_value(k).as_str() {
            "xt" => {
                let lower = v.to_ascii_lowercase();
                if let Some(h) = lower.strip_prefix("urn:btih:") {
                    let h = normalize_info_hash(h).ok_or(TorrentError::Magnet(
                        "The magnet link's info hash is not a valid BitTorrent hash.",
                    ))?;
                    if hash.as_ref().is_some_and(|x| *x != h) {
                        return Err(TorrentError::Magnet(
                            "The magnet link contains two different info hashes.",
                        ));
                    }
                    hash = Some(h);
                } else if let Some(m) = lower.strip_prefix("urn:btmh:") {
                    // Multihash: 0x12 = SHA2-256, 0x20 = 32 bytes.
                    let h = m
                        .strip_prefix("1220")
                        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
                        .ok_or(TorrentError::Magnet(
                            "The magnet link's v2 info hash is not a valid BitTorrent v2 hash.",
                        ))?;
                    if hash_v2.as_deref().is_some_and(|x| x != h) {
                        return Err(TorrentError::Magnet(
                            "The magnet link contains two different info hashes.",
                        ));
                    }
                    hash_v2 = Some(h.to_string());
                }
            }
            "dn" if display_name.is_none() => {
                let d: String = v.chars().filter(|c| !c.is_control()).take(255).collect();
                let d = d.trim().to_string();
                if !d.is_empty() {
                    display_name = Some(d);
                }
            }
            "tr" => push_tracker(&mut trackers, v.as_bytes()),
            _ => {}
        }
    }
    if hash.is_none() && hash_v2.is_none() {
        return Err(TorrentError::Magnet(
            "The magnet link has no BitTorrent info hash.",
        ));
    }
    Ok(MagnetInfo {
        info_hash: hash,
        info_hash_v2: hash_v2,
        display_name,
        trackers,
    })
}

/// A query key or value: `+` is a space, then `%XX` escapes are decoded.
fn query_value(s: &str) -> String {
    crate::naming::percent_decode(&s.replace('+', " "))
}

/// Normalize a user-supplied v1 info hash (hex or base32) to lowercase hex.
pub fn normalize_info_hash(s: &str) -> Option<String> {
    match s.len() {
        40 if s.chars().all(|c| c.is_ascii_hexdigit()) => Some(s.to_ascii_lowercase()),
        32 => {
            let mut bits: u64 = 0;
            let mut nbits = 0;
            let mut out = Vec::with_capacity(20);
            for c in s.bytes() {
                let v = match c.to_ascii_uppercase() {
                    c @ b'A'..=b'Z' => c - b'A',
                    c @ b'2'..=b'7' => c - b'2' + 26,
                    _ => return None,
                };
                bits = (bits << 5) | v as u64;
                nbits += 5;
                if nbits >= 8 {
                    nbits -= 8;
                    out.push((bits >> nbits) as u8);
                    bits &= (1 << nbits) - 1;
                }
            }
            (out.len() == 20).then(|| hex(&out))
        }
        _ => None,
    }
}

/// libtorrent's priority for a file it downloads, and for one it doesn't.
const WANTED: u8 = 4;
const SKIPPED: u8 = 0;

/// The file priorities for downloading exactly `wanted`, by [`TorrentFile::index`]: one per file
/// in `files`' order, padding files included and never wanted. Refuses an empty choice, a
/// duplicate, a padding file and an index `files` hasn't.
pub fn file_priorities(files: &[TorrentFile], wanted: &[u32]) -> TorrentResult<Vec<u8>> {
    if wanted.is_empty() {
        return Err(bad("Choose at least one file to download."));
    }
    let mut priorities = vec![SKIPPED; files.len()];
    for index in wanted {
        let slot = files
            .iter()
            .position(|f| f.index == *index && !f.pad)
            .ok_or_else(|| bad("The chosen files aren't all files of this torrent."))?;
        if priorities[slot] == WANTED {
            return Err(bad("A file is chosen twice."));
        }
        priorities[slot] = WANTED;
    }
    Ok(priorities)
}

/// Check a tracker URL from the settings or the user (`http`, `https` or `udp`).
pub fn check_tracker(t: &str) -> TorrentResult<()> {
    if tracker(t.as_bytes()).as_deref() == Some(t) {
        Ok(())
    } else {
        Err(TorrentError::BadTracker(t.chars().take(80).collect()))
    }
}

/// A tracker URL as it may be shown or logged: scheme, host and port only. The path and query
/// often hold a private tracker's passkey, and a login is never shown (V1's `redact_tracker`).
pub fn redact_tracker(url: &str) -> String {
    let Some(checked) = tracker(url.as_bytes()) else {
        return "[tracker]".into();
    };
    let Some((scheme, rest)) = checked.split_once("://") else {
        return "[tracker]".into();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let more = rest[end..].trim_start_matches('/');
    let tail = if more.is_empty() { "" } else { "/…" };
    format!("{scheme}://{host_port}{tail}")
}

/// The most bytes of resume data accepted: twice a torrent's, since besides the metadata it holds
/// v2 hash trees and a bitfield.
pub const MAX_RESUME_BYTES: usize = 2 * MAX_TORRENT_BYTES;

/// libtorrent resume data that passed [`check_resume_data`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedResume {
    /// The torrent's metadata, `d4:info…e`, as checked by [`parse_metadata`].
    pub metadata: Vec<u8>,
    pub meta: TorrentMeta,
}

/// Checks libtorrent resume data before libtorrent reads it again (#62): it comes back from the
/// app's storage. Only the keys libtorrent 2.0.15 writes are accepted; `mapped_files`, which would
/// rename the torrent's files, never is.
pub fn check_resume_data(bytes: &[u8]) -> TorrentResult<CheckedResume> {
    if bytes.len() > MAX_RESUME_BYTES {
        return Err(bad("The torrent's resume data is larger than 32 MiB."));
    }
    let mut p = Parser {
        b: bytes,
        pos: 0,
        nodes: 0,
    };
    let root = p.value(0)?;
    if p.pos != bytes.len() {
        return Err(bad(
            "The torrent's resume data has unexpected data after its end.",
        ));
    }
    let B::Dict(top) = &root else {
        return Err(bad("The torrent's resume data is not a dictionary."));
    };
    if top.iter().any(|(key, _, _)| !RESUME_KEYS.contains(key)) {
        return Err(bad(
            "The torrent's resume data has an entry libtorrent doesn't write.",
        ));
    }
    if !matches!(
        get(top, "file-format"),
        Some(B::Bytes(b"libtorrent resume file"))
    ) {
        return Err(bad("The torrent's resume data isn't libtorrent's."));
    }
    // Modes V2 never sets: trusting files unchecked, sharing only, super-seeding, and I2P.
    for mode in ["seed_mode", "share_mode", "super_seeding", "i2p"] {
        if !matches!(get(top, mode), None | Some(B::Int(0))) {
            return Err(bad("The torrent's resume data sets a mode V2 never uses."));
        }
    }
    let (_, (start, end)) =
        get_span(top, "info").ok_or_else(|| bad("The torrent's resume data has no metadata."))?;
    let mut metadata = b"d4:info".to_vec();
    metadata.extend_from_slice(&bytes[start..end]);
    metadata.push(b'e');
    let meta = parse_metadata(&metadata)?;
    let raw = |hash: &Option<String>, len: usize| -> Vec<u8> {
        hash.as_deref()
            .map(|h| {
                (0..h.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok())
                    .collect()
            })
            .unwrap_or_else(|| vec![0; len])
    };
    for (key, hash, len) in [
        ("info-hash", &meta.info_hash, 20),
        ("info-hash2", &meta.info_hash_v2, 32),
    ] {
        match get(top, key) {
            None => {}
            Some(B::Bytes(b)) if *b == raw(hash, len).as_slice() => {}
            Some(_) => {
                return Err(bad(
                    "The torrent's resume data names another torrent than its metadata.",
                ))
            }
        }
    }
    for tracker in strings(get(top, "trackers"))? {
        check_tracker(&tracker)?;
    }
    for seed in strings(get(top, "url-list"))?
        .into_iter()
        .chain(strings(get(top, "httpseeds"))?)
    {
        check_tracker(&seed)?;
        if !(seed.starts_with("http://") || seed.starts_with("https://")) {
            return Err(TorrentError::BadTracker(seed.chars().take(80).collect()));
        }
    }
    if let Some(priorities) = get(top, "file_priority") {
        let ok = match priorities {
            B::List(items) => {
                items.len() == meta.files.len() && items.iter().all(|p| matches!(p, B::Int(0..=7)))
            }
            _ => false,
        };
        if !ok {
            return Err(bad(
                "The torrent's resume data has file priorities that don't match its files.",
            ));
        }
    }
    Ok(CheckedResume { metadata, meta })
}

/// The URLs in a resume data entry: a list of URLs, or of lists of them (tracker tiers).
fn strings(v: Option<&B<'_>>) -> TorrentResult<Vec<String>> {
    let mut out = Vec::new();
    let mut stack: Vec<&B<'_>> = v.into_iter().collect();
    while let Some(v) = stack.pop() {
        match v {
            B::List(items) => stack.extend(items.iter().rev()),
            B::Bytes(b) => out.push(
                std::str::from_utf8(b)
                    .map_err(|_| bad("The torrent's resume data has an invalid URL."))?
                    .to_owned(),
            ),
            _ => return Err(bad("The torrent's resume data has an invalid URL.")),
        }
    }
    Ok(out)
}

/// Resume data checked with [`check_resume_data`], without its trackers, and the trackers it had,
/// in tier order. Tracker addresses can carry a private tracker's passkey, so the app stores the
/// resume data without them and keeps the trackers elsewhere (PO decision on #58: the OS
/// keychain, #70).
pub fn strip_trackers(bytes: &[u8]) -> TorrentResult<(Vec<u8>, Vec<String>)> {
    check_resume_data(bytes)?;
    let mut p = Parser {
        b: bytes,
        pos: 0,
        nodes: 0,
    };
    let B::Dict(top) = p.value(0)? else {
        return Err(bad("The torrent's resume data is not a dictionary."));
    };
    let Some((value, (start, end))) = get_span(&top, "trackers") else {
        return Ok((bytes.to_vec(), Vec::new()));
    };
    let trackers = strings(Some(value))?;
    // The key, `8:trackers`, is right before its value.
    let key = b"8:trackers".len();
    let mut without = bytes[..start - key].to_vec();
    without.extend_from_slice(&bytes[end..]);
    Ok((without, trackers))
}

/// A `.torrent` file checked with [`parse_torrent`], without its trackers (`announce` and
/// `announce-list`), and the trackers it had, in tier order: the counterpart of
/// [`strip_trackers`] for a `.torrent` the app keeps (#60). Everything else, such as v2 piece
/// layers and web seeds, is kept as it was.
pub fn strip_torrent_trackers(bytes: &[u8]) -> TorrentResult<(Vec<u8>, Vec<String>)> {
    let meta = parse_torrent(bytes)?;
    let mut p = Parser {
        b: bytes,
        pos: 0,
        nodes: 0,
    };
    let B::Dict(top) = p.value(0)? else {
        return Err(bad("The torrent file is not a dictionary."));
    };
    // Each key, `<length>:<key>`, is right before its value.
    let mut cuts: Vec<(usize, usize)> = ["announce", "announce-list"]
        .iter()
        .filter_map(|key| {
            get_span(&top, key)
                .map(|(_, (start, end))| (start - format!("{}:{key}", key.len()).len(), end))
        })
        .collect();
    cuts.sort_unstable();
    let mut without = Vec::with_capacity(bytes.len());
    let mut at = 0;
    for (start, end) in cuts {
        without.extend_from_slice(&bytes[at..start]);
        at = end;
    }
    without.extend_from_slice(&bytes[at..]);
    Ok((without, meta.trackers))
}

/// The keys libtorrent 2.0.15 writes in resume data (`write_resume_data`): those seen in its
/// output, and those it writes only in some states, such as `file_priority` for a partial choice,
/// `unfinished` for partly downloaded pieces, and the peers it knew. `mapped_files` is left out
/// on purpose: it renames the torrent's files, and V2 never does.
const RESUME_KEYS: &[&[u8]] = &[
    b"active_time",
    b"added_time",
    b"allocation",
    b"apply_ip_filter",
    b"auto_managed",
    b"banned_peers",
    b"banned_peers6",
    b"comment",
    b"completed_time",
    b"created by",
    b"creation date",
    b"disable_dht",
    b"disable_lsd",
    b"disable_pex",
    b"download_rate_limit",
    b"file-format",
    b"file-version",
    b"file_priority",
    b"finished_time",
    b"httpseeds",
    b"i2p",
    b"info",
    b"info-hash",
    b"info-hash2",
    b"last_download",
    b"last_seen_complete",
    b"last_upload",
    b"libtorrent-version",
    b"max_connections",
    b"max_uploads",
    b"name",
    b"num_complete",
    b"num_downloaded",
    b"num_incomplete",
    b"paused",
    b"peers",
    b"peers6",
    b"piece_priority",
    b"pieces",
    b"save_path",
    b"seed_mode",
    b"seeding_time",
    b"sequential_download",
    b"share_mode",
    b"stop_when_ready",
    b"super_seeding",
    b"total_downloaded",
    b"total_uploaded",
    b"trackers",
    b"trees",
    b"unfinished",
    b"upload_mode",
    b"upload_rate_limit",
    b"url-list",
];

fn bad(why: &'static str) -> TorrentError {
    TorrentError::Invalid(why)
}

enum B<'a> {
    Int(i64),
    Bytes(&'a [u8]),
    List(Vec<B<'a>>),
    /// Entries in file order, with the byte span of each value.
    Dict(Vec<(&'a [u8], B<'a>, (usize, usize))>),
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> TorrentResult<u8> {
        self.b
            .get(self.pos)
            .copied()
            .ok_or_else(|| bad("The torrent file is truncated."))
    }

    fn value(&mut self, depth: usize) -> TorrentResult<B<'a>> {
        if depth > MAX_DEPTH {
            return Err(bad("The torrent file is nested too deeply."));
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(bad("The torrent file has too many entries."));
        }
        match self.peek()? {
            b'i' => {
                self.pos += 1;
                let start = self.pos;
                while self.peek()? != b'e' {
                    self.pos += 1;
                }
                let s = std::str::from_utf8(&self.b[start..self.pos])
                    .map_err(|_| bad("The torrent file has an invalid number."))?;
                self.pos += 1;
                let digits = s.strip_prefix('-').unwrap_or(s);
                if digits.is_empty()
                    || !digits.bytes().all(|c| c.is_ascii_digit())
                    || (digits.len() > 1 && digits.starts_with('0'))
                    || s == "-0"
                {
                    return Err(bad("The torrent file has an invalid number."));
                }
                s.parse()
                    .map(B::Int)
                    .map_err(|_| bad("The torrent file has an invalid number."))
            }
            b'l' => {
                self.pos += 1;
                let mut items = Vec::new();
                while self.peek()? != b'e' {
                    items.push(self.value(depth + 1)?);
                }
                self.pos += 1;
                Ok(B::List(items))
            }
            b'd' => {
                self.pos += 1;
                let mut entries = Vec::new();
                while self.peek()? != b'e' {
                    let key = self.bytes()?;
                    let start = self.pos;
                    let v = self.value(depth + 1)?;
                    entries.push((key, v, (start, self.pos)));
                }
                self.pos += 1;
                Ok(B::Dict(entries))
            }
            b'0'..=b'9' => self.bytes().map(B::Bytes),
            _ => Err(bad("The file is not a valid torrent file.")),
        }
    }

    fn bytes(&mut self) -> TorrentResult<&'a [u8]> {
        let start = self.pos;
        while self.peek()? != b':' {
            if !self.peek()?.is_ascii_digit() || self.pos - start > 10 {
                return Err(bad("The torrent file has an invalid string."));
            }
            self.pos += 1;
        }
        let digits = &self.b[start..self.pos];
        if digits.is_empty() || (digits.len() > 1 && digits[0] == b'0') {
            return Err(bad("The torrent file has an invalid string."));
        }
        let len: usize = std::str::from_utf8(digits)
            .ok()
            .and_then(|d| d.parse().ok())
            .ok_or_else(|| bad("The torrent file has an invalid string."))?;
        self.pos += 1;
        let end = self
            .pos
            .checked_add(len)
            .filter(|e| *e <= self.b.len())
            .ok_or_else(|| bad("The torrent file is truncated."))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
}

fn get_span<'b, 'a>(
    d: &'b [(&'a [u8], B<'a>, (usize, usize))],
    key: &str,
) -> Option<(&'b B<'a>, (usize, usize))> {
    d.iter()
        .find(|(k, _, _)| *k == key.as_bytes())
        .map(|(_, v, span)| (v, *span))
}

fn get<'b, 'a>(d: &'b [(&'a [u8], B<'a>, (usize, usize))], key: &str) -> Option<&'b B<'a>> {
    get_span(d, key).map(|(v, _)| v)
}

fn int(v: Option<&B<'_>>) -> Option<i64> {
    match v {
        Some(B::Int(i)) => Some(*i),
        _ => None,
    }
}

/// `name.utf-8` when present, otherwise `name`, which must be valid UTF-8.
fn text(d: &[(&[u8], B<'_>, (usize, usize))], key: &str) -> Option<String> {
    let utf8_key = format!("{key}.utf-8");
    for k in [utf8_key.as_str(), key] {
        if let Some(B::Bytes(b)) = get(d, k) {
            if let Ok(s) = std::str::from_utf8(b) {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn length_of(d: &[(&[u8], B<'_>, (usize, usize))]) -> TorrentResult<u64> {
    int(get(d, "length"))
        .filter(|l| *l >= 0)
        .map(|l| l as u64)
        .ok_or_else(|| bad("A file in the torrent has an invalid size."))
}

/// A torrent-supplied file or folder name must be a safe file name on every platform: the rule
/// for every name the app is given, except that it may start with a dot (PO decision on #62).
fn check_component(c: &str) -> TorrentResult<()> {
    crate::naming::check_file_name_in_folder(c).map_err(|_| {
        TorrentError::UnsafeName(c.chars().take(80).flat_map(char::escape_debug).collect())
    })
}

/// Refuse duplicate paths (compared case-insensitively, as on Windows) and a file that another
/// entry uses as a folder.
fn check_unique_paths(files: &[TorrentFile]) -> TorrentResult<()> {
    let mut seen = std::collections::HashSet::new();
    let mut dirs = std::collections::HashSet::new();
    for f in files {
        let lower: Vec<String> = f.path.iter().map(|c| c.to_lowercase()).collect();
        for i in 1..lower.len() {
            dirs.insert(lower[..i].join("/"));
        }
        if !seen.insert(lower.join("/")) {
            return Err(TorrentError::DuplicatePath(
                "The torrent lists the same file twice.",
            ));
        }
    }
    if seen.iter().any(|p| dirs.contains(p)) {
        return Err(TorrentError::DuplicatePath(
            "The torrent uses the same name for a file and a folder.",
        ));
    }
    Ok(())
}

/// Adds `raw` to `out` when it is a usable tracker URL and not already there.
fn push_tracker(out: &mut Vec<String>, raw: &[u8]) {
    if out.len() >= MAX_TRACKERS {
        return;
    }
    if let Some(t) = tracker(raw) {
        if !out.contains(&t) {
            out.push(t);
        }
    }
}

/// The tracker URL in `raw` (trimmed) when it is `http`, `https` or `udp` with a host, short
/// enough, and free of spaces and control characters.
fn tracker(raw: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(raw).ok()?.trim();
    if s.len() > MAX_TRACKER_LEN || s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (scheme, rest) = s.split_once("://")?;
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "udp"
    ) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    (!host.is_empty()).then(|| s.to_string())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Bencode encoding, used by tests to build fixtures and by nothing that handles untrusted input.
pub mod bencode {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Value {
        Int(i64),
        Bytes(Vec<u8>),
        List(Vec<Value>),
        /// Keys are sorted when encoded, as the format requires.
        Dict(Vec<(Vec<u8>, Value)>),
    }

    impl Value {
        pub fn str(s: &str) -> Self {
            Self::Bytes(s.as_bytes().to_vec())
        }
        pub fn dict(entries: Vec<(&str, Value)>) -> Self {
            Self::Dict(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.as_bytes().to_vec(), v))
                    .collect(),
            )
        }
    }

    pub fn encode(v: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        write(v, &mut out);
        out
    }

    fn write(v: &Value, out: &mut Vec<u8>) {
        match v {
            Value::Int(i) => out.extend_from_slice(format!("i{i}e").as_bytes()),
            Value::Bytes(b) => {
                out.extend_from_slice(format!("{}:", b.len()).as_bytes());
                out.extend_from_slice(b);
            }
            Value::List(l) => {
                out.push(b'l');
                for i in l {
                    write(i, out);
                }
                out.push(b'e');
            }
            Value::Dict(d) => {
                let mut d: Vec<_> = d.iter().collect();
                d.sort_by(|a, b| a.0.cmp(&b.0));
                out.push(b'd');
                for (k, v) in d {
                    write(&Value::Bytes(k.clone()), out);
                    write(v, out);
                }
                out.push(b'e');
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::bencode::{encode, Value};
    use super::*;
    use sha1::{Digest, Sha1};

    pub(crate) fn file(path: &[&str], len: i64) -> Value {
        Value::dict(vec![
            ("length", Value::Int(len)),
            (
                "path",
                Value::List(path.iter().map(|p| Value::str(p)).collect()),
            ),
        ])
    }

    fn pieces(n: usize) -> Value {
        Value::Bytes(vec![7u8; 20 * n])
    }

    pub(crate) fn multi(name: &str, files: Vec<Value>, piece_len: i64, n: usize) -> Value {
        Value::dict(vec![
            ("name", Value::str(name)),
            ("piece length", Value::Int(piece_len)),
            ("pieces", pieces(n)),
            ("files", Value::List(files)),
        ])
    }

    pub(crate) fn torrent(info: Value, extra: Vec<(&str, Value)>) -> Vec<u8> {
        let mut entries = vec![("info", info)];
        entries.extend(extra);
        encode(&Value::dict(entries))
    }

    pub(crate) fn sha1_hex(b: &[u8]) -> String {
        hex(&Sha1::digest(b))
    }

    #[test]
    fn parses_single_file_torrent_and_hashes_the_info_dictionary() {
        let info = Value::dict(vec![
            ("name", Value::str("movie.mkv")),
            ("length", Value::Int(40_000)),
            ("piece length", Value::Int(16_384)),
            ("pieces", pieces(3)),
        ]);
        let expected = sha1_hex(&encode(&info));
        let bytes = torrent(
            info,
            vec![("announce", Value::str("http://tracker.example/announce"))],
        );
        let m = parse_torrent(&bytes).unwrap();
        assert_eq!(m.info_hash, Some(expected));
        assert_eq!(m.name, "movie.mkv");
        assert!(!m.multi_file);
        assert_eq!(
            m.files,
            vec![TorrentFile {
                index: 1,
                path: vec!["movie.mkv".into()],
                length: 40_000,
                pad: false,
            }]
        );
        assert_eq!(m.total_length, 40_000);
        assert_eq!(m.piece_length, 16_384);
        assert_eq!(m.num_pieces, 3);
        assert_eq!(m.trackers, vec!["http://tracker.example/announce"]);
    }

    #[test]
    fn info_hash_uses_the_exact_info_bytes_even_with_unsorted_keys() {
        // Hand-written bencode with keys out of order: the hash must cover the bytes as they
        // appear, not a re-encoding.
        let info = b"d6:pieces20:aaaaaaaaaaaaaaaaaaaa4:name1:x6:lengthi5e12:piece lengthi16384ee";
        let mut bytes = b"d4:info".to_vec();
        bytes.extend_from_slice(info);
        bytes.push(b'e');
        let m = parse_torrent(&bytes).unwrap();
        assert_eq!(m.info_hash, Some(sha1_hex(info)));
    }

    #[test]
    fn parses_nested_multi_file_torrent_with_zero_byte_files() {
        let info = multi(
            "album",
            vec![
                file(&["cd1", "01.flac"], 100),
                file(&["cd1", "empty.txt"], 0),
                file(&["cover.jpg"], 50),
            ],
            16_384,
            1,
        );
        let m = parse_torrent(&torrent(
            info,
            vec![(
                "announce-list",
                Value::List(vec![
                    Value::List(vec![Value::str("udp://a.example:80")]),
                    Value::List(vec![Value::str("https://b.example/announce")]),
                ]),
            )],
        ))
        .unwrap();
        assert!(m.multi_file);
        assert_eq!(m.total_length, 150);
        let paths: Vec<_> = m.files.iter().map(|f| f.path.join("/")).collect();
        assert_eq!(
            paths,
            [
                "album/cd1/01.flac",
                "album/cd1/empty.txt",
                "album/cover.jpg"
            ]
        );
        assert_eq!(
            m.files.iter().map(|f| f.index).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(
            m.trackers,
            ["udp://a.example:80", "https://b.example/announce"]
        );
    }

    #[test]
    fn prefers_utf8_names_and_reads_the_private_flag() {
        let info = Value::dict(vec![
            ("name", Value::Bytes(vec![0xff, 0xfe])),
            ("name.utf-8", Value::str("Ünïcode.bin")),
            ("length", Value::Int(1)),
            ("piece length", Value::Int(16_384)),
            ("pieces", pieces(1)),
            ("private", Value::Int(1)),
        ]);
        let m = parse_torrent(&torrent(info, vec![])).unwrap();
        assert_eq!(m.name, "Ünïcode.bin");
        assert!(m.private);
    }

    /// V1 refused these as invalid input; V2 refuses them with the file-name code, through the
    /// rule for every name the app is given (`naming::check_file_name_in_folder`), which also
    /// refuses names a file system would change, such as a trailing dot or `?`.
    #[test]
    fn rejects_malicious_paths() {
        let bad: Vec<Vec<&str>> = vec![
            vec!["..", "etc", "passwd"],
            vec!["ok", "..", "..", "x"],
            vec!["/abs"],
            vec!["a/b"],
            vec!["a\\b"],
            vec!["C:"],
            vec!["x:y"],
            vec![""],
            vec!["."],
            vec!["nul\0byte"],
            vec!["line\nbreak"],
            vec!["CON"],
            vec!["com1.txt"],
            vec!["trailing."],
            vec!["what?"],
        ];
        for p in bad {
            let info = multi("t", vec![file(&p, 1)], 16_384, 1);
            let e = parse_torrent(&torrent(info, vec![])).unwrap_err();
            assert_eq!(e.code(), ErrorCode::InvalidFilename, "path {p:?}: {e}");
        }
        let info = multi("t", vec![file(&[], 1)], 16_384, 1);
        assert_eq!(
            parse_torrent(&torrent(info, vec![])).unwrap_err().code(),
            ErrorCode::InvalidInput,
            "an empty path"
        );
        for name in ["..", "../x", "/", "a/b", ""] {
            let info = Value::dict(vec![
                ("name", Value::str(name)),
                ("length", Value::Int(1)),
                ("piece length", Value::Int(16_384)),
                ("pieces", pieces(1)),
            ]);
            assert!(
                parse_torrent(&torrent(info, vec![])).is_err(),
                "name {name:?}"
            );
        }
    }

    /// Dotfiles are common in real torrents (PO decision on #62).
    #[test]
    fn hidden_files_are_accepted() {
        let info = multi(
            "src",
            vec![file(&[".gitignore"], 1), file(&[".config", "a"], 1)],
            16_384,
            1,
        );
        let m = parse_torrent(&torrent(info, vec![])).unwrap();
        assert_eq!(m.files[0].path, ["src", ".gitignore"]);
    }

    /// File choice (#62): one priority per file, padding included, in libtorrent's order.
    #[test]
    fn a_file_choice_becomes_one_priority_per_file() {
        let f = |index: u32, pad: bool| TorrentFile {
            index,
            path: vec![format!("f{index}")],
            length: 10,
            pad,
        };
        let files = [f(1, false), f(2, true), f(3, false), f(4, false)];
        assert_eq!(file_priorities(&files, &[1, 4]), Ok(vec![4, 0, 0, 4]));
        assert_eq!(file_priorities(&files, &[4, 3, 1]), Ok(vec![4, 0, 4, 4]));
        for (wanted, why) in [
            (&[][..], "nothing chosen"),
            (&[2][..], "a padding file"),
            (&[5][..], "an index the torrent hasn't"),
            (&[0][..], "index 0"),
            (&[1, 1][..], "a duplicate"),
        ] {
            let e = file_priorities(&files, wanted).unwrap_err();
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{why}");
        }
    }

    #[test]
    fn an_unsafe_name_is_shown_escaped_in_the_message() {
        let info = multi("t", vec![file(&["bad\u{1b}[31m"], 1)], 16_384, 1);
        let e = parse_torrent(&torrent(info, vec![])).unwrap_err();
        assert!(!e.to_string().contains('\u{1b}'), "{e:?}");
        assert!(e.to_string().contains("unsafe file name"), "{e}");
    }

    #[test]
    fn rejects_malformed_and_unsupported_metadata() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", vec![]),
            ("not bencode", b"hello".to_vec()),
            ("truncated", b"d4:infod4:name".to_vec()),
            ("trailing data", {
                let info = multi("t", vec![file(&["a"], 1)], 16_384, 1);
                let mut b = torrent(info, vec![]);
                b.extend_from_slice(b"junk");
                b
            }),
            ("no info", encode(&Value::dict(vec![("x", Value::Int(1))]))),
            ("negative length", {
                torrent(multi("t", vec![file(&["a"], -1)], 16_384, 1), vec![])
            }),
            ("bad pieces", {
                let info = Value::dict(vec![
                    ("name", Value::str("a")),
                    ("length", Value::Int(1)),
                    ("piece length", Value::Int(16_384)),
                    ("pieces", Value::Bytes(vec![1; 19])),
                ]);
                torrent(info, vec![])
            }),
            ("piece count mismatch", {
                torrent(multi("t", vec![file(&["a"], 40_000)], 16_384, 1), vec![])
            }),
            ("zero piece length", {
                torrent(multi("t", vec![file(&["a"], 1)], 0, 1), vec![])
            }),
            ("v2 only with an empty tree", {
                let info = Value::dict(vec![
                    ("name", Value::str("a")),
                    ("meta version", Value::Int(2)),
                    ("piece length", Value::Int(16_384)),
                    ("file tree", Value::dict(vec![])),
                ]);
                torrent(info, vec![])
            }),
            ("leading zero int", b"d4:infod6:lengthi01eee".to_vec()),
            (
                "huge string length",
                b"d4:info99999999999999999999:x".to_vec(),
            ),
        ];
        for (what, bytes) in cases {
            let e = parse_torrent(&bytes).unwrap_err();
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{what}: {e:?}");
        }
    }

    #[test]
    fn deep_nesting_is_rejected_without_stack_overflow() {
        let mut b = b"d4:info".to_vec();
        b.extend(std::iter::repeat_n(b'l', 100_000));
        assert!(parse_torrent(&b).is_err());
    }

    #[test]
    fn oversized_input_is_rejected() {
        let b = vec![b'd'; MAX_TORRENT_BYTES + 1];
        assert!(parse_torrent(&b).is_err());
    }

    #[test]
    fn duplicate_file_paths_are_rejected() {
        let info = multi("t", vec![file(&["a"], 1), file(&["a"], 1)], 16_384, 1);
        assert_eq!(
            parse_torrent(&torrent(info, vec![])).unwrap_err().code(),
            ErrorCode::InvalidFilename
        );
        // Case-insensitively, as on Windows.
        let info = multi("t", vec![file(&["A"], 1), file(&["a"], 1)], 16_384, 1);
        assert!(parse_torrent(&torrent(info, vec![])).is_err());
        // A name used for a file and a folder.
        let info = multi("t", vec![file(&["a"], 1), file(&["a", "b"], 1)], 16_384, 1);
        assert!(parse_torrent(&torrent(info, vec![])).is_err());
    }

    #[test]
    fn info_hash_normalization() {
        let hex = "557f8c61bcecd14914e421389d4f46c5d7c336db";
        assert_eq!(normalize_info_hash(hex).as_deref(), Some(hex));
        assert_eq!(
            normalize_info_hash(&hex.to_ascii_uppercase()).as_deref(),
            Some(hex)
        );
        // The same hash in base32 (RFC 4648, no padding).
        assert_eq!(
            normalize_info_hash("KV7YYYN45TIUSFHEEE4J2T2GYXL4GNW3").as_deref(),
            Some(hex)
        );
        assert_eq!(
            normalize_info_hash("kv7yyyn45tiusfheee4j2t2gyxl4gnw3").as_deref(),
            Some(hex)
        );
        for bad in [
            "",
            "abc",
            "zz7f8c61bcecd14914e421389d4f46c5d7c336db",
            "557f8c61bcecd14914e421389d4f46c5d7c336d",
            "KV7YYYN45TIUSFHEEE4J2T2GYXL4GNW1",
        ] {
            assert_eq!(normalize_info_hash(bad), None, "{bad}");
        }
    }

    #[test]
    fn magnet_parsing() {
        let hex = "557f8c61bcecd14914e421389d4f46c5d7c336db";
        let m = parse_magnet(
            "magnet:?xt=urn:btih:KV7YYYN45TIUSFHEEE4J2T2GYXL4GNW3&dn=My%20File&tr=http%3A%2F%2Ft.example%2Fannounce&tr=udp://u.example:6969&x.pe=10.0.0.1:1",
        )
        .unwrap();
        assert_eq!(m.info_hash.as_deref(), Some(hex));
        assert_eq!(m.display_name.as_deref(), Some("My File"));
        assert_eq!(
            m.trackers,
            ["http://t.example/announce", "udp://u.example:6969"]
        );
        let c = m.canonical_uri();
        assert!(c.starts_with(&format!("magnet:?xt=urn:btih:{hex}")), "{c}");
        assert!(c.contains("dn=My%20File"), "{c}");
        assert!(!c.contains("x.pe"), "{c}");
        assert_eq!(parse_magnet(&c).unwrap(), m, "canonical form round-trips");

        // Unsupported tracker schemes are dropped, not fatal.
        let m = parse_magnet(&format!("magnet:?xt=urn:btih:{hex}&tr=javascript:alert(1)")).unwrap();
        assert!(m.trackers.is_empty());
        // `+` in a query value is a space, as in any URL query.
        let m = parse_magnet(&format!("magnet:?xt=urn:btih:{hex}&dn=My+File")).unwrap();
        assert_eq!(m.display_name.as_deref(), Some("My File"));
        // A v2-only link is a v2-only torrent.
        let m = parse_magnet(
            "magnet:?xt=urn:btmh:1220d2474e86c95b19b8bcfdb92bc12c9d44667cfa36d2474e86c95b19b8bcfdb92b",
        )
        .unwrap();
        assert_eq!(m.version(), TorrentVersion::V2);
    }

    #[test]
    fn magnet_rejections() {
        for bad in [
            "magnet:?dn=x",
            "magnet:?xt=urn:btih:",
            "magnet:?xt=urn:btih:12345",
            "magnet:?xt=urn:btih:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            "magnet:?xt=urn:sha1:557f8c61bcecd14914e421389d4f46c5d7c336db",
            "magnet:?xt=urn:btih:557f8c61bcecd14914e421389d4f46c5d7c336db&xt=urn:btih:0000000000000000000000000000000000000000",
            "https://example.com/?xt=urn:btih:557f8c61bcecd14914e421389d4f46c5d7c336db",
            "magnet:xt=urn:btih:557f8c61bcecd14914e421389d4f46c5d7c336db",
        ] {
            let e = parse_magnet(bad).unwrap_err();
            assert_eq!(e.code(), ErrorCode::InvalidUrl, "{bad}");
        }
    }

    #[test]
    fn tracker_validation() {
        assert!(check_tracker("udp://tracker.example:1337/announce").is_ok());
        assert!(check_tracker("https://t.example/announce?k=1").is_ok());
        assert!(check_tracker("http://[2001:db8::1]:6969/announce").is_ok());
        for bad in [
            "",
            "ftp://x/y",
            "http://",
            "http://:80/a",
            "udp://a b",
            "notaurl",
            " http://t.example/a",
        ] {
            assert!(check_tracker(bad).is_err(), "{bad}");
        }
        let long = format!("http://t.example/{}", "a".repeat(5000));
        let e = check_tracker(&long).unwrap_err();
        assert!(e.to_string().len() < 200, "a long URL is shortened: {e}");
    }

    /// V1's tracker cases in `redacts_tracker_passkeys_and_scrape_queries`, and a login.
    #[test]
    fn a_tracker_is_shown_without_its_passkey_or_login() {
        assert_eq!(
            redact_tracker("http://tracker.test:2710/a1b2c3/announce"),
            "http://tracker.test:2710/…"
        );
        assert_eq!(
            redact_tracker("udp://open.test:1337"),
            "udp://open.test:1337"
        );
        assert_eq!(redact_tracker("not a url"), "[tracker]");
        assert_eq!(
            redact_tracker("https://user:secret@t.test/announce?passkey=abc"),
            "https://t.test/…"
        );
        assert_eq!(
            redact_tracker("http://[2001:db8::1]:6969/announce"),
            "http://[2001:db8::1]:6969/…"
        );
        assert_eq!(redact_tracker("udp://t.test/"), "udp://t.test");
    }

    /// A v1 multi-file info dictionary, and libtorrent resume data for it with the keys
    /// libtorrent 2.0.15 writes for a torrent being downloaded, plus `extra`.
    /// libtorrent-shaped resume data for `info`; `extra` entries replace those of the same key.
    pub(crate) fn resume_for(info: &Value, extra: Vec<(&str, Value)>) -> Vec<u8> {
        let info_hash = Sha1::digest(encode(info)).to_vec();
        let mut entries = vec![
            ("file-format", Value::str("libtorrent resume file")),
            ("file-version", Value::Int(1)),
            ("libtorrent-version", Value::str("2.0.15.0")),
            ("info-hash", Value::Bytes(info_hash)),
            ("info-hash2", Value::Bytes(vec![0; 32])),
            ("info", info.clone()),
            ("name", Value::str("album")),
            ("save_path", Value::str("/somewhere")),
            ("allocation", Value::str("sparse")),
            ("pieces", Value::Bytes(vec![1, 0])),
            ("paused", Value::Int(1)),
            ("auto_managed", Value::Int(0)),
            ("seed_mode", Value::Int(0)),
            ("upload_mode", Value::Int(0)),
            ("share_mode", Value::Int(0)),
            ("super_seeding", Value::Int(0)),
            ("i2p", Value::Int(0)),
            ("total_uploaded", Value::Int(123)),
            ("seeding_time", Value::Int(45)),
            (
                "trackers",
                Value::List(vec![Value::List(vec![Value::str(
                    "udp://tracker.example:80/announce",
                )])]),
            ),
            ("url-list", Value::List(vec![])),
            ("httpseeds", Value::List(vec![])),
            (
                "file_priority",
                Value::List(vec![Value::Int(4), Value::Int(0)]),
            ),
            ("peers", Value::Bytes(vec![127, 0, 0, 1, 0x1a, 0xe1])),
            ("unfinished", Value::List(vec![])),
        ];
        entries.extend(extra);
        let mut seen = std::collections::BTreeSet::new();
        // A later entry replaces an earlier one with the same key.
        let entries: Vec<_> = entries
            .into_iter()
            .rev()
            .filter(|(k, _)| seen.insert(*k))
            .collect();
        encode(&Value::dict(entries))
    }

    /// A two-file torrent's info dictionary: `album/a.bin` (20,000 bytes), `album/b.bin` (12,000).
    pub(crate) fn album() -> Value {
        multi(
            "album",
            vec![file(&["a.bin"], 20_000), file(&["b.bin"], 12_000)],
            16_384,
            2,
        )
    }

    #[test]
    fn resume_data_is_checked_and_gives_its_metadata() {
        let info = album();
        let checked = check_resume_data(&resume_for(&info, vec![])).unwrap();
        let mut metadata = b"d4:info".to_vec();
        metadata.extend(encode(&info));
        metadata.push(b'e');
        assert_eq!(checked.metadata, metadata);
        assert_eq!(checked.meta.name, "album");
        assert_eq!(checked.meta.info_hash, Some(sha1_hex(&encode(&info))));
        assert_eq!(checked.meta.files.len(), 2);
    }

    /// A `.torrent` the app keeps has no tracker in it (#60); the rest, and its identity, stay.
    #[test]
    fn trackers_are_taken_out_of_a_torrent_file_and_the_rest_is_kept() {
        let info = album();
        let with = torrent(
            info,
            vec![
                (
                    "announce",
                    Value::str("https://tracker.example/announce?passkey=secret-key"),
                ),
                (
                    "announce-list",
                    Value::List(vec![Value::List(vec![Value::str(
                        "udp://backup.example:80/announce",
                    )])]),
                ),
                ("url-list", Value::str("https://seed.example/album/")),
            ],
        );
        let (without, trackers) = strip_torrent_trackers(&with).unwrap();
        assert_eq!(
            trackers,
            parse_torrent(&with).unwrap().trackers,
            "in the order the torrent gives them"
        );
        assert!(trackers.len() >= 2, "{trackers:?}");
        let text = String::from_utf8_lossy(&without);
        assert!(
            !text.contains("secret-key") && !text.contains("backup.example"),
            "{text}"
        );
        assert!(text.contains("seed.example"), "web seeds stay");
        let kept = parse_torrent(&without).unwrap();
        assert_eq!(kept.info_hash, parse_torrent(&with).unwrap().info_hash);
        assert!(kept.trackers.is_empty());
        assert!(strip_torrent_trackers(b"junk").is_err());
    }

    /// Tracker passkeys never reach the database (PO decision on #58): the resume data is stored
    /// without its trackers.
    #[test]
    fn trackers_are_taken_out_of_resume_data_and_the_rest_is_kept() {
        let info = album();
        let with = resume_for(&info, vec![]);
        let (without, trackers) = strip_trackers(&with).unwrap();
        assert_eq!(
            trackers,
            vec!["udp://tracker.example:80/announce".to_owned()]
        );
        let text = String::from_utf8_lossy(&without);
        assert!(!text.contains("tracker.example"), "{text}");
        assert!(!text.contains("8:trackers"));
        let checked = check_resume_data(&without).expect("still resume data libtorrent reads");
        assert_eq!(checked.meta.name, "album");
        assert_eq!(
            without.len(),
            with.len() - "8:trackersll33:udp://tracker.example:80/announceee".len()
        );
        // Without trackers already, nothing changes.
        assert_eq!(
            strip_trackers(&without).unwrap(),
            (without.clone(), Vec::new())
        );
    }

    #[test]
    fn every_tier_of_trackers_is_taken_out() {
        let info = album();
        let tiers = Value::List(vec![
            Value::List(vec![
                Value::str("https://a.example/announce?passkey=secret1"),
                Value::str("https://b.example/announce"),
            ]),
            Value::List(vec![Value::str("udp://c.example:6969/secret2/announce")]),
        ]);
        let with = resume_for(&info, vec![("trackers", tiers)]);
        let (without, trackers) = strip_trackers(&with).unwrap();
        assert_eq!(trackers.len(), 3);
        let text = String::from_utf8_lossy(&without);
        assert!(
            !text.contains("secret1") && !text.contains("secret2"),
            "{text}"
        );
    }

    #[test]
    fn resume_data_that_isnt_libtorrents_isnt_stripped_but_refused() {
        assert!(strip_trackers(b"not bencode").is_err());
    }

    #[test]
    fn resume_data_that_could_mislead_libtorrent_is_refused() {
        let info = album();
        let unsafe_info = multi("album", vec![file(&[".."], 5)], 16_384, 1);
        let with = |extra: Vec<(&str, Value)>| resume_for(&info, extra);
        let mut trailing = with(vec![]);
        trailing.push(b'x');
        let mut cases = vec![
            // Renamed files could put the torrent's files anywhere.
            (
                "mapped files",
                with(vec![(
                    "mapped_files",
                    Value::List(vec![Value::str("../x")]),
                )]),
                "an entry libtorrent doesn't write",
            ),
            (
                "a key libtorrent doesn't write",
                with(vec![("x-other", Value::Int(1))]),
                "an entry libtorrent doesn't write",
            ),
            (
                "another format",
                with(vec![("file-format", Value::str("other"))]),
                "isn't libtorrent's",
            ),
            (
                "no metadata",
                with(vec![]).replace_info_with_nothing(),
                "has no metadata",
            ),
            (
                "metadata that fails V2's checks",
                resume_for(&unsafe_info, vec![]),
                "unsafe file name",
            ),
            (
                "a v1 hash that isn't the metadata's",
                with(vec![("info-hash", Value::Bytes(vec![9; 20]))]),
                "names another torrent",
            ),
            (
                "a v2 hash for a v1-only torrent",
                with(vec![("info-hash2", Value::Bytes(vec![9; 32]))]),
                "names another torrent",
            ),
            (
                "a tracker that isn't http, https or udp",
                with(vec![(
                    "trackers",
                    Value::List(vec![Value::List(vec![Value::str("file:///etc/passwd")])]),
                )]),
                "not a valid tracker URL",
            ),
            (
                "a web seed that isn't http or https",
                with(vec![(
                    "url-list",
                    Value::List(vec![Value::str("udp://t.example:1")]),
                )]),
                "not a valid tracker URL",
            ),
            (
                "a priority per file that doesn't match the files",
                with(vec![("file_priority", Value::List(vec![Value::Int(4)]))]),
                "file priorities",
            ),
            (
                "a priority libtorrent doesn't have",
                with(vec![(
                    "file_priority",
                    Value::List(vec![Value::Int(4), Value::Int(9)]),
                )]),
                "file priorities",
            ),
            ("not a dictionary", b"le".to_vec(), "not a dictionary"),
            ("data after its end", trailing, "after its end"),
            (
                "more than the limit",
                with(vec![("pieces", Value::Bytes(vec![0; MAX_RESUME_BYTES]))]),
                "larger than 32 MiB",
            ),
        ];
        for mode in ["seed_mode", "share_mode", "super_seeding", "i2p"] {
            cases.push((
                mode,
                with(vec![(mode, Value::Int(1))]),
                "a mode V2 never uses",
            ));
        }
        // Each is refused for its own reason, not another rule's.
        for (what, bytes, why) in cases {
            match check_resume_data(&bytes) {
                Err(e) => assert!(e.to_string().contains(why), "{what}: {e}"),
                Ok(_) => panic!("{what} was accepted"),
            }
        }
    }

    trait WithoutInfo {
        fn replace_info_with_nothing(self) -> Vec<u8>;
    }

    impl WithoutInfo for Vec<u8> {
        /// The same resume data without its `info` entry.
        fn replace_info_with_nothing(self) -> Vec<u8> {
            let at = self.windows(6).position(|w| w == b"4:info").expect("info");
            let info_start = at + 6;
            let mut p = Parser {
                b: &self,
                pos: info_start,
                nodes: 0,
            };
            p.value(0).unwrap();
            let mut out = self[..at].to_vec();
            out.extend_from_slice(&self[p.pos..]);
            out
        }
    }
}

/// BitTorrent v1, v2 and hybrid metadata checked against the committed corpus, which
/// libtorrent 2.0 generated and described (`scripts/make-torrent-corpus.py`).
#[cfg(test)]
mod corpus_tests {
    use super::*;
    use serde_json::Value as Json;

    fn dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/torrents")
    }

    fn manifest() -> Json {
        serde_json::from_slice(&std::fs::read(dir().join("manifest.json")).unwrap()).unwrap()
    }

    fn load(name: &str) -> Vec<u8> {
        std::fs::read(dir().join(name)).unwrap()
    }

    #[test]
    fn every_corpus_torrent_matches_what_libtorrent_reports() {
        let m = manifest();
        let torrents = m["torrents"].as_object().unwrap();
        assert_eq!(torrents.len(), 6);
        for (name, want) in torrents {
            let meta = parse_torrent(&load(&format!("{name}.torrent")))
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(meta.info_hash.as_deref(), want["v1"].as_str(), "{name} v1");
            assert_eq!(
                meta.info_hash_v2.as_deref(),
                want["v2"].as_str(),
                "{name} v2"
            );
            let version = match want["kind"].as_str().unwrap() {
                "v1" => TorrentVersion::V1,
                "v2" => TorrentVersion::V2,
                _ => TorrentVersion::Hybrid,
            };
            assert_eq!(meta.version, version, "{name}");
            assert_eq!(
                meta.num_pieces,
                want["pieces"].as_u64().unwrap(),
                "{name} pieces"
            );
            // Same files, in the same order (libtorrent's file indexes), with the same padding
            // files libtorrent inserts or reads.
            let got: Vec<(String, u64, bool)> = meta
                .files
                .iter()
                .map(|f| (f.path.join("/"), f.length, f.pad))
                .collect();
            let expected: Vec<(String, u64, bool)> = want["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    (
                        f["path"].as_str().unwrap().to_string(),
                        f["length"].as_u64().unwrap(),
                        f["pad"].as_bool().unwrap(),
                    )
                })
                .collect();
            assert_eq!(got, expected, "{name} files");
            for (i, f) in meta.files.iter().enumerate() {
                assert_eq!(f.index, i as u32 + 1, "{name} index");
            }
            // Padding never counts as data and is never offered for selection.
            let data: u64 = want["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["length"].as_u64().unwrap())
                .sum();
            assert_eq!(meta.total_length, data, "{name} total");
            assert!(meta.user_files().all(|f| !f.pad));
            assert_eq!(
                meta.user_files().count(),
                want["content"].as_array().unwrap().len(),
                "{name} user files"
            );
            assert_eq!(
                meta.key(),
                meta.info_hash
                    .as_deref()
                    .or(meta.info_hash_v2.as_deref())
                    .unwrap()
            );
        }
    }

    #[test]
    fn a_v2_only_torrent_never_gets_a_made_up_v1_hash() {
        for name in ["v2-single", "v2-multi"] {
            let meta = parse_torrent(&load(&format!("{name}.torrent"))).unwrap();
            assert_eq!(meta.info_hash, None, "{name}");
            assert_eq!(meta.key().len(), 64, "{name}: keyed by the SHA-256 hash");
        }
    }

    /// The path-traversal case refuses with the file-name code in V2 (V1: invalid input).
    #[test]
    fn invalid_and_inconsistent_metadata_is_rejected() {
        let m = manifest();
        let invalid = m["invalid"].as_object().unwrap();
        assert_eq!(invalid.len(), 8);
        // libtorrent itself loads some of these (it drops a bad piece layer and sanitizes "..")
        // but the core refuses all of them before libtorrent sees them.
        for (file, why, code) in [
            (
                "v2-piece-layer-corrupt.torrent",
                "piece layer",
                ErrorCode::InvalidInput,
            ),
            (
                "v2-piece-layer-missing.torrent",
                "piece layer",
                ErrorCode::InvalidInput,
            ),
            (
                "v2-piece-layer-short.torrent",
                "piece layer",
                ErrorCode::InvalidInput,
            ),
            (
                "v2-meta-version-3.torrent",
                "version",
                ErrorCode::InvalidInput,
            ),
            (
                "v2-path-traversal.torrent",
                "unsafe file name",
                ErrorCode::InvalidFilename,
            ),
            (
                "v2-pieces-root-size.torrent",
                "root hash",
                ErrorCode::InvalidInput,
            ),
            (
                "hybrid-length-mismatch.torrent",
                "v1 and v2",
                ErrorCode::InvalidInput,
            ),
            (
                "hybrid-name-mismatch.torrent",
                "v1 and v2",
                ErrorCode::InvalidInput,
            ),
        ] {
            assert!(invalid.contains_key(file), "{file}");
            let e = parse_torrent(&load(file)).unwrap_err();
            assert_eq!(e.code(), code, "{file}: {e:?}");
            assert!(e.to_string().contains(why), "{file}: {e}");
        }
    }

    #[test]
    fn corpus_magnets_carry_the_right_hashes() {
        let m = manifest();
        for (name, uri) in m["magnets"].as_object().unwrap() {
            let t = &m["torrents"][name];
            let mg =
                parse_magnet(uri.as_str().unwrap()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(mg.info_hash.as_deref(), t["v1"].as_str(), "{name}");
            assert_eq!(mg.info_hash_v2.as_deref(), t["v2"].as_str(), "{name}");
            // The canonical form keeps both hashes and parses back to the same thing.
            let again = parse_magnet(&mg.canonical_uri()).unwrap();
            assert_eq!(again, mg, "{name}");
            let meta = parse_torrent(&load(&format!("{name}.torrent"))).unwrap();
            assert!(mg.matches(&meta), "{name}");
        }
    }

    #[test]
    fn malformed_and_misleading_v2_magnets() {
        let m = manifest();
        let bad = &m["badMagnets"];
        for k in [
            "wrong-multihash-code",
            "short-v2-hash",
            "non-hex-v2-hash",
            "two-v2-hashes",
        ] {
            let e = parse_magnet(bad[k].as_str().unwrap()).unwrap_err();
            assert_eq!(e.code(), ErrorCode::InvalidUrl, "{k}: {e:?}");
        }
        // A pair of hashes can't be checked until the metadata arrives, so it parses, but it
        // does not match the torrent whose v2 hash it borrowed.
        let pair = parse_magnet(bad["mismatched-pair"].as_str().unwrap()).unwrap();
        let v2 = parse_torrent(&load("v2-multi.torrent")).unwrap();
        let v1 = parse_torrent(&load("v1-multi.torrent")).unwrap();
        assert!(!pair.matches(&v2));
        assert!(!pair.matches(&v1));
    }

    #[test]
    fn hybrid_and_v1_only_magnets_of_the_same_torrent_are_the_same_torrent() {
        let m = manifest();
        let t = &m["torrents"]["hybrid-multi"];
        let v1 = format!("magnet:?xt=urn:btih:{}", t["v1"].as_str().unwrap());
        let v2 = format!("magnet:?xt=urn:btmh:1220{}", t["v2"].as_str().unwrap());
        let both = parse_magnet(m["magnets"]["hybrid-multi"].as_str().unwrap()).unwrap();
        let meta = parse_torrent(&load("hybrid-multi.torrent")).unwrap();
        for uri in [v1, v2] {
            let mg = parse_magnet(&uri).unwrap();
            assert!(mg.matches(&meta), "{uri}");
            assert!(mg.same_torrent(&both), "{uri}");
        }
    }

    /// Metadata received from peers for a magnet link is only the info dictionary: v2 piece
    /// layers are not part of it (peers send them on request).
    #[test]
    fn metadata_from_peers_may_lack_piece_layers_but_a_torrent_file_may_not() {
        let full = load("v2-multi.torrent");
        let meta = parse_torrent(&full).unwrap();
        // d4:info<info>e — what a client saves after fetching a magnet's metadata.
        let info_only = info_only(&full);
        assert_eq!(
            parse_metadata(&info_only).unwrap().info_hash_v2,
            meta.info_hash_v2
        );
        let e = parse_torrent(&info_only).unwrap_err();
        assert!(e.to_string().contains("piece layer"), "{e}");
        // Layers that are present are still checked.
        assert!(parse_metadata(&load("v2-piece-layer-corrupt.torrent")).is_err());
        assert!(parse_metadata(&load("v2-piece-layer-missing.torrent")).is_err());
        assert!(parse_metadata(&load("hybrid-multi.torrent")).is_ok());
    }

    /// `d4:info<info>e` from a whole `.torrent` file.
    fn info_only(full: &[u8]) -> Vec<u8> {
        let start = full.windows(7).position(|w| w == b"4:infod").unwrap() + 6;
        let mut p = Parser {
            b: full,
            pos: start,
            nodes: 0,
        };
        p.value(0).unwrap();
        let mut v = b"d4:info".to_vec();
        v.extend_from_slice(&full[start..p.pos]);
        v.push(b'e');
        v
    }
}
