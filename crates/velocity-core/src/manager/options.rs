//! A download's own speed limit and connections (V1's `set_download_speed_limit` and
//! `set_download_connections`; #47, #49, #58). A change applies at once to a download the engine
//! has, and to the next start of one it doesn't; either way it is stored, so a restart keeps it.

use super::Manager;
use crate::database::StoredOptions;
use crate::domain::{ConfirmKind, DownloadStatus, ErrorCode};
use crate::transfer::{Limits, TransferId};

impl Manager {
    /// Sets the download's own speed limit, which holds alongside the global one: the lower
    /// applies. `InvalidInput`, with nothing sent, for a limit below 1 KiB/s; `InvalidState` for a
    /// finished download.
    pub fn set_download_limits(&mut self, id: &str, limits: Limits) -> Result<(), ErrorCode> {
        limits.check()?;
        if self.torrent_job(id).is_ok() {
            return self.set_torrent_download_limit(id, limits.max_download_bps.unwrap_or(0));
        }
        if let Some(transfer) = self.unfinished_transfer(id)? {
            self.engine.set_limits(transfer, limits)?;
        }
        let d = self.downloads.get_mut(id).expect("known");
        d.direct_mut().expect("a direct download").request.limits = limits;
        d.show_settings();
        self.store_options(id);
        Ok(())
    }

    /// Asks for `connections` for the download and returns how many apply: what the engine allows
    /// for one it has, and at most the profile's cap for one it doesn't have yet. `InvalidInput`
    /// for none; `InvalidState` for a finished download.
    pub fn set_download_connections(
        &mut self,
        id: &str,
        connections: u32,
    ) -> Result<u32, ErrorCode> {
        if connections == 0 {
            return Err(ErrorCode::InvalidInput);
        }
        // libtorrent chooses a torrent's peers itself.
        if self.torrent_job(id).is_ok() {
            return Err(ErrorCode::Unsupported);
        }
        let applied = match self.unfinished_transfer(id)? {
            Some(transfer) => self.engine.set_connections(transfer, connections)?,
            None => connections.min(self.connections_for_new_downloads()),
        };
        let d = self.downloads.get_mut(id).expect("known");
        d.direct_mut()
            .expect("a direct download")
            .request
            .max_connections = applied;
        d.show_settings();
        self.store_options(id);
        Ok(applied)
    }

    /// Allows what a download stopped at for the user's confirmation, and retries it (#67): a
    /// redirect from `https` to `http`, or a link from the browser that reaches this computer or
    /// a private network (ADR-0006). The answer is stored with the download, so it holds after a
    /// restart, and for this download only. `InvalidState`, with nothing changed, unless the
    /// download failed waiting for that confirmation.
    pub fn allow_download(&mut self, id: &str, kind: ConfirmKind) -> Result<(), ErrorCode> {
        let d = self.downloads.get_mut(id).ok_or(ErrorCode::NotFound)?;
        let waiting = d.record.status == DownloadStatus::Failed
            && d.record
                .error
                .as_ref()
                .and_then(|e| e.confirm.as_ref())
                .is_some_and(|c| c.kind() == kind);
        if !waiting {
            return Err(ErrorCode::InvalidState);
        }
        let trust = &mut d
            .direct_mut()
            .ok_or(ErrorCode::InvalidState)?
            .request
            .trust;
        match kind {
            ConfirmKind::InsecureRedirect => trust.insecure_redirect_confirmed = true,
            ConfirmKind::LocalAddress => trust.local_address_confirmed = true,
        }
        self.store_options(id);
        self.retry(id)
    }

    /// The transfer of an unfinished download, if the engine has it.
    fn unfinished_transfer(&self, id: &str) -> Result<Option<TransferId>, ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        if d.record.status.is_terminal() {
            return Err(ErrorCode::InvalidState);
        }
        Ok(d.transfer())
    }

    fn store_options(&mut self, id: &str) {
        let Some(job) = self.downloads[id].direct() else {
            return;
        };
        let options = StoredOptions::of(&job.request);
        if let Err(e) = self.database.update_options(id, &options) {
            self.storage_error = Some(e);
        }
    }
}
