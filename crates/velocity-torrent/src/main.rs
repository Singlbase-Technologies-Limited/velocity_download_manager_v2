//! The `velocity-torrent` program (#62, ADR-0014): the torrent helper, libtorrent in a child
//! process the app starts and supervises.
//!
//! ```text
//! velocity-torrent serve
//! ```

use std::process::ExitCode;

use velocity_torrent::stdio;

const USAGE: &str = "usage:
  velocity-torrent serve";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [serve] if serve == "serve" => stdio::serve(),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("velocity-torrent: {message}");
            ExitCode::FAILURE
        }
    }
}
