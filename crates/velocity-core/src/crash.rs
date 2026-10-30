//! Crash logs (#28's policy, DIAGNOSTICS.md): when the app panics, what happened is written to a
//! file in its own data folder, redacted, and kept there. Nothing is ever sent anywhere; only the
//! newest [`KEPT`] are kept.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fs_safety::is_link;
use crate::redact::redact;

/// How many crash logs are kept; the oldest go first.
pub const KEPT: usize = 5;
/// The longest crash log, in bytes: a panic message can't fill the disk.
pub const MAX_BYTES: usize = 64 * 1024;

/// Writes `report`, redacted and cut to [`MAX_BYTES`], as `crash-<now_ms>.txt` in `dir`, which
/// is created when missing; then deletes the oldest crash logs beyond [`KEPT`]. Returns the file.
pub fn write_crash_log(dir: &Path, report: &str, now_ms: u64) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut text = redact(report);
    if text.len() > MAX_BYTES {
        let mut cut = MAX_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    // Written beside, then renamed: a crash while writing leaves no half crash log.
    let path = dir.join(format!("crash-{now_ms}.txt"));
    let partial = dir.join(format!(".crash-{now_ms}.partial"));
    fs::write(&partial, text)?;
    fs::rename(&partial, &path)?;
    prune(dir)?;
    Ok(path)
}

/// When a file named `crash-<ms>.txt` was written; `None` for any other name.
fn written_at(name: &str) -> Option<u64> {
    name.strip_prefix("crash-")?
        .strip_suffix(".txt")?
        .parse()
        .ok()
}

/// Deletes the oldest crash logs beyond [`KEPT`]: plain files with a crash log's name only, never
/// a link or anything else in the folder.
fn prune(dir: &Path) -> io::Result<()> {
    let mut logs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        let name = entry.file_name();
        if let Some(at) = name.to_str().and_then(written_at) {
            if meta.is_file() && !is_link(&meta) {
                logs.push((at, entry.path()));
            }
        }
    }
    logs.sort();
    let extra = logs.len().saturating_sub(KEPT);
    for (_, path) in logs.into_iter().take(extra) {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("v2-crash-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_crash_log_is_written_redacted_in_its_own_folder() {
        let dir = folder("write");
        let path = write_crash_log(
            &dir,
            "panicked at fetch.rs:12: GET https://me:hunter2pass@example.com/a.zip",
            1_000,
        )
        .unwrap();
        assert_eq!(path, dir.join("crash-1000.txt"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("panicked at fetch.rs:12"), "{text}");
        assert!(!text.contains("hunter2pass"), "{text}");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with("crash-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temporary file is left: {leftovers:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_newest_crash_logs_are_kept_and_other_files_are_left_alone() {
        let dir = folder("keep");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), b"mine").unwrap();
        for at in 1..=(KEPT as u64 + 2) {
            write_crash_log(&dir, "boom", at * 1000).unwrap();
        }
        let mut names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "crash-3000.txt",
                "crash-4000.txt",
                "crash-5000.txt",
                "crash-6000.txt",
                "crash-7000.txt",
                "notes.txt"
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_huge_report_is_cut() {
        let dir = folder("cut");
        let path = write_crash_log(&dir, &"x".repeat(MAX_BYTES * 2), 1).unwrap();
        assert!(fs::metadata(&path).unwrap().len() <= MAX_BYTES as u64);
        fs::remove_dir_all(&dir).unwrap();
    }
}
