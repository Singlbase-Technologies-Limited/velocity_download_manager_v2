-- Schema 7 (#59): when a download first ran, how long it ran and the bytes it received while it
-- ran, for V1's "Started" and "Avg. speed". NULL and 0 for records saved before schema 7: nothing
-- was measured.
ALTER TABLE downloads ADD COLUMN started_at_ms INTEGER;
ALTER TABLE downloads ADD COLUMN active_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE downloads ADD COLUMN transferred_bytes INTEGER NOT NULL DEFAULT 0;
