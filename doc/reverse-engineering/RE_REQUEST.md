# RE request — why does channel-B command dispatch never happen?

**From:** the noobscenic implementation side (clean room: our code derives from
`doc/reverse-engineering/` only; this request asks for more of that kind of evidence).
**Date:** 2026-10-07 · **Unit:** SN `LSLDSM7PRO20403551`, model `6716` · **Server:** this
workstation, `192.168.1.208`, ports 8080/8081 · **Robot:** `192.168.1.243`.

## TL;DR

Our replacement server completes the whole HTTP side with the real robot and keeps the
channel-B link alive, but the robot **never acts on any cloud→device frame we send**,
and never posts a command reply. The device demonstrably *reads* our frames (see the
liveness measurement below), so either the dispatch path has a precondition we have
not satisfied, or the vendor frame shape differs from every variant we could derive
from the docs. A vendor-side channel-B capture (or one device-side log) would settle it.

## What works (positive controls)

* The robot dials us on channel B (`:8081`), sends its handshake
  `{"infoType": 10001, "connectionType" : 1, "data" : {"token":"", "sn" : "LSLDSM7PRO20403551"}}#\t#`
  and then pings `{"infoType":21006,"data":{}}#\t#` every ~6.0–6.1 s. We pong
  `{"infoType":21006,"encrypt":0,"data":{"isExistConnect":true}}#\t#`.
* Channel A works end to end: the robot POSTs `cleanPack/register` (real `sn`/`ld_sn`/
  `sig` body), we mint a 32-hex `session` + `cookies`, the robot **echoes the cookie**
  on its `//cleanPack/unbinding` and `cleanPack/binding` POSTs, and `binding` → `code:0`
  → EID `0x460` (pairing indication ended). So the robot accepts and uses our HTTP
  responses, and it holds a live session from us.

## Liveness measurement (proof the device reads our frames)

With pongs turned off server-side (the socket stays open; the only frames that leave the
server are the handshake acks on each reconnect):

* first disconnect **~30 s** after the last pong;
* then reconnect → disconnect every **~18.5–18.7 s** (7 cycles measured, 10:36–10:38 UTC).

So the device notices server frames and enforces the pong contract — our bytes do reach
it. (This is also the only way we have measured the real drop timeout; the docs leave
it as a runtime variable.)

## The problem

Nothing we send is dispatched or acted on, and no reply ever comes back:

| Sent over channel B | Documented expectation | Observed |
|---|---|---|
| `{"infoType":20001,"encrypt":0,"data":{}}` | handler replies via `POST cleanPack/response` | **no HTTP request at all** |
| `21024` `{"cmd":"setledswitch","value":0/1}` | LED off/on | no visible change (operator watching) |
| `21024` `{"cmd":"reboot","value":0/1}` | reboot (link drops) | nothing |
| `21020` `{"ctrlCode":3010/3011}` | locate sound start/stop | no sound (robot was muted; after a `3022` unmute attempt too) |
| `21005` `{"mode":"smartClean"}` | starts a clean, or replies fail over HTTP | no motion, no HTTP |
| `30000` `{"cmds":[{"infoType":21024,"data":{"cmd":"reboot","value":1}}]}` | batch dispatch | nothing |

Zero channel-A requests (no `cleanPack/response` of any kind) were recorded during the
whole probe session.

## Frame variants already tried (all ignored)

* compact, firmware field order: `{"infoType":20001,"encrypt":0,"data":{}}#\t#`
* jsoncpp-styled: `{"infoType" : 20001, "encrypt" : 0, "data" : {}}#\t#`
* `encrypt:1` with the session key we issued
  (`session[:16]` = `3948a506a454f18f`, ASCII): `{"infoType":20001,"encrypt":1,"data":"/UcNqqufhH6M/bhcx18SGQ=="}#\t#`
  (AES-128-ECB, space padding, implementation verified against FIPS-197 vectors)
* styled + `encrypt:1`
* with `connectionType:1` present: `{"infoType":20001,"encrypt":0,"data":{},"connectionType":1}#\t#`
* gate-passing handshake ack immediately after the handshake:
  `{"infoType":10001,"encrypt":0,"message":"ok","data":{}}#\t#`, and a mid-session 10001
  with `encrypt`
* `encrypt` as 0 and as 1, `infoType` first and last, with/without spaces

`PROTOCOL.md` §B says `encrypt:0` is sufficient for server→device frames. Live evidence
says not even that dispatches (even though the same docs' frame-format claims hold for
everything we can verify elsewhere).

## Ruled out

* **PC firewall** — the robot's `:8081` TCP connection arrives; the executable has a
  Private-profile allow for all ports (the only 8080–8082 rule on the machine belongs
  to Spotify). Channel A requests from the robot arrive on `:8080` too.
* **Channel C** — not reachable on the LAN in this boot (probed `192.168.1.243:7913`),
  so nothing local can be re-issuing `setUrl`.
* **HTTP session/reply plumbing** — register/binding/unbinding all work (above).
* **Frame size** — our frames are 60–160 bytes; the device's own handshake is larger.

## Questions for the RE agent (priority order)

1. **Vendor capture.** What exact bytes does the vendor cloud put on the wire for a
   simple command (e.g. mute `rc 3022`, or any `21024`/`21001` setting)? Key order,
   spacing, whether `encrypt` is 0 or 1, and any extra members (`connectionType`,
   `packId`, `message`, `reason`) would answer it. Even an app→cloud→robot relay capture
   would do.
2. **Dispatch precondition.** Does the onMessage/dispatch path (`FUN_00457f08` →
   `FUN_00470b28` per the docs) check a flag/state before dispatching? E.g. "cloud
   online", "app online", "registered/session present", "bound with userId", or a state
   machine that only enables commands after a specific exchange. If so: which flag, what
   sets it, and is our register/bind (`code:0`, EID `0x460`) not enough?
3. **Handshake exchange.** Does the device expect a specific reply to its `10001`
   handshake (shape/token/timing) before it will dispatch anything? Ours is the ack
   above, sent immediately on receipt.
4. **Device-side log.** If a unit log can be pulled while our server sends probes
   (`/tmp/Run/Log` via a root shell, or `getLog` in AP mode), the lines
   `Error : Unknown infoType %d` / `Push Response Frame: %s` / `on new ping pong msg...`
   would show whether the dispatcher runs at all for our frames — the single most
   decisive artifact.
5. **Gate sanity check.** Confirm whether `encrypt:0` plaintext is really accepted for
   server-originated frames in this build (our `encrypt:1` attempt with the exact
   session key was ignored too, so this is likely not the issue, but the docs' claim
   deserves a live-capture confirmation).

## What we will do with the answer

Implement the correct frame shape or precondition in `src/channel_b/`, re-run the probes
live (the robot stays connected to this workstation), and update `PROTOCOL.md` /
`FUNC_*` with whatever the capture shows.
