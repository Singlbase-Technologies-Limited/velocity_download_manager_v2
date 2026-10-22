//! Measures what it costs to write far past the end of a new file (#42, research report open
//! question 7): on NTFS, the gap up to a write is zero-filled before it returns, unless the file is
//! sparse. Segments written in parallel do exactly that.
//!
//! ```text
//! cargo run --release -p velocity-transfer --example high_offset_write -- [dir] [--gib N] [--runs R]
//!     [--only no_prealloc,set_len,sparse,at_zero]
//! ```
//!
//! For each run, in a fresh file: 1 MiB written at offset N GiB with no preallocation; the same
//! after `set_len` to N GiB + 1 MiB; the same in a part file as the engine makes it (sparse on
//! Windows, #43); and, as the control, 1 MiB at offset 0. Each write is followed
//! by `sync_all`, so the time includes the disk. Results are printed, never claimed.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

const CHUNK: usize = 1 << 20;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("high_offset_write: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = std::env::temp_dir();
    let (mut gib, mut runs) = (4u64, 3u32);
    let mut only: Vec<String> = Vec::new();
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--gib" => gib = number(rest.next())?,
            "--runs" => runs = number(rest.next())?,
            "--only" => only.extend(
                rest.next()
                    .map(|v| v.split(',').map(str::to_owned).collect::<Vec<_>>())
                    .unwrap_or_default(),
            ),
            other => dir = PathBuf::from(other),
        }
    }
    let offset = gib << 30;
    println!(
        "dir={} offset_gib={gib} chunk_mib=1 runs={runs}",
        dir.display()
    );
    for run in 0..runs {
        for (name, how, at) in [
            ("no_prealloc", How::Plain, offset),
            ("set_len", How::SetLen, offset),
            ("sparse", How::Sparse, offset),
            ("at_zero", How::Plain, 0),
        ] {
            if !only.is_empty() && !only.iter().any(|o| o == name) {
                continue;
            }
            let path = dir.join(format!(
                "v2-high-offset-{}-{run}-{name}.bin",
                std::process::id()
            ));
            let ms = measure(&path, how, at, offset + CHUNK as u64);
            let _ = fs::remove_file(&path);
            println!("run {run} {name}: {}", ms?);
        }
    }
    Ok(())
}

/// How the new file is made before the write.
#[derive(Clone, Copy)]
enum How {
    Plain,
    /// Sized to its final length first.
    SetLen,
    /// As the engine creates part files: sparse on Windows (#43).
    Sparse,
}

/// Creates `path` as `how` says, writes 1 MiB at `at` and syncs. Returns the time in milliseconds
/// for the write and the sync.
fn measure(path: &Path, how: How, at: u64, len: u64) -> Result<String, String> {
    let file = match how {
        How::Sparse => {
            let part = velocity_transfer::files::create_part(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if !part.sparse && cfg!(windows) {
                return Err("the file system refused to make it sparse".to_owned());
            }
            part.file
        }
        How::Plain | How::SetLen => OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?,
    };
    if let How::SetLen = how {
        file.set_len(len).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    let data = vec![0x5Au8; CHUNK];
    let started = Instant::now();
    write_at(&file, &data, at).map_err(|e| e.to_string())?;
    let written = started.elapsed();
    file.sync_all().map_err(|e| e.to_string())?;
    let synced = started.elapsed();
    Ok(format!(
        "write_ms={:.1} write_and_sync_ms={:.1}",
        written.as_secs_f64() * 1e3,
        synced.as_secs_f64() * 1e3
    ))
}

#[cfg(windows)]
fn write_at(file: &fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &fs::File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

fn number<T: std::str::FromStr>(value: Option<&String>) -> Result<T, String> {
    value
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| "a number is needed".to_owned())
}
