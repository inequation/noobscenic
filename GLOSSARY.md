# Glossary

Plain-language explanations of terms this project uses for its own abstractions.
See `AGENTS.md` for when to add to this file.

## Sink

A "sink" is one of the places a tapped record actually gets written to — the
destination end of the wire tap, as opposed to `Tap` itself, which is the single
point every caller records *through*. A tap can feed zero, one, or both of its
sinks per event, depending on config (`format: "jsonl" | "raw" | "both"`); each
sink decides independently how to represent the same record on disk.

This project has two:

- `jsonl::JsonlSink` (`src/wire/jsonl.rs`) — appends one JSON object per line to
  a rotating `wire-YYYY-MM-DD.jsonl` file, truncating and redacting the body per
  config.
- `raw::RawSink` (`src/wire/raw.rs`) — writes the body verbatim to its own file
  under `raw/<date>/<seq>.<kind>.bin`, with no truncation or redaction.

Both sinks are owned by `wire::Tap`'s inner state and written to synchronously,
in the same call to `Tap::record()`, so a caller never needs to know which sinks
are active — it just records, and whichever sinks are enabled receive the event.

## Tap

A "tap" is the object you attach to a line to listen in on it without disturbing
what flows through — the same sense as a phone tap. In code, `wire::Tap`
(`src/wire/mod.rs`) is the single component every device read/write path calls
into to record traffic; nothing about the connection's own behavior depends on
whether a tap is attached. It can be `Tap::disabled()` (a no-op, at no cost) or a
live tap backed by the JSONL and raw sinks described under **Wire** below.

Calling code doesn't reach into the sinks directly — it builds a `Record`
(channel, direction, kind, body, and any metadata) and hands it to
`Tap::record()`, which timestamps and sequences it, fans it out to whichever
sinks are enabled, and hands back a `Recorded` reference (e.g. `"#1841"`) that
the caller can store alongside whatever the bytes produced. A tap can also be
switched on or off at runtime (the console's `trace on|off`) without restarting
the server.

## Wire

"The wire" is the traffic between the server and a robot: every byte the server
reads from a device connection or writes back to one, on any of the project's
channels (A, B, or the HTTP catch-all). "Wire tracing" (or "the wire tap") is the
project's debugging feature that copies that traffic out to disk as it passes
through, so a developer can later see exactly what a device said and what the
server answered — this is distinct from the application log, which records what
the server *decided*, not what was physically sent or received.

In code, `wire::Tap` (`src/wire/mod.rs`) is the component every read/write path
calls into. It can write to two independent sinks, controlled by config:

- **JSONL sink** (`src/wire/jsonl.rs`) — one JSON object per line in a rotating
  `wire-YYYY-MM-DD.jsonl` file. This is the queryable, human-readable timeline:
  each line carries a sequence number, timestamp, channel, direction, and (unless
  truncated or redacted) the body itself.
- **Raw sink** (`src/wire/raw.rs`) — the exact, untouched bytes of each unit of
  traffic, one file per event under `raw/<date>/<seq>.<kind>.bin`. Nothing here is
  truncated, redacted, or re-encoded, so it's the copy that settles any dispute
  about framing or about what a device actually sent.

Every database row that was produced by reading the wire keeps a `trace_ref`
(e.g. `"wire-2026-09-07.jsonl#1841"`) pointing back at the exact bytes that
produced it, so any stored fact can be traced back to its source on the wire.

## Connection registry

The gateway's index of which robots are connected *right now*. Channel B is a TCP
connection the **robot** opens, so the server can never dial a device itself —
anything that wants to send it a command has to find the socket the robot already
has open. The registry (`channel_b::Registry`) is that lookup: serial number →
the connection's id, peer address, connect time, and the writer queue for
outbound frames. An entry appears at the `10001` handshake and disappears when
the connection ends, via a `Registration` guard owned by the connection task, so
it is cleaned up even if that task panics; a robot that reconnects simply
replaces its own older entry.

It is deliberately not the database: the `devices` table remembers what has been
seen, while the registry only holds what is live and addressable.

## Command queue

The place outbound commands wait before the robot can receive them. Channel B is
TCP dialled by the *robot*, so a command can only ever go out while it is
connected — the queue decouples "an operator wants this sent" from "the device is
reachable right now". It is not an in-memory object but the `commands` table in
the database: the console, a one-shot CLI invocation (`noobscenic send …`) and
plain `sqlite3` all just insert a row, and a poller in the gateway
(`src/commands.rs`) drains it once a second while the device is online. That is
what keeps the operator surface and the gateway free of any IPC between them.

A row moves through `pending` → `sent` → `acked`; it can also end up `expired`
(older than `gateway.command_ttl_secs`, so a command enqueued while the robot was
away cannot fire hours later) or `failed` (no session key for `encrypt:1`, writer
closed, malformed payload). `sent` frames carry the queue row's id to the
connection's writer, which stores the wire-tap reference of the exact bytes back
on the row; `acked` is best-effort correlation from `cleanPack/response`, since
the protocol has no correlation id.

With the `history` feature off the terminal states are not persisted: `ack_command`
deletes the row, `expire_commands` deletes a stale pending/sent row with a warning
(so a command sent while the robot was away is still visible in the log), and
`fail_command` deletes the row, leaving the caller's warning as the record. The table
then holds only live `pending`/`sent` rows. With `history` on, all terminal rows are
kept for the operator.

## Session gate

The check every channel-A request passes through except `register`
(`channel_a::cookie_gate`, doc/PLAN.md §9.2). It turns the robot's
`Cookie: cookies=<sid>` header into a decision: a known cookie proceeds; a *missing*
header is served with a warning, because refusing it is the fastest way into a
re-register loop; and a header that is present but empty, or names an unknown or
expired session, gets `code:102` — "your sid expired" — which makes the robot
re-run `register` and pick up a fresh session. The empty case is what lets a robot
that paired against the phase-1 catch-all (and therefore never got a session)
upgrade itself to a real one on its next request, with no re-home and no reboot.
Without `history` only the newest session per device is kept; superseded cookies get
the same `102` and the robot re-registers.

## Dock-bounded path run

One logical clean path, beginning when the robot leaves its charging station and
ending when it next enters `charge` or `fullcharge`. A run can contain several
`pathID` values because the robot splits one movement into independently assembled
segments; adjacent segments are joined for display.

`path_state` persists whether the robot is docked and the current/previous run ids.
With `history` off, a new dock advances the run and drops paths older than the
previous run, so the database keeps the current run plus the last completed one.
With `history` on, every run remains in `clean_paths`. The web API selects the newest
drawable run, falling back to the completed run while a new one has only its first
point.

## Build-time feature toggles

The Cargo features that can leave the server's non-essential surfaces out of the
binary: `debug-trace` (the JSONL [wire](#wire) timeline), `debug-raw` (the verbatim
raw byte sink and the `uploads_raw` upload archive), `console` (the stdin/stdout
operator console and the one-shot CLI verbs) and `history` (the full `events`
table, `clean_paths` and terminal `commands` history and the `events` verb; with it
off `events` keeps only the newest status and bind/unbind row, `clean_paths` the
current and previous dock-bounded runs and `sessions` only the newest session per
device, and terminal command rows are dropped, which is all day-to-day operation
reads). All are in `default`, so
a plain build has every surface — the one opt-in exception is `posix-runas`, the
POSIX-only [run-as](#run-as-privilege-drop) feature; `cargo build
--no-default-features` produces the lean server — no trace files (the database's
`trace_ref` columns stay empty), no console, no `noobscenic events` subcommand, a
two-row `events` table, two dock-bounded path runs and one session per device, and
only live commands —
while the protocols, the web UI and the command queue are unchanged. Asking for a
compiled-out sink in `logging.wire` warns and records nothing rather than failing, so
one config file works across builds.

## Robot settings

The settings that live on the robot itself, as opposed to the ones this project
stores. The ⚙️ form shows both kinds side by side but treats them differently: the
label is ours (a `PUT /api/robot/{id}/settings` row in SQLite, saved on Save), while
volume, mute, the LED switch and the suction level are written to the device through
`POST /api/robot/{id}/config` (`21024`/`21022` frames) or the realtime `21020` path
for mute, and read back from the robot's own status echo (`vol`, `mute`, `led`,
`workNoisy`). Nothing about them is persisted server-side, because the robot is the
only copy — the page applies a change immediately and treats the next status push as
the confirmation, which is why the form has no Save for these.

## Consumables ("goods")

The robot's cumulative-usage counters for its wearing parts, kept in
`/tmp/Run/LastRecord/CleanInfo.json` on the device and queried with `21015` /
reset with `21016` (FUNC_STATUS.md §7). They count *cleaning* seconds, not wall-clock
time, so the page shows them as ages (`6 d 0 h 10 min`) with a ✨ button per part.
The server caches the newest reply in the `goods` table (`GET
/api/robot/{id}/goods`) and has no lifetime limits of its own — the firmware stores
no thresholds, so nothing here claims a part is "due". Because `21016` always answers
"ok", every reset is followed by a fresh `21015` read before the page believes it.

## Placed spot cleaning

The zone-cleaning view's 🎯 mode: instead of cleaning a fixed square around wherever
the robot stands (the main bar's instant spot clean, `21020 {"ctrlCode":3001}`), the
operator drags a temporary 1.5 m square to a point on the map and presses ▶️. The
server sends the square's centre as an inline `mode:"point"` region in `21023` (with
stored no-go zones folded in as `-3`), followed by `21005 {"mode":"appointClean"}`;
the robot's own clean task plans the route, drives there and spot-cleans. The square
is never saved to the robot's zone list — it exists only for that one job, which is
why the UI calls it temporary. The placement bar's **Deep** toggle switches the
region's `active` from `normal` to `depth` (cover_mode 1) for the robot's grid-style
deep pass; the default is the plain single-pass clean.

## Router build

The statically linked ARMv7 soft-float binary for the ASUS RT-AC68U on FreshTomato
(target `armv7-unknown-linux-musleabi`), built from the `router` Cargo profile and
documented in `doc/ROUTER_BUILD.md`. It carries its own libc (musl) and embeds the
web page and the migrations, so the USB stick only needs the one executable plus a
writable data directory. The profile is release plus `panic=abort`, fat LTO, one
codegen unit and `-Oz`, which brings the deployment binary to ~3.2 MB.

## Run-as (privilege drop)

The job of the `posix-runas` feature: a router or NAS rc script starts the server
as root, and `runas` in `config.json` names the unprivileged account it should
become instead. At startup the server resolves `runas.user` with `getpwnam` and
`runas.group` with `getgrnam` (no group = the user's primary group from
`/etc/passwd`), then switches with `setgid` → `initgroups` → `setuid` — before it
opens the log file or creates the data directory, so the whole deployment runs and
is owned as that account. The feature is POSIX-only and deliberately not in
`default`; a build without it warns when `runas` is set and keeps the user it was
started as, and enabling it on a non-unix target is a compile error. See
`src/runas.rs` and doc/PLAN.md §6.
