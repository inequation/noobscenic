# noobscenic

A clean-room, reverse-engineered replacement server for the **Proscenic M7 Pro**
robot vacuum, written in Rust.

The goal is to re-home the robot to a server you control — re-implementing the
vendor cloud's REST API, push gateway, and the local UDP config protocol — so it
keeps working without ever talking to Proscenic.

Protocol analysis and reverse-engineering artifacts live in [`doc/`](doc/).

Status: phases 0–1 are done and phase 3 is implemented — the server answers on both
channels now (A with a benign `code:0` catch-all, B with the `10001` handshake and
`21006` pongs), so a re-homed robot can complete its bind; phase 2's real channel-A
handlers and the live "stays connected for minutes" run are still pending.

## Running the first re-home test

This is phase 0's one remaining checklist item — pointing an actual robot at a
server you control — and it doubles as phase 1's "done when": a complete wire
trace of everything the robot tries to do. Both channels answer now: channel A
(HTTP, port 8080) replies `{"code":0}` to every endpoint, and channel B (the
gateway, port 8081) accepts the robot's TCP connection, answers the `10001`
handshake and pongs every `21006` — which, together with channel A's answer to
the bind POST, is what makes the robot leave pairing mode.

You'll need this machine on two networks in turn: the robot's soft-AP first (to
send it setup commands over channel C), then whatever Wi-Fi you're re-homing it
onto, which is where the server needs to be reachable from.

1. Find this machine's LAN IP on the Wi-Fi network the robot will join (e.g.
   `ipconfig`) — call it `<server-ip>`.
2. Start the server so it's listening before the robot ever calls home:
   `cargo run -- --data-dir ./var` (or run the binary from your `cargo build`
   directly, e.g. `target/debug/noobscenic.exe`). Wire tracing is on by
   default; traces land in `var/traces/wire-<date>.jsonl`, with verbatim
   per-event copies under `var/traces/raw/` (the default format is "both").
3. Put the robot into pairing mode (soft-AP `LDRobot`, robot at
   `192.168.78.1`) and join this machine to that AP.
4. Run the re-home:
   `python tools/rehome.py --server-host <server-ip> --ssid <your-wifi-ssid> --pwd '<your-wifi-password>' --userid <any-user-id> --trace rehome-trace.jsonl`
   It finds the robot's UDP port, points its channel-A URL and channel-B gateway
   at this machine, verifies with `getCfg`, arms the bind with `setID`, then
   stores the Wi-Fi credentials and commits with `applyCfg` last — that step
   tears down the soft-AP, so reconnect this machine to its usual network too.
5. Watch `var/traces/wire-<date>.jsonl` (or the server's stderr) for the
   robot's `register` call and whatever else it tries: every channel-A request
   is logged, persisted, and answered with a benign `{"code":0}`, even for
   endpoints not implemented yet. Channel-B frames (the `10001` handshake and
   the `21006` pings) show up in the same trace under `"channel":"B"`, each
   ping followed by the server's pong.

If the tool reports no robot, confirm the robot is actually in pairing mode and
this machine is joined to the `LDRobot` AP, not still on its normal Wi-Fi.
`--dry-run` prints the datagrams instead of sending them (give it `--port` to
skip the discovery sweep).
