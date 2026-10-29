//! A download's own speed limit and connections (V1's `set_download_speed_limit` and
//! `set_download_connections`; #47, #49, #58). A change applies at once to a download the engine
//! has, and to the next start of one it doesn't; either way it is stored, so a restart keeps it.

use super::Manager;
use super::SensitiveOptions;
use crate::database::StoredOptions;
use crate::domain::ErrorCode;
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

    /// Saves the download's login, custom headers and SSH key in the system keychain, at the
    /// user's request (#70), so that after a restart they are read from there instead of asked
    /// for again. `InvalidState` for a download without any; `Unsupported` for a torrent;
    /// `KeychainUnavailable`, with nothing saved, when the keychain is missing or refuses.
    pub fn remember_login(&mut self, id: &str) -> Result<(), ErrorCode> {
        let d = self.downloads.get(id).ok_or(ErrorCode::NotFound)?;
        let job = d.direct().ok_or(ErrorCode::Unsupported)?;
        let login = SensitiveOptions {
            credentials: job.request.credentials.clone(),
            ssh_key: job.request.ssh_key.clone(),
            headers: job.request.headers.clone(),
        };
        if login == SensitiveOptions::default() {
            return Err(ErrorCode::InvalidState);
        }
        self.keychain.save(id, &login)?;
        if let Some(job) = self.downloads.get_mut(id).and_then(|d| d.direct_mut()) {
            job.login_saved = true;
        }
        self.store_options(id);
        Ok(())
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
        let options = StoredOptions {
            login_saved: job.login_saved,
            ..StoredOptions::of(&job.request)
        };
        if let Err(e) = self.database.update_options(id, &options) {
            self.storage_error = Some(e);
        }
    }
}
