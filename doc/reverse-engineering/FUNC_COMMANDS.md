# Proscenic M7 Pro — Command functions (functional catalog for a replacement server and app)

Scope: every user-facing function that the "Proscenic Robotic" 1.5.11 app (`com.baole.blap`) can
command, and the message the robot (LDRobot `LS_S6` firmware, `network_proxy`) expects for it. It
is written for someone building a self-hosted server plus their own UI.

Other documents cover the rest, and this file does not repeat them:

* Transport, framing, encryption flag and keepalive: `PROTOCOL.md` §B, `WIRE_PROTOCOL.md`.
* Replies, state echoes and telemetry (what to show after a command): `FUNC_STATUS.md`.
* Map-related commands (zone, room, spot target and go-to cleaning, no-go zones, segmentation,
  map deletion and backup): `FUNC_MAP.md`. They appear here only as one-line index rows.
* Firmware-internal command-name list: `COMMANDS.md`. Corrections to it are in §11.

**Evidence tags.** **[static]** = read from decompiled code or disassembly. **[dynamic]** = observed
live (none in this file; no command has been sent to a unit yet). **[inferred]** = reasoned from
the evidence, with confidence stated.

**Evidence sources.**

* Robot: `tmp/proscenic/rootfs/bin/network_proxy` (load base `0x400000`), and
  `remote_control_task` and `time_server_tactics` from the same rootfs. The infoType registration
  is `FUN_00475a40`; handler string sequences and decompiles are in `tmp/proscenic/func_cmds/`
  (`seq1..6.txt`, `dec1.c`, `dec2.c`, `rc.c`, `rc2.c`, `tt.c`, `ttseq.txt`). The live infoType →
  handler mapping is the map that `FUN_00475a40` builds at `0x4c6180`, read by the dispatcher
  `FUN_00470b28`, with handler pointers in the vtable at `0x494fc8`. (`tmp/proscenic/status/tbl_out.txt`
  lists an unused map at `0x4c41e8` whose entries are `std::function` thunks, not handlers.)
* App: `tmp/proscenic/jadx_app/sources/com/baole/blap/` (mainly `module/laser/activity/
  {MapLaserActivity,OptionsActivity,JAppoinmentListActivity,JBatchReservationActivity}.java`,
  `module/deviceinfor/utils/CorrectTimeUtils.java`, `module/imsocket/utli/TransitCmdManager.java`).

> **Firmware caveat.** All robot facts come from the 2020 `LS_S6` 0.7.1 image, statically. The
> user's physical unit is a different variant (`FIELD_NOTES.md`). Re-check each command on the real
> unit. Where a handler replies (§0 "Replies"), use the reply; where it does not, or always says ok,
> confirm through the status echo or a read-back query.
>
> **Two vocabularies.** The app speaks BaoLe `transitCmd` numbers. The robot speaks LDRobot
> `infoType` messages. Neither binary converts between them, so the vendor cloud did the
> translation [inferred, high confidence; `FUNC_STATUS.md` header, `APP_COVERAGE.md`]. `FUNC_MAP.md`
> §1 doubts that 1.5.11 is the M7 Pro app. This file follows the user's first-hand statement that
> it is. **A replacement stack should send the robot column directly.** The app column tells you
> which features existed and what the UI offered.

---

## 0. Feature checklist

Robot messages are Channel-B `{"infoType":N,"encrypt":0,"data":{…}}` (`PROTOCOL.md` §B). "rc N"
means `infoType 21020` with `data.ctrlCode = N` (§3).

**Replies.** Most command handlers answer with `{"message":"ok"|"fail","infoType":N,…}`
(builders `FUN_00471460` ok / `FUN_00471938` fail; 21012 and 21017 use a second pair,
`FUN_0046fba0` ok / `FUN_0046f9f8` fail), but not all of them:

* **No reply at all:** 21020 (the Channel-B handler `FUN_0046f5b0` and its wrapper
  `FUN_004703e8` only call `FUN_00413b20`), 21027 (empty stub `FUN_0046d5d0`) and 21029
  (`FUN_0046e760`/`FUN_0046ff50` only log "RequestAutoAreaMap ..."). **A server must not wait for
  an ack to a steering command.**
* **No `message` key:** 20001 and 21026 (`FUNC_STATUS.md` §2.2, §6).
* **Always ok:** 21010 (stub), 21016 (§8) and 21001 (§7.1), so an ok there proves nothing.
* A valid request can still get **fail** when the robot refuses it, for example 21005 (§1.1),
  21012 and 21017 (§1.2, §1.3), 21022 (§4.2) and 21024 `setWaterPump` out of range (§4.3).

Transport [static]: every handler hands its reply to the protocol object's generic reply
callback (the `std::function` at `+0x18`/`+0x20`). At `0x458880`–`0x4588d4` that callback is set to
the invoker at `0x459a50`, which tail-calls `FUN_00459100`. That builds
`sn&infoType&ts&userId&data` and queues it for HTTP `cleanPack/response` (`FUN_0045ed80` → FIFO →
worker `FUN_0045ee18` POSTs it to the cloud singleton's URL member `+0x68`). This confirms the
transport for 20001/21015/21019 (`FUNC_STATUS.md` §2.2, formerly its Open question 5) and agrees
with `FUNC_MAP.md` §3. Replies do **not** come back on the TCP socket; a replacement server reads
them from `cleanPack/response`.

| Feature | App transitCmd | Robot message | Covered where | Status |
|---|---|---|---|---|
| Start cleaning (whole home) | 100 | 21005 `mode:"smartClean"` | §1.1 | Robot side known [static]. App→robot mapping [inferred] |
| Pause / resume / end cleaning | 102 (`isStop` 0/1) | 21017 `cmd:"pause"/"continue"/"stop"` | §1.2 | Known [static] |
| Clean/pause toggle (single button) | — | rc 3002 | §1.2 | Known [static] |
| Return to dock / pause return / cancel return | 104 | 21012 `cmd:"start"/"pause"/"stop"`; also rc 3000 | §1.3 | Known [static] |
| Deep whole-home clean | 106 (one of mode 1..10) [inferred] | 21005 `mode:"depthTotalClean"` | §1.1 | Known [static]. Mode number [inferred] |
| Scheduled clean firing | (schedule fires on robot) | nothing: the robot starts itself (EID `0x420`) | §7.3 | Known [static] |
| App-requested appointment clean, re-run | — | 21005 `mode:"appointClean"/"reAppointClean"` | §1.1 | Known [static]. Exact use [inferred] |
| Smart room clean | — | 21005 `mode:"smartAreaClean"` | `FUNC_MAP.md` §6.4 | Index only |
| Zone / room / point target clean | 164 (+`cleanArea`) | 21023 | `FUNC_MAP.md` §6.1 | Index only |
| Spot clean at current position | — | rc 3001 | `FUNC_MAP.md` §6.3 | Index only |
| No-go / no-mop zones, room list | 166 (+`forbiddenArea`) | 21003 / 21004 | `FUNC_MAP.md` §5 | Index only |
| Room segmentation edit (merge/split/reset) | — | 21030 | `FUNC_MAP.md` §7 | Index only |
| Auto-area map request | — | 21029 | §0 "Replies" | Does nothing on 0.7.1 (log only, no reply) [static] |
| Map upload on request ("send map now") | — | 20002 / 21014 inbound | `FUNC_MAP.md` §2.1 | Index only |
| Clean-path request | — | 21011 | `FUNC_MAP.md` §3 | Index only |
| Device-attribute query | — | 21010 (stub, empty arrays) | `FUNC_STATUS.md` §6 | Index only |
| Delete map | — (no app command; 133 is the map/track poll, see §11) | 21024 `cmd:"delCurMap"`/`"cancleMap"` | `FUNC_MAP.md` §8.1 | Index only |
| Backup map | — | 21025 | `FUNC_MAP.md` §8.2 | Index only |
| Manual steering (forward/back/left/right) | 108 `direction` 1..4, release `"5"` | rc 3005/3006/3007/3008, or rc 3013 free speed | §2 | Known [static]. **Timing requirement, §2.3.** No reply to 21020 |
| Exit manual control | 108 release [inferred] | rc 4000 / rc 4001 | §2.2 | Known [static] |
| Cleaning mode (sweep / mop / sweep+mop) | 106 `mode` 1..10 | none found as a direct setter; see §4.1 | §4.1 | **Partially known** |
| Suction level | 110 `fan` 1/2/3 | 21022 `cmd:"quiet"/"auto"/"strong"/"max"` (also `"mop"`) | §4.2 | Robot side known [static]. Translation [inferred] |
| Mop water level | 145 `waterTank` 20/40/60 | 21024 `cmd:"setWaterPump"`, `value` **1..4** | §4.3 | Known [static]. App→robot mapping [inferred] |
| Y-shaped mopping pattern | — | 21005 `pathType:"y_word"`; or rc 3024 | §4.4 | Known [static] |
| Brushes / fan on-off ("clean components") | — | rc 3025 on, rc 3026 off | §4.5 | Known [static]. Exact meaning [inferred] |
| Carpet setting | 210 `carpetColor` "0"/"1" | none reachable (`SetAutoBoost` exists but has no caller) | §5 | **Unknown on robot side** |
| Obstacle-avoidance (lidar collision) toggle | — | 21024 `cmd:"setLidarCollision"`, `value` 1/0 | §4.6 | Known [static] |
| Auto-empty station frequency / run now | — | 21024 `cmd:"dustCenterFreq"` / `"startDustCenter"` | §4.7 | Known [static] |
| LED switch | — | 21024 `cmd:"setledswitch"`, `value` int | §6.4 | Known [static]. Values [inferred] |
| Volume | 123 `volume` "1.0".."2.0" | 21024 `cmd:"setVolume"`, `value` 0..10 | §6.1 | Known [static]. Scale mapping [inferred] |
| Mute | 125 `volume` "1" | rc 3022 (toggle) | §6.2 | Known [static] |
| Locate robot (play sound) | 143 | rc 3010 start, rc 3011 stop | §6.3 | Known [static] |
| Status / settings query | 98 | 20001 and others | `FUNC_STATUS.md` §2.2, §6 | Index only |
| Schedules: list / add-edit / delete | 200 / 202 / 204 | 21001 set `{timeZone, value:[entries]}` (whole list), 21002 get | §7 | Known [static]: three entry kinds (weekly, one-shot, quiet window); `unlock` = enable; ok reply proves nothing, re-read with 21002 |
| Do-not-disturb (quiet hours): get / set | 211 / 213 (`sTime`/`eTime` "HH:mm") | 21001 entry `active:false, unlock:true` with Unix `startTime`/`endTime` | §7.3, §7.5 | Known [static]; overnight windows need two entries [inferred] |
| Consumables read / reset | — (no UI in 1.5.11) | 21015 / 21016 | §8 | Known [static] |
| Robot clock set | 139 `setTime` | none needed: robot uses NTP; time zone travels in 21001 `timeZone` | §9 | Known [static] |
| Capability gating (what the UI shows) | REST `getRobotInfo` → `robot.modules` | — | §10 | Known [static] (app side) |
| Batch of commands in one message | — | 30000 `data.cmds[]` (reply carries `fastCmds[]`) | §1.4 | Known [static] |
| Dance game (music + motion) | 500 / 502 | none found | §12 | App-only; not on this robot [inferred] |
| Reboot | — | 21024 `cmd:"reboot"` | `COMMANDS.md` | Maintenance, one line only |
| Factory reset | — | rc 3012 | — | Maintenance, one line only |
| Wi-Fi reset | — | rc 3004 | — | Maintenance, one line only |
| Wi-Fi pairing (local config: `setSta` stores credentials, `applyCfg` commits) | — | (local channel, not Channel B) | `PROTOCOL.md` §C | Maintenance, one line only |
| Firmware update | 127 | 21021 / `updateMode` | `COMMANDS.md` | Maintenance, one line only |
| Network-info query | — | 21019 / 21026 | `FUNC_STATUS.md` §6 | Maintenance, one line only |
| Firmware-version query | — | 21018 | §12 | Maintenance, one line only |
| LAN remote-control channel | — | (local channel, not Channel B; handler `FUN_00414140`) | its message format is deliberately left out of this catalog. `PROTOCOL.md` §C covers only the `{"cmd":…}` config commands, but names the same listener as their transport; see §11 | Maintenance, one line only |

---

## 1. Cleaning control

### 1.1 Start cleaning — `infoType 21005` (`FUN_00472360`) [static]

`data.mode` (string) chooses the job. Any other `mode` value, or a missing `mode`, gets a fail
reply. Replies: ok = `FUN_00471460`, fail = `FUN_00471938` (both with `infoType 21005`). A valid
`mode` also gets **fail** when the start function returns non-zero (`FUN_0041b6f8`, `FUN_00411150`,
`FUN_004111d8` or `FUN_0041afb0`), meaning the robot refused the job (for example wrong state). So a
fail on `smartClean` does not mean the request was malformed.

| `data.mode` | Robot action | Extra fields |
|---|---|---|
| `"smartClean"` | whole-home automatic clean (`FUN_0041b6f8`, StartClean) | The whole `data` object is passed on, so optional `pathType` is honoured. `"pathType":"y_word"` = Y-shaped mopping (§4.4) |
| `"depthTotalClean"` | deep whole-home clean | The handler builds a new object `{"cover_mode":1}` and copies `pathType` into it if it is a string. `cover_mode` 1 indexes `CoverModeStr` = `depth` (`FUNC_STATUS.md` §3.4) [static, medium confidence on the key order] |
| `"appointClean"` | scheduled clean (`FUN_00411150`) | none read |
| `"reAppointClean"` | repeat or resume a scheduled clean (`FUN_004111d8`) | none read. Label [inferred, low] |
| `"smartAreaClean"` | smart room clean (`FUN_0041afb0`) | see `FUNC_MAP.md` §6.4 |

Minimal example: `{"infoType":21005,"encrypt":0,"data":{"mode":"smartClean"}}`.

App side [static]: `MapLaserActivity` start button sends `transitCmd "100"` with no parameters.
The app refuses when `workState` is `"5"` (charging) or `"6"`, unless the command is 100, and shows
an error dialog for `workState` `"0"`, `"30"`, `"31"`, `"32"`. A replacement UI can reuse the same
gating with the robot `mode` strings (`FUNC_STATUS.md` §3.1, §5.4).

App→robot mapping [inferred, high]: 100 → 21005 `smartClean`.

### 1.2 Pause, resume, end — `infoType 21017` (`FUN_0046fd48`/`FUN_00470768`) [static]

`data.cmd` (string):

| `data.cmd` | Action |
|---|---|
| `"pause"` | pause the current job (`FUN_004121e0`) |
| `"continue"` | resume a paused job (`FUN_00412268`) |
| `"stop"` | end the job (`FUN_00413970`) |

Example: `{"infoType":21017,"encrypt":0,"data":{"cmd":"pause"}}`.
Replies [static]: ok via `FUN_0046fba0`, fail via `FUN_0046f9f8`. Fail when `cmd` is missing, not a
string or unknown, or when the action function returns non-zero (for example nothing to pause).

Alternative: rc 3002 = "App control clean/pause" toggle (posts EID 0x405) [static].

App side [static]: the pause button opens a dialog. "Pause" sends `transitCmd "102"` with
`isStop:"0"`, "End cleaning" sends `102` with `isStop:"1"`. There is no separate resume button;
the app uses start (100) to resume [inferred, medium].

App→robot mapping [inferred, high]: `102/isStop 0` → `pause`; `102/isStop 1` → `stop`;
100 while `mode` is `pause` → `continue`.

### 1.3 Return to dock — `infoType 21012` (`FUN_004701f0`/`FUN_00470538`) [static]

`data.cmd` (string): `"start"` = go to dock (`FUN_00411350`), `"pause"` = pause the return
(`FUN_004113d8`), `"stop"` = cancel the return (`FUN_004114e8`). An unrecognised `cmd` is logged
(`FindCharge Cmd:%s error`) and **still starts the return**. A missing `cmd` behaves the same way.

Replies [static]: ok via `FUN_0046fba0`, fail via `FUN_0046f9f8` when the start/pause/stop
function returns non-zero. An unknown `cmd` is treated as `start` (above), so it fails only if the
return does.

Alternative: rc 3000 = "DirectRfCtl start find charge" [static].

App side [static]: `transitCmd "104"`, no parameters. Mapping 104 → 21012 `start` [inferred, high].
The robot's "returning, paused" state is `findchargerpause` (`FUNC_STATUS.md` §3.1).

### 1.4 Batch — `infoType 30000` (`FUN_00470d40`) [static]

The robot reads only `data.cmds[]`, an array of `{"infoType":N,"data":{…}}` that it dispatches in
turn (log `recv multiple cmd.. %s`; `dec5.c`). Each `infoType` may be an int or a numeric string
(parsed with `strtol`); entries with `infoType` ≤ 0 are skipped. `fastCmds` is a **reply** field:
the reply is `{"message":"ok","infoType":30000,"data":{"cmds":[ids],"fastCmds":[results]}}`. The
app never uses 30000. A server can use it to apply several settings in one message, for example
suction plus water before a start.

---

## 2. Manual steering (remote control)

### 2.1 Robot message — `infoType 21020` (`FUN_0046f5b0` → `FUN_00413b20`) [static]

`data.ctrlCode` (int, required) and `data.params` (object, optional; when absent the handler
passes an empty object, `Json::Value(ValueType 7)` = `objectValue` at `0x46f66c`). Codes used for
steering. The float values are [static]; the units m/s and rad/s are [inferred, high] from the
magnitudes and the `speed_v`/`speed_w` names:

| ctrlCode | Action | Speed applied |
|---|---|---|
| 3005 | forward | v = **+0.3 m/s**, ω = 0 (float at `0x487a80`) |
| 3006 | backward | v = **−0.1 m/s**, ω = 0 (float at `0x487a84`) |
| 3007 | rotate left (counter-clockwise) [inferred direction] | v = 0, ω = **+1.0 rad/s** |
| 3008 | rotate right (clockwise) [inferred direction] | v = 0, ω = **−1.0 rad/s** |
| 3013 | free speed | `params.speed_v` and `params.speed_w` (numbers; the camel-case `speedV`/`speedW` are also accepted). If either is missing or not numeric, the robot logs `SIG_SET_SPEED Params is incomplete` and does nothing. No range check in `network_proxy`: `FUN_004139f8` publishes the raw values, and `remote_control_task` passes them to `CarrierControl::SetSpeed(v, w, 300, 10000)` (the meaning of 300 and 10000 is unknown; any clamping happens below that call) |
| 4001 | zero speed (stop moving, stay in manual mode) | v = 0 |
| 4000 | stop the remote-control task (leave manual mode) | — |
| 3009 | accepted, no action | — |

Example (forward): `{"infoType":21020,"encrypt":0,"data":{"ctrlCode":3005}}`. The robot sends **no
reply** to 21020 (§0 "Replies"); watch `mode` = `rfctrl` in the status instead.
Example (arc): `{"infoType":21020,"encrypt":0,"data":{"ctrlCode":3013,"params":{"speed_v":0.2,"speed_w":0.5}}}`.
Integers are accepted as well as decimals: `network_proxy` imports `Json::Value::isDouble` from
`libcpc.so` (`0x42cd8`), and that build returns true for int, uint and real values [static].

**Entering manual mode** [static]: there is no separate "start remote control" command. The speed
setter `FUN_004139f8` (used by 3005–3008, 3013 and 4001) posts EID `0x466` "Remote ctrl start"
whenever the current mode string is not `rfctrl`, so the first steering command switches the robot
into manual mode, interrupting a running clean. While manual control is active, the robot `mode`
is `rfctrl` (`FUNC_STATUS.md` §3.1). **Caution:** rc 4001 goes through the same setter, so sending
it while the robot is not being steered (for example during a clean) also starts manual mode. Send
4001 only after a steering command, and use 4000 to leave manual mode.

### 2.2 App behaviour [static]

`MapLaserActivity.onTouch`: four arrow views send `transitCmd "108"` with `direction` =
`"1"` (top/forward), `"2"` (bottom/back), `"3"` (left), `"4"` (right). On button release the app
sends `108` with `direction:"5"` and `tag` = the released direction, then stops its timer. While a
button is held, `startInstruction` re-sends `108` every **2000 ms** (`Timer.schedule(task, 0, 2000)`).

The jadx output of `onTouch` shows the outer `switch` cases without `break`, which would mean a
release sends several `108/"5"` messages. The bytecode does not do that: every case branch ends in a
`goto` to the method's `return` (androguard dump of `MapLaserActivity.onTouch`, offsets
`0x76`, `0xd6`, `0x136`, `0x19c`), so a release sends exactly one `108/"5"` [static]. One real quirk:
pressing a second arrow while the first is held calls `startInstruction` again with the old,
already-scheduled `TimerTask`. `Timer.schedule` throws, the exception is swallowed, and the
first direction keeps repeating until release (`stopTimer` clears both) [static].

App→robot mapping [inferred, high]: direction 1→3005, 2→3006, 3→3007, 4→3008, 5→4001 (or 4000
when leaving the remote screen). Only translate a `"5"` that follows a steering command (§2.1
caution).

### 2.3 Timing requirement (important) [static]

`remote_control_task` (`rc2.c`, `FUN_00407b30`/`FUN_00407fd0`) keeps the time of the last speed
command:

* More than **400 ms** without a new speed command → the commanded speed is set to zero
  (`if (400 < now - last) speed = 0`).
* **30 s** (`0x7531` ms) without a new command → the task reports done and the robot leaves manual
  mode.
* The task can also end earlier: when a status flag (bit 0 of byte `+0xff` of the task object) is
  set and more than 1000 ms have passed since another timestamp (`DAT_004234c0`), it reports done
  (`rc2.c:180`). What sets that flag and timestamp was not traced [static, partial].

The app's 2000 ms repeat is far slower than 400 ms, so the vendor cloud must have repeated the
command itself, or the robot moved in short bursts [inferred]. **A replacement server or UI must
re-send the held steering command every ≤ 300 ms** (allow for network jitter), and send rc 4001 on
release rather than relying on the timeout.

### 2.4 App-side de-duplication [static]

`TransitCmdManager` drops any message whose JSON is identical (same hash) to one sent less than
**600 ms** before. This limits the app's own repeat rate; a server talking to the robot is not
bound by it, and it must not copy it for steering.

---

## 3. Remote-control code table (`infoType 21020`, `FUN_00413b20`) [static]

For reference, every `ctrlCode` the robot accepts. Unknown codes are logged
(`Recv unexpected control code`) and ignored.

| ctrlCode | Function | Section |
|---|---|---|
| 3000 | return to dock | §1.3 |
| 3001 | spot clean at current position | `FUNC_MAP.md` §6.3 |
| 3002 | clean/pause toggle | §1.2 |
| 3003, 3009 | accepted, no action | — |
| 3004 | Wi-Fi reset (maintenance) | — |
| 3005–3008, 3013 | steering | §2 |
| 3010 / 3011 | locate robot start / stop | §6.3 |
| 3012 | factory reset (maintenance) | — |
| 3022 | mute toggle | §6.2 |
| 3024 | Y-shaped mop clean | §4.4 |
| 3025 / 3026 | clean components on / off | §4.5 |
| 4000 | stop remote-control task | §2 |
| 4001 | zero speed | §2 |

---

## 4. Cleaning mode, suction, water, mopping pattern

### 4.1 Cleaning mode (sweep / mop / sweep+mop) — partially known

* Robot vocabulary [static]: `CleanModeStr` = `sweepOnly`, `mopOnly`, `sweepMop`, echoed as the
  status field `cleanMode` (index) (`FUNC_STATUS.md` §3.3).
* No Channel-B handler was found that sets `cleanMode` directly. The firmware string `cleanMode`
  appears in the Y-mop start event and in clean handlers (`COMMANDS.md`) [static].
* Most probable behaviour [inferred, medium]: the robot picks the mode from the fitted hardware
  (dust box versus water tank or mop plugin), as the Y-mop path's "No mop plugin, cannot start
  task" check suggests (rc 3024, error event 4043 = `0xfcb`).
* App side [static]: `OptionsActivity` sends `transitCmd "106"` with `mode` = `"1".."10"`. The
  labels come from the server (`robot.modules.clearModel[]`, entries `clearModel_<n>`,
  `clearName`). The analysed dex has no fixed label table, so which number meant what for this
  robot is **unknown**. Candidates on the robot side are the 21005 modes (§1.1) and the
  `cleanMode` values [inferred, low].

### 4.2 Suction level — `infoType 21022` (`FUN_00474440`/`FUN_00472c80` → `SetWorkMode`, `FUN_00412738`) [static]

`data.cmd` (string) = a `WorkFanLevelStr` value: `"quiet"`, `"auto"`, `"strong"`, `"max"` (the
table also holds `"mop"`, presumably the mop-only fan setting). It is stored as node config
`FanLevel` and echoed as the status field `workNoisy` (`FUNC_STATUS.md` §3.4).
Example: `{"infoType":21022,"encrypt":0,"data":{"cmd":"strong"}}`.
`network_proxy` does not validate the string: `SetWorkMode` (`FUN_00412738`, `dec3.c`) forwards
any non-empty string as `FanLevel` (an empty string gives −1). It sends it to the config node with
`SendEventAndWaitReply` (EID `0x45c`, 500 ms timeout); if that node does not answer, the result is
−1 and 21022 replies **fail** [static]. Whether the config node checks the value is unknown. Send
only the table values.

App side [static]: `OptionsActivity` fan dialog sends `transitCmd "110"` with `fan`:
`rt_fan_close` → `"1"`, `rt_fan_namal` (normal) → `"2"`, `rt_fan_strong` → `"3"`.

App→robot mapping [inferred, medium]: `"2"` → `auto`, `"3"` → `strong` or `max`. `"1"` ("close",
fan off) has no robot equivalent except `quiet` or `mop`. A new UI should offer the robot's four
levels directly.

### 4.3 Mop water level — `infoType 21024` `setWaterPump` [static]

`{"infoType":21024,"encrypt":0,"data":{"cmd":"setWaterPump","value":N}}` → `SetWaterPump(N)`
(`FUN_00413100`), stored as node config `WaterLevel` (log `the water leve is %d`). Only
**1..4** is accepted: `SetWaterPump` returns −1 when `value − 1 > 3` (unsigned compare at
`0x413174`–`0x41317c`), before contacting the config node, and 21024 then replies fail. A missing
`value` (−1) fails the same way [static]. Which end is "most water" is unknown.

App side [static]: `transitCmd "145"`, `waterTank` = `"20"` (High), `"40"` (Middle), `"60"`
(Low). Status decoding in the app treats lower numbers as more water and 255 as a sentinel
(`FUNC_STATUS.md` §5.3).

App→robot mapping [inferred, low–medium]: the app's three levels map onto three of the four robot
levels, so 20/40/60 must be translated (sending them always fails). Test each of 1..4 with the
status echo `water`.

### 4.4 Y-shaped mopping [static]

Two ways:

1. Start with `21005` `{"mode":"smartClean","pathType":"y_word"}` (§1.1).
2. rc 3024. If a mop is fitted (`DAT_004c4d90 == 1`) the robot posts the start event
   `{"mode":"smartClean","pathType":"y_word","cleanMode":"<CleanModeStr[1]>","from":"carrier"}`,
   that is mop-only Y-pattern. Without a mop it raises event 4043 "No mop plugin, cannot start
   task" and does not start.

The app has no Y-mop control.

### 4.5 Clean components on/off — rc 3025 / 3026 [static]

rc 3025 calls `FUN_004122f0(1)`, rc 3026 calls `FUN_004122f0(0)`. The status echo is
`cleanComponents` (bool, "brushes/fan enabled", `FUNC_STATUS.md` §2). Meaning [inferred, medium]:
switch the brushes and fan on or off (for example while mopping only). No app control.

### 4.6 Obstacle avoidance (lidar collision) — `21024` `setLidarCollision` [static]

`value` 1 = on, anything else = off (`FUN_00417890(value == 1)`), stored as `LidarAvoidCollision`,
echoed as `ldAvoidColli`. No app control.

### 4.7 Auto-empty dock — `21024` `dustCenterFreq` / `startDustCenter` [static]

* `{"cmd":"dustCenterFreq","value":N}` → `SetDustCenterFreq(N)` (`FUN_00417030`), stored as
  `DustCenterFreq`, echoed as `dustCenterFreq`. Units unknown (probably "empty every N cleans"
  [inferred, low]).
* `{"cmd":"startDustCenter"}` → empty the dust box now (`FUN_00417400`). The robot `mode` becomes
  `DustCenterWorking` while it runs.
* Only meaningful with a dock that has a dust station (`workstationType` echo). No app control.

---

## 5. Carpet setting — robot setter unreachable

App side [static]: `OptionsActivity` carpet dialog sends `transitCmd "210"` with `carpetColor` =
`"1"` (`rt_carpet_light`, "Mild") or `"0"` (`rt_carpet_deep`, "Deep"). The status echo is
`carpetColor` (`FUNC_STATUS.md` §5.3).

Robot side: no `21024` command name, `infoType` or status field for carpet was found. The robot
status has an `autoBoost` field (`FUNC_STATUS.md` §2), which suggests automatic suction boost on
carpet [inferred, medium]. A setter `SetAutoBoost` exists in `network_proxy` (`FUN_004144e8`), but
it is **unreachable**: no direct call, no address load and no data pointer refers to it [static].
The same holds for `SetFanLevel` (`FUN_00411570`) and `SetForbidMode` (`FUN_004147e8`). So on this
firmware the carpet boost cannot be changed from the network. See Open questions.

---

## 6. Volume, mute, locate, LED

### 6.1 Volume — `21024` `setVolume` [static]

`{"cmd":"setVolume","value":N}` with **N = 0..10**. N ≥ 11 or a missing value fails. The robot
stores N × 10 (0..100) via `SetVolume` (`FUN_00412be8`) and reports it back divided by 10 as `vol`
(`FUNC_STATUS.md` §2).

App side [static]: `setVoiceNote` sends `transitCmd "123"` with `volume` = `1 + slider/100`, one
decimal (`"1.0".."2.0"`; `"2"` at 100). App→robot mapping [inferred, high]:
`N = round((volume − 1) × 10)`.

### 6.2 Mute — rc 3022 [static]

rc 3022 **toggles** mute: if unmuted, enters mute (EID 0x49f "App Set Enter Mute mode"); if muted,
leaves it (EID 0x4a0). Status echo: `mute`. A server must read `mute` first and only send 3022
when the state needs to change. An alternative is `setVolume` 0, which is a different setting
[inferred].

App side [static]: when the speaker icon is selected, `setVoiceNote` sends `transitCmd "125"`
(`RobotConnectFactory.CODE_FAIL_GET_IP` = 125) with `volume:"1"`. Unmuting sends 123 with the
slider value.

### 6.3 Locate robot (play a sound) — rc 3010 / 3011 [static]

rc 3010 starts the "find robot" sound (EIDs 0x412 and 0x47b), rc 3011 stops it (EID 0x47c).

App side [static]: the `tv_search_device` view sends `transitCmd "143"`, no parameters (refused
when `workState` is `"0"`). This corrects `COMMANDS.md` (§11).

### 6.4 LED switch — `21024` `setledswitch` [static]

`{"cmd":"setledswitch","value":N}` → `FUN_00412fe8(N)`. Echo `led`. Values [inferred, medium]:
1 = on, 0 = off. No app control.

---

## 7. Schedules and quiet hours

Re-derived from `time_server_tactics` (`tt.c`, `ttseq.txt`; reviewer disassembly in
`tmp/review_fc3/`). Addresses are in that binary unless stated.

### 7.1 Messages — `21001` set, `21002` get [static]

* **21001** (`network_proxy` `FUN_004716e8`/`FUN_004717c8`) calls `SetTimeTactics`
  (`FUN_00410838`), which only posts the request `data` as event `0x428`; then the handler
  tail-calls the ok builder unconditionally (`0x47174c`). **The ok reply proves nothing; re-read
  with 21002 after every 21001.**
* `time_server_tactics` handles the event (`0x407c28`, reached from `0x404698`):
  * `timeZone` (optional): applied with `SetTimeZoneVal` only if it is a JSON **int** (whole
    hours, negatives allowed); anything else is silently ignored and the previous value kept.
  * `value` (optional): the tactics list, a JSON array, validated per entry (`FUN_004078f8`) and
    saved to `/tmp/Run/Config/time_setting.json`. It **replaces the whole list**.
  * Neither key causes a failure when missing.
  Shape: `{"infoType":21001,"encrypt":0,"data":{"timeZone":<int hours>,"value":[entry, …]}}`.
* **21002** (`FUN_00471c68`/`FUN_00474740` → `GetTimeTacticsWithTimeSec`, `FUN_00410c48`) replies
  `{"data":{"timeZone":…,"timeZoneSec":…,"value":[…]},"message":"ok","infoType":21002}`. It shows
  the stored list, including rewrites the robot made (see one-shot entries below).

### 7.2 Entry fields and validation [static]

`FUN_004078f8` drops (logs "Time Json illegal") any entry that lacks one of:

| Key | Type | Meaning |
|---|---|---|
| `active` | bool, required | **entry kind selector**, not "enabled": true = clean, false = quiet window (§7.3) |
| `unlock` | bool, required | **per-entry enable flag**. The arming loop (`FUN_0040a248`, `isMember("unlock")` at `0x40a94c`, `asBool` test at `0x40a9f0`) skips the entry when it is false (`0x40aa08`). Must be **true** for the entry to do anything; false = disabled but stored |
| `startTime` | uint, required | meaning depends on the kind (§7.3) |
| `endTime` | uint, required | window end; ignored for clean entries |
| `period` | array | weekdays as `tm_wday`, **0 = Sunday … 6 = Saturday**. Validation accepts it missing, but the arming loop **skips entries without a `period` key** (`0x40a964`–`0x40a984`), so always send it (`[]` allowed) |
| `workNoisy` | string | suction level for the run (`quiet`/`auto`/`strong`/`max`), copied into the start event; `"unknown"` if missing (max 15 chars) |
| `cleanNum` | uint | passes, copied into the start event |
| `cleanAreas` / `tagIds` / `cleanId` | int arrays | room/zone ids, copied into the start event as `cleanId` (`FUNC_MAP.md` §4) [static keys, mapping inferred, medium] |

Validation also **clears `period` to `null`** (`DAT_00422520`, built in `_INIT_3` `0x404330` as
`Json::Value(nullValue)`) on every entry with `active:false`. A null `period` still counts as
present, with size 0.

### 7.3 The three entry kinds [static unless marked]

The arming loop computes "now" as UTC + `timeZone × 3600` (`gmtime_r`) and arms timers for
today only. It refuses to arm anything while the clock is before 2018 ("Local Time is not
updated!", `tm_year < 0x76`; §9).

| Kind | Encoding | Timing | Timer status |
|---|---|---|---|
| **Weekly clean** | `active:true`, `unlock:true`, `period:[days…]` (non-empty) | `startTime` = **seconds since local midnight**; arms when today's `tm_wday` is in `period`. `endTime` not read | 0 |
| **One-shot clean** | `active:true`, `unlock:true`, `period:[]` | `startTime` = **absolute Unix time** (UTC epoch); the robot adds `timeZone × 3600` and arms it if it is today and less than 24 h ahead. If it is more than 60 s in the past, the robot rewrites the entry's `unlock` to `false`, disabling it (`active` stays `true` and `period` stays `[]`, so it is skipped from then on — it does **not** become a quiet window), and persists the list (`FUN_00407148` writes `/tmp/Run/Config/time_setting.json`); evidence `0x40b890`–`0x40b8c8` (write) and `0x40ae5c`–`0x40ae64` (60 s test). Delete one-shots after they fire | 1 |
| **Quiet window** | `active:false`, `unlock:true` (`period` is cleared by validation) | `startTime`, `endTime` = **absolute Unix times**; only their time of day (after `+ timeZone × 3600`) is used, so the window repeats **every day** [inferred, medium]. If both fall on the same weekday: window from start to end. Otherwise (crossing midnight, or end before start): window from start to **midnight only** | 2 at start, 3 at end |

Timer status 2 is named `TTS_DISABLE_CLEAN_START` in the tactic sorter (`FUN_00407fa8`): no
scheduled clean starts inside the window. The status-to-event mapping of the timer loop
(`tt.c:160–176`) posts the start event (EID `0x420`), **Enter silent mode** (EID 1057, `0x421`),
"Update net time" (EID 1061, `0x425`, §9) and **Exit silent mode** (EID 1058, `0x422`); that the
window entries drive the silent-mode events is [inferred, high].

`FUN_0040a248` also contains a weekly form of the window (`active:false` with a non-empty
`period`, including carry-over of an overnight window into the next listed day, `(tm_wday+6)%7`).
Because validation clears `period` on every inactive entry, that branch is not reachable through
21001 [static, medium].

**When a clean fires**, the robot starts itself: the timer posts EID `0x420` `EID_I_TIME_TO_WORK`
with `{"workNoisy":<str>,"cleanId":[int…],"cleanNum":<uint>}` (`FUN_00403a08`, `ttseq.txt`
`0x403db8`–`0x403e74`). The server sends nothing. (The 21005 `appointClean` mode is a different
path: EID `0x410` `EID_I_APP_APPOINT_CLEAN` with no payload, `FUN_00411150`.)

### 7.4 App side — laser screens [static]

* **200** (`JAppoinmentListActivity.getTimeList`): fetch the list. No parameters.
* **202** (`JBatchReservationActivity.submitTime`): add or edit one entry, sent as
  `orders:[{orderIds, sign, hour, minute, valid:"1"}]`.
  * `orderIds` = selected weekdays, comma-separated, **0 = Sunday … 6 = Saturday**.
  * `sign` = entry ID, created as `"<unix seconds>,"` on first save and kept for edits.
  * `hour`, `minute` = start time, local.
* **204** (`deleteTime`): delete one entry by `sign`.
* **211** (`OptionsActivity.getDisturbTime`): read quiet hours. **213**: set them, sent from both
  `OptionsActivity.getDisturbTime` and `DisturbTimeActivity.startDisturbTime`: `isOpen`
  `"1"`/`"0"`, `sTime` and `eTime` as **`"HH:mm"`** local time (defaults `startHour "23"`,
  `endHour "07"` at `DisturbTimeActivity.java:57–60`; parsing and sending at lines 116–141).
* Older (non-laser) screens use **113 / 115 / 117 / 149** for single appointments (`orderId`,
  `hour`, `minute`) and batch reservations (`orderIds`). Not used on the laser path.

### 7.5 Translation (server responsibility) [inferred, medium unless marked]

The robot accepts only the whole list, so the server keeps the canonical list (schedules plus the
quiet window), applies 202/204/213 or its own UI's edits, pushes it with 21001 together with
`timeZone`, and confirms with 21002. Entries have no ID key; the server keeps its own `sign` →
index map.

* **Schedule (202):** `{"active":true, "unlock":true, "startTime":hour×3600+minute×60,
  "endTime":0, "period":[orderIds…], "workNoisy":…, "cleanNum":1}`. A disabled schedule is
  `unlock:false` (keep `active:true`; `active:false` would turn it into a quiet window) [static].
  `endTime` is required but unused, so any uint works [static].
* **Quiet hours (213, `isOpen:"1"`):** `active:false, unlock:true`, with `startTime`/`endTime` as
  Unix times on one day whose local time of day equals `sTime`/`eTime`. Because a window that
  crosses midnight only runs to midnight, send an overnight window such as 23:00→07:00 as **two**
  entries: 23:00→23:59:59 and 00:00→07:00, each with both times on the same day [inferred,
  medium; unverified on a unit]. `isOpen:"0"` = the same entries with `unlock:false`, or removed.
* The robot status has no DND field (`FUNC_STATUS.md` §0); 211 is answered from the server's own
  copy or from 21002.

---

## 8. Consumables — `21015` read, `21016` reset [static]

Payloads and keys: `FUNC_STATUS.md` §7.2 (not repeated here). One command-side point: both reply
paths of the 21016 handler (`FUN_0046f2a8`) set `"message":"ok"`, and no fail path was found, so
an ok does **not** prove the reset took effect. Re-read the part with 21015 after every reset.
The 1.5.11 app has no consumables screen [static, high confidence: `APP_COVERAGE.md` shows the dex
dump is complete and no code is loaded at runtime; `FUNC_STATUS.md` §7.3].

---

## 9. Robot clock — NTP on the robot, time zone from 21001 [static]

App side [static]: `CorrectTimeUtils.setTime` sends `transitCmd "139"` with `setTime` =
`yyyyMMddHHmm` + `"00"` + weekday digits (`"00"` = Sunday … `"06"` = Saturday), phone-local time.
It has several triggers [static]: `CorrectTimeUtils.setCorrectTime(List)` calls `setTime` for every
robot whose `workState` is not `"0"` each time the main robot list (`robot/getMyRobotList.do`)
loads (`MainFragment$14.accept`); `BaoLeApplication.setCorrecTime(deviceId)` re-sends it for one
robot, on `workState "100"` (Online; `FUNC_STATUS.md` §5.3); and the non-laser
`NewAppoinmentListActivity` calls `setCorrectTime()`.

Robot side [static]: `time_server_tactics` sets its own clock. Its NTP client (`ntpclientfunc.cpp`)
queries `cn.ntp.org.cn`, `edu.ntp.org.cn`, `us.ntp.org.cn` and `0.pool.ntp.org`, sets UTC with
`date -u -s "…"` and asks for the tactics to be reset ("ntp want set ResetTimerTactics"). The tactic
timer loop then posts EID 1061 `EID_I_NET_TIME_UPDATED` ("Update net time", §7.3) and re-arms the
schedules. Local time = UTC + `timeZone × 3600`, where `timeZone` comes from the 21001 message
(§7.1). No Channel-B message sets the clock itself.

For a replacement: let the robot reach an NTP server (or answer NTP for those host names on the
local network), and include `timeZone` (whole hours) in every 21001. App command 139 has no robot
equivalent; a server can drop it, or use it to learn the phone's UTC offset. Daylight-saving
changes need a new 21001 [inferred, high].

---

## 10. Capability gating (what the UI should offer) [static, app side]

The app builds its controls from the REST `getRobotInfo` reply (`robot.modules`, class
`module/deviceinfor/bean/Modules.java`). Flags are strings, `"1"` = supported:

`radar`, `map`, `camera`, `vision`, `fan`, `waterTank`, `voice`, `suportOrder` (schedules),
`suportVoice`, `suportBattery`, `suportRecharge` (dock), `suportCtrl` (manual control),
`suportErrorRecord`, `suportViewState`, `network` (`softAp`/`smartLink`/`noiseLink` pairing
methods), plus `clearModel[]` (`clearSign`, `clearName`: the mode list for §4.1). `funDefine` (passed to
the update screen) is a top-level `RobotInfo` field (`RobotInfo.java:17`), not part of `modules`.

A self-hosted server that keeps the vendor app must return these flags. Suggested values for an
M7 Pro [inferred, medium]: `radar`, `map`, `fan`, `waterTank`, `voice`, `suportOrder`,
`suportVoice`, `suportBattery`, `suportRecharge`, `suportCtrl` = `"1"`; `camera`, `vision` = `"0"`.
A new UI on the robot vocabulary can instead gate on robot echoes: `workstationType` (dust
station), `mop` (mop fitted) and `cleanComponents`.

---

## 11. Corrections to existing documents

| Document | Says | Correct |
|---|---|---|
| `COMMANDS.md` app table | 143 = "setting / error info" | 143 = **locate robot** (`tv_search_device`) → rc 3010 [static] |
| `COMMANDS.md` app table | 98 = "a device setting" | 98 = **status/settings query** (`getRobotState`) [static] |
| `COMMANDS.md` app table | 213 = DND window; 211 absent | 211 = read DND, 213 = set DND (`isOpen`, `sTime`, `eTime`) [static] |
| `COMMANDS.md` app table | 100/102/104/108/123/125/164/166/500/502 absent (200/202/204 are listed, in the schedule row) | see §0 checklist [static] |
| `COMMANDS.md` app table | 131/133 = "map ops" | 133 is the laser map/track **poll** (`MapLaserActivity.getIncrementMap`, `trackNum`, `mapSign`, every 2–5 s; `FUNC_MAP.md` §9.1). It is not a delete. The app has no delete-map command; its delete option calls the REST `unBindRobot` [static] |
| `COMMANDS.md` encodings | `fan` "1"/"2"/"3" (unlabelled) | `"1"` = off (close), `"2"` = normal, `"3"` = strong [static] |
| `COMMANDS.md` "Driving a rehomed robot" | `data` key names and routing infoTypes not proven | Proven [static]: 21005 `data.mode`; 21012/21017/21022 `data.cmd`; 21024 `data.cmd` + optional int `data.value`; 21020 `data.ctrlCode` + `data.params` (§1–§4) |
| `COMMANDS.md` "Clean / navigation" | go-home is not a JSON literal | Go-home is 21012 `data.cmd:"start"`, or rc 3000 [static] |
| `COMMANDS.md` "Clean / navigation" | schedules exist only as app-side `orders[]` | The robot has its own schedule list, 21001 set / 21002 get (§7.1) [static] |
| `PROTOCOL.md` infoType map, `COMMANDS.md` reports, `schemas/channelB_gateway.schema.json` (`packId` property, line 14; `pack_21020`, lines 105–108) | 21020 = "chunked pack transfer (`packId`)" | On Channel B, 21020 is the **remote-control command**: `data.ctrlCode` + `data.params` (`FUN_0046f5b0`). The `packId` ack and the "invalid json" replies come from the separate LAN control channel's handler (`FUN_00414140`), which reads `ctrlParams`, not `params` [static] |
| `PROTOCOL.md` §C (transport bullet), `FUNC_STATUS.md` §6 (21026 row) | the `OpenUdpRemoteCtrl` listener on the `rand()%1000+9000` port is the `{"cmd":…}` config channel; 21026 reports "the local UDP control endpoint (§C)" | In `network_proxy`, `OpenUdpRemoteCtrl` (`np_all.c:8245–8248`) registers the **LAN remote-control handler** `FUN_00414140` on that port. Whether the `{"cmd":…}` config handler shares the same listener is not shown, so the two documents may be describing two different listeners as one. Reconcile before relying on either (this catalog deliberately gives no LAN message format) [static] |
| `FUNC_MAP.md` §3 and §10 | *(was)* 21020 = "ctrlCode command with `packId` ack" | **Resolved 2026-10-05:** FUNC_MAP's revision now states the same split as §2.1 — the Channel-B 21020 handler sends **no reply at all**; the `packId` ack exists only on the LAN handler (`FUN_00414140`) [static] |
| `COMMANDS.md` app table | 210 = "a device setting" | 210 = **carpet setting** (`carpetColor` "0" deep / "1" mild; §5) [static] |
| `PROTOCOL.md` infoType map | lists only 21003 as cloud→device | `FUN_00475a40` registers handlers for all of: 20001, 20002, 21001–21006, 21010–21012, 21014–21031 and 30000. The inbound ones are requests or commands from the server; 21006 is the keepalive (`PROTOCOL.md`) [static] |

---

## 12. Not supported on this robot

* **Dance game** (`module/egg/BuildEggActivity`): `transitCmd "500"` start / `"502"` stop, with
  music and a list of motion actions. No matching robot handler [static]; treat it as an app
  feature for other BaoLe models [inferred, medium].
* **Go to a point without cleaning**: not found (`FUNC_MAP.md` §0).
* Other registered infoTypes [static]:
  * **21018** → `FUN_0046f430` calls `GetVersion` (`FUN_00415240`) and replies
    `{"message":"ok","infoType":21018,"data":{version, fullversion, hasUpdateFile, mcu}}`:
    firmware-version query (maintenance).
  * **21027** → `FUN_0046d5d0`, an empty stub, no reply.
  * **21028** → `FUN_00473248`, a `data.cmd` query that echoes `cmd` with `data`; purpose unknown.
  * **21029** → `FUN_0046e760` (wrapper `FUN_0046ff50`) only writes the log line
    "RequestAutoAreaMap ..." (`protocol_ld.cpp:0x227`): no action and no reply on 0.7.1. The name
    suggests a request for the auto-segmented room map; `FUNC_MAP.md` §7 covers only 21030.
  * **21031** → device identity data, not a user function.

---

## Open questions

1. **App→robot translation tables** (vendor cloud): `mode` 1..10 → 21005 mode or `cleanMode`;
   `fan` 1/2/3 → `quiet`/`auto`/`strong`/`max`; `waterTank` 20/40/60 → `setWaterPump` value.
   Resolve by sending each robot value and watching the status echo (`workNoisy`, `water`,
   `cleanMode`).
2. **`setWaterPump` direction**: which of 1..4 is most water, and its relation to the `water` echo.
3. **Schedules**: confirm on a unit that a quiet window repeats daily, that the two-entry overnight
   split works, that a stale one-shot comes back from 21002 with `unlock:false` (rewrite is static,
   §7.3), and how `cleanAreas`/`tagIds`/`cleanId` map to the start event's `cleanId`.
4. **Robot clock**: solved in principle (NTP plus 21001 `timeZone`, §9). Open: which NTP servers
   the real unit's firmware uses, and how half-hour time zones are handled (`timeZone` is int hours).
5. **Silent mode effects**: what silent mode changes besides blocking scheduled starts (voice,
   fan level), §7.3.
6. **Carpet setting**: `SetAutoBoost` is unreachable on 0.7.1 (§5); check whether newer firmware wires it up.
7. **Cleaning mode selection**: whether `cleanMode` can be set, or follows the fitted hardware.
8. *(Resolved: replies go out over HTTP `cleanPack/response`; §0 "Replies", `FUNC_STATUS.md` §2.2.)*
9. **`depthTotalClean`** exact object (`cover_mode` key placement) and whether it also needs
   `mode` inside the forwarded object.
10. **Voice language / voice pack**: no command found on either side.
11. **Purpose of 21028.**
12. **Steering**: confirm 3007 = left and 3008 = right on the real unit, that a 250–300 ms resend
    gives smooth motion, what the `SetSpeed(…, 300, 10000)` arguments mean, and what triggers the
    early exit in §2.3.
13. **Firmware variant**: every robot fact here comes from the 0.7.1 `LS_S6` image; the user's unit
    differs (`FIELD_NOTES.md`). Re-check each command on the real unit: use the ok/fail reply where one exists, and a read-back
    where it does not or always says ok (21001 → 21002, 21016 → 21015, 21020 → status `mode`; §0).
