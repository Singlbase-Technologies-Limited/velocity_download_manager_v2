//! Commands over several downloads at once (#58, V1's batch commands): each id is tried, in
//! order, and the result says which were done and why the others weren't. The ids come from the
//! UI, so their number and length are checked first.

use super::Manager;
use crate::domain::{BatchFailure, BatchResult, ErrorCode, KeptFile, KeptReason};
use crate::torrent_files::Kept;

/// The most ids one command takes.
pub const MAX_BATCH: usize = 10_000;
/// The longest id: the manager's are `d-` and a number.
const MAX_ID_LEN: usize = 64;

/// `InvalidInput` for more than [`MAX_BATCH`] ids, or one longer than [`MAX_ID_LEN`].
fn check_ids(ids: &[String]) -> Result<(), ErrorCode> {
    if ids.len() > MAX_BATCH || ids.iter().any(|id| id.len() > MAX_ID_LEN) {
        return Err(ErrorCode::InvalidInput);
    }
    Ok(())
}

impl Manager {
    /// Runs `op` for each id, such as [`Manager::pause`]. `InvalidInput`, with nothing done, for
    /// too many ids or an overlong one.
    pub fn batch(
        &mut self,
        ids: &[String],
        mut op: impl FnMut(&mut Manager, &str) -> Result<(), ErrorCode>,
    ) -> Result<BatchResult, ErrorCode> {
        check_ids(ids)?;
        let mut result = BatchResult::default();
        for id in ids {
            match op(self, id) {
                Ok(()) => result.done.push(id.clone()),
                Err(code) => result.failed.push(BatchFailure {
                    id: id.clone(),
                    code,
                }),
            }
        }
        Ok(result)
    }

    /// Removes each download ([`Manager::remove`]), with the files kept and why.
    pub fn remove_many(
        &mut self,
        ids: &[String],
        delete_files: bool,
    ) -> Result<BatchResult, ErrorCode> {
        let mut kept = Vec::new();
        let mut result = self.batch(ids, |m, id| {
            for (path, why) in m.remove(id, delete_files)? {
                kept.push(kept_file(&path, why));
            }
            Ok(())
        })?;
        result.kept = kept;
        Ok(result)
    }
}

/// A file left in place, as the UI is told it.
pub(super) fn kept_file(path: &std::path::Path, why: Kept) -> KeptFile {
    let (reason, code) = match why {
        Kept::NotPlain => (KeptReason::NotPlain, None),
        Kept::TooLarge => (KeptReason::TooLarge, None),
        Kept::BehindLink => (KeptReason::BehindLink, None),
        Kept::NotEmpty => (KeptReason::NotEmpty, None),
        Kept::Failed(code) => (KeptReason::Failed, Some(code)),
    };
    KeptFile {
        path: path.to_string_lossy().into_owned(),
        reason,
        code,
    }
}
