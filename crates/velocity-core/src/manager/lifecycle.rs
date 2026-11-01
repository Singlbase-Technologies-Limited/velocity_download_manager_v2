//! What each download did, in the app's log (#251): added, started, paused, retried, stalled,
//! failed, completed and removed, and its connection count. Each download remembers what was last
//! logged for it, and a change to its record becomes at most a few lines. A link's path, query and
//! login are never logged, only its protocol and host (PO decision on #251), and every line is
//! redacted as it is kept ([`crate::logs::AppLog`]).

use crate::domain::{DownloadKind, DownloadRecord, DownloadStatus, FailureClass, TransferError};

/// The fewest milliseconds between two connection lines of one download.
const CONNECTIONS_EVERY_MS: u64 = 30_000;
/// The most connection lines one run of a download makes, so a long download that keeps
/// changing its connections can't push the rest out of the log's 1,000 lines.
const CONNECTION_LINES: u32 = 20;

/// What was last logged for one download.
pub(super) struct Logged {
    status: DownloadStatus,
    /// The time of the retry last logged, so a new one is told from the same one reported again.
    retry_at_ms: Option<u64>,
    /// Retries since it last started.
    retries: u32,
    connections: u32,
    connections_at_ms: Option<u64>,
    /// Connection lines since it last started.
    connection_lines: u32,
}

/// One line, at warn for what went wrong and at info for the rest.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub warn: bool,
    pub text: String,
}

impl Line {
    fn info(text: String) -> Self {
        Self { warn: false, text }
    }

    fn warn(text: String) -> Self {
        Self { warn: true, text }
    }

    /// Writes it to the app's log.
    pub(super) fn emit(&self) {
        if self.warn {
            tracing::warn!("{}", self.text);
        } else {
            tracing::info!("{}", self.text);
        }
    }
}

impl Logged {
    /// Starts from `record` as it is, without a line: a download read back at start-up isn't news.
    pub(super) fn new(record: &DownloadRecord) -> Self {
        Self {
            status: record.status,
            retry_at_ms: record.retry_at_ms,
            retries: 0,
            connections: record.progress.connections,
            connections_at_ms: None,
            connection_lines: 0,
        }
    }

    /// The lines `record`'s change since the last call makes, remembered so each is made once.
    pub(super) fn lines(&mut self, record: &DownloadRecord, now_ms: u64) -> Vec<Line> {
        let mut lines = Vec::new();
        let id = &record.id;
        let at = bytes(record);
        let before = std::mem::replace(&mut self.status, record.status);
        let new_retry = record.status == DownloadStatus::Retrying
            && (before != DownloadStatus::Retrying || record.retry_at_ms != self.retry_at_ms);
        self.retry_at_ms = record.retry_at_ms;
        if new_retry {
            self.retries += 1;
            let wait = record
                .retry_at_ms
                .map(|t| format!(" in {} s", t.saturating_sub(now_ms).div_ceil(1000)))
                .unwrap_or_default();
            lines.push(Line::warn(format!(
                "download {id} retry {}{wait} at {at}{}",
                self.retries,
                why(record.error.as_ref())
            )));
        } else if before != record.status {
            if let Some(line) = status_line(before, record, &at) {
                lines.push(line);
            }
        }
        if working(record.status) && !on_its_way(before) {
            // A new run: its retries and connection lines count again.
            self.retries = 0;
            self.connection_lines = 0;
        }
        self.connection_line(record, now_ms, &mut lines);
        lines
    }

    /// A direct download's connection count, when it changed: at most every
    /// [`CONNECTIONS_EVERY_MS`] and [`CONNECTION_LINES`] times a run. A torrent's are its peers,
    /// which change all the time, and aren't logged.
    fn connection_line(&mut self, record: &DownloadRecord, now_ms: u64, lines: &mut Vec<Line>) {
        let Some(detail) = &record.progress.connection_detail else {
            return;
        };
        let count = record.progress.connections;
        let due = self
            .connections_at_ms
            .is_none_or(|at| now_ms.saturating_sub(at) >= CONNECTIONS_EVERY_MS);
        if record.status != DownloadStatus::Downloading
            || count == self.connections
            || !due
            || self.connection_lines >= CONNECTION_LINES
        {
            return;
        }
        self.connections = count;
        self.connections_at_ms = Some(now_ms);
        self.connection_lines += 1;
        let id = &record.id;
        let noun = if count == 1 {
            "connection"
        } else {
            "connections"
        };
        let limit = detail
            .limited_by
            .map(|l| format!(", limited by {l:?}"))
            .unwrap_or_default();
        lines.push(Line::info(format!(
            "download {id} uses {count} {noun} (allowed {} of {}{limit})",
            detail.allowed, detail.requested
        )));
        if self.connection_lines == CONNECTION_LINES {
            lines.push(Line::info(format!(
                "download {id}: further connection changes aren't logged until it starts again"
            )));
        }
    }
}

/// The line for a change of status other than a retry, or `None` for one that says nothing new:
/// a running download that connects or gets going again after a retry or a stall, whose line said
/// why it stopped.
fn status_line(before: DownloadStatus, record: &DownloadRecord, at: &str) -> Option<Line> {
    let id = &record.id;
    let why = why(record.error.as_ref());
    Some(match record.status {
        DownloadStatus::Connecting
        | DownloadStatus::Downloading
        | DownloadStatus::FetchingMetadata => {
            if on_its_way(before) {
                return None;
            }
            Line::info(format!("download {id} started at {at}"))
        }
        DownloadStatus::Queued => Line::info(format!("download {id} queued")),
        DownloadStatus::Paused => Line::info(format!("download {id} paused at {at}")),
        DownloadStatus::Verifying => Line::info(format!("download {id} verifying")),
        DownloadStatus::ChoosingFiles => {
            Line::info(format!("download {id} waits for its files to be chosen"))
        }
        DownloadStatus::Seeding => Line::info(format!("download {id} seeding")),
        DownloadStatus::Completed => Line::info(format!(
            "download {id} completed: {} bytes",
            record.progress.completed_bytes
        )),
        DownloadStatus::Removed => Line::info(format!("download {id} removed")),
        DownloadStatus::Stalled => Line::warn(format!("download {id} stalled at {at}{why}")),
        DownloadStatus::Failed => Line::warn(format!("download {id} failed at {at}{why}")),
        // Told by [`Logged::lines`], which counts it.
        DownloadStatus::Retrying => return None,
        DownloadStatus::Unknown => Line::warn(format!(
            "download {id} is in a status Velocity doesn't know"
        )),
    })
}

/// Getting its bytes now.
fn working(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Connecting | DownloadStatus::Downloading | DownloadStatus::FetchingMetadata
    )
}

/// Working, or held up on the way: a retry or a stall is part of the same run.
fn on_its_way(status: DownloadStatus) -> bool {
    working(status) || matches!(status, DownloadStatus::Retrying | DownloadStatus::Stalled)
}

/// How far it got: `1048576 of 4194304 bytes`.
fn bytes(record: &DownloadRecord) -> String {
    let done = record.progress.completed_bytes;
    match record.progress.total_bytes {
        Some(total) => format!("{done} of {total} bytes"),
        None => format!("{done} bytes of an unknown size"),
    }
}

/// `: message [class, HTTP status]` for an error, with the engine's technical detail in its
/// message; nothing without one.
fn why(error: Option<&TransferError>) -> String {
    let Some(e) = error else {
        return String::new();
    };
    let class = match e.class {
        FailureClass::Transient => "transient",
        FailureClass::Throttled => "throttled",
        FailureClass::Permanent => "permanent",
    };
    let status = e
        .http_status
        .map(|s| format!(", HTTP {s}"))
        .unwrap_or_default();
    format!(": {} [{class}{status}]", e.message)
}

impl super::Download {
    /// Logs what changed since it was last looked at. The first look only remembers it: a
    /// download read back at start-up says nothing until it changes.
    pub(super) fn note(&mut self, now_ms: u64) {
        match &mut self.logged {
            Some(logged) => logged
                .lines(&self.record, now_ms)
                .iter()
                .for_each(Line::emit),
            None => self.logged = Some(Logged::new(&self.record)),
        }
    }

    /// Logs that it was just added, and remembers it from there.
    pub(super) fn note_added(&mut self) {
        added(&self.record).emit();
        self.logged = Some(Logged::new(&self.record));
    }
}

/// The line for a download just added: its id, its protocol and its link's host, never the rest
/// of the link (PO decision on #251).
pub(super) fn added(record: &DownloadRecord) -> Line {
    let url = url::Url::parse(&record.source_url).ok();
    let host = url
        .as_ref()
        .and_then(|u| u.host_str().map(|h| (u.scheme(), h)));
    let kind = match record.kind {
        DownloadKind::Http => "http",
        DownloadKind::Ftp => "ftp",
        DownloadKind::Sftp => "sftp",
        DownloadKind::Metalink => "metalink",
        DownloadKind::Torrent => "torrent",
        DownloadKind::Magnet => "magnet",
    };
    let mut parts = Vec::new();
    // The scheme says the protocol of a direct link; a Metalink file or a torrent says its kind.
    let direct = matches!(
        record.kind,
        DownloadKind::Http | DownloadKind::Ftp | DownloadKind::Sftp
    );
    if !direct || host.is_none() {
        parts.push(kind);
    }
    if let Some((scheme, host)) = host {
        parts.push(scheme);
        parts.push(host);
    }
    Line::info(format!(
        "download {} added ({})",
        record.id,
        parts.join(", ")
    ))
}

/// The line for a download that left the list (#264): with its files deleted when they were,
/// and a warning naming each file that was kept, and why.
pub(super) fn removed(
    id: &str,
    delete_files: bool,
    kept: &[(std::path::PathBuf, crate::torrent_files::Kept)],
) -> Line {
    if !kept.is_empty() {
        // The file's name only: its folder is the user's.
        let files = kept
            .iter()
            .map(|(path, why)| {
                let name = path
                    .file_name()
                    .map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy());
                format!("{name} ({why:?})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Line::warn(format!(
            "download {id} removed from the list, but {} of its files were kept: {files}",
            kept.len()
        ));
    }
    if delete_files {
        Line::info(format!(
            "download {id} removed from the list, and its files deleted"
        ))
    } else {
        Line::info(format!("download {id} removed from the list"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ConnectionDetail, ConnectionLimit, DownloadKind, FailureClass, Progress, TransferError,
    };

    const T0: u64 = 1_792_600_000_000;

    fn record(status: DownloadStatus) -> DownloadRecord {
        DownloadRecord {
            status,
            progress: Progress {
                completed_bytes: 0,
                total_bytes: Some(4_194_304),
                download_speed_bps: 0,
                upload_speed_bps: 0,
                connections: 0,
                connection_detail: None,
            },
            error: None,
            retry_at_ms: None,
            ..DownloadRecord::sample()
        }
    }

    fn texts(lines: Vec<Line>) -> Vec<String> {
        lines.into_iter().map(|l| l.text).collect()
    }

    fn info(text: &str) -> Line {
        Line {
            warn: false,
            text: text.to_owned(),
        }
    }

    fn warn(text: &str) -> Line {
        Line {
            warn: true,
            text: text.to_owned(),
        }
    }

    fn downloading(completed_bytes: u64, connections: u32) -> DownloadRecord {
        let mut r = record(DownloadStatus::Downloading);
        r.progress.completed_bytes = completed_bytes;
        r.progress.connections = connections;
        r.progress.connection_detail = Some(ConnectionDetail {
            requested: 8,
            allowed: 8,
            limited_by: None,
        });
        r
    }

    fn retrying(attempt_at_ms: u64) -> DownloadRecord {
        let mut r = record(DownloadStatus::Retrying);
        r.progress.completed_bytes = 1_048_576;
        r.retry_at_ms = Some(attempt_at_ms);
        r.error = Some(TransferError::new(
            FailureClass::Transient,
            None,
            "Velocity couldn't connect to the server (libcurl error 28: Failed to connect to \
             example.com port 443 after 21073 ms)",
        ));
        r
    }

    #[test]
    fn an_added_download_names_its_protocol_and_host_but_not_its_path_or_query() {
        let mut r = record(DownloadStatus::Queued);
        r.source_url = "https://me:pw@cdn.example.com/t0ken-in-the-path/a.zip?sig=abc".to_owned();
        assert_eq!(
            added(&r),
            info("download d-0001 added (https, cdn.example.com)")
        );
        r.kind = DownloadKind::Magnet;
        r.source_url = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned();
        assert_eq!(added(&r), info("download d-0001 added (magnet)"));
        r.kind = DownloadKind::Torrent;
        r.source_url = "https://tracker.example.org/t/1.torrent".to_owned();
        assert_eq!(
            added(&r),
            info("download d-0001 added (torrent, https, tracker.example.org)")
        );
        r.source_url = "C:\\Users\\me\\a.torrent".to_owned();
        assert_eq!(added(&r), info("download d-0001 added (torrent)"));
    }

    #[test]
    fn a_download_read_back_at_start_up_makes_no_line_until_it_changes() {
        let r = record(DownloadStatus::Completed);
        let mut logged = Logged::new(&r);
        assert!(logged.lines(&r, T0).is_empty());
    }

    #[test]
    fn its_life_is_logged_once_per_change_with_how_far_it_got() {
        let mut logged = Logged::new(&record(DownloadStatus::Queued));
        let mut lines = Vec::new();
        let mut step = |r: DownloadRecord| lines.extend(logged.lines(&r, T0));
        step(record(DownloadStatus::Connecting));
        step(downloading(1_048_576, 0));
        step(downloading(2_097_152, 0));
        step(record(DownloadStatus::Paused).with_bytes(2_097_152));
        step(record(DownloadStatus::Queued).with_bytes(2_097_152));
        step(downloading(2_097_152, 0));
        step(record(DownloadStatus::Verifying).with_bytes(4_194_304));
        step(record(DownloadStatus::Completed).with_bytes(4_194_304));
        step(record(DownloadStatus::Removed).with_bytes(4_194_304));
        assert_eq!(
            lines,
            [
                info("download d-0001 started at 0 of 4194304 bytes"),
                info("download d-0001 paused at 2097152 of 4194304 bytes"),
                info("download d-0001 queued"),
                info("download d-0001 started at 2097152 of 4194304 bytes"),
                info("download d-0001 verifying"),
                info("download d-0001 completed: 4194304 bytes"),
                info("download d-0001 removed"),
            ]
        );
    }

    #[test]
    fn a_size_not_known_yet_is_said() {
        let mut logged = Logged::new(&record(DownloadStatus::Queued));
        let mut r = record(DownloadStatus::Connecting);
        r.progress.total_bytes = None;
        assert_eq!(
            texts(logged.lines(&r, T0)),
            ["download d-0001 started at 0 bytes of an unknown size"]
        );
    }

    #[test]
    fn each_retry_is_one_warning_with_the_failure_its_detail_and_the_wait() {
        let mut logged = Logged::new(&downloading(1_048_576, 4));
        let mut lines = Vec::new();
        for n in 0..10 {
            let now = T0 + n * 60_000;
            lines.extend(logged.lines(&retrying(now + 11_000), now));
            // Back at it: the retry line said why it stopped, so this one is quiet.
            lines.extend(logged.lines(&record(DownloadStatus::Connecting), now + 11_000));
        }
        assert_eq!(lines.len(), 10, "{lines:?}");
        assert!(lines.iter().all(|l| l.warn));
        assert_eq!(
            lines[0].text,
            "download d-0001 retry 1 in 11 s at 1048576 of 4194304 bytes: Velocity couldn't \
             connect to the server (libcurl error 28: Failed to connect to example.com port 443 \
             after 21073 ms) [transient]"
        );
        assert!(lines[9]
            .text
            .starts_with("download d-0001 retry 10 in 11 s"));
    }

    #[test]
    fn a_new_retry_time_while_still_retrying_is_another_retry() {
        let mut logged = Logged::new(&downloading(0, 1));
        assert_eq!(logged.lines(&retrying(T0 + 5_000), T0).len(), 1);
        assert!(logged.lines(&retrying(T0 + 5_000), T0 + 100).is_empty());
        let again = texts(logged.lines(&retrying(T0 + 20_000), T0 + 5_000));
        assert!(
            again[0].starts_with("download d-0001 retry 2 in 15 s"),
            "{again:?}"
        );
    }

    #[test]
    fn retries_count_again_from_one_after_the_user_starts_it_again() {
        let mut logged = Logged::new(&downloading(0, 1));
        logged.lines(&retrying(T0 + 5_000), T0);
        logged.lines(&record(DownloadStatus::Paused), T0);
        logged.lines(&record(DownloadStatus::Queued), T0);
        logged.lines(&record(DownloadStatus::Connecting), T0);
        let again = texts(logged.lines(&retrying(T0 + 5_000), T0));
        assert!(
            again[0].starts_with("download d-0001 retry 1 in 5 s"),
            "{again:?}"
        );
    }

    #[test]
    fn a_failure_and_a_stall_are_warnings_with_the_detail() {
        let mut logged = Logged::new(&downloading(1_048_576, 4));
        let mut stalled = record(DownloadStatus::Stalled).with_bytes(1_048_576);
        stalled.error = Some(TransferError::new(
            FailureClass::Transient,
            None,
            "No data received",
        ));
        let mut failed = record(DownloadStatus::Failed).with_bytes(1_048_576);
        failed.error = Some(TransferError::new(
            FailureClass::Permanent,
            Some(403),
            "The server refused the download",
        ));
        let mut lines = logged.lines(&stalled, T0);
        // A stall that ends is quiet, as the end of a retry is.
        lines.extend(logged.lines(&downloading(1_048_576, 4), T0));
        lines.extend(logged.lines(&failed, T0));
        assert_eq!(
            lines,
            [
                warn("download d-0001 stalled at 1048576 of 4194304 bytes: No data received [transient]"),
                warn(
                    "download d-0001 failed at 1048576 of 4194304 bytes: The server refused the \
                     download [permanent, HTTP 403]"
                ),
            ]
        );
    }

    #[test]
    fn connection_changes_are_logged_with_what_holds_them_back() {
        let mut logged = Logged::new(&record(DownloadStatus::Queued));
        logged.lines(&downloading(0, 0), T0);
        let mut r = downloading(0, 2);
        r.progress.connection_detail = Some(ConnectionDetail {
            requested: 8,
            allowed: 2,
            limited_by: Some(ConnectionLimit::ServerThrottling),
        });
        assert_eq!(
            texts(logged.lines(&r, T0)),
            ["download d-0001 uses 2 connections (allowed 2 of 8, limited by ServerThrottling)"]
        );
        assert!(logged.lines(&r, T0 + 60_000).is_empty());
        assert_eq!(
            texts(logged.lines(&downloading(0, 1), T0 + 60_000)),
            ["download d-0001 uses 1 connection (allowed 8 of 8)"]
        );
    }

    #[test]
    fn connection_changes_are_logged_at_most_every_30_seconds() {
        let mut logged = Logged::new(&downloading(0, 8));
        assert_eq!(logged.lines(&downloading(0, 4), T0).len(), 1);
        assert!(logged.lines(&downloading(0, 3), T0 + 29_999).is_empty());
        // The change held back is still news once the time has passed.
        assert_eq!(
            texts(logged.lines(&downloading(0, 3), T0 + 30_000)),
            ["download d-0001 uses 3 connections (allowed 8 of 8)"]
        );
    }

    #[test]
    fn connection_changes_stop_after_20_lines_until_it_starts_again() {
        let mut logged = Logged::new(&downloading(0, 1));
        let mut lines = Vec::new();
        for n in 0..100u32 {
            let now = T0 + u64::from(n) * 30_000;
            lines.extend(texts(logged.lines(&downloading(0, 2 + n % 2), now)));
        }
        assert_eq!(lines.len(), 21, "{lines:?}");
        assert_eq!(
            lines[20],
            "download d-0001: further connection changes aren't logged until it starts again"
        );
        logged.lines(&record(DownloadStatus::Paused), T0);
        logged.lines(&downloading(0, 0), T0);
        assert_eq!(logged.lines(&downloading(0, 5), T0 + 9_000_000).len(), 1);
    }

    #[test]
    fn a_torrent_s_peer_count_is_not_logged_as_connections() {
        let mut r = record(DownloadStatus::Downloading);
        r.kind = DownloadKind::Magnet;
        let mut logged = Logged::new(&r);
        r.progress.connections = 30;
        assert!(logged.lines(&r, T0).is_empty());
    }

    #[test]
    fn a_download_that_leaves_the_list_says_so_and_whether_its_files_went() {
        use crate::torrent_files::Kept;
        use std::path::PathBuf;
        assert_eq!(
            removed("d-0006", false, &[]),
            info("download d-0006 removed from the list")
        );
        assert_eq!(
            removed("d-0006", true, &[]),
            info("download d-0006 removed from the list, and its files deleted")
        );
        let kept = [
            (
                PathBuf::from("C:/Downloads/ubuntu.iso"),
                Kept::Failed(crate::domain::ErrorCode::Io),
            ),
            (PathBuf::from("C:/Downloads/album"), Kept::NotEmpty),
        ];
        assert_eq!(
            removed("d-0006", true, &kept),
            warn(
                "download d-0006 removed from the list, but 2 of its files were kept: \
                 ubuntu.iso (Failed(Io)), album (NotEmpty)"
            )
        );
    }

    impl DownloadRecord {
        fn with_bytes(mut self, completed_bytes: u64) -> Self {
            self.progress.completed_bytes = completed_bytes;
            self
        }
    }
}
