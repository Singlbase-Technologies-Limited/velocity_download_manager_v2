-- Schema 5 (#58): what the app keeps of each torrent besides its `downloads` row, which has the
-- engine `libtorrent`. `options_json` is how the torrent is added again: where it came from (a
-- `.torrent` file's path, or its magnet link without trackers), the file choice, the seeding
-- policy and the limits. `metadata` is the torrent's metadata (`d4:info…e`), checked before it is
-- stored. `resume_data` is libtorrent's resume data without its trackers, which can carry a
-- private tracker's passkey (PO decision on #58). Removed with its download.
CREATE TABLE torrents (
    download_id   TEXT PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
    options_json  TEXT NOT NULL CHECK (json_valid(options_json)),
    metadata      BLOB,
    resume_data   BLOB,
    updated_at_ms INTEGER NOT NULL
);
