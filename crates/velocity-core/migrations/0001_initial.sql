-- Velocity Download Manager V2: schema 1, the clean V2 baseline (#26, docs/DATA_AND_MIGRATIONS.md).
-- Released migrations are never edited; every change is a new, additive migration.
-- No credential, cookie, header value or SSH key is ever stored here.

-- Built-in and user categories. The built-in rows arrive with #31.
CREATE TABLE categories (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE COLLATE NOCASE,
    color           TEXT NOT NULL,
    icon            TEXT NOT NULL,
    extensions_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(extensions_json)),
    directory       TEXT,
    builtin         INTEGER NOT NULL DEFAULT 0 CHECK (builtin IN (0, 1)),
    sort_order      INTEGER NOT NULL DEFAULT 0
);

-- One download per row. Values the build doesn't know (an engine, a kind or a status) are read as
-- explicitly unavailable, never mapped to a default that would act on them.
CREATE TABLE downloads (
    id                    TEXT PRIMARY KEY,
    -- 'velocity' (the direct-download engine) or 'libtorrent'. A record never changes engine.
    engine                TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    status                TEXT NOT NULL,
    -- The source with any login removed (#70).
    source_url            TEXT NOT NULL,
    directory             TEXT NOT NULL,
    file_name             TEXT,
    category_id           TEXT REFERENCES categories(id) ON DELETE SET NULL,
    total_bytes           INTEGER CHECK (total_bytes IS NULL OR total_bytes >= 0),
    completed_bytes       INTEGER NOT NULL DEFAULT 0 CHECK (completed_bytes >= 0),
    uploaded_bytes        INTEGER NOT NULL DEFAULT 0 CHECK (uploaded_bytes >= 0),
    error_json            TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
    -- Options needed to retry, without secrets.
    options_json          TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(options_json)),
    -- The download had credentials or custom headers, which are kept in memory only (#70).
    has_sensitive_options INTEGER NOT NULL DEFAULT 0 CHECK (has_sensitive_options IN (0, 1)),
    created_at_ms         INTEGER NOT NULL,
    completed_at_ms       INTEGER,
    -- History flags: shown in the download list, kept in the history.
    in_list               INTEGER NOT NULL DEFAULT 1 CHECK (in_list IN (0, 1)),
    in_history            INTEGER NOT NULL DEFAULT 1 CHECK (in_history IN (0, 1)),
    -- Torrents: the v1 and v2 info hashes, and when seeding started.
    info_hash             TEXT,
    info_hash_v2          TEXT,
    seeding_started_at_ms INTEGER,
    -- Direct downloads: the validator seen (#45) and the checkpoint's reference (#43).
    etag                  TEXT,
    last_modified         TEXT,
    validator_kind        TEXT,
    checkpoint_ref        TEXT
);

CREATE INDEX idx_downloads_status ON downloads(status);
CREATE INDEX idx_downloads_in_list ON downloads(in_list);
CREATE INDEX idx_downloads_info_hash ON downloads(info_hash);
CREATE INDEX idx_downloads_info_hash_v2 ON downloads(info_hash_v2);

-- The settings, as one JSON object; #30 defines and validates its fields.
CREATE TABLE settings (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    document      TEXT NOT NULL CHECK (json_valid(document) AND json_type(document) = 'object'),
    updated_at_ms INTEGER NOT NULL
);
