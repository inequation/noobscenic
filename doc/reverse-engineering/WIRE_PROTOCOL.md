# Channel A — app ↔ cloud IM ("imsocket") wire protocol

> **Scope / important:** this is the **phone app ↔ cloud** channel (the "IM"/transit
> server, `bl-im-<region>.robotbona.com:20008`). **The robot does NOT speak this
> protocol.** The robot's control/telemetry link is **Channel B** (raw TCP,
> `#\t#`-framed `{"infoType":…}` JSON, AES-optional) — see `PROTOCOL.md` §B and
> `rehome_server.py`. A headless rehome implements **Channel B only**; this document
> is here to explain how the *official app* reaches the robot (app → Channel A → cloud
> → Channel B → robot) and is **not** the protocol your replacement server serves to
> the robot.

Reverse-engineered from the decrypted app dex
(`com.baole.blap.module.imsocket.*`). The firmware was checked too: it contains **no**
imsocket strings (`appId`/`sendId`/`clientType`/`20008`/`robotbona`), confirming this
channel is app-side only.

Everything below is **little-endian**.

## 1. Frame = 20-byte header + body

Source: `Packet.packetHeader()` / `PacketUtils` in the dex.

| offset | size | type | field | notes |
|-------:|-----:|------|-------|-------|
| 0  | 4 | u32 LE | `total_len` | = `body_len + 20` (whole frame incl. header) |
| 4  | 2 | u16 LE | `cmd` | command code (see §2) |
| 6  | 2 | u16 LE | `type` | app always sends **200** (0x00C8) |
| 8  | 2 | u16 LE | `flag` | app sends **0** |
| 10 | 2 | u16 LE | `seq16` | rolling seq, 10001..20000 |
| 12 | 4 | u32 LE | `seq32` | request id — the app matches replies on this (`bytesToInt(buf,12)`); note it is the **same** 10001..20000 counter, not an independent 32-bit id |
| 16 | 4 | u32 LE | `ext` | app sends **0** |

- `body` = UTF-8 JSON, `body_len = total_len - 20`. Reader accumulates until it has
  `total_len` bytes (handles TCP fragmentation).
- Parse: `cmd` from `[4:6]`, id from `[12:16]` (`PacketUtils`).

## 2. Command codes (`CmdCode.java`)

Requests even, responses `req+1`.

| name | req | rsp | meaning |
|------|----:|----:|---------|
| LOGIN     | 16 | 17 | authenticate the app connection |
| LOGOUT    | 18 | 19 | |
| CLEARINFO | 20 | 21 | clean-record info |
| ROBOTERROR| 22 | 23 | robot error report |
| ROBOTINFO | 24 | 25 | robot info |
| ROBOTSTATE| 34 | 35 | robot mode/water/fan query (`checkRobotState`) |
| LASTCLEAR | 36 | 37 | last clean / map fetch (`addIMSendQueue`) |
| TRANSIT   | 250 | — | **relay envelope**: app→robot command (body = `ImMessage` JSON) |
| PUSHMSG   | 251 | — | server→app push |
| KICK      | 4135 | — | server kicks a duplicate login |
| HEADTER_SEND | 256 | — | header marker constant |

## 3. Login (app → cloud)

App connects → **LOGIN (16)** → server replies **17**. Body (`SocketBody.getLoginJson`):
```json
{"appId":"<appId>","clientType":<int>,"token":"<token>","userId":"<userId>",
 "uuid":"<uuid>","userType":"<userType>"}
```
This is the **user/phone** login. (There is no device login here — the robot logs in on
Channel B with `{"infoType":10001,…}`; see `PROTOCOL.md` §B.)

## 4. Heartbeat (app → cloud)

`HeartBeat.java`: every **6 s** the app writes a frame and reads a 20-byte reply. (This
is Channel A's keepalive; the robot's Channel-B keepalive is `{"infoType":21006}` and
is unrelated — see §B.)

## 5. TRANSIT envelope (app → robot command, via the cloud)

The **real** control path (not `SocketBody.getSendMsgJson`, which is unused dead SDK
code): `IMSocket.addSendQueue(ImRequestValue, ControlBean, cb)` builds an
`ImMessage<ImRequestValue>` and `TransitCmdManager.sendImMessage` serializes it with
fastjson and wraps it as a **TRANSIT (250)** frame
(`SocketMsgPacket.socketSendBytes(250, json)`), matching the reply by `seq32`.

Body = the `ImMessage` JSON:
```json
{ "cmd": <int>, "seq": <int>, "version": "<app version>",
  "control": { "targetId": "<robot deviceId>", "targetType": "1",
               "authCode": "..", "broadcast": "..",
               "deviceIp": "..", "devicePort": ".." },
  "value":  { "transitCmd": "<numeric-string command>", ...params... } }
```
- **`value.transitCmd`** (a numeric string, e.g. `"110"`, `"145"`) is the actual robot
  command selector; the parameters are other `value.*` fields (`mode`, `fan`,
  `waterTank`, `clearArea`, `forbiddenArea`, …). See `COMMANDS.md`.
- The cloud translates this Channel-A `ImMessage`/`transitCmd` into a Channel-B
  `{"infoType":N,"data":{…}}` for the robot. **That app-numeric ↔ robot-Channel-B
  mapping crosses Proscenic's cloud and is not fully recoverable from the client; a
  replacement server drives the robot in Channel-B terms directly (COMMANDS.md).**

## 6. Robot state the app parses back (`IMValue` / `IMResult.ValueEntity`)

`battery, brush, clearArea, clearId, clearModule, clearTime, direction, error,
errorCode, fan, map, noteCmd, track, voice, waterTank, workMode, workState`.

## 7. Reference app-side mock

`transit_server.py` is a **Channel-A** logger/mock (useful only for experimenting with
the app side). **It is NOT the rehome server** — to talk to the robot use
`rehome_server.py` (Channel B). The header struct there (`'<IHHHHII'`) matches §1.

## Cross-references
- Robot channel / rehome target: `PROTOCOL.md` §B + `rehome_server.py`.
- Command vocabulary: `COMMANDS.md`.
- Source: `dex/app_main.dex` →
  `com/baole/blap/module/imsocket/{Packet,PacketUtils,SocketBody,SocketMsgPacket,IMSocket,HeartBeat}.java`,
  `.../imsocket/utli/CmdCode.java`, `.../imsocket/utli/TransitCmdManager.java`.
