---
name: security-check
description: V2-specific security review of the current branch's diff (untrusted input, URL and redirect allowlists, TLS, credentials and secrets, SFTP host keys, filesystem safety, engine IPC, dependencies). Complements the built-in /security-review, which it does not repeat. Manual.
disable-model-invocation: true
---

# V2 security check

Review `git diff origin/main...HEAD` against the V2-specific controls below. For the generic review (injection, auth, crypto misuse), also run the built-in `/security-review`; don't repeat it here. The rules are in AGENTS.md ("Security and data") and `.claude/rules/security.md`.

## Checklist

For each item, answer **not touched**, **OK** (with file and line evidence) or **finding**.

| Area | What to look for in the diff |
|---|---|
| Untrusted input | URLs, names, paths, headers, server responses, torrent metadata, extension and engine messages parsed with types at the native boundary, before use |
| Processes | No shell; argument lists only; no secret in arguments or environment |
| URL and redirects | Scheme allowlist; redirects only to `http` and `https`, so never SFTP or `file`; https→http needs the user's confirmation (#67) |
| TLS | Verification on for every backend; no option or test helper that disables it in production code (#69) |
| Credentials | Sent only to the original host; cookies and custom headers dropped across origins; no `.netrc`; proxies set explicitly, with no ambient `http_proxy` (#70) |
| Secrets at rest | Never in the database, logs, checkpoints, temporary files, the UI or diagnostics; redaction applied (#28, #71) |
| SFTP | Unknown host key refused until the user confirms it; a changed key always refused; unhashed known_hosts plus a SHA-256 pin (#53) |
| Filesystem | Path containment; no following links or reparse points; deletes only files the operation owns; atomic, no-clobber finalisation; owner-only permissions on checkpoints (#43, #72) |
| Engine boundary | Engine messages validated for size, schema and version; no listening socket or port for engine IPC (#39) |
| Browser bridge | Message shape, version and size checked in the native host and again in the app; command allowlist; private-network policy (#65, #68) |
| Tauri surface | Capabilities in `src-tauri/capabilities/` and the CSP not widened without PO approval |
| Dependencies | New or updated dependencies have PO approval, a licence check and an advisory check (#9, #75) |
| Tests | A regression test for each security-relevant change; no control weakened to make a test pass |

## Report

List the findings first, most severe first. Give each one a file and line, a concrete failure scenario, and a proposed fix. Then list the areas marked OK, with their evidence, and the ones not touched. A finding that needs a security trade-off is a PO decision: ask, don't decide.
