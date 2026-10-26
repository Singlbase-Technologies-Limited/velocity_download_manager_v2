# Data and migrations

V2 keeps its download records, settings and categories in one SQLite database (#26). This document describes the file, the schema and how it is upgraded. The rules behind it are in [`.claude/rules/persistence.md`](../.claude/rules/persistence.md).

The code is in [`crates/velocity-core/src/database/`](../crates/velocity-core/src/database/mod.rs), and the migrations are in [`crates/velocity-core/migrations/`](../crates/velocity-core/migrations/). The app opens it when it starts, and the manager saves its downloads there (#58, [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#saving-and-starting-again-58)).

## The file

- **Where:** `Paths::database()` gives the location, in V2's own per-user data folder, separate from V1's ([COEXISTENCE.md](COEXISTENCE.md)).
- **Permissions:** a new database is created owner-only (`0600` on Unix) before SQLite opens it, and SQLite's `-wal` and `-shm` files take its mode. On Windows the per-user data folder's permissions apply.
- **No links:** a link, or anything else that isn't a plain file, in place of the database is refused (`INVALID_PATH`); it is never followed.
- **How SQLite runs:**
  - WAL journal mode, for atomic, crash-safe commits;
  - `synchronous=NORMAL`;
  - foreign keys on;
  - a 5-second busy timeout;
  - *defensive* mode, which refuses SQL that could corrupt the file. Both SQLite CVEs fixed in 3.53.2 needed defensive mode off.
- **SQLite:** 3.53.2, compiled into V2 from the source inside `libsqlite3-sys` ([DEPENDENCIES.md](DEPENDENCIES.md#approved-direct-dependencies)).

## Schema

The schema version is `PRAGMA user_version`. Version 1 ([`0001_initial.sql`](../crates/velocity-core/migrations/0001_initial.sql)) is V2's clean baseline, version 2 adds checkpoints, version 3 the download queue, and version 4 the verification. V1's data isn't imported ([ADR-0007](decisions/0007-no-v1-import-at-first.md)).

| Table | Holds |
|---|---|
| `downloads` | One download per row. It records the engine (`velocity` or `libtorrent`), kind, status, source URL (never with a login), folder, file name and category. It also has the byte counts, the last error, the options to retry it, and `has_sensitive_options`. The history flags (`in_list`, `in_history`) are here too, along with the torrent info hashes (v1 and v2) and when seeding started. For direct downloads, nullable `etag`, `last_modified`, `validator_kind` (#45) and `checkpoint_ref` (#43) columns are reserved. They stay unused: #45 keeps what the first answer said about the file in the checkpoint |
| `downloads` (schema 3) | [`0003_queue.sql`](../crates/velocity-core/migrations/0003_queue.sql), #50: `queue_position`, an unfinished download's place in the queue (0 first; NULL outside it), and `scheduled`, whether it runs only while the schedule's window is open. `Database::save_queue` replaces the whole queue in one transaction and refuses an unknown download with nothing changed; `Database::queue` reads it back in order. A download added paused keeps `paused` as its status. The manager gives its queue in that shape (`Manager::queue`); loading it when the app starts is #58 |
| `downloads` (schema 4) | [`0004_verification.sql`](../crates/velocity-core/migrations/0004_verification.sql), #58: `verification`, what was proven about a completed download's bytes (#46), by its wire name; NULL until it completes. Its last error is in `error_json` (since schema 1), read back as `Stored::Unknown` when this build can't read it. `Database::update_download` saves a record's status, name, byte counts, error, verification and completion time; `update_options` its options after a limit or connections change; `hide_download` takes it out of the list (`in_list = 0`), keeping it in the history. A record is written with only the columns every schema has, so the upgrade tests can insert into older schemas |
| `categories` | Built-in and user categories, with their extensions as JSON. The built-in rows arrive with #31 |
| `settings` | One row: the settings document as a JSON object, checked by SQLite (`json_type = 'object'`). Its fields are #30 |
| `checkpoints` | Schema 2 ([`0002_checkpoints.sql`](../crates/velocity-core/migrations/0002_checkpoints.sql), #43): one row per direct download, its last `Checkpoint` as JSON. It holds the durable byte ranges, the total, the validators and the content encoding (#45), with no credential and no URL query. A checkpoint saved before the encoding existed reads as having none. It is removed with its download. A checkpoint in another format, or one this build can't read, is `Stored::Unknown` and never used. `downloads.checkpoint_ref`, reserved in schema 1, stays unused |

Kinds and statuses are stored by their wire names (`http`, `queued`, `fetchingMetadata`), as in [CONTRACTS.md](CONTRACTS.md). Counts and times are integers; times are milliseconds since the Unix epoch.

## Unknown values

A value this build doesn't know, read back from the database, is kept as `Stored::Unknown(raw)`, never mapped to a default. That covers an engine, a kind, a status, or options it can't read. `StoredDownload::route()` hands a record to an engine only when every one of those is known. Otherwise it's `ENGINE_UNAVAILABLE`: the record is shown and kept, never acted on (report section 6). A record never changes engine.

## Secrets

- **Never stored:** credentials, cookies, custom header values and SSH keys. They stay in memory (#70).
- **By construction:** `StoredOptions` has no field for any of them.
- **URL logins:** `without_login` removes a login from a URL (`https://user:pass@host/` becomes `https://host/`), and `insert_download` applies it again.
- **What is kept:** `has_sensitive_options` records that a download had credentials, headers, an SSH key or a URL login. After a restart such a download comes back paused, and resumes or is retried only once they are given again (PO decision on #58).
- **The options to start again** (`StoredOptions`, #58): every source without its login, the user agent, the referrer without its login, query and fragment (which can carry a session), the checksum, the piece hashes, the speed limit and the connections. Fields added since schema 1 default when absent, so older records still read.
- **Tested:** a test adds a download with a password in its URL, a login (`credentials`, #40) and a cookie header. It then searches every file SQLite wrote, with the WAL and after closing, and finds none of them.

## Upgrades

When the app opens the database:

1. **A newer database is refused unmodified.** The version is read from the file's header before SQLite opens it, so nothing is written. A database from a later V2 is `DATABASE_MIGRATION`, and the file and its folder are left exactly as they were. Downgrading needs the backup that the later version made.
2. **Before an upgrade, a backup.** An existing database at an older version is copied next to itself with `VACUUM INTO`, as `<file>.bak-v<old version>-<ms>`, owner-only.
3. **The migrations run in one transaction** ([`rusqlite_migration`](DEPENDENCIES.md#approved-direct-dependencies)), and `user_version` is set to the new version.
4. **A failed upgrade changes nothing.** The transaction rolls back, and the database stays at its old version with its data. The error is `DATABASE_MIGRATION`, and the backup stays. User data is never deleted or recreated to repair a migration.

## Adding a migration

- **Add a numbered file** to `crates/velocity-core/migrations/` (`0002_….sql`) and list it in `MIGRATIONS` in `database/mod.rs`.
- **Never edit a released one.**
- **Keep it additive:** new tables, new nullable columns, or columns with defaults. Schema choices for checkpoints (#43) and validators (#45) come this way.
- **Test it** from the previous schema with representative data. The tests in `database/mod.rs` show the pattern with a test-only second migration: the upgrade keeps the data, the backup holds the old version, a failed migration keeps both, and a newer database is refused.
