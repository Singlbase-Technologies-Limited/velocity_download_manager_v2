---
# .github/ is created in #5 (templates) and #14 (workflows).
paths:
  - ".github/**/*"
  - "scripts/**/*"
  - "package.json"
  - "Cargo.toml"
  - "rust-toolchain.toml"
  - "src-tauri/tauri.conf.json"
---

# CI, scripts, packaging and releases

- **CI is GitHub Actions** (#12 to #14). CI runs the same commands as local `pnpm` scripts, so a check never exists in only one place.
- **Selection:** `check:pr` and CI change detection share the path rules in `scripts/checks-lib.mjs`. Change them only with tests in `scripts/checks-lib.test.mjs`. Unknown, shared and CI paths keep selecting everything.
- **Keep working validation** until its replacement runs and is verified. Never disable or delete a check because a new one was added.
- **Least privilege:**
  - Workflow permissions are least privilege, with a read-only token by default.
  - Pin third-party actions to a full commit SHA and review their updates.
  - Never expose secrets to pull requests from forks, or run fork code on a runner that has credentials.
- **Bounded runs:** keep routine runs deterministic and bounded. Cancel superseded runs, and add no duplicate pipelines. Before claiming a run is cheaper, measure its minutes and cache size.
- **Native inputs are supply-chain inputs:** each one needs its official source, version, checksum, licence and notice obligations recorded (#9, #35, #76).
- **No releases without explicit PO approval:** never publish, sign, upload, tag or release without it. A release is a tag on a verified `main` commit ([docs/DEVELOPMENT_WORKFLOW.md](../../docs/DEVELOPMENT_WORKFLOW.md#releases)).
- **Report platform coverage exactly.** A Linux build proves nothing about Windows, macOS, installers or signing.
- **Packaging identity:** changes to `tauri.conf.json` keep V2's identity separate from V1's. The tests in `src-tauri/src/lib.rs` guard this.
