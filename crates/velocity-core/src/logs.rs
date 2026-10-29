//! The app's log (#28): what the app records with `tracing`, kept in memory for diagnostics. Each
//! line is redacted ([`crate::redact`]) before it is kept, only the last [`AppLog::LINES`] lines
//! are, and nothing is written to disk or sent anywhere (DIAGNOSTICS.md).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::redact::redact;

/// How serious a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// One line of the app's log, redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogLine {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub level: LogLevel,
    /// Where it came from, such as `velocity_core::manager`.
    pub target: String,
    pub message: String,
}

/// The app's last log lines, shared by the layer that writes them and the diagnostics that read
/// them.
#[derive(Clone, Default)]
pub struct AppLog {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
}

impl AppLog {
    /// How many lines are kept: V1's log size.
    pub const LINES: usize = 1000;
    /// The longest message kept, in characters.
    pub const MAX_CHARS: usize = 2000;

    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<LogLine>> {
        self.lines.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Keeps `line`, redacted and cut to [`AppLog::MAX_CHARS`], dropping the oldest beyond
    /// [`AppLog::LINES`].
    pub fn push(&self, mut line: LogLine) {
        line.message = redact(&line.message);
        if let Some((cut, _)) = line.message.char_indices().nth(Self::MAX_CHARS) {
            line.message.truncate(cut);
        }
        let mut lines = self.lock();
        if lines.len() == Self::LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The lines kept, oldest first.
    pub fn lines(&self) -> Vec<LogLine> {
        self.lock().iter().cloned().collect()
    }

    /// The layer that keeps every event of `Info` and above in this log.
    pub fn layer(&self) -> AppLogLayer {
        AppLogLayer { log: self.clone() }
    }
}

/// A `tracing` layer that writes into an [`AppLog`].
pub struct AppLogLayer {
    log: AppLog,
}

/// The message and the other fields of an event, as one line: `message key=value …`.
#[derive(Default)]
struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for AppLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = match *event.metadata().level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            Level::INFO => LogLevel::Info,
            // Debug and trace lines aren't kept: diagnostics show what matters to the user.
            _ => return,
        };
        let mut line = Line::default();
        event.record(&mut line);
        self.log.push(LogLine {
            at_ms: crate::manager::now_ms(),
            level,
            target: event.metadata().target().to_owned(),
            message: line.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    fn line(message: &str) -> LogLine {
        LogLine {
            at_ms: 1,
            level: LogLevel::Info,
            target: "test".to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn events_are_kept_redacted_with_their_fields() {
        let log = AppLog::new();
        let subscriber = tracing_subscriber::registry().with(log.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(url = "https://me:hunter2pass@example.com/a.zip", "retrying");
            tracing::info!("added Authorization: Bearer abc.def.ghi");
            tracing::debug!("not kept");
        });
        let lines = log.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0].level, LogLevel::Warn);
        assert!(
            lines[0].message.starts_with("retrying url="),
            "{:?}",
            lines[0]
        );
        assert!(
            lines[0].message.contains("example.com/a.zip"),
            "the host and path stay"
        );
        for l in &lines {
            assert!(
                !l.message.contains("hunter2pass") && !l.message.contains("abc.def.ghi"),
                "{l:?}"
            );
        }
        assert!(lines[0].target.contains("logs"), "{:?}", lines[0].target);
    }

    #[test]
    fn only_the_last_lines_are_kept_each_cut_to_its_bound() {
        let log = AppLog::new();
        for n in 0..=AppLog::LINES {
            log.push(line(&format!("line {n}")));
        }
        let lines = log.lines();
        assert_eq!(lines.len(), AppLog::LINES);
        assert_eq!(lines[0].message, "line 1");
        log.push(line(&"é".repeat(AppLog::MAX_CHARS + 10)));
        let last = log.lines().pop().unwrap();
        assert_eq!(last.message.chars().count(), AppLog::MAX_CHARS);
    }
}
