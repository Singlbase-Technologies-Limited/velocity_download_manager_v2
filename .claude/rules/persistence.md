---
# pending: "crates/*/src/**/checkpoint*.rs" (#43)
paths:
  - "crates/*/src/database/**/*"
  - "crates/*/migrations/**/*"
  - "crates/*/src/**/checkpoint*.rs"
---

# Persistence, migrations and checkpoints

- **Migrations:**
  - Every schema change is a new, numbered, versioned migration that runs in a transaction.
  - A migration that has been released is never edited.
  - Back up the database before migrating.
- **User data is never deleted or recreated** to repair a failed migration. Fail with a clear error and keep the original file.
- **Newer schemas:** a database with a newer schema than the build understands is refused unmodified, never downgraded in place.
- **Unknown values:** an unknown enum value read from storage (an engine, status or kind) becomes an explicit *unavailable* state. It is never mapped to a default that would act on it (report section 6).
- **No secrets:** credentials, cookies, custom headers and SSH keys are never stored in the database or in checkpoints. Checkpoints store a URL fingerprint, not query tokens.
- **Atomic writes:** checkpoint and state-file writes are atomic (write a temporary file, then rename, or use one transaction). Files are opened without following links, with owner-only permissions where the platform supports it.
- **Tests:** every migration is tested from the previous schema with representative data, including a refused newer schema.
