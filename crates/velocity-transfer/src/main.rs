//! The `velocity-transfer` program (#37): the engine as a child process, and a command line for
//! running one download by hand.
//!
//! ```text
//! velocity-transfer serve
//! velocity-transfer get <url> <destination> [--connections N] [--probe-bytes B]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use velocity_curl::{CurlLoop, Libcurl, LoopOptions};
use velocity_transfer::download::{download_with, Control, DownloadOptions};
use velocity_transfer::engine::{MAX_CONNECTIONS, PROBE_BYTES};
use velocity_transfer::stdio;

const USAGE: &str = "usage:
  velocity-transfer serve
  velocity-transfer get <url> <destination> [--connections N] [--probe-bytes B]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("serve") if args.len() == 1 => stdio::serve(),
        Some("get") => {
            parse_get(&args[1..]).and_then(|(url, dest, options)| get(&url, dest, options))
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("velocity-transfer: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_get(args: &[String]) -> Result<(String, PathBuf, DownloadOptions), String> {
    let mut positional = Vec::new();
    let mut options = DownloadOptions {
        connections: 4,
        probe_bytes: PROBE_BYTES,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--connections" => {
                let n: u32 = number(rest.next(), "--connections")?;
                if !(1..=MAX_CONNECTIONS).contains(&n) {
                    return Err(format!("--connections must be 1 to {MAX_CONNECTIONS}"));
                }
                options.connections = n;
            }
            "--probe-bytes" => {
                options.probe_bytes = number(rest.next(), "--probe-bytes")?;
                if options.probe_bytes == 0 {
                    return Err("--probe-bytes must be at least 1".into());
                }
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n{USAGE}"))
            }
            value => positional.push(value),
        }
    }
    match positional.as_slice() {
        [url, dest] => Ok((url.to_string(), PathBuf::from(dest), options)),
        _ => Err(USAGE.to_string()),
    }
}

fn number<T: std::str::FromStr>(value: Option<&String>, flag: &str) -> Result<T, String> {
    value
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{flag} needs a number"))
}

fn get(url: &str, destination: PathBuf, options: DownloadOptions) -> Result<(), String> {
    let libcurl = Libcurl::start().map_err(|e| e.to_string())?;
    let lp = CurlLoop::start(&libcurl, LoopOptions::default()).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let shown = AtomicU64::new(0);
    let control = Control::with_progress(move |written, total| {
        // A line per 8 MiB, on stderr, so stdout carries only the report.
        if written >= shown.load(Ordering::Relaxed) + 8 * 1024 * 1024 {
            shown.store(written, Ordering::Relaxed);
            match total {
                Some(t) => eprintln!("{written} / {t} bytes"),
                None => eprintln!("{written} bytes"),
            }
        }
    });
    let report =
        download_with(&lp, url, &destination, options, &control).map_err(|e| format!("{e:?}"))?;
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "mode={:?} bytes={} fetches={} written={} wasted={} seconds={seconds:.3} mib_per_s={:.2}",
        report.mode,
        report.total,
        report.fetches,
        report.written_bytes,
        report.wasted_bytes,
        report.total as f64 / (1024.0 * 1024.0) / seconds.max(f64::EPSILON)
    );
    Ok(())
}
