-- doc/PLAN.md §20.7: the newest `21019` network/version reply per device —
-- Wi-Fi addresses and signal, `compileVer`/`mcuVer`. One row per device.
CREATE TABLE device_info (
    sn        TEXT PRIMARY KEY,
    read_ms   INTEGER,
    payload   TEXT,
    trace_ref TEXT
);
