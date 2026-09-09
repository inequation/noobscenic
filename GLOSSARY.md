# Glossary

Plain-language explanations of terms this project uses for its own abstractions.
See `AGENTS.md` for when to add to this file.

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
