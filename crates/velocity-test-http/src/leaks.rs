//! The secret-leak scan (#71): after a test has used known secrets, it looks for them everywhere
//! V2 writes or shows something: files under a folder (the database, its WAL, checkpoints, part
//! files), temporary files, a text such as the app's log or a diagnostics export, and a running
//! child process's arguments and environment. Each secret is looked for as written, percent-encoded
//! and in base64 (as in a `Basic` login), so an encoded copy is caught too.
//!
//! A running process is read on Linux only (`/proc/<pid>/cmdline` and `environ`). Elsewhere that
//! part says NOT RUN: on Windows the tests check the command a child is started with instead
//! (PO decision on #71).

use std::path::Path;
use std::time::SystemTime;

/// The shortest secret the scan accepts: a short one could match by chance, and a common one
/// would hide a real leak behind false hits. Secrets are unique strings (#71).
const MIN_SECRET_LEN: usize = 8;

/// How deep the scan goes into the temporary folder, which other programs share.
const TEMP_DEPTH: usize = 3;

/// One scan over known secrets. Collects every place one was found; [`LeakScan::assert_clean`]
/// fails the test with all of them.
pub struct LeakScan {
    /// Each secret with the forms it is looked for in.
    secrets: Vec<(String, Vec<Vec<u8>>)>,
    found: Vec<String>,
}

impl LeakScan {
    /// A scan for `secrets`. Panics on a secret shorter than 8 bytes.
    pub fn new<S: AsRef<str>>(secrets: &[S]) -> Self {
        let secrets = secrets
            .iter()
            .map(|s| {
                let s = s.as_ref();
                assert!(
                    s.len() >= MIN_SECRET_LEN,
                    "{s:?} is too short to be a unique test secret"
                );
                (s.to_owned(), forms(s.as_bytes()))
            })
            .collect();
        LeakScan {
            secrets,
            found: Vec::new(),
        }
    }

    /// Every place a secret was found so far, with the secret.
    pub fn found(&self) -> &[String] {
        &self.found
    }

    /// Fails the test if any secret was found, naming every place.
    pub fn assert_clean(&self) {
        assert!(
            self.found.is_empty(),
            "secrets leaked:\n{}",
            self.found.join("\n")
        );
    }

    /// Looks in `text`, named `place` in what is reported.
    pub fn text(&mut self, place: &str, text: &str) -> &mut Self {
        self.bytes(place, text.as_bytes())
    }

    /// Looks in every file under `dir`, at any depth. Links are not followed.
    pub fn files(&mut self, dir: &Path) -> &mut Self {
        self.walk(dir, usize::MAX, None);
        self
    }

    /// Looks in the files of the system's temporary folder, a few levels deep, that changed since
    /// `since`: the test's own temporary files, among other programs'.
    pub fn temp_files(&mut self, since: SystemTime) -> &mut Self {
        self.walk(&std::env::temp_dir(), TEMP_DEPTH, Some(since));
        self
    }

    /// Looks in the arguments and environment of the running process `pid`, named `place`. On
    /// Linux only; elsewhere it prints NOT RUN and finds nothing.
    pub fn process(&mut self, place: &str, pid: u32) -> &mut Self {
        if cfg!(target_os = "linux") {
            for part in ["cmdline", "environ"] {
                let path = format!("/proc/{pid}/{part}");
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        self.bytes(&format!("{place} ({part})"), &bytes);
                    }
                    Err(e) => panic!("{place}: {path} can't be read: {e}"),
                }
            }
        } else {
            eprintln!("{place}: running process NOT RUN (read on Linux only)");
        }
        self
    }

    fn walk(&mut self, dir: &Path, depth: usize, since: Option<SystemTime>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Without following links: a link is never read through.
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if depth > 1 {
                    self.walk(&path, depth - 1, since);
                }
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if let Some(since) = since {
                if !meta.modified().is_ok_and(|m| m >= since) {
                    continue;
                }
            }
            // A file another program holds open, or one gone since it was listed, is skipped.
            if let Ok(bytes) = std::fs::read(&path) {
                self.bytes(&path.display().to_string(), &bytes);
            }
        }
    }

    fn bytes(&mut self, place: &str, bytes: &[u8]) -> &mut Self {
        for (secret, forms) in &self.secrets {
            if forms.iter().any(|f| contains(bytes, f)) {
                self.found.push(format!("{place} holds {secret}"));
            }
        }
        self
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The forms `secret` is looked for in: as written, percent-encoded, and the base64 characters
/// that depend on it alone at each of the three alignments it can have inside a longer value.
fn forms(secret: &[u8]) -> Vec<Vec<u8>> {
    let mut forms = vec![secret.to_vec()];
    let encoded = percent_encode(secret);
    if encoded != secret {
        forms.push(encoded);
    }
    for offset in 0..3 {
        let mut input = vec![0; offset];
        input.extend_from_slice(secret);
        let b64 = base64(&input);
        // Character i covers bits 6i..6i+6; it belongs to the secret alone when those bits do.
        let start = (8 * offset).div_ceil(6);
        let end = 8 * input.len() / 6;
        if end > start {
            forms.push(b64[start..end].to_vec());
        }
    }
    forms
}

fn percent_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("%{b:02X}").as_bytes());
        }
    }
    out
}

fn base64(bytes: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SECRET: &str = "LEAK-SCAN-SECRET-71";

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("v2-leakscan-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("nested")).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b"user:pass"), b"dXNlcjpwYXNz");
        assert_eq!(base64(b"ab"), b"YWI");
    }

    #[test]
    fn a_secret_in_a_nested_file_is_found_and_a_clean_folder_passes() {
        let dir = Dir::new("files");
        std::fs::write(dir.0.join("clean.txt"), "nothing here").unwrap();
        let mut scan = LeakScan::new(&[SECRET]);
        scan.files(&dir.0);
        scan.assert_clean();

        std::fs::write(dir.0.join("nested").join("db-wal"), format!("x{SECRET}y")).unwrap();
        scan.files(&dir.0);
        assert_eq!(scan.found().len(), 1, "{:?}", scan.found());
        assert!(scan.found()[0].contains("db-wal"), "{:?}", scan.found());
    }

    #[test]
    fn a_secret_inside_a_basic_login_or_a_link_is_found() {
        for prefix in ["", "u", "u:"] {
            let header = format!("Authorization: Basic {}", {
                let b = base64(format!("{prefix}{SECRET}").as_bytes());
                String::from_utf8(b).unwrap()
            });
            let mut scan = LeakScan::new(&[SECRET]);
            scan.text("log", &header);
            assert_eq!(scan.found(), [format!("log holds {SECRET}")], "{header}");
        }
        let mut scan = LeakScan::new(&["pass word&key"]);
        scan.text("export", "https://h.test/?k=pass%20word%26key");
        assert_eq!(scan.found().len(), 1);
    }

    #[test]
    fn only_temporary_files_changed_since_the_start_are_read() {
        let dir = Dir::new("temp");
        let since = SystemTime::now() - std::time::Duration::from_secs(5);
        std::fs::write(dir.0.join("nested").join("t.tmp"), SECRET).unwrap();
        let mut scan = LeakScan::new(&[SECRET]);
        scan.temp_files(since);
        assert!(
            scan.found().iter().any(|f| f.contains("t.tmp")),
            "{:?}",
            scan.found()
        );
        let mut later = LeakScan::new(&[SECRET]);
        later.temp_files(SystemTime::now() + std::time::Duration::from_secs(60));
        later.assert_clean();
    }

    #[test]
    #[should_panic(expected = "too short")]
    fn a_short_secret_is_refused() {
        LeakScan::new(&["abc"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_secret_in_a_running_process_is_found() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .env("LEAKY_TOKEN", SECRET)
            .spawn()
            .unwrap();
        // Right after the spawn the child can still be this test's copy, before `sleep` replaced
        // it with its own arguments and environment: wait until it has.
        let cmdline = format!("/proc/{}/cmdline", child.id());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !std::fs::read(&cmdline).is_ok_and(|c| c.starts_with(b"sleep")) {
            assert!(std::time::Instant::now() < deadline, "sleep never started");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut scan = LeakScan::new(&[SECRET]);
        scan.process("sleep", child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(scan.found(), [format!("sleep (environ) holds {SECRET}")]);
    }
}
