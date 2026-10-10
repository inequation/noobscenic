-- doc/PLAN.md §20.6: the one-slot map backup the robot uploads when a clean ends.
-- We keep only the newest per device (operator decision, 2026-10-10) — enough to
-- export it to the user on demand and to serve it back for a 21025 restore. `token`
-- is the unguessable path segment the robot fetches the blob from; `md5` is computed
-- by us from the exact bytes stored, because that is the value the robot compares.
CREATE TABLE map_backups (
    sn          TEXT PRIMARY KEY,
    received_ms INTEGER NOT NULL,
    record_name TEXT,
    md5         TEXT NOT NULL,
    size        INTEGER NOT NULL,
    token       TEXT NOT NULL,
    blob        BLOB NOT NULL
);
