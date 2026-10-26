-- Schema 4 (#58): what was proven about a completed download's bytes (#46), by its wire name
-- (`verified`, `sizeVerified`, `notVerified`); NULL until it completes, and for records saved before
-- schema 4.
ALTER TABLE downloads ADD COLUMN verification TEXT;
