//! Telling links apart from plain files and folders, without following them (AGENTS.md, "File
//! safety"): Metalink folders (#54) and torrent deletion (#62) refuse anything that could lead
//! outside the folder the user chose.

use std::fs::Metadata;

/// Whether `meta`, read with `symlink_metadata`, is a link: a symbolic link, or on Windows any
/// reparse point, such as a junction or a mount point.
pub(crate) fn is_link(meta: &Metadata) -> bool {
    meta.file_type().is_symlink() || reparse_point(meta)
}

#[cfg(windows)]
fn reparse_point(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT: junctions and mount points as well as symbolic links.
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Unix has no reparse points; its links are caught by `is_symlink`.
#[cfg(not(windows))]
fn reparse_point(_meta: &Metadata) -> bool {
    false
}
