//! Test-only write faults (#74, PO decision): a test makes the engine's writes to one download's
//! part file fail as a full disk or a refused write would, after a chosen number of bytes, and
//! clears the fault again as a user freeing space would. Only the `test-faults` feature builds this,
//! and an optimised build refuses that feature, so a release can't contain it. No real full disk is
//! made (PO decision on #74).
//!
//! Faults are keyed by the download's file name, so tests running at once don't meet each other's.

use std::io;
use std::path::Path;
use std::sync::Mutex;

use velocity_core::naming::PART_FILE_SUFFIX;

/// A planned failure: `left` more bytes are written, then every write fails with `kind`.
struct Plan {
    part_name: String,
    left: u64,
    kind: io::ErrorKind,
}

static PLANS: Mutex<Vec<Plan>> = Mutex::new(Vec::new());

fn plans() -> std::sync::MutexGuard<'static, Vec<Plan>> {
    PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// From now on, writes to the part file of the download named `file_name` take `bytes` more bytes,
/// then fail with `kind`: `StorageFull` for a full disk, `PermissionDenied` for a refused write.
/// The write that crosses the limit lands its first bytes before it fails, as an interrupted write
/// does.
pub fn fail_writes_after(file_name: &str, bytes: u64, kind: io::ErrorKind) {
    let part_name = format!("{file_name}{PART_FILE_SUFFIX}");
    let mut plans = plans();
    plans.retain(|p| p.part_name != part_name);
    plans.push(Plan {
        part_name,
        left: bytes,
        kind,
    });
}

/// Writes to `file_name`'s part file succeed again, as after the user freed space.
pub fn clear(file_name: &str) {
    let part_name = format!("{file_name}{PART_FILE_SUFFIX}");
    plans().retain(|p| p.part_name != part_name);
}

/// How many of `len` bytes may be written to the part file at `path` now, and the error that
/// follows them when the write can't be whole.
pub(crate) fn allowed(path: &Path, len: usize) -> (usize, Option<io::Error>) {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return (len, None);
    };
    let mut plans = plans();
    let Some(plan) = plans.iter_mut().find(|p| p.part_name == name) else {
        return (len, None);
    };
    if plan.left >= len as u64 {
        plan.left -= len as u64;
        return (len, None);
    }
    let take = plan.left as usize;
    plan.left = 0;
    (take, Some(io::Error::from(plan.kind)))
}
