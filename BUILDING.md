# Building noobscenic

This is a bog-standard Rust project. A plain `cargo build` / `cargo run` gets
the debug configuration built.

## Build variants

The debug traces and the operator console are build-time Cargo features, all
enabled by default, so a plain `cargo build` / `cargo run` is the full server
described in [the README](README.md):

- `debug-trace` — the JSONL wire timeline (`var/traces/wire-*.jsonl`) and the
  database `trace_ref`s that point into it,
- `debug-raw` — the verbatim byte captures (`var/traces/raw/…`) and the
  `uploads_raw` archive of `uploadLogs`/`uploadStats`/`uploadSingle` bodies,
- `console` — the operator console and the one-shot CLI verbs
  (`noobscenic devices`, `noobscenic send …`),
- `history` — the full `events` table, `clean_paths` and terminal-`commands`
  history plus the `events` console verb; without it events and sessions keep
  only the newest row per device, paths keep the current and previous
  dock-bounded runs, and terminal command rows are dropped (expired and failed
  ones with a warning).

Those five are all in `default`. `posix-runas` is the one feature outside it
(and the only POSIX-only one): it makes the server act on `runas.user` /
`runas.group` from `config.json` — resolving the account with `getpwnam` /
`getgrnam` and, when started as root, dropping to it before the log file and the
data directory are created. A build without the feature warns when `runas` is
configured and keeps the user it was started as.

A lean build drops them all:

    cargo build --release --no-default-features

That server still serves the web UI, answers the robot and drains the command queue,
but writes no traces (the `trace_ref` columns stay empty) and has no console or
state/queue/send subcommands, no `events` verb and a two-row-per-device `events`
table — stop it with Ctrl-C. Pick a subset with, for example, `--no-default-features
--features console` or `--features debug-trace`.

## Router (embedded device) Build

For a lean-sized and bounded-growth DB build variant, see
[`doc/ROUTER_BUILD.md`](doc/ROUTER_BUILD.md).