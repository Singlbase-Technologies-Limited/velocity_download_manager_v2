//! Compiles the C++ shim against the libtorrent V2 builds with vcpkg (`pnpm native:build`,
//! docs/BUILDING_NATIVE.md), never a system copy. The build writes `libtorrent-info.json` beside
//! libcurl's `build-info.json`: the install prefix, the compile definitions libtorrent was built
//! with (the shim must use the same ones, or the two disagree about libtorrent's types) and the
//! libraries to link. This script refuses to build without that record.

use std::env;
use std::path::{Path, PathBuf};

/// The libtorrent version V2 builds and tests against (PO decision on #62).
const LIBTORRENT_VERSION: &str = "2.0.15";

fn main() {
    for f in ["src/lib.rs", "cpp/shim.h", "cpp/shim.cpp"] {
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rerun-if-env-changed=VELOCITY_NATIVE_DIR");
    let target = env::var("TARGET").expect("cargo sets TARGET");
    let native = env::var("VELOCITY_NATIVE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| fail("VELOCITY_NATIVE_DIR is not set; .cargo/config.toml sets it, so build from the V2 workspace"));
    let info_path = native.join(&target).join("libtorrent-info.json");
    println!("cargo:rerun-if-changed={}", info_path.display());

    let text = std::fs::read_to_string(&info_path).unwrap_or_else(|_| {
        fail(&format!(
            "libtorrent for {target} isn't built: {} is missing. Run `pnpm native:build` first (docs/BUILDING_NATIVE.md)",
            info_path.display()
        ))
    });
    let info: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        fail(&format!(
            "{} is not valid JSON ({e}). Run `pnpm native:build --force`",
            info_path.display()
        ))
    });
    let record = Record::read(&info, &target, &info_path);

    let mut b = cxx_build::bridge("src/lib.rs");
    b.file("cpp/shim.cpp")
        .std("c++17")
        .include(record.prefix.join("include"));
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        b.flag("/EHsc").flag("/bigobj").flag("/utf-8");
    }
    for d in &record.defines {
        match d.split_once('=') {
            Some((k, v)) => b.define(k, v),
            None => b.define(d, None),
        };
    }
    b.compile("velocity_lt_shim");

    println!(
        "cargo:rustc-link-search=native={}",
        record.prefix.join("lib").display()
    );
    for name in &record.static_libs {
        println!("cargo:rustc-link-lib=static={name}");
    }
    for name in &record.system_libs {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
}

/// The parts of `libtorrent-info.json` the build uses. `checkLibtorrentInfo` in
/// scripts/native/native-lib.mjs is the full check.
struct Record {
    prefix: PathBuf,
    defines: Vec<String>,
    static_libs: Vec<String>,
    system_libs: Vec<String>,
}

impl Record {
    fn read(info: &serde_json::Value, target: &str, path: &Path) -> Self {
        let rebuild = "Run `pnpm native:build` (docs/BUILDING_NATIVE.md)";
        if info["schema"] != 1 {
            fail(&format!(
                "{} has schema {}, but this build reads schema 1. {rebuild}",
                path.display(),
                info["schema"]
            ));
        }
        if info["target"] != target {
            fail(&format!(
                "{} is for {}, not {target}. {rebuild}",
                path.display(),
                info["target"]
            ));
        }
        if info["libtorrent"] != LIBTORRENT_VERSION {
            fail(&format!(
                "{} records libtorrent {}, but V2 builds against {LIBTORRENT_VERSION}. {rebuild}",
                path.display(),
                info["libtorrent"]
            ));
        }
        let prefix = PathBuf::from(
            info["prefix"]
                .as_str()
                .unwrap_or_else(|| fail(&format!("{} has no prefix. {rebuild}", path.display()))),
        );
        if !prefix.join("include").join("libtorrent").is_dir() {
            fail(&format!(
                "{} has no libtorrent headers. {rebuild}",
                prefix.display()
            ));
        }
        let list = |key: &str| -> Vec<String> {
            info[key]
                .as_array()
                .unwrap_or_else(|| {
                    fail(&format!("{} has no {key} list. {rebuild}", path.display()))
                })
                .iter()
                .map(|v| {
                    v.as_str()
                        .unwrap_or_else(|| {
                            fail(&format!(
                                "{} has a non-text entry in {key}. {rebuild}",
                                path.display()
                            ))
                        })
                        .to_owned()
                })
                .collect()
        };
        Self {
            prefix,
            defines: list("defines"),
            static_libs: list("staticLibs"),
            system_libs: list("systemLibs"),
        }
    }
}

fn fail(message: &str) -> ! {
    panic!("velocity-lt: {message}");
}
