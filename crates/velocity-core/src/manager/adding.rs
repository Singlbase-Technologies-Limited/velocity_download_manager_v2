//! Adding from the Add dialog (#60): links, magnet links and `.torrent` files in one request,
//! each with its own result, so one bad link never hides what happened to the others.
//!
//! - **Duplicates (PO decision on #60, V1's rules):** a direct download is the same when its link
//!   is, as typed; a torrent when it shares an info hash. A torrent still in the list is never
//!   added twice, because two copies would download the same pieces into the same files. Anything
//!   else added before, in the list or only in the history, is reported, and "Add anyway"
//!   ([`AddRequest::allow_duplicate`]) adds it again.
//! - **A taken name** is asked about or renamed, never replaced (PO decision on #31). A torrent's
//!   names come from its metadata and aren't changed, so a taken one is only asked about.
//! - **The chosen folder** is created only when the user agreed ([`AddRequest::create_directory`]),
//!   one level at a time and never through a link ([`create_folders`]).
//! - **The rest is checked again here:** the UI's preview mirrors these rules, but only this
//!   decides.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{now_ms, AddOptions, Manager, ManagerService, TorrentRequest};
use crate::database::TorrentOrigin;
use crate::destination::{Choice, OnConflict, Placement};
use crate::domain::{
    DestinationCheck, DownloadKind, DownloadStatus, ErrorCode, FailureClass, TransferError,
};
use crate::links::is_link;
use crate::metalink::{self, create_folders};
use crate::request::{
    check_credentials, check_headers, check_referrer, split_login, MAX_URL_BYTES,
};
use crate::seeding::SeedPolicy;
use crate::torrent::{parse_magnet, parse_torrent, TorrentMeta, MAX_TORRENT_BYTES};
use crate::torrent_engine::MAX_RATE_LIMIT;
use crate::transfer::{Checksum, Credentials, Header, Limits, SshKey, TransferSpec};

/// The most links and `.torrent` files one request adds (V1's `MAX_SOURCES`).
pub const MAX_ADD: usize = 500;

/// What the Add dialog asks for: the links and `.torrent` files, and the choices that apply to
/// each of them. Secrets in it (the login, the headers, the key's passphrase) stay in memory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddRequest {
    /// Direct links (`http`, `https`, `ftp`, `ftps`, `sftp`) and magnet links, in order.
    pub links: Vec<String>,
    /// Absolute paths of `.torrent` files.
    pub torrent_files: Vec<String>,
    /// The chosen folder; `None` for the automatic one.
    pub directory: Option<String>,
    /// The file's name, for a single direct link only; `None` to take the link's or the server's.
    pub file_name: Option<String>,
    /// The chosen category; `None` for the automatic one.
    pub category_id: Option<String>,
    /// Create [`AddRequest::directory`] when it doesn't exist: the user agreed to.
    pub create_directory: bool,
    pub on_conflict: OnConflict,
    /// "Add anyway": add what was added before, except a torrent still in the list.
    pub allow_duplicate: bool,
    pub start_paused: bool,
    /// Connections for each direct download; `None` for the performance profile's.
    pub connections: Option<u32>,
    /// KiB/s for each download; 0 for no limit.
    pub speed_limit_kib: u64,
    pub headers: Vec<Header>,
    pub referrer: Option<String>,
    pub credentials: Option<Credentials>,
    /// The private key for `sftp://` links; other links ignore it.
    pub ssh_key: Option<SshKey>,
    pub checksum: Option<Checksum>,
    /// Insist on TLS for `ftp://` links (#51); other links ignore it.
    pub require_encryption: bool,
    /// Torrents wait in Choosing files until the files are chosen.
    pub choose_files: bool,
    /// The files chosen for some of the `.torrent` files, by index (#63); the others get every
    /// file.
    pub file_choices: Vec<FileChoice>,
    /// When the torrents stop seeding (#63); `None` for the default.
    pub seeding: Option<SeedPolicy>,
    /// KiB/s each torrent uploads at most (#63); 0 for no limit.
    pub upload_limit_kib: u64,
}

/// The files chosen for one `.torrent` file of an [`AddRequest`] (#63).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChoice {
    /// One of the request's `torrent_files`, as given.
    pub torrent_file: String,
    /// The files, by [`crate::torrent::TorrentFile::index`]; never empty.
    pub files: Vec<u32>,
}

/// What adding one link or `.torrent` file did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddResult {
    /// The link or path, as it was given.
    pub source: String,
    pub outcome: AddOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum AddOutcome {
    Added {
        id: String,
    },
    /// The name is taken: nothing was added, and the check says what is there.
    Conflict {
        check: DestinationCheck,
    },
    /// It was added before: nothing was added.
    Duplicate {
        duplicate: Duplicate,
    },
    Failed {
        code: ErrorCode,
    },
    /// A `.torrent` link couldn't be fetched: the server's or the network's reason.
    NotFetched {
        error: TransferError,
    },
}

/// The download a new one would repeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Duplicate {
    pub existing_id: String,
    pub name: Option<String>,
    pub status: DownloadStatus,
    /// It is still in the list; otherwise only in the history.
    pub in_list: bool,
    /// "Add anyway" may add it again: false only for a torrent still in the list.
    pub can_add_anyway: bool,
}

impl AddRequest {
    /// A fixed example for the contract fixtures: every field set.
    pub fn sample() -> Self {
        AddRequest {
            links: vec!["https://example.com/files/archive.zip".to_owned()],
            torrent_files: vec!["/home/user/album.torrent".to_owned()],
            directory: Some("/home/user/Downloads".to_owned()),
            file_name: None,
            category_id: Some("compressed".to_owned()),
            create_directory: true,
            on_conflict: OnConflict::Rename,
            allow_duplicate: true,
            start_paused: true,
            connections: Some(4),
            speed_limit_kib: 512,
            headers: vec![Header {
                name: "Cookie".to_owned(),
                value: "session=1".to_owned(),
            }],
            referrer: Some("https://example.com/page".to_owned()),
            credentials: Some(Credentials {
                user_name: "me".to_owned(),
                password: "secret".to_owned(),
            }),
            ssh_key: Some(SshKey {
                path: PathBuf::from("/home/user/.ssh/id_ed25519"),
                passphrase: Some("phrase".to_owned()),
            }),
            checksum: Some(Checksum {
                algorithm: crate::transfer::HashAlgorithm::Sha256,
                hex: "ab".repeat(32),
            }),
            require_encryption: true,
            choose_files: true,
            file_choices: vec![FileChoice {
                torrent_file: "/home/user/album.torrent".to_owned(),
                files: vec![1, 3],
            }],
            seeding: Some(SeedPolicy::Ratio { ratio: 2.0 }),
            upload_limit_kib: 64,
        }
    }
}

impl AddResult {
    /// Fixed examples for the contract fixtures: one of each outcome.
    pub fn samples() -> Vec<Self> {
        let result = |source: &str, outcome| AddResult {
            source: source.to_owned(),
            outcome,
        };
        vec![
            result(
                "https://example.com/a.zip",
                AddOutcome::Added {
                    id: "d-0002".to_owned(),
                },
            ),
            result(
                "https://example.com/files/archive.zip",
                AddOutcome::Conflict {
                    check: DestinationCheck::sample(),
                },
            ),
            result(
                "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                AddOutcome::Duplicate {
                    duplicate: Duplicate {
                        existing_id: "d-0001".to_owned(),
                        name: Some("album".to_owned()),
                        status: DownloadStatus::Seeding,
                        in_list: true,
                        can_add_anyway: false,
                    },
                },
            ),
            result(
                "file:///etc/passwd",
                AddOutcome::Failed {
                    code: ErrorCode::UnsupportedScheme,
                },
            ),
            result(
                "https://example.com/album.torrent",
                AddOutcome::NotFetched {
                    error: TransferError::new(FailureClass::Permanent, Some(404), "Not found"),
                },
            ),
        ]
    }
}

/// A torrent to add, from a magnet link or a `.torrent` file.
struct NewTorrent {
    origin: TorrentOrigin,
    /// The file name that chooses its automatic category.
    hint: Option<String>,
    /// Its own name, a file or a folder, once known: asked about when taken.
    name: Option<String>,
    /// Its v1 and v2 hashes to save now, when they are known from its file.
    hashes: Option<(Option<String>, Option<String>)>,
    /// Its hashes as duplicate keys.
    keys: Vec<String>,
    /// The files the user chose from its file (#63); `None` for every file.
    files: Option<Vec<u32>>,
}

/// What every download of a request shares, checked once.
struct Shared {
    checksum: Option<Checksum>,
    limits: Limits,
    seeding: SeedPolicy,
    /// Bytes per second; 0 for no limit.
    upload_limit: u64,
}

/// The downloads added before, by link and by info hash.
#[derive(Default)]
pub(super) struct Known {
    links: HashMap<String, Duplicate>,
    pub(super) hashes: HashMap<String, Duplicate>,
}

impl Known {
    /// Keeps the one in the list when two downloads have the same key.
    fn note(map: &mut HashMap<String, Duplicate>, key: String, duplicate: Duplicate) {
        match map.get(&key) {
            Some(kept) if kept.in_list || !duplicate.in_list => {}
            _ => {
                map.insert(key, duplicate);
            }
        }
    }

    /// The duplicate that stops `keys` being added, unless the user said "Add anyway" and it may
    /// be.
    fn blocking(
        map: &HashMap<String, Duplicate>,
        keys: &[String],
        allow: bool,
    ) -> Option<Duplicate> {
        keys.iter()
            .filter_map(|k| map.get(k))
            .find(|d| !allow || !d.can_add_anyway)
            .cloned()
    }
}

/// A direct link's kind, from its scheme.
pub(super) fn direct_kind(link: &str) -> Result<DownloadKind, ErrorCode> {
    if let Some(kind) = metalink::source_kind(link) {
        return Ok(kind);
    }
    match link.split_once("://") {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("sftp") => Ok(DownloadKind::Sftp),
        Some(_) => Err(ErrorCode::UnsupportedScheme),
        None if link.contains(':') && !link.contains('/') => Err(ErrorCode::UnsupportedScheme),
        None => Err(ErrorCode::InvalidUrl),
    }
}

/// Whether `link` is to a `.torrent` file, which is fetched and added as a torrent (PO decision
/// on #60): an `http` or `https` link whose path ends in `.torrent`, as V1 told them apart
/// (`isTorrentSource`). `src/features/add/addLogic.ts` mirrors it.
pub fn is_torrent_link(link: &str) -> bool {
    let Some((scheme, rest)) = link.trim().split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    rest.split_once('/').is_some_and(|(_, path)| {
        path.len() >= 8
            && path.is_char_boundary(path.len() - 8)
            && path[path.len() - 8..].eq_ignore_ascii_case(".torrent")
    })
}

fn is_magnet(link: &str) -> bool {
    link.get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
}

/// A torrent's hashes as duplicate keys.
pub(super) fn hash_keys(v1: Option<&String>, v2: Option<&String>) -> Vec<String> {
    v1.into_iter().chain(v2).cloned().collect()
}

/// Creates the folder the user chose and agreed to create: inside its nearest existing parent, one
/// level at a time. `InvalidPath` for a path that isn't absolute or plain, or that meets a link.
fn create_chosen_folder(directory: &Path) -> Result<(), ErrorCode> {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(ErrorCode::InvalidPath);
    }
    let mut parent = directory;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(parent) {
            // A link met on the way is never followed, so nothing is created through it.
            Ok(meta) if is_link(&meta) && !missing.is_empty() => {
                return Err(ErrorCode::InvalidPath)
            }
            Ok(_) => break,
            Err(_) => {
                let name = parent.file_name().ok_or(ErrorCode::InvalidPath)?;
                missing.push(name.to_string_lossy().into_owned());
                parent = parent.parent().ok_or(ErrorCode::InvalidPath)?;
            }
        }
    }
    missing.reverse();
    create_folders(parent, &missing).map(|_| ())
}

impl Manager {
    /// Adds every link and `.torrent` file of `request`, in order, and returns a result for each.
    /// The request is refused whole, with nothing added, when it is empty or larger than
    /// [`MAX_ADD`], names a file for more than one, or has an option outside its rules.
    pub fn add_request(
        &mut self,
        request: &AddRequest,
        now_ms: u64,
    ) -> Result<Vec<AddResult>, ErrorCode> {
        let count = request.links.len() + request.torrent_files.len();
        if count == 0
            || count > MAX_ADD
            || (request.file_name.is_some() && count != 1)
            || request.connections == Some(0)
        {
            return Err(ErrorCode::InvalidInput);
        }
        check_headers(&request.headers)?;
        if let Some(referrer) = &request.referrer {
            check_referrer(referrer)?;
        }
        if let Some(credentials) = &request.credentials {
            check_credentials(credentials)?;
        }
        check_file_choices(request)?;
        let seeding = request.seeding.unwrap_or_default();
        seeding.check()?;
        let upload_limit = request
            .upload_limit_kib
            .checked_mul(1024)
            .filter(|&bps| bps <= MAX_RATE_LIMIT)
            .ok_or(ErrorCode::InvalidInput)?;
        let shared = Shared {
            checksum: request
                .checksum
                .as_ref()
                .map(Checksum::normalized)
                .transpose()?,
            limits: Limits::from_kib(request.speed_limit_kib)?,
            seeding,
            upload_limit,
        };
        if request.create_directory {
            let directory = request.directory.as_deref().map(str::trim);
            let directory = directory.filter(|d| !d.is_empty());
            let directory = PathBuf::from(directory.ok_or(ErrorCode::InvalidInput)?);
            if std::fs::symlink_metadata(&directory).is_err() {
                create_chosen_folder(&directory)?;
            }
        }
        let mut known = self.known_downloads()?;
        let mut results = Vec::with_capacity(count);
        for link in &request.links {
            let outcome = self
                .add_link(link.trim(), request, &shared, &mut known, now_ms)
                .unwrap_or_else(|code| AddOutcome::Failed { code });
            results.push(AddResult {
                source: link.clone(),
                outcome,
            });
        }
        for file in &request.torrent_files {
            let files = request
                .file_choices
                .iter()
                .find(|c| c.torrent_file == *file)
                .map(|c| c.files.clone());
            let outcome = self
                .add_torrent_file(
                    Path::new(file.trim()),
                    files,
                    request,
                    &shared,
                    &mut known,
                    now_ms,
                )
                .unwrap_or_else(|code| AddOutcome::Failed { code });
            results.push(AddResult {
                source: file.clone(),
                outcome,
            });
        }
        Ok(results)
    }

    fn choice(request: &AddRequest) -> Choice {
        Choice {
            directory: request.directory.clone(),
            file_name: request.file_name.clone(),
            category_id: request.category_id.clone(),
        }
    }

    fn options(request: &AddRequest, category_id: Option<String>) -> AddOptions {
        AddOptions {
            start_paused: request.start_paused,
            scheduled: false,
            category_id,
        }
    }

    fn add_link(
        &mut self,
        link: &str,
        request: &AddRequest,
        shared: &Shared,
        known: &mut Known,
        now_ms: u64,
    ) -> Result<AddOutcome, ErrorCode> {
        if link.len() > MAX_URL_BYTES {
            return Err(ErrorCode::InvalidUrl);
        }
        if is_magnet(link) {
            return self.add_magnet(link, request, shared, known, now_ms);
        }
        let kind = direct_kind(link)?;
        // A protocol the engine lacks is refused before placing it can make a folder.
        if self.engine.capabilities().protocol(kind).is_none() {
            return Err(ErrorCode::Unsupported);
        }
        // A login goes in its own fields, never in the link (#40).
        if split_login(link).1.is_some() {
            return Err(ErrorCode::InvalidUrl);
        }
        let key = [link.to_owned()];
        if let Some(duplicate) = Known::blocking(&known.links, &key, request.allow_duplicate) {
            return Ok(AddOutcome::Duplicate { duplicate });
        }
        let place = match self.place(link, &Self::choice(request), request.on_conflict)? {
            Placement::Placed(place) => place,
            Placement::Conflict(check) => return Ok(AddOutcome::Conflict { check }),
        };
        let spec = TransferSpec {
            urls: vec![link.to_owned()],
            directory: place.directory,
            file_name: place.file_name,
            credentials: request.credentials.clone(),
            require_encryption: request.require_encryption && kind == DownloadKind::Ftp,
            ssh_key: request
                .ssh_key
                .clone()
                .filter(|_| kind == DownloadKind::Sftp),
            headers: request.headers.clone(),
            referrer: request.referrer.clone(),
            user_agent: self.user_agent.clone(),
            checksum: shared.checksum.clone(),
            pieces: None,
            limits: shared.limits,
            // The profile caps it ([`Manager::add_with`]).
            max_connections: request.connections.unwrap_or(u32::MAX),
            resume: None,
        };
        spec.check_request()?;
        let options = Self::options(request, place.category_id);
        let id = self.add_with(kind, spec, options, now_ms)?;
        let [key] = key;
        Known::note(&mut known.links, key, self.listed(&id, false));
        Ok(AddOutcome::Added { id })
    }

    fn add_magnet(
        &mut self,
        link: &str,
        request: &AddRequest,
        shared: &Shared,
        known: &mut Known,
        now_ms: u64,
    ) -> Result<AddOutcome, ErrorCode> {
        let magnet = parse_magnet(link).map_err(|e| e.code())?;
        let torrent = NewTorrent {
            origin: TorrentOrigin::Magnet {
                uri: link.to_owned(),
            },
            // Its names aren't known until its metadata is: its display name chooses the
            // category.
            hint: magnet.display_name.clone(),
            name: None,
            // The hashes are in the stored link.
            hashes: None,
            keys: hash_keys(magnet.info_hash.as_ref(), magnet.info_hash_v2.as_ref()),
            files: None,
        };
        self.add_new_torrent(torrent, request, shared, known, now_ms)
    }

    fn add_torrent_file(
        &mut self,
        path: &Path,
        files: Option<Vec<u32>>,
        request: &AddRequest,
        shared: &Shared,
        known: &mut Known,
        now_ms: u64,
    ) -> Result<AddOutcome, ErrorCode> {
        let meta = read_torrent_file(path)?;
        // Only files the torrent has, and never only padding (the helper refuses the rest too).
        if let Some(files) = &files {
            let real = |i: &u32| meta.user_files().any(|f| f.index == *i);
            if !files.iter().all(real) {
                return Err(ErrorCode::InvalidInput);
            }
        }
        let torrent = NewTorrent {
            origin: TorrentOrigin::File {
                path: path.to_path_buf(),
            },
            // Its largest file chooses the category (#31).
            hint: meta
                .user_files()
                .max_by_key(|f| f.length)
                .and_then(|f| f.path.last().cloned()),
            keys: hash_keys(meta.info_hash.as_ref(), meta.info_hash_v2.as_ref()),
            hashes: Some((meta.info_hash, meta.info_hash_v2)),
            name: Some(meta.name),
            files,
        };
        self.add_new_torrent(torrent, request, shared, known, now_ms)
    }

    /// `ProtocolDisabled` while BitTorrent is off (#30).
    fn add_new_torrent(
        &mut self,
        torrent: NewTorrent,
        request: &AddRequest,
        shared: &Shared,
        known: &mut Known,
        now_ms: u64,
    ) -> Result<AddOutcome, ErrorCode> {
        if !self.torrents_enabled {
            return Err(ErrorCode::ProtocolDisabled);
        }
        if let Some(duplicate) =
            Known::blocking(&known.hashes, &torrent.keys, request.allow_duplicate)
        {
            return Ok(AddOutcome::Duplicate { duplicate });
        }
        let choice = Choice {
            file_name: torrent.hint,
            ..Self::choice(request)
        };
        let place = match self.place_torrent(&choice, torrent.name.as_deref())? {
            Placement::Placed(place) => place,
            Placement::Conflict(check) => return Ok(AddOutcome::Conflict { check }),
        };
        let added = TorrentRequest {
            trackers: self.fetched_trackers(&torrent.origin),
            origin: torrent.origin,
            directory: place.directory,
            // A file choice made before adding is the choice: nothing waits to be chosen.
            choose_files: request.choose_files && torrent.files.is_none(),
            files: torrent.files,
            seeding: shared.seeding,
            upload_limit: shared.upload_limit,
            download_limit: shared.limits.max_download_bps.unwrap_or(0),
        };
        let options = Self::options(request, place.category_id);
        let id = self.add_torrent(added, options, now_ms)?;
        // A file's hashes are known now, before the helper reports them, so the next request
        // finds it as a duplicate too.
        if let Some((v1, v2)) = torrent.hashes {
            if let Err(e) =
                self.database
                    .update_torrent_identity(&id, v1.as_deref(), v2.as_deref(), 0)
            {
                self.storage_error = Some(e);
            }
        }
        for key in torrent.keys {
            Known::note(&mut known.hashes, key, self.listed(&id, true));
        }
        Ok(AddOutcome::Added { id })
    }

    /// The downloads in the list or the history, by link and by info hash.
    pub(super) fn known_downloads(&self) -> Result<Known, ErrorCode> {
        let mut known = Known::default();
        for stored in self.database.downloads().map_err(|e| e.code)? {
            if !stored.in_list && !stored.in_history {
                continue;
            }
            let torrent = matches!(
                stored.kind.known(),
                Some(DownloadKind::Torrent | DownloadKind::Magnet)
            );
            let in_list = stored.in_list && self.downloads.contains_key(&stored.id);
            let duplicate = match self.downloads.get(&stored.id) {
                Some(_) if in_list => self.listed(&stored.id, torrent),
                _ => Duplicate {
                    existing_id: stored.id.clone(),
                    name: stored.file_name.clone(),
                    status: stored
                        .status
                        .known()
                        .copied()
                        .unwrap_or(DownloadStatus::Unknown),
                    in_list: false,
                    can_add_anyway: true,
                },
            };
            if !torrent {
                Known::note(&mut known.links, stored.source_url.clone(), duplicate);
                continue;
            }
            let mut keys = hash_keys(stored.info_hash.as_ref(), stored.info_hash_v2.as_ref());
            if let Ok(magnet) = parse_magnet(&stored.source_url) {
                keys.extend(hash_keys(
                    magnet.info_hash.as_ref(),
                    magnet.info_hash_v2.as_ref(),
                ));
            }
            for key in keys {
                Known::note(&mut known.hashes, key, duplicate.clone());
            }
        }
        Ok(known)
    }

    /// A download in the list, as a duplicate of a new one.
    fn listed(&self, id: &str, torrent: bool) -> Duplicate {
        let record = &self.downloads[id].record;
        Duplicate {
            existing_id: id.to_owned(),
            name: record.file_name.clone(),
            status: record.status,
            in_list: true,
            can_add_anyway: !torrent,
        }
    }
}

/// How long adding waits for a `.torrent` link to be fetched before giving up on it.
pub const FETCH_WAIT: Duration = Duration::from_secs(60);

/// Why a fetch given up on failed, in words for the user.
const TOO_SLOW: &str = "The .torrent file took too long to download.";

impl ManagerService {
    /// Adds what the Add dialog asks for, as [`Manager::add_request`] does, after fetching each
    /// `.torrent` link ([`is_torrent_link`]) to add it as the file it is. Waits at most `wait` for
    /// the fetches, off the manager's thread, which keeps running meanwhile; a fetch still running
    /// then is cancelled. The results are in the request's order, a fetched link's under the
    /// link.
    pub fn add(&self, request: AddRequest, wait: Duration) -> Result<Vec<AddResult>, ErrorCode> {
        let count = request.links.len() + request.torrent_files.len();
        if count == 0 || count > MAX_ADD {
            return Err(ErrorCode::InvalidInput);
        }
        let mut outcomes: Vec<Option<AddOutcome>> = vec![None; request.links.len()];
        let mut fetching = Vec::new();
        for (i, link) in request.links.iter().enumerate() {
            if !is_torrent_link(link) {
                continue;
            }
            let (reply, answer) = channel();
            let (link, fetch_request) = (link.clone(), request.clone());
            match self.call(move |m| m.fetch_torrent_link(&link, &fetch_request, reply))? {
                Ok(fetch) => fetching.push((i, fetch, answer)),
                Err(code) => outcomes[i] = Some(AddOutcome::Failed { code }),
            }
        }
        let deadline = Instant::now() + wait;
        let mut fetched = Vec::new();
        for (i, fetch, answer) in fetching {
            match answer.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(path)) => fetched.push((i, path)),
                Ok(Err(outcome)) => outcomes[i] = Some(outcome),
                Err(_) => {
                    self.call(move |m| m.cancel_torrent_link(fetch))?;
                    outcomes[i] = Some(AddOutcome::NotFetched {
                        error: TransferError::new(FailureClass::Transient, None, TOO_SLOW),
                    });
                }
            }
        }
        // The rest in one request: the other links, the user's files, then the fetched ones.
        let direct: Vec<usize> = (0..request.links.len())
            .filter(|&i| !is_torrent_link(&request.links[i]))
            .collect();
        let rest = AddRequest {
            links: direct.iter().map(|&i| request.links[i].clone()).collect(),
            torrent_files: request
                .torrent_files
                .iter()
                .cloned()
                .chain(
                    fetched
                        .iter()
                        .map(|(_, path)| path.to_string_lossy().into_owned()),
                )
                .collect(),
            ..request.clone()
        };
        let added = if rest.links.is_empty() && rest.torrent_files.is_empty() {
            Vec::new()
        } else {
            self.call(move |m| m.add_request(&rest, now_ms()))??
        };
        let mut added = added.into_iter().map(|r| r.outcome);
        for &i in &direct {
            outcomes[i] = added.next();
        }
        let files: Vec<AddResult> = request
            .torrent_files
            .iter()
            .zip(added.by_ref())
            .map(|(source, outcome)| AddResult {
                source: source.clone(),
                outcome,
            })
            .collect();
        for (i, _) in &fetched {
            outcomes[*i] = added.next();
        }
        let mut results = Vec::with_capacity(count);
        for (source, outcome) in request.links.into_iter().zip(outcomes) {
            results.push(AddResult {
                source,
                outcome: outcome.ok_or(ErrorCode::Internal)?,
            });
        }
        results.extend(files);
        Ok(results)
    }
}

/// Reads and checks the `.torrent` file at `path`, an absolute path: never through a link, and at
/// most [`MAX_TORRENT_BYTES`].
pub(super) fn read_torrent_file(path: &Path) -> Result<TorrentMeta, ErrorCode> {
    if !path.is_absolute() {
        return Err(ErrorCode::InvalidPath);
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => ErrorCode::Io,
    })?;
    // Never read through a link (AGENTS.md, "File safety").
    if is_link(&meta) || !meta.is_file() {
        return Err(ErrorCode::InvalidPath);
    }
    if meta.len() > MAX_TORRENT_BYTES as u64 {
        return Err(ErrorCode::InvalidInput);
    }
    let bytes = std::fs::read(path).map_err(|_| ErrorCode::Io)?;
    parse_torrent(&bytes).map_err(|e| e.code())
}

/// `InvalidInput` unless each file choice names one of the request's `.torrent` files, once, and
/// chooses some files, each once.
fn check_file_choices(request: &AddRequest) -> Result<(), ErrorCode> {
    let mut named = std::collections::HashSet::new();
    for choice in &request.file_choices {
        let mut files = std::collections::HashSet::new();
        let ok = request.torrent_files.contains(&choice.torrent_file)
            && named.insert(choice.torrent_file.as_str())
            && !choice.files.is_empty()
            && choice.files.iter().all(|f| files.insert(*f));
        if !ok {
            return Err(ErrorCode::InvalidInput);
        }
    }
    Ok(())
}
