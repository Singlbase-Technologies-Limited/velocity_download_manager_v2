//! Links the libcurl V2 builds itself (`pnpm native:build`, docs/BUILDING_NATIVE.md), never a
//! bundled or system copy (ADR-0004). `.cargo/config.toml` overrides the build scripts of
//! `curl-sys` and `libz-sys`; this script supplies the libraries instead, and refuses to build
//! without a build identity record whose libcurl meets the version floor.

use std::env;
use std::path::{Path, PathBuf};

/// The libcurl version floor (research report section 8.1); `LIBCURL_FLOOR` in
/// scripts/native/native-lib.mjs and `velocity_curl::LIBCURL_FLOOR` state the same value.
const LIBCURL_FLOOR: [u32; 3] = [8, 23, 0];

fn main() {
    println!("cargo:rerun-if-env-changed=VELOCITY_NATIVE_DIR");
    let target = env::var("TARGET").expect("cargo sets TARGET");
    let native = env::var("VELOCITY_NATIVE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| fail("VELOCITY_NATIVE_DIR is not set; .cargo/config.toml sets it, so build from the V2 workspace"));
    let out = native.join(&target);
    let info_path = out.join("build-info.json");
    println!("cargo:rerun-if-changed={}", info_path.display());

    let text = std::fs::read_to_string(&info_path).unwrap_or_else(|_| {
        fail(&format!(
            "the native libraries for {target} aren't built: {} is missing. Run `pnpm native:build` first (docs/BUILDING_NATIVE.md)",
            info_path.display()
        ))
    });
    let info: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        fail(&format!(
            "{} is not valid JSON ({e}). Run `pnpm native:build --force`",
            info_path.display()
        ))
    });
    check_record(&info, &target, &out);

    let lib = out.join("prefix").join("lib");
    println!("cargo:rustc-link-search=native={}", lib.display());
    let (static_libs, system_libs) = libraries(&target);
    for name in static_libs {
        println!("cargo:rustc-link-lib=static={name}");
    }
    for name in system_libs {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
}

/// The static libraries from the native build, in link order, and the system libraries they need.
fn libraries(target: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match target {
        // The system libraries are the `Libs.private` curl's own build writes into libcurl.pc.
        "x86_64-pc-windows-msvc" => (
            &["libcurl", "nghttp2", "zs"],
            &[
                "bcrypt", "advapi32", "crypt32", "secur32", "ws2_32", "iphlpapi",
            ],
        ),
        // libcurl.pc lists no system libraries here. A static OpenSSL still needs libpthread and
        // libdl on glibc before 2.34, which keeps them out of libc; later glibc ignores them.
        "x86_64-unknown-linux-gnu" => (
            &["curl", "nghttp2", "ssl", "crypto", "z"],
            &["pthread", "dl"],
        ),
        other => fail(&format!(
            "no native build is established for {other} (docs/BUILDING_NATIVE.md)"
        )),
    }
}

/// The parts of build-info.json a build depends on. `checkBuildInfo` in native-lib.mjs is the full
/// check (sources, signatures, artefact hashes), run by `pnpm native:verify`.
fn check_record(info: &serde_json::Value, target: &str, out: &Path) {
    let rebuild = "Run `pnpm native:build` (docs/BUILDING_NATIVE.md)";
    if info["schema"] != 1 {
        fail(&format!(
            "build-info.json has schema {}, but this build reads schema 1. {rebuild}",
            info["schema"]
        ));
    }
    if info["target"] != target {
        fail(&format!(
            "build-info.json is for {}, not {target}. {rebuild}",
            info["target"]
        ));
    }
    let libcurl = info["linked"]["libcurl"].as_str().unwrap_or_else(|| {
        fail(&format!(
            "build-info.json doesn't record the linked libcurl. {rebuild}"
        ))
    });
    let parsed: Vec<u32> = libcurl.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    if parsed.as_slice() < LIBCURL_FLOOR.as_slice() {
        fail(&format!("the native build links libcurl {libcurl}, below the version floor 8.23.0. Update scripts/native/deps.lock, then {rebuild}"));
    }
    let artifacts = info["artifacts"]
        .as_array()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| fail(&format!("build-info.json lists no artefacts. {rebuild}")));
    for a in artifacts {
        let rel = a["path"].as_str().unwrap_or_default();
        let path = out.join("prefix").join(rel);
        if rel.is_empty() || !path.is_file() {
            fail(&format!(
                "{} is listed in build-info.json but missing. {rebuild}",
                path.display()
            ));
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    panic!("velocity-curl: {message}");
}
