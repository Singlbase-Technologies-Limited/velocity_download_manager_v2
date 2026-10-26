//! Deleting a torrent's files (#62, PO decision): only what the torrent may have written in its
//! folder, and never anything reached through a link. The torrent helper never deletes; the manager
//! calls this once the helper has let go of the torrent (#58).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::ErrorCode;
use crate::links::is_link;
use crate::torrent::TorrentMeta;

/// How often a file that can't be deleted yet is tried again.
const RETRY_EVERY: Duration = Duration::from_millis(100);

/// One file a torrent may have written, and the most bytes it can hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFile {
    pub path: PathBuf,
    pub max_len: u64,
}

/// Everything a torrent may have written in its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPaths {
    /// The folder the torrent was added with: nothing outside it is touched, nor the folder itself.
    pub root: PathBuf,
    /// The files ever chosen. Padding files are never written; unchosen files never either, because
    /// libtorrent keeps their part of a shared piece in its part file.
    pub files: Vec<OwnedFile>,
    /// libtorrent's part file, `.<info hash>.parts`: for a v1 hash, and for a v2 hash cut to 40
    /// digits, since libtorrent names it by whichever it uses.
    pub parts: Vec<PathBuf>,
    /// The folders inside `root` the torrent's files are in, deepest first.
    pub folders: Vec<PathBuf>,
}

/// The paths `meta`'s torrent owns in `root`, when the files `chosen` (by
/// [`crate::torrent::TorrentFile::index`]) were ever chosen.
pub fn owned_paths(root: &Path, meta: &TorrentMeta, chosen: &[u32]) -> OwnedPaths {
    let files = meta
        .user_files()
        .filter(|f| chosen.contains(&f.index))
        .map(|f| OwnedFile {
            path: root.join(f.relative_path()),
            max_len: f.length,
        })
        .collect();
    let parts = meta
        .info_hash
        .iter()
        .map(String::as_str)
        .chain(meta.info_hash_v2.iter().filter_map(|h| h.get(..40)))
        .map(|h| root.join(format!(".{h}.parts")))
        .collect();
    let mut folders = BTreeSet::new();
    for f in &meta.files {
        for depth in 1..f.path.len() {
            folders.insert(&f.path[..depth]);
        }
    }
    let mut folders: Vec<_> = folders.into_iter().collect();
    // Deepest first, so a folder comes after the folders inside it.
    folders.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    OwnedPaths {
        root: root.to_path_buf(),
        files,
        parts,
        folders: folders
            .into_iter()
            .map(|parts| root.join(parts.iter().collect::<PathBuf>()))
            .collect(),
    }
}

/// Why a path was left in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// A link, a reparse point, or not a plain file (or folder): never followed.
    NotPlain,
    /// Larger than the torrent's file: not the torrent's own.
    TooLarge,
    /// Reached through a folder that is a link or a reparse point: never followed.
    BehindLink,
    /// A folder with something else still in it.
    NotEmpty,
    /// Deleting failed, after waiting for the file to be closed.
    Failed(ErrorCode),
}

/// What [`delete_owned`] did. Paths that weren't there are in neither list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deletion {
    pub deleted: Vec<PathBuf>,
    pub kept: Vec<(PathBuf, Kept)>,
}

/// Deletes the paths a torrent owns: each file only if it is a plain file no larger than the
/// torrent says, inside `root` through plain folders; then each folder that is left empty. A file
/// that can't be deleted yet is tried again for up to `wait`, because Windows doesn't delete a file
/// still open, and libtorrent closes its files a moment after it lets go of a torrent.
pub fn delete_owned(owned: &OwnedPaths, wait: Duration) -> Deletion {
    let mut done = Deletion::default();
    match fs::symlink_metadata(&owned.root) {
        Ok(meta) if !is_link(&meta) && meta.is_dir() => {}
        Ok(_) => {
            done.kept.push((owned.root.clone(), Kept::NotPlain));
            return done;
        }
        // No folder, so nothing in it to delete.
        Err(_) => return done,
    }
    let parts = owned.parts.iter().map(|path| (path, u64::MAX));
    for (path, max_len) in owned
        .files
        .iter()
        .map(|f| (&f.path, f.max_len))
        .chain(parts)
    {
        let outcome = match reached_through_plain_folders(&owned.root, path) {
            Ok(()) => delete_file(path, max_len, wait),
            Err(kept) => Some(Err(kept)),
        };
        done.record(path, outcome);
    }
    for path in &owned.folders {
        let outcome = match reached_through_plain_folders(&owned.root, path) {
            Ok(()) => delete_folder(path),
            Err(kept) => Some(Err(kept)),
        };
        done.record(path, outcome);
    }
    done
}

impl Deletion {
    /// `None`: the path wasn't there.
    fn record(&mut self, path: &Path, outcome: Option<Result<(), Kept>>) {
        match outcome {
            Some(Ok(())) => self.deleted.push(path.to_path_buf()),
            Some(Err(kept)) => self.kept.push((path.to_path_buf(), kept)),
            None => {}
        }
    }
}

/// `Ok` when every folder between `root` and `path` is a plain folder, so `path` is inside `root`.
/// A folder missing on the way is fine: `path` isn't there either.
fn reached_through_plain_folders(root: &Path, path: &Path) -> Result<(), Kept> {
    let Ok(inside) = path.strip_prefix(root) else {
        return Err(Kept::BehindLink);
    };
    let mut folder = root.to_path_buf();
    let mut levels = inside.components().peekable();
    while let Some(level) = levels.next() {
        if levels.peek().is_none() {
            break;
        }
        folder.push(level);
        match fs::symlink_metadata(&folder) {
            Ok(meta) if is_link(&meta) || !meta.is_dir() => return Err(Kept::BehindLink),
            Ok(_) | Err(_) => {}
        }
    }
    Ok(())
}

fn delete_file(path: &Path, max_len: u64, wait: Duration) -> Option<Result<(), Kept>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if is_link(&meta) || !meta.is_file() {
        return Some(Err(Kept::NotPlain));
    }
    if meta.len() > max_len {
        return Some(Err(Kept::TooLarge));
    }
    let until = Instant::now() + wait;
    loop {
        match fs::remove_file(path) {
            Ok(()) => return Some(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) if Instant::now() >= until => return Some(Err(Kept::Failed(io_code(&e)))),
            Err(_) => thread::sleep(RETRY_EVERY),
        }
    }
}

fn delete_folder(path: &Path) -> Option<Result<(), Kept>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if is_link(&meta) || !meta.is_dir() {
        return Some(Err(Kept::NotPlain));
    }
    let empty = match fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(e) => return Some(Err(Kept::Failed(io_code(&e)))),
    };
    if !empty {
        return Some(Err(Kept::NotEmpty));
    }
    // `remove_dir` removes only an empty folder, even if something arrived since the check.
    match fs::remove_dir(path) {
        Ok(()) => Some(Ok(())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => Some(Err(Kept::NotEmpty)),
        Err(e) => Some(Err(Kept::Failed(io_code(&e)))),
    }
}

fn io_code(e: &io::Error) -> ErrorCode {
    match e.kind() {
        io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => ErrorCode::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::torrent::{TorrentFile, TorrentVersion};
    use std::fs;

    const V1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const V2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn file(index: u32, path: &[&str], length: u64, pad: bool) -> TorrentFile {
        TorrentFile {
            index,
            path: path.iter().map(|p| p.to_string()).collect(),
            length,
            pad,
        }
    }

    /// A hybrid multi-file torrent: `album/a.bin`, a padding file, `album/disc/b.bin` and
    /// `album/disc/deep/c.bin`.
    fn album() -> TorrentMeta {
        let files = vec![
            file(1, &["album", "a.bin"], 10, false),
            file(2, &["album", ".pad", "6"], 6, true),
            file(3, &["album", "disc", "b.bin"], 20, false),
            file(4, &["album", "disc", "deep", "c.bin"], 30, false),
        ];
        TorrentMeta {
            info_hash: Some(V1.into()),
            info_hash_v2: Some(V2.into()),
            version: TorrentVersion::Hybrid,
            name: "album".into(),
            files,
            total_length: 60,
            piece_length: 16,
            num_pieces: 4,
            private: false,
            multi_file: true,
            trackers: vec![],
        }
    }

    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("v2-torrent-files-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; len]).unwrap();
    }

    /// V1's `owned_paths_cover_every_file_folder_and_the_control_file`, for V2's rule: the chosen
    /// files only, and no aria2 control file.
    #[test]
    fn owned_paths_are_the_chosen_files_the_part_files_and_the_folders_deepest_first() {
        let root = Path::new("/downloads");
        let owned = owned_paths(root, &album(), &[1, 4]);
        assert_eq!(owned.root, root);
        assert_eq!(
            owned.files,
            vec![
                OwnedFile {
                    path: root.join("album").join("a.bin"),
                    max_len: 10
                },
                OwnedFile {
                    path: root.join("album").join("disc").join("deep").join("c.bin"),
                    max_len: 30
                },
            ]
        );
        assert_eq!(
            owned.parts,
            vec![
                root.join(format!(".{V1}.parts")),
                root.join(format!(".{}.parts", &V2[..40])),
            ]
        );
        // Every folder of the torrent, deepest first; never the root.
        assert_eq!(
            owned.folders,
            vec![
                root.join("album").join("disc").join("deep"),
                root.join("album").join(".pad"),
                root.join("album").join("disc"),
                root.join("album"),
            ]
        );
        // A padding file is never owned, even if named in the choice.
        let owned = owned_paths(root, &album(), &[2]);
        assert!(owned.files.is_empty());
    }

    /// V1's `deleting_a_completed_multi_file_torrent_removes_only_its_files` and
    /// `deleting_a_torrent_with_skipped_files_removes_the_engines_own_files`.
    #[test]
    fn deleting_removes_the_chosen_files_the_part_file_and_folders_left_empty() {
        let root = scratch("only-its-own");
        let album_dir = root.join("album");
        write(&album_dir.join("a.bin"), 10);
        write(&album_dir.join("disc").join("b.bin"), 20);
        write(&root.join(format!(".{V1}.parts")), 16);
        // Not chosen, so not the torrent's: a file of the same name the user had.
        write(&album_dir.join("disc").join("deep").join("c.bin"), 30);
        // Someone else's files in the same folders.
        write(&album_dir.join("notes.txt"), 3);
        write(&root.join("other.iso"), 5);

        let owned = owned_paths(&root, &album(), &[1, 3]);
        let done = delete_owned(&owned, Duration::ZERO);

        assert!(!album_dir.join("a.bin").exists());
        assert!(!album_dir.join("disc").join("b.bin").exists());
        assert!(!root.join(format!(".{V1}.parts")).exists());
        assert!(album_dir.join("disc").join("deep").join("c.bin").exists());
        assert!(album_dir.join("notes.txt").exists());
        assert!(root.join("other.iso").exists());
        assert!(root.exists());
        assert_eq!(
            done.deleted,
            vec![
                album_dir.join("a.bin"),
                album_dir.join("disc").join("b.bin"),
                root.join(format!(".{V1}.parts")),
            ]
        );
        assert_eq!(
            done.kept,
            vec![
                (album_dir.join("disc").join("deep"), Kept::NotEmpty),
                (album_dir.join("disc"), Kept::NotEmpty),
                (album_dir.clone(), Kept::NotEmpty),
            ]
        );

        // With the user's files gone, the torrent's empty folders go too.
        fs::remove_file(album_dir.join("notes.txt")).unwrap();
        fs::remove_file(album_dir.join("disc").join("deep").join("c.bin")).unwrap();
        let done = delete_owned(&owned, Duration::ZERO);
        assert!(!album_dir.exists());
        assert!(root.join("other.iso").exists());
        assert_eq!(
            done.deleted,
            vec![
                album_dir.join("disc").join("deep"),
                album_dir.join("disc"),
                album_dir.clone(),
            ]
        );
        assert!(done.kept.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_file_larger_than_the_torrent_says_is_kept() {
        let root = scratch("too-large");
        let path = root.join("album").join("a.bin");
        write(&path, 11);
        let done = delete_owned(&owned_paths(&root, &album(), &[1]), Duration::ZERO);
        assert!(path.exists());
        assert!(done.deleted.is_empty());
        assert_eq!(done.kept[0], (path, Kept::TooLarge));
        fs::remove_dir_all(&root).unwrap();
    }

    /// V1's `deletion_never_follows_links_out_of_the_download_folder`, with Windows junctions.
    #[test]
    fn deletion_never_follows_a_folder_link_out_of_the_torrents_folder() {
        let root = scratch("folder-link");
        let outside = scratch("folder-link-outside");
        write(&outside.join("b.bin"), 20);
        write(&root.join("album").join("a.bin"), 10);
        let link = root.join("album").join("disc");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // A junction needs no administrator rights, unlike a symbolic link.
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .output()
                .unwrap();
            assert!(made.status.success(), "{made:?}");
        }

        let done = delete_owned(&owned_paths(&root, &album(), &[1, 3]), Duration::ZERO);

        assert!(outside.join("b.bin").exists());
        assert!(!root.join("album").join("a.bin").exists());
        assert!(fs::symlink_metadata(&link).is_ok(), "the link itself stays");
        assert!(done.kept.contains(&(link.join("b.bin"), Kept::BehindLink)));
        assert!(done.kept.contains(&(link.clone(), Kept::NotPlain)));
        // Removing the junction itself, not what it points to.
        #[cfg(windows)]
        fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link).unwrap();
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    /// A link where the torrent's file should be: neither it nor what it points to is deleted.
    /// Creating a file link needs extra rights on Windows, so this part runs on Unix.
    #[cfg(unix)]
    #[test]
    fn deletion_never_follows_a_file_link() {
        let root = scratch("file-link");
        let outside = scratch("file-link-outside");
        write(&outside.join("secret"), 5);
        fs::create_dir_all(root.join("album")).unwrap();
        let link = root.join("album").join("a.bin");
        std::os::unix::fs::symlink(outside.join("secret"), &link).unwrap();
        let done = delete_owned(&owned_paths(&root, &album(), &[1]), Duration::ZERO);
        assert!(outside.join("secret").exists());
        assert!(fs::symlink_metadata(&link).is_ok());
        assert_eq!(done.kept[0], (link, Kept::NotPlain));
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    /// The root must itself be a plain folder: through a link to it, nothing is deleted.
    #[cfg(unix)]
    #[test]
    fn nothing_is_deleted_through_a_linked_root() {
        let real = scratch("linked-root-real");
        write(&real.join("album").join("a.bin"), 10);
        let root =
            std::env::temp_dir().join(format!("v2-torrent-files-root-{}", std::process::id()));
        let _ = fs::remove_file(&root);
        std::os::unix::fs::symlink(&real, &root).unwrap();
        let done = delete_owned(&owned_paths(&root, &album(), &[1]), Duration::ZERO);
        assert!(real.join("album").join("a.bin").exists());
        assert!(done.deleted.is_empty());
        assert_eq!(done.kept, vec![(root.clone(), Kept::NotPlain)]);
        fs::remove_file(&root).unwrap();
        fs::remove_dir_all(&real).unwrap();
    }

    /// Windows doesn't delete an open file: deleting waits until libtorrent has closed it, and
    /// reports it kept if it never is.
    #[cfg(windows)]
    #[test]
    fn deleting_waits_for_an_open_file_to_be_closed() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = scratch("open-file");
        let path = root.join("album").join("a.bin");
        write(&path, 10);
        // Shared with nobody, as a file another program holds without delete sharing.
        let open = || {
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path)
                .unwrap()
        };
        let owned = owned_paths(&root, &album(), &[1]);

        let held = open();
        let done = delete_owned(&owned, Duration::from_millis(300));
        assert!(path.exists());
        assert!(
            matches!(done.kept.first(), Some((p, Kept::Failed(_))) if *p == path),
            "{done:?}"
        );
        drop(held);

        let held = open();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let done = delete_owned(&owned, Duration::from_secs(5));
        closer.join().unwrap();
        assert!(!path.exists());
        assert_eq!(done.deleted[0], path);
        fs::remove_dir_all(&root).unwrap();
    }
}
