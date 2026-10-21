---
# browser-extension/ is created in #64.
paths:
  - "crates/**/*"
  - "src-tauri/**/*"
  - "scripts/**/*"
  - "browser-extension/**/*"
---

# Security

The baseline rules are in [AGENTS.md](../../AGENTS.md#security-and-data). In these paths, also:

- **Least privilege:** keep explicit trust boundaries between the UI, native commands, native messaging, the engine process and the file system. Don't widen Tauri capabilities (`src-tauri/capabilities/`) or the content security policy without PO approval.
- **Protocol allowlists:** direct downloads allow only the schemes in the allowlist. Redirects may go only to `http` and `https`, so they can never reach SFTP or `file`. An https→http downgrade needs the user's confirmation (#67).
- **TLS:** verification stays on for every backend, with the platform trust store. There is no setting to turn it off (#69).
- **Credentials:** they go only to the original host. Cookies and custom headers are dropped when a redirect leaves the origin. No `.netrc`, and no ambient proxy variables (#70).
- **SFTP:** an unknown host key is refused until the user confirms its fingerprint, and a changed key is always refused (#53).
- **Downloaded content:** never execute it, and never run commands supplied by torrents or servers.
- **Logs and diagnostics:** route them through the redaction layer (#28). A secret-leak scan covers the database, logs, checkpoints, temporary files and child-process arguments (#71).
- **Dependencies:** before adding or updating one, check its known advisories and its licence. Bundled binaries need their distribution obligations recorded (#9, #75, #76).
- **Regression tests:** every security fix gets one. Never weaken a control to make a test or an integration pass.
