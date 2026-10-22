//! Measures what each engine mode costs, for the #38 decision (research report section 4.3):
//! throughput, CPU, peak memory, command round trips, startup, shutdown, and, for the child
//! process, how fast a crash is noticed and a new child is ready.
//!
//! ```text
//! cargo build --release -p velocity-transfer --bins --examples
//! target/release/examples/mode_cost [--mib 256] [--runs 5] [--connections 4]
//! ```
//!
//! Everything runs on this machine: a local test server in its own process (so its memory and CPU
//! count against neither mode), and each measured run in a fresh process, alternating which mode
//! goes first. Results are printed, never claimed: see docs/research/engine-mode-costs.md.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use velocity_core::domain::DownloadStatus;
use velocity_core::transfer::{
    DirectTransfer, Limits, TransferEvent, TransferId, TransferSnapshot, TransferSpec,
};
use velocity_test_http::{kill_process, seeded_bytes, ServerOptions, TestServer};
use velocity_transfer::engine::InProcessEngine;
use velocity_transfer::stdio::ChildProcessEngine;

const SEED: u64 = 38;
const WAIT: Duration = Duration::from_secs(300);
/// How often a command's round trip is timed while a download runs.
const PROBE_EVERY: Duration = Duration::from_millis(10);
const IDLE_ROUND_TRIPS: usize = 1000;
/// The metrics, in the order they are printed.
const METRICS: [&str; 12] = [
    "startup_ms",
    "seconds",
    "mib_per_s",
    "cpu_s",
    "parent_peak_mib",
    "child_peak_mib",
    "rt_busy_p50_us",
    "rt_busy_p99_us",
    "rt_idle_p50_us",
    "rt_idle_p99_us",
    "shutdown_ms",
    "crash_detect_ms",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        Some("once") => once(&args[1..]),
        _ => orchestrate(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mode_cost: {message}");
            ExitCode::FAILURE
        }
    }
}

fn flag(args: &[String], name: &str, default: u64) -> Result<u64, String> {
    match args.iter().position(|a| a == name) {
        None => Ok(default),
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("{name} needs a number")),
    }
}

/// `serve <mib>`: serves one seeded file until stdin closes.
fn serve(args: &[String]) -> Result<(), String> {
    let mib: usize = args
        .first()
        .and_then(|v| v.parse().ok())
        .ok_or("serve <mib>")?;
    let server = TestServer::start(
        vec![("f.bin".into(), seeded_bytes(mib << 20, SEED))],
        ServerOptions::default(),
    );
    println!("{}", server.url("f.bin"));
    let mut rest = Vec::new();
    std::io::stdin()
        .read_to_end(&mut rest)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn orchestrate(args: &[String]) -> Result<(), String> {
    let mib = flag(args, "--mib", 256)?;
    let runs = flag(args, "--runs", 5)?;
    let connections = flag(args, "--connections", 4)?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    // target/<profile>/examples/mode_cost → target/<profile>/velocity-transfer
    let engine = me
        .parent()
        .and_then(Path::parent)
        .map(|d| d.join(format!("velocity-transfer{}", std::env::consts::EXE_SUFFIX)))
        .filter(|p| p.is_file())
        .ok_or(
            "build the engine first: cargo build --release -p velocity-transfer --bins --examples",
        )?;
    let dir = std::env::temp_dir().join(format!("v2-mode-cost-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut server = Command::new(&me)
        .args(["serve", &mib.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut url = String::new();
    BufReader::new(server.stdout.take().ok_or("no server stdout")?)
        .read_line(&mut url)
        .map_err(|e| e.to_string())?;
    let url = url.trim().to_string();
    println!(
        "{mib} MiB over loopback, {connections} connections, {runs} runs per mode, {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let mut results: BTreeMap<&str, Vec<BTreeMap<String, f64>>> = BTreeMap::new();
    for run in 0..runs {
        let order = if run % 2 == 0 {
            ["in-process", "child"]
        } else {
            ["child", "in-process"]
        };
        for mode in order {
            let out = Command::new(&me)
                .args([
                    "once",
                    mode,
                    &url,
                    &mib.to_string(),
                    &connections.to_string(),
                ])
                .arg(&engine)
                .arg(&dir)
                .stderr(Stdio::inherit())
                .output()
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text
                .lines()
                .find_map(|l| l.strip_prefix("result "))
                .ok_or_else(|| format!("run {run} {mode} failed: {text}"))?;
            for crash in text.lines().filter(|l| l.starts_with("crash:")) {
                println!("run {run} {mode} {crash}");
            }
            println!("run {run} {mode}: {line}");
            let values = line
                .split_whitespace()
                .filter_map(|kv| kv.split_once('='))
                .filter_map(|(k, v)| v.parse().ok().map(|v| (k.to_string(), v)))
                .collect();
            results.entry(mode).or_default().push(values);
        }
    }
    drop(server.stdin.take());
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&dir);

    println!("\nmetric: in-process median (min to max) | child median (min to max)");
    for metric in METRICS {
        let cell = |mode: &str| {
            let mut v: Vec<f64> = results
                .get(mode)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get(metric).copied())
                .collect();
            if v.is_empty() {
                return "n/a".to_string();
            }
            v.sort_by(f64::total_cmp);
            format!(
                "{:.2} ({:.2} to {:.2})",
                v[v.len() / 2],
                v[0],
                v[v.len() - 1]
            )
        };
        println!("{metric}: {} | {}", cell("in-process"), cell("child"));
    }
    Ok(())
}

enum Engine {
    InProcess(InProcessEngine),
    Child(ChildProcessEngine),
}

impl Engine {
    fn start(mode: &str, program: &Path) -> Result<Self, String> {
        match mode {
            "in-process" => InProcessEngine::start().map(Engine::InProcess),
            "child" => ChildProcessEngine::spawn(program)
                .map(Engine::Child)
                .map_err(|e| format!("{e:?}")),
            other => Err(format!("unknown mode {other}")),
        }
    }

    fn api(&mut self) -> &mut dyn DirectTransfer {
        match self {
            Engine::InProcess(e) => e,
            Engine::Child(e) => e,
        }
    }

    fn child_pid(&self) -> Option<u32> {
        match self {
            Engine::InProcess(_) => None,
            Engine::Child(e) => Some(e.process_id()),
        }
    }
}

fn spec(url: &str, destination: PathBuf, connections: u32) -> TransferSpec {
    TransferSpec {
        urls: vec![url.to_string()],
        destination,
        credentials: None,
        headers: Vec::new(),
        checksum: None,
        limits: Limits::default(),
        max_connections: connections,
    }
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".vdmpart");
    PathBuf::from(p)
}

/// Waits for `id` to reach a status `done` accepts, timing a command round trip every
/// [`PROBE_EVERY`] meanwhile when `probes` is given.
fn wait_for(
    engine: &mut Engine,
    events: &Receiver<TransferEvent>,
    id: TransferId,
    done: impl Fn(DownloadStatus) -> bool,
    mut probes: Option<&mut Vec<f64>>,
) -> Result<TransferSnapshot, String> {
    let deadline = Instant::now() + WAIT;
    let mut next_probe = Instant::now();
    while Instant::now() < deadline {
        if let Some(p) = probes.as_deref_mut() {
            if Instant::now() >= next_probe {
                let t = Instant::now();
                engine.api().snapshot().map_err(|e| format!("{e:?}"))?;
                p.push(t.elapsed().as_secs_f64() * 1e6);
                next_probe = Instant::now() + PROBE_EVERY;
            }
        }
        match events.recv_timeout(PROBE_EVERY) {
            Ok(TransferEvent::Changed { snapshot }) if snapshot.id == id => {
                if done(snapshot.status) {
                    return Ok(snapshot);
                }
                if snapshot.status == DownloadStatus::Failed {
                    return Err(format!("the download failed: {:?}", snapshot.error));
                }
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("the engine went away".into()),
        }
    }
    Err("timed out".into())
}

fn percentile(values: &mut [f64], p: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * p / 100).min(values.len() - 1)]
}

/// `once <mode> <url> <mib> <connections> <engine> <dir>`: one measured run, in a fresh process.
fn once(args: &[String]) -> Result<(), String> {
    let [mode, url, mib, connections, program, dir] = args else {
        return Err("once <mode> <url> <mib> <connections> <engine> <dir>".into());
    };
    let len: usize = mib.parse::<usize>().map_err(|e| e.to_string())? << 20;
    let connections: u32 = connections
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    let (program, dir) = (Path::new(program), Path::new(dir));
    let tag = format!("{mode}-{}", std::process::id());
    let me = std::process::id();
    let mut r: Vec<(&str, f64)> = Vec::new();

    // Startup and the download.
    let t = Instant::now();
    let mut engine = Engine::start(mode, program)?;
    r.push(("startup_ms", t.elapsed().as_secs_f64() * 1e3));
    let pid = engine.child_pid();
    let events = engine.api().subscribe();
    let cpu_before = cpu_seconds(me)? + pid.map_or(Ok(0.0), cpu_seconds)?;
    let dest = dir.join(format!("{tag}.bin"));
    let mut busy = Vec::new();
    let t = Instant::now();
    let id = engine
        .api()
        .add(spec(url, dest.clone(), connections))
        .map_err(|e| format!("{e:?}"))?;
    wait_for(
        &mut engine,
        &events,
        id,
        |s| s == DownloadStatus::Completed,
        Some(&mut busy),
    )?;
    let seconds = t.elapsed().as_secs_f64();
    let cpu_after = cpu_seconds(me)? + pid.map_or(Ok(0.0), cpu_seconds)?;
    r.push(("seconds", seconds));
    r.push(("mib_per_s", len as f64 / 1048576.0 / seconds));
    r.push(("cpu_s", cpu_after - cpu_before));
    r.push(("parent_peak_mib", peak_bytes(me)? / 1048576.0));
    if let Some(pid) = pid {
        r.push(("child_peak_mib", peak_bytes(pid)? / 1048576.0));
    }
    r.push(("rt_busy_p50_us", percentile(&mut busy, 50)));
    r.push(("rt_busy_p99_us", percentile(&mut busy, 99)));

    // Round trips with nothing running.
    let mut idle = Vec::with_capacity(IDLE_ROUND_TRIPS);
    for _ in 0..IDLE_ROUND_TRIPS {
        let t = Instant::now();
        engine.api().snapshot().map_err(|e| format!("{e:?}"))?;
        idle.push(t.elapsed().as_secs_f64() * 1e6);
    }
    r.push(("rt_idle_p50_us", percentile(&mut idle, 50)));
    r.push(("rt_idle_p99_us", percentile(&mut idle, 99)));

    // Shutdown with a download under way: the in-process engine cancels and waits for its workers;
    // the child client closes the child's stdin and waits for it to exit.
    let dest2 = dir.join(format!("{tag}-shutdown.bin"));
    let id2 = engine
        .api()
        .add(spec(url, dest2.clone(), connections))
        .map_err(|e| format!("{e:?}"))?;
    wait_for(
        &mut engine,
        &events,
        id2,
        |s| s == DownloadStatus::Downloading,
        None,
    )?;
    let t = Instant::now();
    match engine {
        Engine::InProcess(e) => {
            e.cancel_all();
            while !e.idle() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Engine::Child(e) => drop(e),
    }
    r.push(("shutdown_ms", t.elapsed().as_secs_f64() * 1e3));
    if dest2.exists() || part_of(&dest2).exists() {
        return Err("shutdown left a file behind, or the download finished first".into());
    }

    // A killed child: how long until the client notices, and what it leaves behind.
    if mode == "child" {
        let mut engine = Engine::start(mode, program)?;
        let events = engine.api().subscribe();
        let dest3 = dir.join(format!("{tag}-crash.bin"));
        let id3 = engine
            .api()
            .add(spec(url, dest3.clone(), connections))
            .map_err(|e| format!("{e:?}"))?;
        wait_for(
            &mut engine,
            &events,
            id3,
            |s| s == DownloadStatus::Downloading,
            None,
        )?;
        let pid = engine.child_pid().ok_or("no child")?;
        // Timed from when the kill has finished, so the kill command's own start-up isn't counted.
        kill_process(pid)?;
        let t = Instant::now();
        loop {
            match events.recv_timeout(WAIT) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => return Err("the crash went unnoticed".into()),
            }
        }
        r.push(("crash_detect_ms", t.elapsed().as_secs_f64() * 1e3));
        let unavailable = engine.api().snapshot().is_err();
        let left_part = part_of(&dest3).exists();
        println!("crash: later commands fail = {unavailable}, part file left = {left_part}");
        drop(engine);
        let _ = std::fs::remove_file(part_of(&dest3));
    }

    // Correctness last, so its memory doesn't count.
    let ok = std::fs::read(&dest).map_err(|e| e.to_string())? == seeded_bytes(len, SEED);
    let _ = std::fs::remove_file(&dest);
    if !ok {
        return Err("the downloaded file differs from the source".into());
    }
    let line: Vec<String> = std::iter::once(format!("mode={mode}"))
        .chain(r.iter().map(|(k, v)| format!("{k}={v:.3}")))
        .collect();
    println!("result {}", line.join(" "));
    Ok(())
}

#[cfg(target_os = "linux")]
fn cpu_seconds(pid: u32) -> Result<f64, String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    // Fields after the command name, which ends at the last ')': utime and stime are fields 14
    // and 15 of proc(5), so indexes 11 and 12 here.
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .ok_or("bad stat")?
        .1
        .split_whitespace()
        .collect();
    let ticks: f64 = fields[11].parse::<f64>().map_err(|e| e.to_string())?
        + fields[12].parse::<f64>().map_err(|e| e.to_string())?;
    let out = Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .map_err(|e| e.to_string())?;
    let hz: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| "getconf CLK_TCK".to_string())?;
    Ok(ticks / hz)
}

#[cfg(target_os = "linux")]
fn peak_bytes(pid: u32) -> Result<f64, String> {
    let status =
        std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| e.to_string())?;
    let kib: f64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .ok_or("no VmHWM")?;
    Ok(kib * 1024.0)
}

/// Windows has no `/proc`; without a new dependency for the API, `Get-Process` reads the same
/// counters. The command is fixed text plus a process id, never outside input.
#[cfg(windows)]
fn process_counter(pid: u32, expression: &str) -> Result<f64, String> {
    let script = format!(
        "$p = Get-Process -Id {pid}; [string]::Format([Globalization.CultureInfo]::InvariantCulture, '{{0}}', {expression})"
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| format!("Get-Process for {pid} failed"))
}

#[cfg(windows)]
fn cpu_seconds(pid: u32) -> Result<f64, String> {
    process_counter(pid, "$p.TotalProcessorTime.TotalSeconds")
}

#[cfg(windows)]
fn peak_bytes(pid: u32) -> Result<f64, String> {
    process_counter(pid, "$p.PeakWorkingSet64")
}
