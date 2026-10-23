-- Schema 2 (#43): the checkpoints of direct downloads, to resume them after a pause, a restart or
-- a crash. The engine sends one after syncing its part file; the app saves it here.
-- `document` is a `Checkpoint` (crates/velocity-core/src/transfer.rs) as JSON: the durable byte
-- ranges, the total and the validators, with no credential and no URL query.
-- (`downloads.checkpoint_ref`, reserved in schema 1, stays unused: a checkpoint has its own row.)
CREATE TABLE checkpoints (
    download_id   TEXT PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
    document      TEXT NOT NULL CHECK (json_valid(document)),
    updated_at_ms INTEGER NOT NULL
);
