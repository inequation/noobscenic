-- doc/PLAN.md §20.7: the newest `21015` goods reply per device — the consumables
-- counters (battery, filter, mainBrush, sideBrush, sensors) in seconds of cleaning
-- time (FUNC_STATUS.md §7). One row per device, overwritten on every read.
CREATE TABLE goods (
    sn        TEXT PRIMARY KEY,
    read_ms   INTEGER,
    payload   TEXT,
    trace_ref TEXT
);
