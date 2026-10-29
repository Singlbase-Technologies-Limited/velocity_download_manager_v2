//! What the desktop notifications say (#29, V1's notifications): found by comparing two
//! snapshots of the downloads, so the shell needs no events of its own from the manager.

use crate::app_settings::NotificationSettings;
use crate::domain::{DownloadRecord, DownloadStatus, DownloadsSnapshot, EngineHealth};

/// What happened, for a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Completed,
    /// A torrent waits for its files to be chosen.
    ChooseFiles,
    Failed,
    Started,
    /// The transfer engine stopped and won't start again on its own.
    EngineFailed,
    /// The torrent helper stopped and won't start again on its own.
    TorrentHelperFailed,
}

/// One desktop notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
}

/// The notifications for what changed from `before` to `after`, as `settings` allow.
pub fn notices(
    before: &DownloadsSnapshot,
    after: &DownloadsSnapshot,
    settings: &NotificationSettings,
) -> Vec<Notice> {
    use DownloadStatus::*;
    let mut out = Vec::new();
    for record in &after.downloads {
        let was = before
            .downloads
            .iter()
            .find(|r| r.id == record.id)
            .map(|r| r.status);
        if was == Some(record.status) {
            continue;
        }
        let finished = |s: Option<DownloadStatus>| matches!(s, Some(Completed | Seeding));
        let notice = match record.status {
            Completed | Seeding if settings.on_complete && !finished(was) => {
                Some((NoticeKind::Completed, "Download complete", name(record)))
            }
            ChoosingFiles if settings.on_complete => Some((
                NoticeKind::ChooseFiles,
                "Choose files to download",
                name(record),
            )),
            Failed if settings.on_failure => {
                let mut body = name(record);
                if let Some(error) = &record.error {
                    body = format!(
                        "{body}
{}",
                        error.message
                    );
                }
                Some((NoticeKind::Failed, "Download failed", body))
            }
            // Leaving the queue, or a pause; a retry or a stall recovering isn't a start.
            Connecting | Downloading
                if settings.on_start && matches!(was, None | Some(Queued | Paused)) =>
            {
                Some((NoticeKind::Started, "Download started", name(record)))
            }
            _ => None,
        };
        out.extend(notice.map(|(kind, title, body)| Notice {
            kind,
            title: title.to_owned(),
            body,
        }));
    }
    if settings.on_engine_failure {
        let engines = [
            (
                &before.engine,
                &after.engine,
                NoticeKind::EngineFailed,
                "Transfer engine stopped",
            ),
            (
                &before.torrent_engine,
                &after.torrent_engine,
                NoticeKind::TorrentHelperFailed,
                "Torrent helper stopped",
            ),
        ];
        for (was, now, kind, title) in engines {
            if let (false, EngineHealth::Unavailable { code }) = (unavailable(was), now) {
                out.push(Notice {
                    kind,
                    title: title.to_owned(),
                    body: code.user_message().to_owned(),
                });
            }
        }
    }
    out
}

fn unavailable(health: &EngineHealth) -> bool {
    matches!(health, EngineHealth::Unavailable { .. })
}

/// The download's file name, or its link while the server hasn't named it yet.
fn name(record: &DownloadRecord) -> String {
    record
        .file_name
        .clone()
        .unwrap_or_else(|| record.source_url.clone())
}

/// How many downloads are moving data or about to: what quitting would interrupt.
pub fn active_downloads(snapshot: &DownloadsSnapshot) -> usize {
    use DownloadStatus::*;
    snapshot
        .downloads
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                Connecting | Downloading | Verifying | Retrying | Stalled | FetchingMetadata
            )
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ErrorCode;

    fn record(id: &str, status: DownloadStatus) -> DownloadRecord {
        DownloadRecord {
            id: id.to_owned(),
            status,
            error: None,
            ..DownloadRecord::sample()
        }
    }

    fn snapshot(downloads: Vec<DownloadRecord>) -> DownloadsSnapshot {
        DownloadsSnapshot {
            downloads,
            unavailable: Vec::new(),
            needs_login: Vec::new(),
            engine: EngineHealth::Running,
            torrent_engine: EngineHealth::Running,
            storage_error: None,
        }
    }

    fn all_on() -> NotificationSettings {
        NotificationSettings {
            on_complete: true,
            on_failure: true,
            on_start: true,
            on_engine_failure: true,
        }
    }

    fn kinds(notices: &[Notice]) -> Vec<NoticeKind> {
        notices.iter().map(|n| n.kind).collect()
    }

    #[test]
    fn a_download_that_completes_is_notified_once_with_its_name() {
        let before = snapshot(vec![record("a", DownloadStatus::Downloading)]);
        let after = snapshot(vec![record("a", DownloadStatus::Completed)]);
        let n = notices(&before, &after, &all_on());
        assert_eq!(kinds(&n), [NoticeKind::Completed]);
        assert_eq!(n[0].title, "Download complete");
        assert_eq!(n[0].body, "archive.zip");
        assert!(
            notices(&after, &after, &all_on()).is_empty(),
            "only when it changes"
        );
    }

    #[test]
    fn a_torrent_that_finishes_downloading_and_seeds_counts_as_complete() {
        let before = snapshot(vec![record("t", DownloadStatus::Downloading)]);
        let after = snapshot(vec![record("t", DownloadStatus::Seeding)]);
        assert_eq!(
            kinds(&notices(&before, &after, &all_on())),
            [NoticeKind::Completed]
        );
        let completed = snapshot(vec![record("t", DownloadStatus::Completed)]);
        assert!(
            notices(&after, &completed, &all_on()).is_empty(),
            "seeding, then stopping: already notified"
        );
    }

    #[test]
    fn a_failure_says_why() {
        let before = snapshot(vec![record("a", DownloadStatus::Retrying)]);
        let mut failed = DownloadRecord::sample();
        failed.id = "a".to_owned();
        failed.status = DownloadStatus::Failed;
        let after = snapshot(vec![failed]);
        let n = notices(&before, &after, &all_on());
        assert_eq!(kinds(&n), [NoticeKind::Failed]);
        assert_eq!(n[0].title, "Download failed");
        assert_eq!(n[0].body, "archive.zip\nService unavailable");
    }

    #[test]
    fn leaving_the_queue_is_a_start_and_recovering_isnt() {
        let n = |from: Option<DownloadStatus>, to: DownloadStatus| {
            let before = snapshot(from.map(|s| record("a", s)).into_iter().collect());
            kinds(&notices(
                &before,
                &snapshot(vec![record("a", to)]),
                &all_on(),
            ))
        };
        use DownloadStatus::*;
        for (from, to) in [
            (Some(Queued), Connecting),
            (Some(Queued), Downloading),
            (Some(Paused), Connecting),
            (None, Downloading),
        ] {
            assert_eq!(n(from, to), [NoticeKind::Started], "{from:?} to {to:?}");
        }
        for (from, to) in [
            (Some(Connecting), Downloading),
            (Some(Retrying), Connecting),
            (Some(Stalled), Downloading),
            (None, Queued),
        ] {
            assert!(n(from, to).is_empty(), "{from:?} to {to:?}");
        }
    }

    #[test]
    fn a_torrent_waiting_for_its_files_to_be_chosen_is_notified() {
        let before = snapshot(vec![record("t", DownloadStatus::FetchingMetadata)]);
        let after = snapshot(vec![record("t", DownloadStatus::ChoosingFiles)]);
        let n = notices(&before, &after, &all_on());
        assert_eq!(kinds(&n), [NoticeKind::ChooseFiles]);
        assert_eq!(n[0].title, "Choose files to download");
    }

    #[test]
    fn an_engine_that_becomes_unavailable_is_notified_with_the_reason() {
        let before = snapshot(Vec::new());
        let mut after = snapshot(Vec::new());
        after.engine = EngineHealth::Unavailable {
            code: ErrorCode::EngineMissing,
        };
        after.torrent_engine = EngineHealth::Unavailable {
            code: ErrorCode::EngineStartFailed,
        };
        let n = notices(&before, &after, &all_on());
        assert_eq!(
            kinds(&n),
            [NoticeKind::EngineFailed, NoticeKind::TorrentHelperFailed]
        );
        assert_eq!(n[0].title, "Transfer engine stopped");
        assert_eq!(n[0].body, ErrorCode::EngineMissing.user_message());
        assert_eq!(n[1].title, "Torrent helper stopped");
        assert!(notices(&after, &after, &all_on()).is_empty(), "only once");
        let mut restarting = snapshot(Vec::new());
        restarting.engine = EngineHealth::Restarting { attempt: 1 };
        assert!(
            notices(&before, &restarting, &all_on()).is_empty(),
            "a restart on its own isn't a failure"
        );
    }

    #[test]
    fn each_kind_follows_its_setting() {
        let before = snapshot(vec![
            record("done", DownloadStatus::Downloading),
            record("bad", DownloadStatus::Downloading),
            record("new", DownloadStatus::Queued),
            record("pick", DownloadStatus::FetchingMetadata),
        ]);
        let mut after = snapshot(vec![
            record("done", DownloadStatus::Completed),
            record("bad", DownloadStatus::Failed),
            record("new", DownloadStatus::Downloading),
            record("pick", DownloadStatus::ChoosingFiles),
        ]);
        after.engine = EngineHealth::Unavailable {
            code: ErrorCode::EngineMissing,
        };
        let off = NotificationSettings {
            on_complete: false,
            on_failure: false,
            on_start: false,
            on_engine_failure: false,
        };
        assert!(notices(&before, &after, &off).is_empty());
        let defaults = notices(&before, &after, &NotificationSettings::default());
        assert_eq!(
            kinds(&defaults),
            [
                NoticeKind::Completed,
                NoticeKind::Failed,
                NoticeKind::ChooseFiles,
                NoticeKind::EngineFailed
            ],
            "V1's defaults: no start"
        );
    }

    #[test]
    fn a_download_without_a_name_yet_is_named_by_its_link() {
        let before = snapshot(vec![record("a", DownloadStatus::Downloading)]);
        let mut done = record("a", DownloadStatus::Completed);
        done.file_name = None;
        let n = notices(&before, &snapshot(vec![done]), &all_on());
        assert_eq!(n[0].body, "https://example.com/files/archive.zip");
    }

    #[test]
    fn active_downloads_are_those_moving_data_or_about_to() {
        use DownloadStatus::*;
        let s = snapshot(
            [
                Queued,
                Connecting,
                Downloading,
                Paused,
                Verifying,
                Retrying,
                Stalled,
                Completed,
                Failed,
                FetchingMetadata,
                ChoosingFiles,
                Seeding,
            ]
            .iter()
            .enumerate()
            .map(|(i, s)| record(&i.to_string(), *s))
            .collect(),
        );
        // Connecting, downloading, verifying, retrying, stalled and fetching metadata.
        assert_eq!(active_downloads(&s), 6);
    }
}
