-- doc/PLAN.md §20.5: the newest known `AreaSetting` (the robot's zone list) per
-- device. `version` is an optimistic-concurrency etag: it increments on every read
-- back from the robot and on every accepted write, so an editor can only save the
-- list it actually read. `previous_*` keeps one snapshot of the list as it was
-- before the last write.
CREATE TABLE area_settings (
    sn               TEXT PRIMARY KEY,
    version          INTEGER NOT NULL DEFAULT 0,
    read_ms          INTEGER,
    payload          TEXT,
    trace_ref        TEXT,
    previous_payload TEXT,
    previous_ms      INTEGER
);
