# noobscenic — implementation plan

A clean-room replacement cloud for the Proscenic M7 Pro (LDRobot `LS_S6`), in Rust.
Everything below is derived **only** from `doc/reverse-engineering/` — chiefly
`PROTOCOL.md` (the wire contract), `REPORT.md` (§2, §4, §6), `MAP.md`, and
`schemas/*.json`. Section references in brackets point back at those documents.

---

## 1. Scope

**In scope (v1):** a single native executable that a re-homed M7 Pro can register
against and stay connected to indefinitely, that records everything the robot says,
and that can push commands back to it.

* **Channel A** — HTTP server for the `cleanPack/*` REST surface. [PROTOCOL §A]
* **Channel B** — raw-TCP push gateway, `#\t#`-framed JSON, including the mandatory
  `21006` keepalive pong. [PROTOCOL §B]
* **Channel C** — *client* side only, as a standalone Python tool used to re-home the
  robot (`setUrl` / `setSta`). The server never speaks channel C. [PROTOCOL §C]
* Persistence in SQLite via `sqlx`; JSON configuration; optional verbatim wire traces.
* An interactive **stdin/stdout console** to inspect state and enqueue commands.

**Out of scope (v1), deliberately:**

* systemd units, packaging, containers. It runs as `./noobscenic --config config.json`.
* Serving OTA firmware. The version endpoints answer "no update available".
  [REPORT §2.2]
* The vendor *app*-facing API (`/user/login`, `/instructions/<sn>/<code>`). The
  operator console replaces it. [PROTOCOL §A "App ⇄ cloud"]
* Anything touching signed firmware, the RSA keys, or root on the device. Re-homing
  needs none of it. [REPORT §6]
* Map rendering / a web UI / a Home Assistant bridge. The database and the command
  queue are the seam they will hang off later.

**Non-negotiable behaviours** (each one is a documented device failure mode):

1. `register` must return **`data.session`** *and* **`data.cookies`** — a `token`
   field instead means no AES key and an endless re-register loop. [PROTOCOL §B]
2. Every server→device channel-B frame must end in `#\t#` (`23 09 23`) — there is no
   length prefix. [PROTOCOL §B]
3. Every cloud→device command must carry an **integer `encrypt`** field, or the device
   silently drops the frame. `encrypt:0` = plaintext `data` object. [PROTOCOL §B]
4. Every `21006` Ping must be answered, or the robot loops connect→drop→reconnect.
   [PROTOCOL §B "Heartbeat"]
5. Device→cloud `data` is **always plaintext**; we never need to decrypt.
   [PROTOCOL §B "Payload crypto"]

---

## 2. Design principles

* **Permissive by default; log everything.** The RE docs flag several unknowns
  (§16). Every one of them is handled the same way: accept the input, persist it
  verbatim, answer with the most benign valid response, and emit a warning. A
  replacement server that 400s an unrecognised field just pushes the robot into a
  re-register or reconnect loop, which destroys the very traces we need.
* **Structured state in SQLite, verbatim bytes in trace files.** The database is the
  queryable model (devices, sessions, events, maps, commands). The trace files are the
  ground truth for protocol debugging, and can be replayed. Neither duplicates the
  other beyond a `trace_ref` pointer.
* **One dispatcher, two transports.** `infoType` payloads arrive on *both* channel B
  and channel A `uploadEvents` — `map_send.cpp` emits `20002` on both. [MAP.md] So
  message handling lives in `proto::dispatch`, and the channels are only framing.
* **Nothing reaches the vendor.** No outbound requests to `mobile.proscenic.cn` or the
  OSS host, ever. Offline operation is a feature, not an accident.
* **Clean room.** Implementation derives from `doc/reverse-engineering/` only. No
  vendor code, no decompiler output, no strings lifted from the binaries beyond the
  protocol constants already documented there.

---

## 3. Architecture

```
                         ┌──────────────────────── noobscenic ────────────────────────┐
                         │                                                            │
  robot ──HTTP POST────▶ │  channel_a  (axum)      ┐                                  │
  cleanPack/register     │    lenient form parser  │                                  │
  getSockAddr, sync,     │    session store        ├─▶ proto::dispatch ──▶ db (sqlx)  │
  response, upload*      │    catch-all logger     │      infoType router      SQLite  │
                         │                         │      map/path decode             │
  robot ──raw TCP──────▶ │  channel_b  (tokio)     ┘            │                     │
  {json}#\t#             │    frame codec  ◀───────── command queue ◀── console      │
  10001 / 21006 / 20002  │    conn registry (sn → writer)              (stdin/stdout) │
                         │                                                            │
                         │  wire::tap ──▶ traces/*.jsonl + traces/raw/*.bin (optional) │
                         └────────────────────────────────────────────────────────────┘

  operator ── tools/rehome.py ──UDP {"cmd":…}──▶ robot     (channel C, one-time setup)
```

Two tokio listeners in one process: HTTP (default `0.0.0.0:8080`) and the gateway
(`0.0.0.0:8081`). No privileged ports are needed — the robot's base URL and gateway
address are both fully configurable on the device. The operator surface is the process
console, not a third socket.

---

## 4. Technology choices

| Concern | Crate | Notes |
|---|---|---|
| async runtime | `tokio` | `rt-multi-thread`, `net`, `macros`, `signal`, `sync`, `time` |
| HTTP | `axum` + `tower`/`tower-http` | routing, middleware; body captured by our own layer |
| serialisation | `serde`, `serde_json` | `serde_json::value::RawValue` for verbatim passthrough |
| database | **`sqlx`** | `runtime-tokio`, `sqlite`, `migrate`; runtime-checked queries |
| logging | `tracing`, `tracing-subscriber`, `tracing-appender` | `EnvFilter` + rolling file |
| CLI | `clap` (derive) | `serve`, `migrate`, `devices`, `send`, `trace` |
| crypto (optional) | `aes` + `cipher` | AES-128-ECB, **padding disabled**, manual 16-byte block loop |
| base64 | `base64` | `Engine` API |
| map decode | `lz4_flex` | **block** format (`LZ4_compress_default`), not frame [MAP.md] |
| ids/keys | `rand` | session/cookie minting |
| time | `chrono` | display only; stored as `INTEGER` epoch millis |

Pin exact versions at `cargo add` time; the table names crates, not releases. Layout
is a single crate with `src/lib.rs` + `src/main.rs` so integration tests can drive the
server in-process.

`sqlx` is used with **runtime-checked** `query`/`query_as` rather than the compile-time
macros, so a build never depends on a live database or on checked-in `.sqlx` offline
data. Migrations are embedded with `sqlx::migrate!("./migrations")` and run at start-up.

---

## 5. Repository layout

```
Cargo.toml
config.example.json
migrations/
  0001_init.sql
src/
  main.rs               clap, config load, tracing init, signal handling
  lib.rs                run(config) -> the three listeners + shutdown
  config.rs             serde structs + defaults + validation
  error.rs              thiserror; never panics a connection task
  session.rs            mint/lookup/revoke; length limits (§9.2)
  db/
    mod.rs              pool setup, pragmas, migrate
    models.rs           row structs
    queries.rs          all SQL in one place
  wire/
    mod.rs              Tap: record(direction, channel, conn_id, bytes, meta)
    jsonl.rs            rotating JSONL sink
    raw.rs              per-connection verbatim byte sink
  channel_a/
    mod.rs              router, listener
    form.rs             lenient form-urlencoded parser (§9.1)
    handlers/           register, sock_addr, sync, response, uploads, binding, fallback
  channel_b/
    mod.rs              listener, accept loop
    codec.rs            #\t# framing (encode + incremental decode)
    conn.rs             per-connection read/write tasks, watchdog
    registry.rs         sn -> command sender; online state
    crypto.rs           optional AES-128-ECB for encrypt:1
  proto/
    mod.rs
    info_type.rs        constants: 10001, 20002, 21003, 21006, 21011, 21020
    messages.rs         typed payloads (map upload, clean path, …)
    dispatch.rs         shared A/B message handling
  map.rs                base64 -> lz4 block -> occupancy grid; area/region parsing
  path.rs               21011 chunk assembly
  console.rs            stdin/stdout operator console (§12)
  web.rs                the web UI's API handlers (§19)
  bin/
    simdev.rs           device simulator for tests (§14)
web/
  index.html            the UI page, embedded at compile time (§19)
tools/
  rehome.py             channel C client (§13)
  catchall.py           logs whatever connects, with TLS SNI extraction (diagnosis)
  fakerobot.py          channel C stand-in, for testing rehome.py without hardware
  map2png.py            renders a stored 20002 grid as a PNG, optionally with a
                        clean-path overlay (needs pillow + lz4)
tests/
  channel_b.rs  framing.rs  form.rs  register_flow.rs  e2e_simdev.rs
```

---

## 6. Configuration

Single JSON file, path from `--config` (default `./config.json`); missing file =
all defaults. A handful of CLI flags (`--data-dir`, `--http-bind`, `--log-level`,
`--no-wire-trace`) override it; `RUST_LOG` overrides the log filter. No env-var
soup beyond that.

```json
{
  "data_dir": "./var",
  "database": { "url": "sqlite://./var/noobscenic.db", "max_connections": 5 },

  "http":    { "bind": "0.0.0.0:8080", "tls": null },
  "gateway": { "bind": "0.0.0.0:8081",
               "advertise": null,
               "ping_timeout_secs": 120,
               "max_frame_bytes": 8388608,
               "ack_handshake": true,
               "announce_app_online": true,
               "encrypt_commands": false,
               "command_ttl_secs": 300 },
  "console": { "enabled": true },

  "registration": { "accept_all": true, "session_ttl_secs": null },
  "ota":     { "enabled": false },

  "logging": { "level": "info",
               "file": "./var/logs/noobscenic.log",
               "wire": { "enabled": true,
                         "dir": "./var/traces",
                         "format": "both",
                         "max_body_bytes": 262144,
                         "rotate_mb": 64,
                         "retain_days": 14,
                         "redact_secrets": false } }
}
```

* `gateway.advertise` is what `getSockAddr` hands the robot, as
  `data.addr_list:[{ip,port}]`. It **must be an address the robot can reach**, not the
  bind address. `null` = auto-detect: open a UDP socket, `connect()` it to the
  requesting peer's IP, read back the local address. Explicit form:
  `[{"ip":"192.168.1.10","port":8081}]`; multiple entries are allowed, the device
  iterates the list. [PROTOCOL §A]
* `registration.session_ttl_secs: null` = sessions never expire. Setting it makes the
  server answer `code:102` after the TTL, which is the documented way to force a
  re-register — useful for exercising that path on purpose. [PROTOCOL §B]
* `gateway.announce_app_online` (default `true`) makes the `21006` pong carry
  `"isExistConnect":true`. That is the pong the device both accepts *and* acts on:
  without the integer `encrypt` the frame is dropped at the inbound gate, and
  without the flag the robot never pushes status or map uploads. [FUNC_STATUS §2.3]
* `http.tls` stays `null` for v1. The robot does **no** certificate validation
  [REPORT §3], so a self-signed cert is enough whenever we do want `https://` — the
  only reason to bother is a DNS-override deployment that impersonates
  `mobile.proscenic.cn` instead of using `setUrl`.

---

## 7. Persistence (SQLite via sqlx)

Opened with `SqliteConnectOptions`: `create_if_missing(true)`, `journal_mode(Wal)`,
`foreign_keys(true)`, `busy_timeout(5s)`. Timestamps are `INTEGER` epoch millis
(server clock; the device's own `ts`/`createtime` are kept verbatim as text next to
them, because the robot's clock is not trustworthy before it syncs).

```sql
-- 0001_init.sql (sketch)
CREATE TABLE devices (
  sn TEXT PRIMARY KEY, ld_sn TEXT, company_id INTEGER,
  mcu_ver TEXT, app_version TEXT, version_code INTEGER, git_sha TEXT, cloud TEXT,
  bound_user TEXT, bind_state TEXT NOT NULL DEFAULT 'unbound',
  first_seen_ms INTEGER NOT NULL, last_seen_ms INTEGER NOT NULL);

CREATE TABLE sessions (
  id INTEGER PRIMARY KEY,
  sn TEXT NOT NULL REFERENCES devices(sn),
  session_key TEXT NOT NULL,          -- <=63 chars; [0..16) is the channel-B AES key
  cookie      TEXT NOT NULL UNIQUE,   -- <=55 chars; echoed as `Cookie: cookies=<v>`
  sig_raw TEXT,                       -- register `sig`, stored, never verified
  created_ms INTEGER NOT NULL, last_used_ms INTEGER, revoked_ms INTEGER);

CREATE TABLE events (                 -- every semantic message, either channel
  id INTEGER PRIMARY KEY, sn TEXT, channel TEXT NOT NULL, direction TEXT NOT NULL,
  endpoint TEXT, info_type INTEGER, event_code INTEGER, task_id TEXT, user_id TEXT,
  device_ts TEXT, payload TEXT, trace_ref TEXT, received_ms INTEGER NOT NULL);

CREATE TABLE map_uploads (
  id INTEGER PRIMARY KEY, sn TEXT NOT NULL, map_id INTEGER, auto_area_id INTEGER,
  path_id INTEGER, width INTEGER, height INTEGER, resolution REAL,
  x_min REAL, y_min REAL, lz4_len INTEGER, cells_lz4 BLOB,   -- stored compressed
  dock_x INTEGER, dock_y INTEGER, dock_phi INTEGER, dock_state TEXT,
  areas_json TEXT, trace_ref TEXT, received_ms INTEGER NOT NULL);

CREATE TABLE clean_paths (
  id INTEGER PRIMARY KEY, sn TEXT NOT NULL, path_id INTEGER, user_id TEXT,
  total_points INTEGER, points_json TEXT, complete INTEGER NOT NULL DEFAULT 0,
  updated_ms INTEGER NOT NULL, UNIQUE(sn, path_id));

CREATE TABLE commands (
  id INTEGER PRIMARY KEY, sn TEXT NOT NULL, info_type INTEGER NOT NULL,
  payload TEXT NOT NULL, encrypt INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL,                -- pending|sent|acked|expired|failed
  created_ms INTEGER NOT NULL, sent_ms INTEGER, ack_ms INTEGER,
  ack_payload TEXT, error TEXT, trace_ref TEXT);

CREATE TABLE uploads_raw (            -- uploadLogs / uploadStats / uploadSingle
  id INTEGER PRIMARY KEY, sn TEXT, endpoint TEXT NOT NULL, fields_json TEXT,
  trace_ref TEXT, received_ms INTEGER NOT NULL);
```

Map grids are kept **as received** (LZ4 block, plus `width`/`height`/`lz4_len`) and
decoded on demand. That keeps rows small, and it means an incorrect guess about cell
semantics (§16) costs nothing — re-decode later.

---

## 8. Logging & wire tracing

Two independent things:

**Application logs** — `tracing` with an `EnvFilter`, to stderr and (if configured) a
daily-rolled file. Spans carry `sn`, `conn_id`, `channel` so a device's activity can be
grepped out of a busy log.

**Wire tap** — the debugging feature this project lives or dies by. Enabled by default,
switchable per format:

* `format: "jsonl"` — one record per line in `traces/wire-YYYY-MM-DD.jsonl`:
  ```json
  {"seq":1841,"ts":"2026-09-07T13:02:11.418Z","channel":"B","direction":"in",
   "conn_id":"b-000017","peer":"192.168.1.243:41022","sn":"…","kind":"frame",
   "info_type":21006,"len":34,"body":"{\"infoType\":21006,\"data\":{}}"}
  ```
  Non-UTF-8 or oversized bodies are stored as `body_b64` + `truncated:true`
  (`max_body_bytes`). Channel-A records add `method`, `path`, `status`, and the full
  header map; the response record references the request `seq`.
* `format: "raw"` — verbatim byte streams, no interpretation:
  `traces/raw/<date>/<conn_id>.in.bin` / `.out.bin` for channel B (delimiters
  included, exactly as read from and written to the socket), and
  `traces/raw/<date>/<seq>.<req|resp>.bin` for channel A bodies. This is what settles
  framing arguments.
* `format: "both"` — default.

The `seq` is a process-global monotonic counter; every `events` /`map_uploads` /
`commands` row stores a `trace_ref` of `"<file>#<seq>"`, so a database row can always be
traced back to the exact bytes that produced it. Rotation by size (`rotate_mb`), deletion
by age (`retain_days`), both `0` = unlimited. `redact_secrets` masks `session`,
`cookies` and `sig` in the JSONL sink (never in the raw sink — the point of raw is
that it is raw).

A `noobscenic trace <file>` subcommand renders a JSONL trace as a readable timeline
(`→`/`←` per frame, decoded `infoType` names, ping/pong latency).

---

## 9. Channel A — HTTP

`axum` router; a `from_fn` middleware buffers the request body, hands it to the tap,
and rebuilds the request; the response body is tapped on the way out.

| Route | Handler | Response |
|---|---|---|
| `POST /cleanPack/register` | mint session | `{"code":0,"message":"ok","data":{"session":…,"cookies":…}}` |
| `GET`/`POST /cleanPack/getSockAddr` | gateway address | `data.addr_list:[{ip,port}]` |
| `POST /cleanPack/sync` | device attrs + OTA check | `{"code":0,…,"hasUpdateFile":0}` |
| `POST /cleanPack/response` | command ACK | `{"code":0}` |
| `POST /cleanPack/uploadEvents` | telemetry → `proto::dispatch` | `{"code":0}` |
| `POST /cleanPack/uploadLogs\|uploadStats\|uploadSingle` | store raw | `{"code":0}` |
| `POST /cleanPack/binding\|unbinding` | record bind | `{"code":0}` |
| `GET /` with `?version=1&sn=…` | version check | see below |
| **any other path/method** | tap + persist + warn | `{"code":0,"message":"ok","data":{}}` |

The fallback is load-bearing: `PROTOCOL.md` does not claim its endpoint list is
exhaustive, and an unknown endpoint answered with `code:0` costs nothing while an
unknown endpoint answered with 404 may stall the device. Every fallback hit is a
`WARN` with the full trace ref — that is how the endpoint list gets completed.

### 9.1 The form parser (the one place a naive implementation breaks)

Bodies are `application/x-www-form-urlencoded`, but **`data=` is plaintext JSON and is
not always URL-escaped** [PROTOCOL §A, "proven"]. `axum::Form` / `serde_urlencoded`
will corrupt or reject those bodies: raw JSON contains `&`, `=`, `+` and `%`, and `+`
would be decoded to a space inside string values.

`channel_a::form` therefore scans the raw body itself:

1. Split on `&` **only until** a key named `data` is reached. In every observed
   template (`sn&ts&data`, `sn&infoType&ts&userId&data`,
   `devType&sn&taskid&event&createtime&data`) `data` is last. [PROTOCOL §A]
2. Everything after that `data=` is taken verbatim to end-of-body.
3. Scalar fields are percent-decoded (`sig` is genuinely URL-encoded); `data` is
   parsed with `serde_json::from_str` on the raw text first, then on the
   percent-decoded text as a fallback, and if both fail it is stored as an opaque
   string with a `WARN` plus trace ref.
4. Unknown extra fields are collected into a map and persisted, never rejected. A
   parser that only accepts `sn`/`ts`/`data` will drop real uploads. [PROTOCOL §A]

### 9.2 Registration and sessions

`register` body: `sn`, `ld_sn`, `sig`, `ts=0`. **`sig` is not verifiable** — it is
`RSA_public_encrypt("<sn>:<time>")` under a key whose private half we do not have, and
the robot never checks *our* identity anyway [PROTOCOL §B]. We store it verbatim and
always succeed (`registration.accept_all`, default `true`): the device row is upserted
on first sight, so any vacuum that shows up is adopted.

Minting, with limits derived from the device's own buffers — it does
`strncpy(dst, data.session, 0x40)` and `strncpy(dst+0x48, data.cookies, 0x38)`
[PROTOCOL §B], so an over-long value would arrive unterminated:

* `session_key` — **32 ASCII alphanumerics** (≤63 required). Its first 16 *bytes* are
  the channel-B AES-128-ECB key; alphanumeric avoids any escaping question.
* `cookie` — 32 ASCII alphanumerics (≤55 required), unique, indexed.

Every subsequent channel-A request carries `Cookie: cookies=<value>`. Lookup rules:

* known cookie → touch `last_used_ms`, proceed;
* unknown or revoked cookie (e.g. the database was reset) → **`{"code":102}`**, which
  is exactly the documented signal that makes the device re-register [PROTOCOL §B];
* absent cookie on an endpoint that should have one → serve it anyway, log a `WARN`.
  Being strict here would be the fastest way to build a re-register loop.
* a `Cookie:` header that is present but **empty** — exactly what a robot sends while
  it still holds the empty session the phase-1 catch-all gave it — counts as unknown
  and gets `code:102`, so the robot re-registers and picks up a real session on its
  next request. No re-home, no power cycle.

Sessions are persisted, so a server restart does **not** disturb a connected robot.

### 9.3 sync / version check

`sync` records `companyId`, `mcuVer`, `version`, `versionCode`, `gitSha`, `cloud` onto
the device row and answers `code:0`. OTA stays off: `hasUpdateFile: 0`, no `downUrl`.

The two RE sources disagree on the shape of the version-check reply — `PROTOCOL.md`
shows the fields at the top level, `schemas/channelA_rest.schema.json` puts them under
`data`. We emit **both**: `{"code":0,"message":"ok","hasUpdateFile":0,"data":{"hasUpdateFile":0}}`.
Cheap, and it removes an unknown.

---

## 10. Channel B — push gateway

### 10.1 Framing (`codec.rs`)

Frame = UTF-8 JSON followed by `#\t#` = `23 09 23`, in **both** directions, with no
length prefix [PROTOCOL §B]. The decoder is an incremental buffer scan, because the
delimiter can be split across reads and multiple frames can arrive in one read. It
must survive: partial frames, back-to-back frames, a delimiter straddling a read
boundary, and an oversized frame (`max_frame_bytes`, default 8 MiB — a full map upload
is base64 of an LZ4 grid and can be hundreds of KB) which is logged and drops the
connection rather than growing the buffer without bound. Encoding is
`serde_json::to_vec` + the three delimiter bytes, and it is the *only* way frames are
written — no ad-hoc `write_all` anywhere else.

Unit tests cover each of those cases; they are cheap and this is where a subtle bug
would cost days of confusing device behaviour.

### 10.2 Connection lifecycle (`conn.rs`)

Accept → assign `conn_id` → open the raw tap sinks → spawn a read task and a write
task joined by an `mpsc` channel (so pongs, command pushes and acks can never
interleave mid-frame).

* **`10001` handshake** — `{"infoType":10001,"connectionType":1,"data":{"token":"","sn":"…"}}`;
  `token` is empty, that is expected [PROTOCOL §B]. Bind the connection to `sn` in the
  registry (replacing any older connection for the same `sn`), mark the device online,
  then flush any pending commands. `gateway.ack_handshake` (default `true`) sends
  the ack in the corrected envelope (below) with inner `infoType` 10001; the dispatcher
  has no 10001 handler, so it only earns a device-side `"Unknown infoType"` log — a
  config toggle, not a guess baked into the code (§16).
* **`21006` Ping** — pong **immediately**, before any other work on that frame:
  `{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}` with
  `announce_app_online` (default on), or inner `"data":{}` with it off. The device
  dispatches the **inner** message [CHANNEL_B_INBOUND.md]: the outer integer `encrypt`
  passes the inbound gate, the inner `infoType` reaches the pong handler, and the flag
  tells the robot an app/cloud is online so it pushes status and maps. A *flat* pong
  only refreshes the link timer — it never runs the handler, so no pushes ever start
  (verified live 2026-10-07).
* **Watchdog** — no ping within `ping_timeout_secs` (default 120; the device's actual
  interval is a runtime variable, so this is measured from the first live session and
  tuned) → mark offline, close, let the device reconnect.
* **`20002` / `21011` / anything else** → `proto::dispatch`.
* **Unknown `infoType`** → persist to `events` with the raw frame as payload, `WARN`,
  carry on. Never close the connection over a frame we do not understand.
* A panic in one connection task must not take down the listener: each task is
  wrapped, and its failure logs and closes only that socket.

### 10.3 Sending commands

```
{"encrypt":0,"data":{"infoType":<N>,"data":{ … },"dInfo":{"ts":"<ms>","userId":"<id>"}}}#\t#
```

The dispatcher processes the **inner** object, not the frame; the outer envelope is
discarded as soon as it passes the gate. `encrypt` **must be an integer** or the frame
is dropped; `encrypt:0` means the inner message is plaintext JSON — the default and the
simplest correct path. `dInfo` (`ts` and `userId`, both **strings**) is required for any
command whose handler replies: without it the reply builder refuses to POST, and the
robot echoes both back in `cleanPack/response` [CHANNEL_B_INBOUND.md]. Verified live
2026-10-07 — `21012 {"cmd":"start"}` walked the robot back to its dock, streaming status
pushes the whole way.

`encrypt:1` support lives in `crypto.rs` behind `gateway.encrypt_commands`, for when
we want to prove the path works: `base64(AES-128-ECB(inner message, session_key[0..16]))`, with
**padding disabled** and the plaintext **space-padded (`0x20`)** to a 16-byte multiple
— the device does not strip padding, it NUL-terminates and hands the buffer to jsoncpp,
so trailing spaces are safe and trailing NULs are not [PROTOCOL §B].

**Verified live (2026-10-07):** with `gateway.encrypt_commands` on, a `21018`
enqueued with `encrypt:1` went out as `{"data":"<base64 ciphertext>","encrypt":1}`
and the robot decrypted it, handled it and replied — the space-padded,
padding-disabled AES-128-ECB guess is right. With the flag off the same row fails
with a visible error and nothing is sent, so there is no silent plaintext downgrade
(`tests/control.rs` covers both).

Queue semantics: `commands` rows are created `pending`; the registry pushes them when
the device is online and marks them `sent`; rows older than `command_ttl_secs` become
`expired` rather than firing hours later when the robot next connects.

**The `commands` table is the queue.** The gateway polls it (once a second — trivially
cheap, and it removes the need for any IPC) rather than taking work over an in-process
channel. That is what lets the console, a one-shot CLI invocation, or `sqlite3` itself
all enqueue a command with no API in between.

### 10.4 ACK correlation

Command results come back as `POST cleanPack/response` (channel A) with an
`infoType` payload. There is no documented correlation id, so correlation is
best-effort: match on `(sn, info_type)` against the most recent `sent` command within a
time window, mark it `acked`, store `ack_payload`. Unmatched responses are still
persisted as `events` — under-reporting an ACK is harmless, inventing one is not.

**Verified live (2026-10-07):** a `21011` queued from the console was pushed by the
poller within a second and the robot's reply landed as `acked` on that command; the
outbound frame in the trace carries the integer `encrypt:0` and the string-typed
`dInfo` the device echoes back.

---

## 11. Shared protocol layer

`proto::dispatch` takes `(sn, channel, info_type, data, trace_ref)` from either
transport and is the only place that understands payloads.

* **`20002` map upload** [MAP.md] — fields `SN`, `mapId`, `autoAreaId`, `pathId`,
  `width`, `height`, `resolution`, `x_min`, `y_min`, `lz4_len`, `map` (base64),
  `area[]`, `chargeHandlePos/Phi/State`. It arrives over **HTTP** (`uploadEvents`), not
  channel B [FUNC_MAP §2.1]. Store the row compressed as received; decode lazily in
  `map.rs`: `base64 -> LZ4 block -> width*height row-major cells`, with a histogram.
  Cell `(col,row)`'s centre is `x = x_min + 0.05 + col*resolution` (the wire origin is
  half a cell off the corner), `y` likewise [MAP.md]. A live upload settled the cell
  semantics: `0x00` wall, `0x7F` unknown, `0xFF` free, other bytes room labels
  [FUNC_MAP §2.3].
* **`21011` clean path** — a **response** to a cloud request
  (`{"startPos":N,"mask":N}` over channel B), not a stream: the robot POSTs
  `cleanPack/response` with `posArray` as `[[x,y],…]` pairs in **millimetres** whose low
  2 bits tag the point type [FUNC_MAP §3]. `path.rs` assembles chunks keyed by
  `(sn, pathID)` in a sparse, index-addressed slot list (`points_json` holds `null`
  for points not yet seen) and marks `complete` once every slot is filled;
  out-of-order chunks, duplicates and corrections are all tolerated.
* **`21020`** — *not* a pack transfer: it is the direct remote-control command
  (`data.ctrlCode`), no reply on this channel, and the `packId` ack belongs to the
  separate LAN UDP handler [FUNC_COMMANDS §2.1, FUNC_MAP §3].
* **`21003` SetAreaTactics** — outbound only in v1: the console accepts a region list
  and enqueues it. Region descriptors (`id`, `name`, `tag`, `active`, `mode`,
  `forbidType`) are shared with the map's `area[]` parsing.
* Everything else — persisted, named where `eid_catalog.tsv` gives a name, warned about
  otherwise. `info_type.rs` also loads the 355-entry catalog as a static name table for
  readable logs (note: `EID_*` bus ids and wire `infoType`s are **different number
  spaces** [PROTOCOL §B] — the catalog is used for the `event` field of `uploadEvents`
  and for human-facing labels, never to interpret an `infoType`).

---

## 12. Operator console (stdin/stdout)

No admin API in v1 — the operator surface is the process's own terminal. `serve` reads
lines from stdin and writes results to stdout, alongside the log stream on stderr (so
`./noobscenic 2>var/logs/run.log` gives a clean console). If stdin is not a TTY or is
closed, the console simply does not start and the server runs headless; nothing else
changes.

Line-based, one command per line, no dependencies beyond `tokio::io::stdin`:

```
> help
> devices                          sn, online, last seen, version
> device <sn>                      full row + session state
> events [<sn>] [n]                last n events (default 20)
> map <sn>                         latest map: dims, resolution, origin, dock, areas
> path <sn> [<path_id>]            assembled clean path summary
> send <sn> <infoType> <json>      enqueue a command   e.g. send X 21012 {}
> commands [<sn>]                  queue state: pending / sent / acked / expired
> trace on|off                     toggle the wire tap at runtime
> quit
```

Three experiment toggles sit alongside the queue verbs: `send-enc` (the same send
with `encrypt:1`; it fails the row unless `gateway.encrypt_commands` is on),
`send-full` (one raw inner message, sent immediately, bypassing the queue), and the
`style` / `pongs` toggles left over from the framing and liveness experiments.

Output is plain aligned text; `map` and `events` print summaries rather than dumping
grids — the database and the traces are there for the full picture.

The same verbs exist as CLI subcommands for one-shot use (`noobscenic devices`,
`noobscenic send …`, `noobscenic migrate`, `noobscenic trace <file>`). They work
against the SQLite file directly, so they are useful whether or not a server is
running: a `send` from a second terminal lands in the `commands` table and the running
gateway picks it up on its next poll (§10.3).

A JSON API can be added later if something wants to drive this remotely (§18); it is
not needed to reach v1, and building it now would be speculative.

---

## 13. `tools/rehome.py` — channel C client

Python 3, **standard library only** (`socket`, `json`, `argparse`) so it runs from a
laptop that has just joined the robot's soft-AP with nothing installed.

Context: the robot's LAN control server binds a UDP port chosen as
`rand()%1000 + 9000`, i.e. somewhere in 9000–9999, and the app finds it with a
broadcast `getID` exchange [PROTOCOL §C]. In pairing mode the robot is the AP at
`192.168.78.1` (DHCP .50–.150) — `LDRobot` on the firmware sample, vendor-branded
on shipping units (`Proscenic-6716_<serial>`).

It is a one-shot tool: running it performs the whole re-home, there are no
subcommands, and the server itself never speaks channel C. The bench unit answers on
**UDP 7913**, outside the documented range, so the discovery sweep defaults to
7000–9999 and `--discover-ports` widens it.

The run, in order (this order is the tool):

| Step | Datagram | Purpose |
|---|---|---|
| discovery | `{"cmd":"getID"}` | sweep the ports, collect any well-formed JSON reply (`invalue cmd` counts — not every unit implements `getID`), pin the robot's `ip:port` |
| `setUrl` | `{"cmd":"setUrl","url":"http://<host>:8080/"}` | point channel A at us |
| `setUrl` | `{"cmd":"setUrl","ip":"<host>","port":8081}` | write `ip_port.json` — the write that re-homes |
| `getCfg` | — | verify the writes landed (a read; a failure does not undo them) |
| `setID` | `{"cmd":"setID","id":<user>,"deviceSN":<sn>}` | arm the bind: the robot resolves the gateway and starts dialing |
| `setSta` | `{"cmd":"setSta","staName":…,"staPwd":…}` | store the Wi-Fi credentials |
| `applyCfg` | `{"cmd":"applyCfg"}` | commit: drop the soft-AP, join the network |

Details that matter:

* `applyCfg` (not `setSta`) is what switches the robot to station mode, so it is
  **last** — it tears down the AP we are talking over and its reply often never
  arrives; that silence is expected, not a failure.
* The robot leaves pairing mode only once the full chain succeeds: channel B
  `10001` handshake + `21006` pongs, then channel A answering the preBind with
  `code:0`. noobscenic must be listening on **both** ports before `setID` is sent
  (PAIRING_LOG_ANALYSIS.md §4/§8).
* `bindOk` is never sent: on this firmware it kills the local channel and starts a
  fork storm that only a reboot clears (PAIRING_LOG_ANALYSIS.md §5).
* Passphrase length is validated client-side to 8–64 characters, matching the device
  handler, so a bad password fails locally instead of half-way through.
* `--dry-run` prints the exact datagrams and sends nothing (give `--port` to skip
  discovery); `--timeout` and `--retries` tune a flaky UDP path; responses are
  `{"result":"ok"|"fail","code":N}` and a `fail` is reported loudly.
* The script writes its own JSONL trace (`--trace FILE`) of every datagram sent and
  received, same spirit as the server's tap. The Wi-Fi password is redacted in it
  (and on the console) unless `--show-secrets`.
* Field names `ssid`/`staPwd` and the `url` vs `ip`/`port` forms of `setUrl` come from
  firmware JSON templates and are flagged in the RE docs as "confirm against a live
  capture" — the script therefore accepts `--ssid-key`/`--pwd-key` to switch to the
  documented names if a capture says a unit wants them.
* Documented alternative for a rooted device: these commands only write
  `/data/bin/Run/Config/url`, `ip_port.json`, `wpa_supplicant.conf` and `wifi_mode`
  [PROTOCOL §C] — the README notes that shell access can set them directly.

---

## 14. Testing

* **Unit** — framing (split delimiters, back-to-back frames, oversize); the lenient
  form parser against every documented body template, including raw JSON with `&` and
  `=` inside strings; session minting length bounds; AES-ECB space padding; LZ4 map
  decode against a synthetic grid.
* **Integration** — a temp-file SQLite database plus `tower::ServiceExt::oneshot` for
  the channel-A routes: register → cookie present → `getSockAddr` → unknown cookie
  yields `code:102`.
* **`src/bin/simdev.rs` — device simulator.** This is what lets phases 1–5 be built and
  regression-tested without the robot: it registers, calls `getSockAddr`, opens the
  gateway socket, sends the `10001` handshake, pings on an interval, asserts a pong
  arrives, replays a synthetic `20002` map and chunked `21011` path, and prints any
  command it receives (checking the `encrypt` field is an integer, exactly as the
  device does). `tests/e2e_simdev.rs` runs it against an in-process server.
* **Replay** — `noobscenic trace` reads a JSONL trace; a `--replay` mode feeds recorded
  device frames back into a fresh server, so a real capture becomes a regression test.

---

## 15. Milestones

**This is the live progress checklist for the project.** Tick a box (`- [ ]` → `- [x]`)
the moment the item is genuinely done, and commit that tick together with the work it
describes — see `AGENTS.md`. Each phase ends in something observable.

### Phase 0 — Re-home tool (no Rust; unblocks every live test)
- [x] `tools/rehome.py`: argparse, UDP send/recv with timeout + retries, `--dry-run`, `--trace`
- [x] discovery sweeps 7000–9999 and finds the bench unit on **UDP 7913** (`getID` answered as `invalue cmd`)
- [x] the run is one command: discover → `setUrl` (URL + gateway) → `getCfg` verify → `setID` → `setSta` + `applyCfg`
- [x] end-to-end on the bench unit: it joins Wi-Fi, dials channel B (`10001` handshake) and its `register`/`binding` posts are answered `code:0`

Every step is exercised against `tools/fakerobot.py`, a stand-in that answers the
documented channel-C shapes including `setID`. The bench unit
(`Proscenic-6716_20403551`) answers on **UDP 7913**, outside the documented
`rand()%1000 + 9000` range, which is why the sweep defaults to `7000-9999`. (`getWifi`
never returned a scan on the bench unit — see
[`doc/reverse-engineering/FIELD_NOTES.md`](reverse-engineering/FIELD_NOTES.md) — and
scanning is no longer part of the tool.)

**Done when:** the robot's channel-A base URL and channel-B gateway point at a host we
choose, and it can reach that host — first done on the bench unit on 2026-10-06: it
took both writes, joined the LAN and dialed the gateway.

### Phase 1 — Skeleton
- [x] Cargo project, module layout, `error.rs`, graceful shutdown on Ctrl-C
- [x] JSON config load + defaults + CLI overrides
- [x] `tracing` set-up: stderr + rolling file, `EnvFilter`
- [x] Wire tap: JSONL sink, raw sink, rotation/retention, `seq` counter
- [x] SQLite pool + pragmas + `0001_init.sql` migration, run at start-up
- [x] HTTP listener with the catch-all handler and full request/response tapping

**Done when:** pointing the robot at it yields a complete trace of everything it tries
to do — valuable before a single endpoint is implemented.

### Phase 2 — Channel A core
- [x] Lenient form parser + unit tests over every documented body template (§9.1)
- [x] `register`: session/cookie minting within the device's buffer limits, persisted
- [x] Cookie lookup middleware; unknown/expired cookie → `code:102`
- [x] `getSockAddr` with configured address, and peer-based auto-detect
- [x] `sync` records device attributes; version check answers "no update", both shapes

**Done when:** the robot stops re-registering and opens a TCP connection to the gateway.
**Status (2026-10-07):** implemented and covered by unit + in-process HTTP tests. The
robot has not made a channel-A request since, so nothing is live-verified yet; the
next request it makes (a bind, a sync, an upload, or an empty-cookie nudge) exercises
the whole flow, and the session it gets is then visible in the `sessions` table.

### Phase 3 — Channel B alive
- [x] `#\t#` frame codec with unit tests: partial, back-to-back, split delimiter, oversize
- [x] Listener, per-connection read/write tasks, raw byte capture, panic isolation
- [x] `10001` handshake → registry binding, online state, optional ack
- [x] `21006` ping → immediate pong; watchdog on `ping_timeout_secs`
- [x] Unknown `infoType` persisted and warned about, never fatal

**Done when:** *the milestone that matters* — the robot stays connected for many
minutes with no reconnect loop, and the trace shows every ping answered.
**Verified on the bench unit (2026-10-06):** a 54-minute session of 559 `21006`
pings and 559 pongs with no reconnect loop, and after re-homing the robot dialed
the gateway within seconds and its `register`/`binding` posts were answered `code:0`.

### Phase 4 — Telemetry
- [x] `uploadEvents` → shared `proto::dispatch`; `response`, `uploadLogs`, `uploadStats`, `uploadSingle`
- [x] `20002` map upload stored; `map.rs` base64 + LZ4-block decode with a cell histogram
- [x] `21011` clean-path chunk assembly (out-of-order and duplicate tolerant)
- [x] `21020` "pack reassembly" — superseded by the corrected RE: 21020 is the
  remote-control command with no reply, and there is no pack transfer on this channel
  to reassemble [FUNC_COMMANDS §2.1]
- [x] Every stored row carries a working `trace_ref`

**Done when:** a real cleaning run leaves a decodable map and path in the database.
**Status (2026-10-07):** verified against the live robot — a forced map upload decoded
as 172×107 (419 wall / 12634 unknown / 5351 free), a 21011 request produced a complete
(single-chunk) path row, and status pushes plus command replies landed in `events` with
trace references. A one-room smart clean then assembled a 1545-point path across seven
21011 fetches and left a separate 225-point return-to-dock row; both draw correctly
over the map via `tools/map2png.py --path`.

### Phase 5 — Control
- [x] `commands` table as the queue, polled by the gateway; TTL expiry
- [x] `encrypt:0` sender; integer `encrypt` field asserted in tests
- [x] Best-effort ACK correlation from `cleanPack/response`
- [x] Operator console (§12) and the matching one-shot CLI subcommands

**Done when:** a clean job can be started from the console and its result observed.
**Status (2026-10-07):** the queue, the one-second poller, TTL expiry, ACK
correlation and every console/CLI verb are implemented and covered by
`tests/control.rs` (a fake device receives the queued frame in the documented
envelope). Live gate, all from the console: `21005 {"mode":"smartClean"}`
(FUNC_COMMANDS.md §1.1) started a clean — status `cleanTime` ticking, map
updating — `21017 {"cmd":"stop"}` ended it ~13 s later, and `21012 {"cmd":"start"}`
(return-to-dock, §1.3) drove it home: `backcharge` → `charge` → `fullcharge`. All
three replies were `"message":"ok"` and every row was ACKed within a second. A
`21011` fetch and a `21018` version query round-tripped the same way earlier, and
`send-enc` correctly failed its row while `gateway.encrypt_commands` is off.

### Phase 6 — Polish
- [x] Binding state machine, driven from what the traces actually show
- [x] `encrypt:1` (AES-128-ECB, space padding) behind the config flag
- [ ] `noobscenic trace` renderer and `--replay` regression mode — planned, not a
  priority (README "Not yet implemented")
- [ ] Optional TLS for a DNS-override deployment — planned, not a priority
  (README "Not yet implemented")
- [x] Operator README: re-home, run, back out

**Done when:** someone who is not us can re-home a vacuum and keep it running.
**Status (2026-10-07):** the operator guide is in (re-home → run → check → back
out), and the binding state machine plus `encrypt:1` are implemented and
live-verified. The trace renderer, replay mode and TLS are deliberately postponed;
nothing else in this phase is needed for the phase's goal.

### Phase 7 — Web UI
- [x] `GET /` serves the page and still answers the robot's version-check query
- [x] `GET /api/robots` list, `?id=` selection (`bind_user` first, `sn` fallback)
- [x] map + path endpoints, canvas rendering, ~1.5 s summary poll
- [x] presence-gated `21011` path poller (only while a UI session is live)
- [x] command dropdown (`smartClean` / `pause` / `continue` / `stop` / `findCharge`) → the queue
- [x] integration tests for the API, the `GET /` split and the catalog
- [x] README: a short "web UI" note in the operator guide

**Done when:** a phone on the home LAN can watch a clean on the map, start one and
stop it, from a bookmarked `?id=` URL. Design and non-goals: §19.
**Status (2026-10-07):** implemented and verified live against the bench unit — the
page serves at `/`, the robot's `?version=` check still gets its JSON, the map and
path endpoints returned the real 314×170 grid and the 225-point return path (the
served data renders to the same picture `tools/map2png.py` produces), and a `pause`
sent through the API was ACKed by the robot within a second. The bench unit still
has its id (`Foo`) recovered from the stored preBind, so `?id=Foo` resolves; presence
gating and the charging skip are covered by `tests/web.rs`.

### Phase 8 — Web UI, control depth (design: §20)
- [x] pan and zoom for the map canvas, paths and zones included (§20.1)
- [ ] spot clean button (§20.2)
- [ ] "more" menu: specialised modes plus the consumables view (§20.3)
- [x] manual steering pad with the ≤300 ms repeat, the 4001 release and the 4000 watchdog (§20.4)
- [ ] zone editor: 21004 → edit → 21003 round-trip for no-go, no-mop and clean zones (§20.5)

**Done when:** a phone can steer the robot, spot-clean it, pick a specialised mode
and draw a no-go zone, all on the same page.
**Status (2026-10-08):** pan and zoom are in. The map starts fitted to the client
area (contain, centred), pinch zooms and drag pans on mobile, the wheel zooms about
the pointer on desktop, and double-click / double-tap refits. Zooming all the way
out — by wheel or pinch — snaps back to the fitted, centred view, so the minimum
zoom can never leave the map showing empty space. The grid is rendered
once per map revision into an offscreen canvas and drawn under the view transform, so
paths and (one day) zones share the transform and the inverse is ready for hit-testing.
Steering is in too: the drawer's `🕹️` opens a bottom sheet whose four buttons send
realtime `21020` frames straight to the device writer (no queue, no rows), repeating
every 250 ms while held, with `4001` on release and `4000` on close; the status poll
speeds up to 300 ms while control frames flow and falls back to 1.5 s after they stop.
The server-side watchdog from §20.4 is not implemented yet — the robot zeroes its own
speed after 400 ms and leaves manual mode after 30 s, so a dead client cannot leave it
driving. The phase's other three items are untouched.

---

## 16. Known unknowns

The RE docs mark these as unverified. None of them blocks implementation, because each
has a designed-in tolerance; all are settled by reading one real trace.

| Unknown | Source | How the design tolerates it |
|---|---|---|
| `getSockAddr` request body fields | PROTOCOL §A "Gap 1" | The handler requires **no** fields; it answers on the cookie alone. |
| Binding state machine; whether maps/commands are gated on bind | PROTOCOL §A "Gap 2" | **Settled:** `binding`/`unbinding` are recorded (`devices.bind_state`, `bind_user`, timestamps) and answered `code:0` idempotently, so a preBind retry cannot fail the bind; nothing is gated on the state (`tests/binding.rs`). |
| Whether the device expects a reply to `10001` | PROTOCOL §B | `gateway.ack_handshake` toggle, default on; an unexpected `infoType` only costs a device-side log line. |
| The pong's exact `infoType` demux (21006 near-certain) | PROTOCOL §B | Pong `21006` by default; the value is a constant in `info_type.rs`, and the ping/pong pairing is visible in the trace timeline. |
| Ping interval and drop timeout (runtime variables) | PROTOCOL §B | Pong immediately, never on a timer; `ping_timeout_secs` is generous (120) and gets tuned from a measured session. |
| Whether uploads want a channel-B ACK | PROTOCOL §B | Config toggle per class, default off; a missing ACK shows up as a device retry in the trace. |
| AES `encrypt:1` pad byte | PROTOCOL §B | Default is `encrypt:0`, which sidesteps it entirely; space padding when enabled. |
| Map cell value semantics (free/occupied/unknown) | MAP.md | **Settled live 2026-10-07:** `0x00` wall, `0x7F` unknown, `0xFF` free; grids still stored as received, decoding stays replaceable. |
| `setSta` key names (`staPwd` vs `pwd`); discovery port | PROTOCOL §C | `--pwd-key` override; discovery sprays the whole 9000–9999 range. |
| Version-check response shape (top level vs `data`) | PROTOCOL §A vs schema | Emit both. |

---

## 17. Risks and failure modes

* **Re-register loop** — caused by a `register` response missing `session`/`cookies`,
  or by answering `102` too eagerly. Guarded by §9.2 and by a metric: more than N
  registrations per device per hour is a `WARN`.
* **Reconnect loop** — caused by an unanswered ping. The pong is written before any
  other processing of that frame, and the trace timeline makes ping→pong latency
  visible at a glance.
* **Disk growth** — verbatim traces of a chatty device plus map uploads. Bounded by
  `max_body_bytes`, `rotate_mb`, `retain_days`; maps are stored compressed as received.
* **Unbounded memory** — a hostile or buggy stream could grow the frame buffer or the
  `21020` reassembly map. Both are capped and both drop the connection when exceeded.
* **Clock skew** — the robot's `ts` may be wrong before it syncs; server time is
  authoritative for ordering, device time is kept verbatim alongside.
* **Exposure** — channels A and B cannot authenticate anything (the device has no
  credentials to offer), so the server must live on a trusted LAN or VLAN. The operator
  surface is the process console, so it is not reachable over the network at all. This
  is stated plainly in the operator README.
* **Robot bricking** — nothing in this project writes to the device beyond the
  documented `setUrl`/`setSta` config commands, which only rewrite plaintext config
  files [PROTOCOL §C]; `resetWifi`/`setAp` are the documented way back.

---

## 18. Later

A Home Assistant / MQTT bridge on top of the phase-7 API (§19); scheduling;
multi-device fleet view; serving our own OTA (the `updater` trust anchor is a vendor
RSA-1024 key, so this needs the device-side key replacement described in `REPORT.md`
§6.3 and stays firmly optional); support for other LDRobot-platform vacuums, which
the channel abstraction already anticipates.

---

## 19. Web UI (phase 7)

A single page served by the channel-A listener at `GET /` — same process, same
database, same command queue. That is deliberate: the deployment target is one home
and the page's only client is the operator on the same LAN. **There is no
authentication in the MVP**: anyone who can reach port 8080 can drive the robot.
That is the accepted threat model, and it is stated here so nobody mistakes it for an
oversight.

Surface (all on 8080; `{id}` is `devices.bind_user` — the id `setID` set — with the
`sn` as fallback. A robot that bound before the binding handler existed has its id
recovered once at startup from the stored preBind, `web::recover_bind_ids`, so the
DB row is the only place the id has to live):

| Route | Purpose |
|---|---|
| `GET /` | the page; if the query carries `version`/`sn` (the device's OTA check) the request goes to the existing `version_check` instead |
| `GET /api/robots` | `{robots:[{id,sn,online,bind_state,last_seen_ms}]}` for the dropdown |
| `GET /api/robot/{id}/summary` | latest 20001 status plus path and map metadata; polled every ~1.5 s |
| `GET /api/robot/{id}/map` | base64 of the *decompressed* grid + frame (w, h, resolution, x_min, y_min, dock) |
| `GET /api/robot/{id}/path` | stored points with the 2-bit type tags stripped |
| `POST /api/robot/{id}/command` | form `name=<raw name>` from the catalog below → `insert_command` |
| `POST /api/robot/{id}/control` | form `code=<ctrlCode>` → a realtime `21020` frame straight to the device writer, bypassing the queue (§20.4) |

`web/index.html` is embedded at compile time (`include_str!`), so there is no static
file serving and no runtime path lookup. Layout: top bar with a `☰` drawer toggle,
the robot `<select>` and the status line; a `<canvas>` filling the viewport; a left
drawer with a tools grid (`🕹️` Remote control) above the raw command `<select>` and
Send; a bottom sheet with the steering pad (`↪️`/`⬆️`/`⬇️`/`↩️`, hold to move,
`❌` to close); and a centred bottom bar with
three buttons — `⚡` Charge, a stateful `▶️`/`⏸️`/`⏯️` button that follows the robot's
`mode` (`charge`/`fullcharge`/`idle` → smart clean, `sweep` → pause the clean,
`backcharge` → pause the return, paused/dormant/fault → continue, anything else
disabled), and a disabled `🗺️` zone-cleaning placeholder.
Changing the robot sets `location.search`, so `?id=` stays bookmarkable and the back
button works. Rendering is client-side: base64 → `Uint8Array` → `ImageData` (0x00
wall / 0x7F unknown / 0xFF free / other bytes = label hue, as in
`tools/map2png.py`), `putImageData`, then CSS scaling with
`image-rendering: pixelated`; the path, its start/end dots and the dock are drawn on
the same canvas with the mapping map2png.py already validated. No image crate, no
PNG encoder, no new dependencies.

**Auto-refresh is polling, not push.** The 1.5 s `/summary` poll carries status and
metadata only; the ~70 KB map and the path are fetched only when their revisions
change. That is at most 1.5 s behind telemetry that itself arrives at ~1 s cadence,
and it avoids a broadcast channel, publish hooks and an SSE feature flag. (axum's
`sse` feature is a clean later upgrade — `futures-util` and `tokio-stream` are
already in the lockfile — if polling ever feels laggy.)

**Path polling is presence-gated.** The robot pushes maps but not paths: the cloud
must ask (`21011 {"startPos":N,"mask":0}` — the request the manual collection used).
An in-memory watcher registry in `AppState` records the last time each device's API
was touched, and the page's own poll is the heartbeat. A 5 s task enqueues a path
fetch only when the heartbeat is fresh (someone has the page open and visible), the
device is online, no `21011` row is already pending or sent, and the last status mode
is not a charging one. Closing or hiding the tab stops the heartbeat and, within one
interval, the path polling — no watcher, no extra traffic.

**Command catalog** — argument-less actions only, raw names:

| Name | Frame |
|---|---|
| `smartClean` | `21005 {"mode":"smartClean"}` |
| `pause` / `continue` / `stop` | `21017 {"cmd":"pause"\|"continue"\|"stop"}` |
| `findCharge` | `21012 {"cmd":"start"}` (return to dock) |

The query commands earn no button: `getStatus` (20001) returns the record the robot
already pushes every ~1 s, `getVersion` (21018) returns what `sync` already put in the
`devices` row (live-verified: `{"version":"0.7.1","fullversion":1241,"hasUpdateFile":0,
"mcu":"h185v60_0"}`), and `getDeviceAttr` (21010) is a stub of five empty arrays.
Consumables (`21015`, whose replies already land in `events`) gets a dedicated view
later.

Reused as-is: `map::decode`, the `path` assembly, `queries::{list_devices,
latest_map_summary, clean_path_summary, insert_command}`, the queue with its poller,
TTL and ACK correlation, and the console's enqueue semantics. The web paths are
exempt from the session gate, so browser traffic does not log "no Cookie header"
warnings.

Not in the MVP: auth, TLS, zoom/pan, room targeting, schedules, clean records,
consumables, human-friendly labels, i18n, SSE/WebSocket.

---

## 20. Web UI roadmap (after phase 7)

The phase-7 MVP deliberately stops at "watch, plus five argument-less commands". This
is what comes next, roughly in the order it makes sense to build. Almost all of it is
client work on top of the existing API; two items need new cloud→device frames that
the RE docs already pin down.

### 20.1 Pan and zoom (do this first)

Pure client-side: give the canvas a view `{scale, x, y}`, draw the grid into an
offscreen canvas once per map revision, and `drawImage` it under the transform with
`imageSmoothingEnabled = false` so cells stay crisp; draw the path and zones as
vectors with constant on-screen widths (`lineWidth = px / scale`). Wheel and pinch
(two pointers) zoom, drag pans, double-tap fits. The zone editor's hit-testing needs
the inverse transform, so this lands before it. Optional: keep the view in the URL
fragment so a bookmark remembers it.

### 20.2 Spot clean

`21020 {"ctrlCode":3001}` spot-cleans a 1.5 m square around the robot (FUNC_MAP
§6.3). The robot sends no reply to 21020, so the queue row simply stays `sent`; the
UI watches `mode` instead. One button, and it can go through the normal queue — a
second of latency is fine for a one-shot action.

### 20.3 The "more" menu (specialised modes)

A sheet behind a three-dots button; every entry is backed by a documented frame:

| Entry | Frame | Notes |
|---|---|---|
| Zone / room clean | `21023` with `cleanId` (−1 whole map, −2 all stored, −3 forbid only, −4 non-forbid, N = stored region) and/or `extraAreas` (`mode:"area"` zone, `mode:"point"` spot) | whether 21023 alone *starts* the job is unestablished (FUNC_MAP §6.2): test on hardware, possibly follow with 21005 |
| Smart room clean | `21005 {"mode":"smartAreaClean"}` | auto-segmentation, then room by room |
| Deep clean | `21005 {"mode":"depthTotalClean"}` | cover_mode 1 |
| Y-shaped mopping | `21005 {"mode":"smartClean","pathType":"y_word"}` (or `21020` 3024) | |
| Suction | `21022 {"cmd":"quiet"\|"auto"\|"strong"\|"max"}` | |
| Water level | `21024 {"cmd":"setWaterPump","value":1..4}` | |
| Locate robot | `21020 {"ctrlCode":3010}`, 3011 to stop | plays a sound |
| Mute toggle | `21020 {"ctrlCode":3022}` | |
| Obstacle avoidance | `21024 {"cmd":"setLidarCollision","value":0\|1}` | |
| Clean components | `21020` 3025 / 3026 | brushes/fan on-off |
| Do-not-disturb, schedules | `21001` (whole list) / `21002` (read) | needs its own view; entries carry weekly, one-shot and quiet-window kinds |
| Consumables | `21015` read, `21016` reset | a dedicated view, not a menu entry: the five counters as runtime hours; reset always answers "ok", so re-read to confirm |

The catalog grows from `(name, infoType, fixed payload)` to entries with an optional
parameter form. Before any free-text field ships, mind the firmware string hazards:
`name`/`tag` ≤ 31 UTF-8 bytes and `mode` ≤ 30, or the robot's parser is overrun
(FUNC_MAP §4).

### 20.4 Manual steering (remote control)

`21020 data.ctrlCode`: 3005 forward (+0.3 m/s), 3006 backward (−0.1 m/s), 3007 rotate
left (+1.0 rad/s), 3008 rotate right (−1.0), 3013 free speed
(`params.speed_v`/`speed_w`), 4001 zero speed, 4000 leave manual mode. No reply to
21020 — the UI watches `mode: rfctrl` in the status instead, and steering frames are
fire-and-forget.

> The 3007/3008 directions in FUNC_COMMANDS §2.1 are *[inferred]*; a live check from
> the pad showed they are the other way round, so the UI binds `↪️` → 3007 and
> `↩️` → 3008 (FIELD_NOTES.md, 2026-10-08).

Three facts shape the design:

* The robot zeroes the commanded speed after **400 ms** without a new command and
  leaves manual mode after 30 s (FUNC_COMMANDS §2.3). The UI must re-send the held
  direction every **≤ 300 ms**, send 4001 on release and 4000 when leaving the pad;
  the vendor app's 2 s repeat is far too slow and a replacement must not copy it.
* **The commands queue is too slow**: its poller runs once a second. Steering needs a
  realtime path — write straight to the device's writer the way pongs do, not into
  `commands`.
* The first steering command **interrupts a running clean** and enters manual mode;
  4001 also starts manual mode, so it is only ever sent after a steering command.

Safety: a server-side watchdog (no steering frame from a watched client for ~2 s →
send 4000) so a closed tab or dead Wi-Fi cannot leave the robot in manual mode. The
UI is a four-button pad (`pointerdown`/`pointerup`/`pointercancel`, arrow keys on
desktop) that shows when the robot is in `rfctrl`.

### 20.5 Editing designated zones (the big one)

No-go, no-mop and deep-clean zones live in the robot's `AreaSetting` list, and the
protocol is a read–modify–write of the **whole list**:

* read: `21004 {}` → the stored JSON, verbatim;
* write: `21003 {"mapId":N,"value":[region,…]}` → replaces everything and replies
  "ok" through `cleanPack/response` (so ACK correlation works).

Never rebuild the list from the 20002 `area[]`: it omits `cleanType` and `workNoisy`
and carries snapped vertices, so writing it back silently resets every region to
sweep-only/auto (FUNC_MAP §5.1). Always 21004 → edit → 21003, and keep unknown keys
verbatim.

A region is `{"vertexs":[[x,y]…], "active":"normal|depth|forbid",
"forbidType":"all|sweep|mop", "cleanType":…, "workNoisy":…, "name":…, "tag":…,
"id":…, "mode":…}` in the **mm world frame** — the frame the path already uses.
Vertices are snapped by the firmware to 50 mm cell centres (`(v/50)*50 + 25`); fewer
than three vertices are rejected, and a polygon thinner than one cell can collapse,
so a virtual wall must be a thin non-degenerate polygon — there is no line primitive.

The editor is the real work: a mode on the map where you tap out a polygon with a
live preview, select/move/delete existing zones, choose the type (no-go / no-mop /
deep clean / clean zone), snap to the grid, and are prevented by the UI from sending
fewer than three vertices, degenerate polygons or over-long strings. It needs §20.1
and the inverse of the `toPixel` transform for hit-testing.

Operational hygiene: keep the last-read list in the database so edits always start
from something real, write only after a successful 21004, re-read to confirm, and
offer "restore the previous list". Whether an edit takes effect mid-clean is unknown;
assume it applies to the next job.
