//! Diagnostics (#28, V1's `get_diagnostics` and `export_diagnostics`): what the app, its engines and
//! their recent logs say, safe to share. Nothing is sent anywhere: the user saves the export and
//! decides whom to give it to (DIAGNOSTICS.md, PRIVACY.md).

use serde::Serialize;

use crate::domain::{Capabilities, DownloadStatus, EngineHealth, ErrorCode};
use crate::logs::LogLine;
use crate::manager::Manager;
use crate::redact::redact;
use crate::{app_info, AppInfo};

/// What the diagnostics show, and what the export holds. Every log line in it is redacted, and
/// the export is redacted again as a whole.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    /// Milliseconds since the Unix epoch.
    pub generated_at_ms: u64,
    pub app: AppInfo,
    /// The operating system and processor, as Rust names them (`windows`, `x86_64`).
    pub os: &'static str,
    pub arch: &'static str,
    pub transfer_engine: EngineHealth,
    /// The protocols and limits of the transfer engine, and the libcurl it runs on.
    pub capabilities: Capabilities,
    pub torrent_engine: EngineHealth,
    /// The libtorrent the torrent helper runs; `None` while it can't say.
    pub libtorrent: Option<String>,
    /// How many downloads are in the list, by status, in the contract's order: no names or links.
    pub downloads: Vec<StatusCount>,
    pub storage_error: Option<ErrorCode>,
    /// The app's last log lines, oldest first.
    pub app_log: Vec<LogLine>,
    /// The engines' last stderr lines.
    pub transfer_engine_log: Vec<String>,
    pub torrent_engine_log: Vec<String>,
}

impl DiagnosticReport {
    /// A fixed example for the contract fixtures and UI tests (`diagnostics.json`).
    pub fn sample() -> Self {
        use crate::logs::LogLevel;
        Self {
            generated_at_ms: 1_792_600_000_000,
            app: AppInfo {
                name: "Velocity Download Manager (dev)",
                version: "0.1.0",
                command_api_version: crate::COMMAND_API_VERSION,
            },
            os: "linux",
            arch: "x86_64",
            transfer_engine: EngineHealth::Running,
            capabilities: Capabilities::sample(),
            torrent_engine: EngineHealth::Unavailable {
                code: ErrorCode::EngineMissing,
            },
            libtorrent: Some("2.0.11.0".to_owned()),
            downloads: vec![
                StatusCount {
                    status: DownloadStatus::Downloading,
                    count: 2,
                },
                StatusCount {
                    status: DownloadStatus::Completed,
                    count: 5,
                },
            ],
            storage_error: None,
            app_log: vec![LogLine {
                at_ms: 1_792_599_999_000,
                level: LogLevel::Warn,
                target: "velocity_desktop_lib::downloads".to_owned(),
                message: "the torrent helper couldn't start: EngineMissing".to_owned(),
            }],
            transfer_engine_log: vec!["engine ready".to_owned()],
            torrent_engine_log: Vec::new(),
        }
    }
}

/// How many downloads have one status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCount {
    pub status: DownloadStatus,
    pub count: usize,
}

impl DiagnosticReport {
    /// The export: the report as indented JSON, with every text in it redacted again, so nothing
    /// a part missed is written. Each string is redacted on its own, so the JSON stays whole.
    pub fn export(&self) -> Result<String, ErrorCode> {
        let mut value = serde_json::to_value(self).map_err(|_| ErrorCode::Internal)?;
        redact_strings(&mut value);
        serde_json::to_string_pretty(&value).map_err(|_| ErrorCode::Internal)
    }
}

fn redact_strings(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => *s = redact(s),
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_strings),
        serde_json::Value::Object(fields) => fields.values_mut().for_each(redact_strings),
        _ => {}
    }
}

impl Manager {
    /// The diagnostics now, with `app_log` as the app's log. The engines' logs are their own,
    /// redacted as they were kept.
    pub fn diagnostics(&self, app_log: Vec<LogLine>, now_ms: u64) -> DiagnosticReport {
        let snapshot = self.snapshot();
        let downloads = DownloadStatus::ALL
            .iter()
            .map(|&status| StatusCount {
                status,
                count: snapshot
                    .downloads
                    .iter()
                    .filter(|d| d.status == status)
                    .count(),
            })
            .filter(|c| c.count > 0)
            .collect();
        DiagnosticReport {
            generated_at_ms: now_ms,
            app: app_info(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            transfer_engine: snapshot.engine,
            capabilities: self.capabilities(),
            torrent_engine: snapshot.torrent_engine,
            libtorrent: self.torrent_version(),
            downloads,
            storage_error: snapshot.storage_error,
            app_log,
            transfer_engine_log: self.transfer_engine_log(),
            torrent_engine_log: self.torrent_engine_log(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::database::Database;
    use crate::domain::DownloadKind;
    use crate::logs::LogLevel;
    use crate::redact::register_secret;
    use crate::transfer::fake::FakeTransfer;
    use crate::transfer::{Limits, TransferSpec};

    const T0: u64 = 1_792_600_000_000;

    fn manager() -> Manager {
        Manager::open(
            Box::new(FakeTransfer::new()),
            Arc::new(Database::open_in_memory().unwrap()),
        )
        .unwrap()
    }

    fn spec(url: &str) -> TransferSpec {
        TransferSpec {
            urls: vec![url.to_owned()],
            directory: std::path::PathBuf::from("downloads"),
            file_name: Some("a.zip".to_owned()),
            credentials: None,
            headers: Vec::new(),
            referrer: None,
            user_agent: None,
            checksum: None,
            pieces: None,
            limits: Limits::default(),
            max_connections: 4,
            resume: None,
            require_encryption: false,
            ssh_key: None,
        }
    }

    #[test]
    fn the_report_says_what_the_app_and_its_engines_are_without_names_or_links() {
        let mut m = manager();
        m.add(
            DownloadKind::Http,
            spec("https://example.com/secret-name.zip"),
            T0,
        )
        .unwrap();
        let report = m.diagnostics(Vec::new(), T0);
        assert_eq!(report.app.version, app_info().version);
        assert_eq!(report.os, std::env::consts::OS);
        assert_eq!(report.transfer_engine, EngineHealth::Running);
        assert_eq!(report.capabilities, m.capabilities());
        assert_eq!(
            report.downloads,
            [StatusCount {
                status: m.snapshot().downloads[0].status,
                count: 1
            }]
        );
        let export = report.export().unwrap();
        assert!(!export.contains("secret-name"), "{export}");
        assert!(export.contains("\"transferEngine\""), "camelCase JSON");
    }

    /// The acceptance criterion: an exported report holds no test secret, wherever it was.
    #[test]
    fn an_export_holds_no_secret_from_any_log() {
        let m = manager();
        register_secret("registered-passphrase-42");
        let secrets = [
            "hunter2pass",
            "abc.def.ghi",
            "deadbeef99",
            "registered-passphrase-42",
            "QUJDREVGR0g=",
            "0f1e2d3c",
            "sk-live-12345678",
        ];
        let app_log = vec![LogLine {
            at_ms: T0,
            level: LogLevel::Warn,
            target: "test".to_owned(),
            message:
                "GET https://me:hunter2pass@example.com/ with Authorization: Bearer abc.def.ghi"
                    .to_owned(),
        }];
        let mut report = m.diagnostics(app_log, T0);
        // Engine lines straight from a child, as if its own redaction had missed them.
        report.transfer_engine_log = vec![
            "> Cookie: session=0f1e2d3c".to_owned(),
            "> X-Api-Key: sk-live-12345678".to_owned(),
            "passphrase registered-passphrase-42 used".to_owned(),
        ];
        report.torrent_engine_log = vec![
            "tracker https://t.test/announce?passkey=deadbeef99 failed".to_owned(),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nQUJDREVGR0g=\n-----END OPENSSH PRIVATE KEY-----"
                .to_owned(),
        ];
        let export = report.export().unwrap();
        for secret in secrets {
            assert!(!export.contains(secret), "{secret} leaked:\n{export}");
        }
        assert!(export.contains("example.com"), "hosts stay, for diagnosis");
    }
}
