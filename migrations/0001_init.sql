-- noobscenic initial schema — see doc/PLAN.md section 7.
--
-- Timestamps ending in _ms are server-clock epoch milliseconds. The device's own
-- ts/createtime values are kept verbatim as text alongside them, because the robot's
-- clock is not trustworthy before it syncs.
--
-- trace_ref is "<trace file>#<seq>", pointing at the exact bytes in the wire trace
-- that produced the row.

CREATE TABLE devices (
    sn            TEXT PRIMARY KEY,
    ld_sn         TEXT,
    company_id    INTEGER,
    mcu_ver       TEXT,
    app_version   TEXT,
    version_code  INTEGER,
    git_sha       TEXT,
    cloud         TEXT,
    bound_user    TEXT,
    bind_state    TEXT    NOT NULL DEFAULT 'unbound',
    first_seen_ms INTEGER NOT NULL,
    last_seen_ms  INTEGER NOT NULL
);

CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY,
    sn           TEXT    NOT NULL REFERENCES devices(sn),
    -- <=63 chars: the device does strncpy(dst, data.session, 0x40). Bytes [0..16)
    -- are the channel-B AES-128-ECB key.
    session_key  TEXT    NOT NULL,
    -- <=55 chars: strncpy(dst, data.cookies, 0x38). Echoed back by the device on
    -- every channel-A request as the header `Cookie: cookies=<value>`.
    cookie       TEXT    NOT NULL UNIQUE,
    -- The register `sig`, stored verbatim and never verified: it is
    -- RSA_public_encrypt("<sn>:<time>") under a key whose private half we do not have.
    sig_raw      TEXT,
    created_ms   INTEGER NOT NULL,
    last_used_ms INTEGER,
    revoked_ms   INTEGER
);

CREATE INDEX sessions_sn ON sessions(sn);

-- Every semantic message, from either channel.
CREATE TABLE events (
    id          INTEGER PRIMARY KEY,
    sn          TEXT,
    channel     TEXT    NOT NULL,  -- 'A' | 'B'
    direction   TEXT    NOT NULL,  -- 'in' | 'out'
    endpoint    TEXT,              -- channel-A path
    info_type   INTEGER,
    event_code  INTEGER,           -- the `event` field of uploadEvents
    task_id     TEXT,
    user_id     TEXT,
    device_ts   TEXT,
    payload     TEXT,              -- the `data` JSON, or the whole frame if unparsable
    trace_ref   TEXT,
    received_ms INTEGER NOT NULL
);

CREATE INDEX events_sn_ms ON events(sn, received_ms);
CREATE INDEX events_type ON events(info_type);

-- infoType 20002. Grids are kept exactly as received (LZ4 block format) and decoded
-- on demand, so a wrong guess about cell semantics costs nothing.
CREATE TABLE map_uploads (
    id           INTEGER PRIMARY KEY,
    sn           TEXT    NOT NULL,
    map_id       INTEGER,
    auto_area_id INTEGER,
    path_id      INTEGER,
    width        INTEGER,
    height       INTEGER,
    resolution   REAL,              -- metres per cell
    x_min        REAL,
    y_min        REAL,
    lz4_len      INTEGER,
    cells_lz4    BLOB,
    dock_x       INTEGER,
    dock_y       INTEGER,
    dock_phi     INTEGER,
    dock_state   TEXT,              -- 'find' | 'notFind'
    areas_json   TEXT,
    trace_ref    TEXT,
    received_ms  INTEGER NOT NULL
);

CREATE INDEX map_uploads_sn_ms ON map_uploads(sn, received_ms);

-- infoType 21011, reassembled from startPos/totalPoints chunks.
CREATE TABLE clean_paths (
    id           INTEGER PRIMARY KEY,
    sn           TEXT    NOT NULL,
    path_id      INTEGER,
    user_id      TEXT,
    total_points INTEGER,
    points_json  TEXT,
    complete     INTEGER NOT NULL DEFAULT 0,
    updated_ms   INTEGER NOT NULL,
    UNIQUE(sn, path_id)
);

-- The outbound command queue. The gateway polls this table, which is what lets the
-- console, a one-shot CLI run, or an external process all enqueue work with no IPC.
CREATE TABLE commands (
    id          INTEGER PRIMARY KEY,
    sn          TEXT    NOT NULL,
    info_type   INTEGER NOT NULL,
    payload     TEXT    NOT NULL,
    encrypt     INTEGER NOT NULL DEFAULT 0,
    state       TEXT    NOT NULL,  -- pending|sent|acked|expired|failed
    created_ms  INTEGER NOT NULL,
    sent_ms     INTEGER,
    ack_ms      INTEGER,
    ack_payload TEXT,
    error       TEXT
);

CREATE INDEX commands_pending ON commands(sn, state, created_ms);

-- uploadLogs / uploadStats / uploadSingle, and anything else we do not model yet.
CREATE TABLE uploads_raw (
    id          INTEGER PRIMARY KEY,
    sn          TEXT,
    endpoint    TEXT    NOT NULL,
    fields_json TEXT,
    trace_ref   TEXT,
    received_ms INTEGER NOT NULL
);
