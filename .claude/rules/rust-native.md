---
paths:
  - "crates/**/*.rs"
  - "src-tauri/**/*.rs"
  - "crates/*/Cargo.toml"
  - "src-tauri/Cargo.toml"
---

# Rust and native code

- **Layers:** keep `velocity-core` free of Tauri and UI types. Tauri commands in `src-tauri/` stay thin and typed. They validate input, call the core and map the result.
- **Errors:** propagate them with the crate's established `Result` and error types. Never discard an error, `unwrap` on input from outside the process, or turn a failure into a success state.
- **Concurrency:** never hold a lock across network, file or process I/O. Bound every timeout, retry, buffer, channel and pool.
- **Unsafe code and FFI:** `unsafe` and FFI live only in the crate that owns that native library: libcurl in the libcurl layer, libtorrent in the torrent bridge. Every `unsafe` block gets a `// SAFETY:` comment stating the invariant it relies on.
- **Processes:** start them with `std::process::Command` and argument lists. Never use a shell, and never pass secrets in arguments or environment variables.
- **Toolchain and versions:** use the pinned toolchain (`rust-toolchain.toml`) and the workspace dependency versions (`[workspace.dependencies]`). Check version-sensitive APIs against the docs for that exact version.
- **Tests:** unit tests sit beside the code in `mod tests`. Behaviour that crosses a process, network or file system boundary also gets an integration test against a local fixture.
- **Before a PR:** `pnpm test:focused -- <crate>` while iterating, then `pnpm check:fast`. Clippy runs with `-D warnings`; fix the warning instead of adding `#[allow]`, unless you explain the exception in a comment.
- **Names:** see [docs/CONVENTIONS.md](../../docs/CONVENTIONS.md#rust).
