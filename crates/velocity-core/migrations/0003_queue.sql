-- Schema 3 (#50): the download queue. `queue_position` is a waiting download's place, 0 first;
-- NULL for one outside the queue (finished, or saved before schema 3). `scheduled` downloads run
-- only while the schedule's window is open. Paused is the record's status. The manager saves the
-- queue; loading it when the app starts is #58.
ALTER TABLE downloads ADD COLUMN queue_position INTEGER CHECK (queue_position IS NULL OR queue_position >= 0);
ALTER TABLE downloads ADD COLUMN scheduled INTEGER NOT NULL DEFAULT 0 CHECK (scheduled IN (0, 1));
CREATE INDEX idx_downloads_queue_position ON downloads(queue_position);
