//! Measures what the hash frontier's read-back costs (#46, research report section 5.3: "worst
//! case is one full sequential read"). Hashes don't compose, so bytes that arrive out of order are
//! read back from the part file and hashed once the prefix from byte 0 reaches them.
//!
//! ```text
//! cargo run --release -p velocity-transfer --example hash_readback -- [dir] [--mib N] [--runs R]
//!     [--algorithms md5,sha1,sha256,sha512]
//! ```
//!
//! For each run and algorithm, with a file of N MiB written in 1 MiB chunks:
//! - `memory`: the hash alone, over the same bytes from memory;
//! - `reverse`: the chunks are written last first, so the prefix from byte 0 is covered only by
//!   the last write, and completion waits for the whole file to be read back and hashed (the worst
//!   case);
//! - `in_order`: the chunks are written first to last, the frontier following, and completion waits
//!   only for what it hasn't hashed yet.
//!
//! The file was just written, so the read-back comes from the operating system's cache; a read
//! from the disk itself would take longer, and isn't measured here. Results are printed, never
//! claimed.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use velocity_core::transfer::HashAlgorithm;
use velocity_test_http::seeded_bytes;
use velocity_transfer::files;
use velocity_transfer::integrity::{HashFrontier, Hasher};

const CHUNK: usize = 1 << 20;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hash_readback: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = std::env::temp_dir();
    let (mut mib, mut runs) = (1024usize, 3u32);
    let mut algorithms = vec![
        HashAlgorithm::Md5,
        HashAlgorithm::Sha1,
        HashAlgorithm::Sha256,
        HashAlgorithm::Sha512,
    ];
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--mib" => mib = number(rest.next())?,
            "--runs" => runs = number(rest.next())?,
            "--algorithms" => {
                algorithms = rest
                    .next()
                    .ok_or("--algorithms needs a list")?
                    .split(',')
                    .map(algorithm)
                    .collect::<Result<_, _>>()?;
            }
            other => dir = PathBuf::from(other),
        }
    }
    // Every chunk differs, so no chunk can be mistaken for another in the file.
    let chunks: Vec<Vec<u8>> = (0..mib).map(|i| seeded_bytes(CHUNK, i as u64)).collect();
    let total = (mib * CHUNK) as u64;
    println!("dir={} mib={mib} chunk_mib=1 runs={runs}", dir.display());
    for run in 0..runs {
        for &algorithm in &algorithms {
            let started = Instant::now();
            let mut hasher = Hasher::new(algorithm);
            for chunk in &chunks {
                hasher.update(chunk);
            }
            let expected = hasher.finish_hex();
            report(run, algorithm, "memory", started.elapsed(), mib);

            for (name, reverse) in [("reverse", true), ("in_order", false)] {
                let path = dir.join(format!("hash_readback-{}.vdmpart", std::process::id()));
                let wait = frontier_wait(&path, &chunks, algorithm, reverse, total, &expected);
                let _ = fs::remove_file(&path);
                report(run, algorithm, name, wait?, mib);
            }
        }
    }
    Ok(())
}

/// Writes `chunks` to a new part file in the given order, the frontier following as the engine's
/// part file drives it, and returns how long completion then waits for the digest.
fn frontier_wait(
    path: &Path,
    chunks: &[Vec<u8>],
    algorithm: HashAlgorithm,
    reverse: bool,
    total: u64,
    expected: &str,
) -> Result<Duration, String> {
    let part = files::create_part(path, true).map_err(|e| format!("{}: {e}", path.display()))?;
    let reader = part.file.try_clone().map_err(|e| e.to_string())?;
    let frontier = HashFrontier::start(reader, algorithm).map_err(|e| e.to_string())?;
    let order: Vec<usize> = if reverse {
        (0..chunks.len()).rev().collect()
    } else {
        (0..chunks.len()).collect()
    };
    for (written, &i) in order.iter().enumerate() {
        write_at(&part.file, &chunks[i], (i * CHUNK) as u64).map_err(|e| e.to_string())?;
        // In reverse, byte 0 is covered only by the last write, and then the whole file is.
        let prefix = match (reverse, written + 1 == order.len()) {
            (true, false) => 0,
            (true, true) => total,
            (false, _) => ((i + 1) * CHUNK) as u64,
        };
        frontier.advance(prefix);
    }
    let started = Instant::now();
    let digest = frontier.finish(total).map_err(|e| e.to_string())?;
    let wait = started.elapsed();
    if digest != expected {
        return Err(format!("{algorithm:?}: the frontier's digest differs"));
    }
    Ok(wait)
}

fn report(run: u32, algorithm: HashAlgorithm, case: &str, took: Duration, mib: usize) {
    let secs = took.as_secs_f64();
    let rate = if secs > 0.0 { mib as f64 / secs } else { 0.0 };
    println!("run={run} algorithm={algorithm:?} case={case} seconds={secs:.3} mib_per_s={rate:.0}");
}

fn algorithm(name: &str) -> Result<HashAlgorithm, String> {
    Ok(match name {
        "md5" => HashAlgorithm::Md5,
        "sha1" => HashAlgorithm::Sha1,
        "sha224" => HashAlgorithm::Sha224,
        "sha256" => HashAlgorithm::Sha256,
        "sha384" => HashAlgorithm::Sha384,
        "sha512" => HashAlgorithm::Sha512,
        other => return Err(format!("unknown algorithm {other:?}")),
    })
}

fn number<T: std::str::FromStr>(value: Option<&String>) -> Result<T, String> {
    value
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| "expected a number".to_owned())
}

#[cfg(windows)]
fn write_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}
