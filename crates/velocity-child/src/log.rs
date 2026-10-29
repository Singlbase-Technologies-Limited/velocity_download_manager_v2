//! A child process's stderr, kept in memory for diagnostics (#39, #62). Only the last [`LOG_LINES`]
//! lines are kept, each cut at [`MAX_LINE_CHARS`] characters, and nothing is written to disk. Every
//! line passes through the shared redaction (`velocity_core::redact`, #28) before it is stored.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Mutex};
use std::thread;

use velocity_core::redact::redact;

/// How many lines are kept: V1's engine log size.
pub const LOG_LINES: usize = 500;
/// The longest line kept, in characters: V1's limit.
pub const MAX_LINE_CHARS: usize = 2000;
/// The most bytes read for one line, so a stream without newlines can't grow a buffer without
/// limit. A character is at most 4 bytes in UTF-8.
const MAX_LINE_BYTES: u64 = 4 * MAX_LINE_CHARS as u64;

/// The last lines a child wrote to stderr, shared by every child a supervisor starts.
#[derive(Clone, Default)]
pub struct ChildLog {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl ChildLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores one line, redacted, cut to [`MAX_LINE_CHARS`].
    pub fn push(&self, line: &str) {
        let mut line = redact(line);
        if let Some((cut, _)) = line.char_indices().nth(MAX_LINE_CHARS) {
            line.truncate(cut);
        }
        let mut lines = self.lines.lock().unwrap_or_else(|p| p.into_inner());
        if lines.len() == LOG_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The kept lines, oldest first.
    pub fn lines(&self) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(|p| p.into_inner());
        lines.iter().cloned().collect()
    }

    /// Reads `stream` line by line into the log on a thread of its own, until it ends. The stream
    /// must be read, or a child that writes a lot to stderr would block on a full pipe.
    pub(crate) fn capture(
        &self,
        name: &str,
        stream: impl Read + Send + 'static,
    ) -> std::io::Result<()> {
        let log = self.clone();
        thread::Builder::new()
            .name(format!("velocity-{name}-stderr"))
            .spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match (&mut reader)
                        .take(MAX_LINE_BYTES)
                        .read_until(b'\n', &mut line)
                    {
                        Ok(0) => break,
                        Ok(_) => {
                            let text = String::from_utf8_lossy(&line);
                            log.push(text.trim_end_matches(['\n', '\r']));
                        }
                        Err(e) => {
                            log.push(&format!("stderr couldn't be read: {e}"));
                            break;
                        }
                    }
                }
            })
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn keeps_only_the_last_lines() {
        let log = ChildLog::new();
        for n in 0..=LOG_LINES {
            log.push(&format!("line {n}"));
        }
        let lines = log.lines();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[LOG_LINES - 1], format!("line {LOG_LINES}"));
    }

    #[test]
    fn cuts_long_lines_and_hides_logins_when_stored() {
        let log = ChildLog::new();
        log.push(&"é".repeat(MAX_LINE_CHARS + 10));
        log.push("retrying https://user:secret@example.com/");
        let lines = log.lines();
        assert_eq!(lines[0].chars().count(), MAX_LINE_CHARS);
        assert_eq!(lines[1], "retrying https://[redacted]@example.com/");
    }

    #[test]
    fn captures_a_stream_line_by_line_within_bounds() {
        let log = ChildLog::new();
        let mut input = b"first\r\nhttps://a:b@h/ second\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 3 * MAX_LINE_BYTES as usize));
        input.extend_from_slice(b"\nlast");
        log.capture("test", std::io::Cursor::new(input)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while log.lines().last().map(String::as_str) != Some("last") {
            assert!(Instant::now() < deadline, "{:?}", log.lines());
            thread::sleep(Duration::from_millis(5));
        }
        let lines = log.lines();
        assert_eq!(lines[0], "first");
        assert_eq!(lines[1], "https://[redacted]@h/ second");
        assert!(lines.iter().all(|l| l.chars().count() <= MAX_LINE_CHARS));
        assert!(
            lines.len() > 4,
            "a line without a newline is read in bounded pieces"
        );
    }
}
