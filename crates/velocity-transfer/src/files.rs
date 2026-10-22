//! The engine's file operations on part files (#43): created exclusively and sparse, reopened
//! without following links, and renamed into place without ever replacing an existing file.
//!
//! The only `unsafe` code here calls the operating system through `windows-sys` and `libc`
//! (PO-approved on #43), with a `SAFETY` note on each call.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// A new part file.
pub struct NewPart {
    pub file: File,
    /// Marked sparse (Windows), so writing far into it doesn't zero-fill the gap first. Unix file
    /// systems such as ext4 leave gaps empty without being asked.
    pub sparse: bool,
}

/// Creates a part file that doesn't exist yet: never over an existing file, never through a link.
/// On Windows it is marked sparse before any byte is written.
pub fn create_part(path: &Path) -> io::Result<NewPart> {
    let mut options = OpenOptions::new();
    // `create_new` fails on any existing name, a link included, so nothing is ever followed.
    options.read(true).write(true).create_new(true);
    let file = options.open(path)?;
    let sparse = mark_sparse(&file);
    Ok(NewPart { file, sparse })
}

/// Opens a part file this download owns for writing, without truncating it. A link, or anything
/// that isn't a plain file, is refused, never followed.
pub fn open_part(path: &Path) -> io::Result<File> {
    plain_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    no_follow(&mut options);
    let file = options.open(path)?;
    // Checked again on the open handle, in case the name changed in between.
    if !file.metadata()?.is_file() {
        return Err(not_plain(path));
    }
    Ok(file)
}

/// Removes a part file this download owns. A link, or anything that isn't a plain file, is
/// refused; nothing it points to is touched.
pub fn remove_part(path: &Path) -> io::Result<()> {
    plain_file(path)?;
    fs::remove_file(path)
}

fn not_plain(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} is a link or not a plain file", path.display()),
    )
}

/// `path` is a plain file, not a link (checked without following it).
fn plain_file(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(not_plain(path));
    }
    Ok(())
}

#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    // Opens a reparse point itself instead of what it points to; `open_part` then refuses it.
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

/// Marks `file` sparse, so that NTFS never zero-fills the gap in front of a write (#42's
/// measurement). Returns whether it worked: a file system without sparse files (FAT, exFAT) still
/// works, only slower, and the download reports it.
#[cfg(windows)]
fn mark_sparse(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let mut returned = 0u32;
    // SAFETY: the handle is the open file's own and outlives the call; FSCTL_SET_SPARSE takes no
    // input or output buffer (null with size 0 sets the sparse flag), and `returned` is a valid
    // place for the byte count. The call is synchronous (no OVERLAPPED).
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    ok != 0
}

/// Unix file systems such as ext4 leave gaps empty without being asked; nothing is marked.
#[cfg(not(windows))]
fn mark_sparse(_: &File) -> bool {
    false
}

/// Renames `from` to `to` only if nothing exists at `to`: an existing file is never replaced,
/// whatever appeared there meanwhile (`ErrorKind::AlreadyExists`). On Unix the folder is synced
/// afterwards, so the new name survives a power loss.
#[cfg(windows)]
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
    let (from_w, to_w) = (wide(from), wide(to));
    // SAFETY: both are NUL-terminated UTF-16 strings that live until the call returns. Without
    // MOVEFILE_REPLACE_EXISTING, MoveFileExW fails if `to` exists; MOVEFILE_WRITE_THROUGH makes it
    // return only once the rename is on disk.
    let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    rename_exclusive(from, to)?;
    // The rename is durable once its folder is synced.
    if let Some(folder) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        File::open(folder)?.sync_all()?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL byte in a path"))
    };
    let (from_c, to_c) = (c(from)?, c(to)?);
    // SAFETY: both are NUL-terminated C strings that live until the call returns; AT_FDCWD makes
    // them relative to the working directory, as `std::fs::rename` does.
    let r = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // A file system without RENAME_NOREPLACE: a hard link also refuses an existing name.
        Some(libc::EINVAL) | Some(libc::ENOSYS) => link_then_remove(from, to),
        _ => Err(e),
    }
}

/// Other Unix systems (macOS is #83): a hard link refuses an existing name, then the old name goes.
#[cfg(all(unix, not(target_os = "linux")))]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    link_then_remove(from, to)
}

#[cfg(unix)]
fn link_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("v2-files-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_rename_moves_the_file_when_the_name_is_free() {
        let dir = Dir::new("rename");
        let (from, to) = (dir.0.join("a.vdmpart"), dir.0.join("a.zip"));
        std::fs::write(&from, b"done").unwrap();
        rename_no_replace(&from, &to).expect("renamed");
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"done");
    }

    #[test]
    fn a_rename_never_replaces_an_existing_file() {
        let dir = Dir::new("noreplace");
        let (from, to) = (dir.0.join("a.vdmpart"), dir.0.join("a.zip"));
        std::fs::write(&from, b"download").unwrap();
        std::fs::write(&to, b"keep me").unwrap();
        let e = rename_no_replace(&from, &to).expect_err("refused");
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}");
        assert_eq!(std::fs::read(&to).unwrap(), b"keep me");
        assert_eq!(std::fs::read(&from).unwrap(), b"download");
    }

    #[test]
    fn a_part_file_is_created_only_where_none_exists() {
        let dir = Dir::new("create");
        let path = dir.0.join("a.vdmpart");
        let mut part = create_part(&path).expect("created");
        part.file.write_all(b"x").unwrap();
        drop(part);
        let e = create_part(&path).err().expect("refused");
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"x",
            "the first one is untouched"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_part_file_is_sparse_on_windows() {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
        let dir = Dir::new("sparse");
        let path = dir.0.join("a.vdmpart");
        let part = create_part(&path).expect("created");
        assert!(part.sparse);
        drop(part);
        let attributes = std::fs::metadata(&path).unwrap().file_attributes();
        assert_ne!(
            attributes & FILE_ATTRIBUTE_SPARSE_FILE,
            0,
            "{attributes:#x}"
        );
    }

    #[test]
    fn an_owned_part_file_is_reopened_without_truncating_and_removed() {
        let dir = Dir::new("reopen");
        let path = dir.0.join("a.vdmpart");
        std::fs::write(&path, b"0123456789").unwrap();
        drop(open_part(&path).expect("reopened"));
        assert_eq!(std::fs::read(&path).unwrap(), b"0123456789");
        remove_part(&path).expect("removed");
        assert!(!path.exists());
        assert_eq!(
            open_part(&path).err().map(|e| e.kind()),
            Some(io::ErrorKind::NotFound)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_a_part_file_is_never_followed() {
        let dir = Dir::new("link");
        let target = dir.0.join("someone-elses.bin");
        std::fs::write(&target, b"theirs").unwrap();
        let path = dir.0.join("a.vdmpart");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(open_part(&path).is_err());
        assert!(remove_part(&path).is_err());
        assert!(create_part(&path).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs", "untouched");
    }
}
