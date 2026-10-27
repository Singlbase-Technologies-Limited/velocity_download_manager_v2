//! Where a download is saved (#31): the folder from the user, the category or the settings, and
//! what is already there. Checks paths by their text only; following links and the other file
//! system checks are #72's.

use std::path::PathBuf;

use crate::domain::ErrorCode;

/// The longest folder path, in bytes, accepted from the user.
pub const MAX_FOLDER_BYTES: usize = 4096;

/// `path` as a folder to save into, or `InvalidPath` when it is empty, too long, relative, has a
/// control character or a `..` (V1's `validate_directory_syntax`). The folder needn't exist.
pub fn check_folder(path: &str) -> Result<PathBuf, ErrorCode> {
    // Both separators on every platform: `a\..\b` is refused on Linux too.
    let parent = path.split(['/', '\\']).any(|part| part == "..");
    let folder = PathBuf::from(path);
    if path.is_empty()
        || path.len() > MAX_FOLDER_BYTES
        || path.chars().any(char::is_control)
        || parent
        || !folder.is_absolute()
    {
        return Err(ErrorCode::InvalidPath);
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute folder on this platform.
    fn absolute(rest: &str) -> String {
        if cfg!(windows) {
            format!("C:\\Users\\me\\{rest}")
        } else {
            format!("/home/me/{rest}")
        }
    }

    #[test]
    fn a_folder_must_be_a_plain_absolute_path() {
        let ok = absolute("Downloads");
        assert_eq!(check_folder(&ok), Ok(PathBuf::from(&ok)));
        assert_eq!(
            check_folder(&absolute("Unicode ✓ résumé")),
            Ok(PathBuf::from(absolute("Unicode ✓ résumé")))
        );
        let too_long = absolute(&"x".repeat(MAX_FOLDER_BYTES));
        for bad in [
            String::new(),
            "relative/path".to_owned(),
            "Downloads".to_owned(),
            absolute("a/../b"),
            absolute("a\\..\\b"),
            absolute("tab\there"),
            absolute("nul\0"),
            too_long,
        ] {
            assert_eq!(check_folder(&bad), Err(ErrorCode::InvalidPath), "{bad:?}");
        }
    }
}
