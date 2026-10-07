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
