# noobscenic

A clean-room, reverse-engineered replacement server for the **Proscenic M7 Pro**
robot vacuum, written in Rust.

The goal is to re-home the robot to a server you control — re-implementing the
vendor cloud's REST API, push gateway, and the local UDP config protocol — so it
keeps working without ever talking to Proscenic.

Protocol analysis and reverse-engineering artifacts live in [`doc/`](doc/).

Status: phases 0–1 are done and phase 3 is implemented and verified on the bench unit
— the robot has been re-homed, stays connected on channel B with every `21006` ping
answered, and its `register`/`binding` posts get `code:0`; channel A's real endpoint
handlers (phase 2) are the next implementation step.

## Re-homing a robot

1. Start the server first and leave it running:

       cargo run -- --data-dir ./var

   It listens on `0.0.0.0:8080` (channel A) and `0.0.0.0:8081` (channel B). The
   listeners survive the Wi-Fi switch below — the robot dials in, the server never
   dials out.

2. Put the robot into pairing mode, join this machine to its own open Wi-Fi (the
   network name begins with `Proscenic-`), and run:

       python tools/rehome.py --server-host <server-ip> \
           --ssid <target-wifi-ssid> --pwd '<target-wifi-password>' --userid <any-id>

   It finds the robot's UDP port, points its cloud URL and push gateway at the given
   server, arms the bind, then stores the Wi-Fi credentials and commits the
   configuration.

3. The commit disables the robot's AP, so reconnect this machine to your normal Wi-Fi
   and watch the server's log and traces for the `10001` handshake and the `21006`
   pings.
   
### Re-homing - troubleshooting

If the tool reports no robot, check the robot is in pairing mode and this machine is
on the `Proscenic-` network, not still on its normal Wi-Fi.

If nothing reaches the server after the switch, check the firewall — a missing
inbound "allow" rule, or a "deny" rule may cause the robot's packets to be dropped
silently. The ports used by default are 8080 and 8081, TCP.

`--dry-run` prints the datagrams instead of sending them.
`--port 7913` skips the port discovery sweep on this model.
