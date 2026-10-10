# Proscenic M7 Pro — Status / Telemetry Reporting (functional overview)

Scope: how the robot **reports** its state (battery, charging, work mode, errors, pose,
consumables, statistics, settings echo, network and firmware info), and how the Android app
(`com.baole.blap`, 1.5.11) receives and decodes it. It is written for someone building a
replacement server or app.

Other documents cover the rest, and none of it is repeated here:

* Map transfer and rendering, and how the pose is drawn: `FUNC_MAP.md`.
* Command encodings, including consumable reset and settings: `FUNC_COMMANDS.md`. Where both
  documents touch the same feature, this one owns the status and echo decoding and
  `FUNC_COMMANDS.md` owns the setters.
* Transport, framing, crypto and the keepalive: `PROTOCOL.md` §A/§B and `WIRE_PROTOCOL.md`.

**Evidence tags:**

* **[static]** = read from decompiled code or the binary.
* **[dynamic]** = observed live.
* **[inferred]** = reasoned conclusion, with the confidence stated where it matters.

**Where the evidence lives:**

* **Robot side ("R")** comes from `rootfs/bin/network_proxy` (load base `0x400000`) unless
  another binary is named.
* **App side ("A")** comes from `jadx_app/sources/com/baole/blap`. Every section is marked R or A.
* Helper scripts and output are in `tmp/proscenic/status/`. That covers the Ghidra project
  copies `gproj/np` (`network_proxy`) and `gproj/tm` (`task_manager`). It also covers
  `tbl.py` / `tbl_out.txt` (the infoType dispatch table) and `res.py` (the app string-ID
  resolver).

> **Firmware caveat:** all robot-side facts come from the 2020 `LS_S6` 0.7.1 sample, statically
> (`FIELD_NOTES.md`). No status traffic from any unit has been captured.
>
> **App identity:** the user confirms first-hand that app 1.5.11 is the app that controlled their
> M7 Pro. So the app's BaoLe vocabulary (§5) and the robot's LDRobot vocabulary (§2–§4, §6–§7)
> are two ends of one working system. No code in either binary converts between them. The
> conversion therefore lives in the vendor cloud **[inferred, high confidence]**. The claim in
> `FUNC_MAP.md` §1 that 1.5.11 is "a different product's app" is not adopted here.
> `APP_COVERAGE.md` has since established that the dex dump is complete **at class level** and
> that the app has no dynamic-code mechanism. **Caveat [static]:** 75 `com.baole` activities
> (77 total in `app_main.dex`) declare `protected native void onCreate` (Jiagu method
> protection). Their bodies are not in the dex. Affected activities include `DialogActivity`, `OptionsActivity`,
> `MapLaserActivity`, `GetRobotErrorInfoActivity`, `GetRobotErrorsListActivity`,
> `CleanRecordActivity` and `MyDeviceActivity`. Their private start-up methods have no Java
> callers (e.g. `DialogActivity.initMyView`, `GetRobotErrorInfoActivity.getData`,
> `OptionsActivity.getIMRobotState`/`getRobotState`). So **when** those screens fetch data is
> [inferred]. **What** they parse is still [static]. Two native libraries are missing. The packer's own `jgbEC` is dead
> code for interop. The only one that matters is the native map decoder `libtoBitmap.so`, which
> affects map rendering but not status or control.

---

## 0. Feature checklist

| Feature | Covered where | Status |
|---|---|---|
| R: robot status record (battery, mode, sub-mode, pose, area and time, errors, settings echo) | §2. Corrects `COMMANDS.md` "Status frame" | Field list and pose encoding known [static, LS_S6]: `pos` = metres×1000, `phi` = rad×1000. Area unit **unknown** |
| R: how status reaches the server (HTTP push 20001 and Channel-B request 20001) | §2.1–2.3 | Push format and loop known [static]. The change→dirty-flag link is [inferred]. Reply channel for the 20001 request is known [static] |
| R: push gate (app-online flag from pong `isExistConnect`) | §2.3. Extends `PROTOCOL.md` "Pong contract" | Known [static]. Initial flag value unknown |
| R: work-mode (CPS), sub-mode, clean-mode and fan strings | §3 | String tables fully known [static]. Whether `subMode` carries the `--` prefix and the default `cleanMode` index are unknown |
| A: `workState` codes and their UI strings | §5.3 | Fully known [static] |
| R→A: robot `mode` → app `workState` | §5.4 | Partially. Cloud-side; mapping is [inferred] |
| R: error codes (firmware index → cloud code) | §4.1 | Partially. Table dumped; meaning of the index is unresolved |
| A: error text shown in the app | §5.6 | Fully known: text comes from the server, the app has no table |
| R: event notifications (`infoType` 20003, LD 6xxx codes) | §4.2 | 20003 body fully known [static]. The `ReportPush` (LD 6xxx) record is not decoded |
| R: device attributes, `infoType` 21010 | §6 | Fully known: it is a **stub** that returns empty arrays |
| R/A: live pose | R §2 (`pos`/`phi`); A §5.5 (taken from the track, see `FUNC_MAP.md` §9.2) | Robot encoding known [static]. The cloud's conversion to app map units is unknown |
| R: consumables (side/main brush, filter, sensors, battery, mop, dust box) | §7 | Firmware side fully known [static] |
| A: consumables UI | §7.3 | None exists in the analysed dex |
| R: cleaning statistics (current run and lifetime) | §2, §7.1 | Fully known [static] (area unit unknown) |
| A: cleaning history and records | §5.7. Robot upload 20004 is in `FUNC_MAP.md` §8.4 | Partially (REST backend) |
| R/A: volume, mute, LED, DND, carpet, fan, water echo | §2, §5.3 | Known for app decoding. The robot status has no DND field, and no carpet-*colour* field — the only carpet-related robot field is `autoBoost` |
| R/A: firmware version, Wi-Fi signal, IP, MAC | §6 (21019, 21026), §5.8 | Known [static], except the `staSignal` unit |
| A: polling vs push | §5.1 | Push handling and map polling are [static]. When screens with a native `onCreate` start their fetches is [inferred] (header) |

---

## 1. Architecture (who talks to whom)

`PROTOCOL.md` covers the transport. This section adds what each side does with **status**:

```
robot ── HTTP POST cleanPack/uploadEvents (status 20001, events 20003, records 20004) ─▶ vendor cloud
robot ◀─ Channel B request 20001 / 21015 / 21019 / 21026 … ── reply via HTTP POST
         cleanPack/response (generic reply callback, §2.2) ────────────────────────────▶ vendor cloud
vendor cloud ── imsocket PUSH (value.noteCmd = 100/101/102/103/109/200) ────────────────▶ app
app ── imsocket ROBOTSTATE(34) / LASTCLEAR(36) / TRANSIT(250) polls ────────────────────▶ vendor cloud
app ── REST bl-app*.robotbona.com/robot/*.do (robot info, records, errors) ─────────────▶ vendor cloud
```

The two vocabularies differ:

* **Robot** = LDRobot JSON: `mode` strings and int fields.
* **App** = BaoLe `workState` code strings.

Because nothing on either device converts between them, a replacement stack has two options:

* **(a)** Build its own app on the robot vocabulary (§2–§4, §6–§7).
* **(b)** Keep the vendor app and re-implement the cloud bridge (§5, mapped as in §5.4, [inferred]).

---

## 2. R: the robot status record (`DevAttrDataReal`)

The robot keeps one status object (`logic/devattr_data.cpp`, RTTI `15DevAttrDataReal`). Two
builders serialise it:

* `FUN_0044d270` (`event_send.cpp`) is the **wire** builder, used by the push and by the 20001 request.
* `FUN_0041f010` writes a debug dump to `/tmp/devattr` (`FUN_0041f868`).

The table below is in **wire order**, read from the instruction stream of `FUN_0044d270`.
Keys, types and order are [static]. The meaning column is static only where a source is given.
Otherwise it comes from the key name and is tagged.

| key | type | meaning | notes |
|---|---|---|---|
| `pos` | `[int x, int y]` | robot position, **world-frame `Pose2D` × 1000** | Producer `FUN_00443190` (dup `FUN_004430f0`), a ZMQ subscriber (`AddSubscriber(ZmqMgr,0x1e7e,…)`, message id `0x3fb`). It computes `(int)(x*1000)`, `(int)(y*1000)` (`DAT_0048cbf0`=1000.0) into globals `0x4c4600`/`0x4c4604` = devattr `+0x2f8`/`+0x2fc` (devattr base `0x4c4308`) [static]. With `Pose2D` in metres this is **mm** [inferred, high confidence], consistent with 21011 paths in mm (`FUNC_MAP.md` §3) |
| `phi` | int | heading, **milliradians** | `(int)(phi*1000)` → `0x4c4608` = `+0x300`, from the same producer. **No π offset** [static]. The range is about −3142..3142 *if* `Pose2D.phi` is normalised to (−π, π] [inferred; the normalisation was not checked]. ⚠ The dock heading `chargeHandlePhi` in the 20002 map upload is encoded **differently**: `(int)((phi+π)*1000)`, range 0..6283 (`ChargerControl::ReadCharger` `FUN_0043eda0`; constants `0x48c2b8`=1000.0, `0x48c2c0`=π). `chargeHandlePos` is `(int)(x*1000),(int)(y*1000)`. The earlier `MAP.md` line 37 (`<int deg>`) and `schemas/channelB_gateway.schema.json` (`chargeHandlePhi` "degrees") texts were wrong and were corrected 2026-10-05 |
| `reliable` | int | pose-valid / localised flag | [inferred from the name] |
| `vol` | int | volume | Stored 0..100. Both the push (`FUN_0044dbd8`) and the 20001 request handler (`FUN_0046ebc8`) **divide it by 10** (`udiv` via the `0xcccd` magic on key `vol`), so the wire value is **0..10**. This matches `setVolume` in `COMMANDS.md` |
| `led` | int | LED switch [inferred from the key name] | |
| `elec` | int | battery level | percent [inferred]. The OTA guard in `FUN_00414af0` requires mode `charge`/`fullcharge` **and** `elecReal` ≥ 15, which fits a percentage |
| `elecReal` | int | battery | **Identical to `elec` on this firmware** [static]: both are written only in `FUN_00443380` (ZMQ topic `0x1e79`, message `0x3f1`) from the same byte (message offset `0x44`). Either can feed the app's `battery` |
| `mode` | string | CleanPack status (CPS), one of the 13 strings in §3.1 | |
| `subMode` | string | job type (§3.2) | |
| `isInForbidMode` | int | forbid mode on (`SetForbidMode` `FUN_004147e4` writes `+0x270`) [static]; its exact meaning, e.g. robot inside a no-go zone, is [inferred] | |
| `cleanArea` | int | area of the current or last job | **Robot-side unit unknown.** The app shows `clearArea` unconverted as m², but the cloud may have converted it |
| `allArea` | int | lifetime area | Same unit as `cleanArea` |
| `cleanTime` | int | duration of the current or last job | **Seconds** [inferred, medium–high confidence]. `task_manager` `FUN_0041c808` logs `"cleanTime:%us(<%us)"` with a 60 s minimum for the same quantity. The app displays seconds→minutes (§5.3) |
| `cleanMode` | int | index into `CleanModeStr` (§3.3) | The default index is unknown. `"…set to default cleanmop"` is only a fallback log string, and `cleanmop` is not a `CleanModeStr` value |
| `allTime` | int | lifetime cleaning time | Seconds [inferred, as for `cleanTime`] |
| `workNoisy` | string | fan level name (§3.4) | |
| `errorState` | `[int]` | cloud error code(s), already mapped (§4.1) | `0` = none |
| `timeStamp` | uint | `time()` when the record was built (epoch seconds) | |
| `mop` | int | mop attached / mop state [inferred from the key name] | |
| `mute` | int | mute flag [inferred from the key name] | |
| `water` | int | water-pump level [inferred from the key name] | The robot-side scale is not confirmed. The app's interpretation is in §5.3 |
| `autoBoost` | bool | automatic suction boost (on carpet) [inferred, medium confidence; `FUNC_COMMANDS.md` §5]. It is the only carpet-related robot field. There is no carpet-colour field | |
| `cleanComponents` | bool | brushes/fan enabled [inferred, medium confidence; `FUNC_COMMANDS.md` §4.5] | Also emitted by `FUN_0041f010` and listed in `COMMANDS.md` |
| `dustCenterFreq` | int | auto-empty frequency (cleans between emptyings) [inferred from the name and the `task_manager` logic] | |
| `workstationType` | int | dock type [inferred from the key name] | |
| `ldAvoidColli` | int | LiDAR collision-avoidance setting [inferred; set by `setLidarCollision`, `FUNC_COMMANDS.md` §4.6] | |

`backWashArea` and `backupMapSwitch` appear only in the `/tmp/devattr` dump, not on the wire [static].

**Correction to `COMMANDS.md`:** its "Status frame (event_send.cpp)" list omits `pos`, `phi`,
`vol`, `led`, `elec`, `mode`, `mop` and `mute`. It also includes `backWashArea` and
`backupMapSwitch`, which the wire builder does not emit.

### 2.1 R: push path — `POST cleanPack/uploadEvents`, `infoType` 20001 [static]

`EventSendProcess` (`FUN_0044dbd8`) wakes every **1 s**, or every 2 s while disabled
(timespecs at `0x487820` and `0x48e200`). It sends when two flags are both set: the dirty
flag `this+0x30` and the app-online flag `this+0xa9`.

```
POST <base>cleanPack/uploadEvents     (URL = string member +0x28 of the cloud singleton, FUN_00461538)
Cookie: cookies=<sid>
body: sn=<sn>&ts=<GetCurrentTickMs()>&data={"data":{<record §2>},"infoType":20001}
```

* The body is a raw `sprintf("sn=%s&ts=%llu&data=%s")`. `data` is **not** URL-encoded. A server
  must parse it leniently: split on the first `&data=` and take the rest as JSON.
* `ts` is **monotonic milliseconds since boot, not epoch time** [static]. `libcpc.so`
  `GetCurrentTickMs` (`0x2d8b8`) calls `std::chrono::steady_clock::now()` and divides by 10⁶.
  Use `data.timeStamp` (epoch seconds) for wall-clock time.
  **Correction:** `schemas/channelA_rest.schema.json` describes `ts` as "epoch (ms or s…)", which
  is wrong for this template.
* The server must reply `{"code":0,…}`. Any other code sets the retry flag, and `code 102`
  forces a re-register (`FUN_0044f268`).

**Extension to `PROTOCOL.md` §A:** `uploadEvents` carries status as well as maps. Status uses
the generic `sn&ts&data` template; events (§4.2) use the `devType=3…event=` template.

**What sets the dirty flag:** each `DevAttr<T>` member has an on-change callback.

* **Static:** the pose producer `FUN_00443190` compares the new ints with
  `0x4c4600..08` and calls the change callback (`*DAT_004c4628`, reason 2) only when they differ.
  The battery producer `FUN_00443380` does the same with reasons 8 and 2. `FUN_00420960` does
  the same for the CleanInfo fields.
* **Inferred, medium–high confidence:** that the callback sets the event-send dirty flag. The
  callback→flag link was not traced.

Consequence: expect a push on every state change, rate-limited to **about 1 Hz**. That includes
continuous pushes **while the robot is moving**, since every pose change is a change. Pushes
only happen while the app is "online".

### 2.2 R: request path — Channel-B `infoType` 20001 [static]

Cloud→device Channel-B messages are dispatched by `protocol_ld.cpp` `FUN_00470b28` (log
`"Error : Unknown infoType %d"`). It reads the `std::map<int, std::function>` at `0x4c6180`,
which `FUN_00475a40` builds; `FUNC_COMMANDS.md` describes the same dispatcher. The keys are
20001, 20002, 21001–21006, 21010–21012, 21014–21031 and 30000.

**Entry types in the live map:**

* Most entries are thunks that call a slot of the vtable at `0x494fc8`.
* A few are devirtualising invokers (e.g. `FUN_00470538`, `FUN_00470768`, `FUN_00474440`). For
  instance, `FUN_00470538` calls slot `+0x68` unless that slot is `FUN_004701f0`, which it then
  calls directly.

**Second map (dead).** `FUN_0040e430` builds a second map at `0x4c41e8`. Only its own init and
destructor reference it (`0x40e468`, `0x40e4b0`), so it is dead. Its entries are different
invoker thunks (`FUN_0040cdb0`, `FUN_0040d958`, …), each of which calls a vtable slot.
`tbl_out.txt` lists key→thunk pairs only.

**How handlers were identified.** The handler addresses in this document (e.g. 20001 →
`FUN_0046ebc8`, 21019 → `FUN_00472758`) come from resolving each thunk's slot offset through
the vtable at `0x494fc8`.

**Not every handler replies `message:"ok"|"fail"`.** See the per-handler reply shapes below and
in §6, and `FUNC_COMMANDS.md` §0.

Key 20001 maps to `FUN_0046ebc8`. That handler builds `{"data":{<record>},"infoType":20001}`,
with **no** `"message"` key. 21026 also has none; 21010, 21015, 21016 and 21019 do. It then
calls the generic reply callback at `this+0x20`.

* **Null status provider** (`this+0x40`): `data` starts as `{}` (ValueType 7), but the handler
  still writes `timeStamp` (`0x46ec50`) and `vol = vol/10` (`0x46ec8c`/`0x46ec9c`). The reply
  is then `{"data":{"timeStamp":<t>,"vol":0},"infoType":20001}`.
* **Batch path:** when called inside a 30000 batch (caller array `param_4`), the handler
  appends to that array instead of calling the reply callback (`FUNC_COMMANDS.md` §1.4).

**Reply transport [static]:** HTTP **`POST cleanPack/response`**, the same path as every other
handler reply. The callback at `this+0x20` resolves to `0x459a50` → `FUN_00459100` →
`FUN_0045ed80` (`response_handle.cpp`, log `"Push Response Frame: %s"`), which POSTs to the
cloud singleton's URL member `+0x68` = `cleanPack/response` (`np_all.c` ~77451). The body
template is in `FUNC_MAP.md` §3 and `FUNC_COMMANDS.md` §0. Replies do **not** come back on the
TCP socket.

Request form: `{"encrypt":0,"data":{"infoType":20001,"data":{}}}#\t#` (enveloped; corrected
2026-10-07 — the robot dispatches the outer `data`'s contents, `CHANNEL_B_INBOUND.md`). The integer `encrypt` field is
mandatory (`PROTOCOL.md` §B).

### 2.3 R: app-online gate (extends `PROTOCOL.md` "Pong contract") [static]

The pong handler `FUN_00457b30` calls `SetIsAppOnline(isExistConnect)` (`FUN_0044e6c8`, log
`"SetIsAppOnline Status Change Old=%d,New=%d"`). That sets `this+0xa9`, the push gate in §2.1.

The pong must pass the inbound gate in `FUN_00457f08` (integer `encrypt`, `data` present),
**or the handler never runs**. The required pong is:

```
{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}#\t#
```

This is consistent with `FUNC_MAP.md` §2.2.

**Correction to existing documents (2026-10-07, second revision).** An earlier correction kept
the pong flat (`{"infoType":21006,"encrypt":0,"data":{…}}`) — that is still wrong: the robot
dispatches the contents of the outer `data`, so the pong must be enveloped as shown above.
`PROTOCOL.md`, `schemas/README.md` and `schemas/channelB_gateway.schema.json` were updated the
same day (`CHANNEL_B_INBOUND.md`).

A flat or unenveloped pong still keeps the link alive, because any complete frame refreshes
the online timer before the gate. But it is dropped at the dispatcher, so the pong handler
never runs and the robot **never sends status pushes**. Use the form above.

* `isExistConnect:false` turns the push off.
* Leaving the field out skips `SetIsAppOnline`, so the flag **keeps its previous value**.
* The flag's initial value at start-up was not traced. It is probably `false` (zero-initialised
  member) [inferred, low–medium confidence].
* Without pushes, poll with 20001 (§2.2).

---

## 3. R: state vocabularies (string tables in `libcpc.so`) [static]

These tables were dumped from `libcpc.so` data plus its `R_AARCH64_RELATIVE` relocations. They
are copy-relocated into `network_proxy`.

### 3.1 `mode` — `CleanPackStatusStr[13]`

| idx | string | meaning | EID |
|---|---|---|---|
| 0 | `idle` | standby | 1157 `EID_I_CPS_IDLE` |
| 1 | `dormant` | sleep | 1158 |
| 2 | `sweep` | cleaning | 1159 |
| 3 | `pause` | paused | 1160 |
| 4 | `backcharge` | returning to dock | 1161 |
| 5 | `charge` | charging | 1162 |
| 6 | `fault` | error | 1163 |
| 7 | `rfctrl` | remote control | 1164 |
| 8 | `fullcharge` | docked, full | 1165 |
| 9 | `shutdown` | powering off | 1166 |
| 10 | `findchargerpause` | return paused | 1167 |
| 11 | `FindChargerAndWash` | returning to wash the mop | 1168 |
| 12 | `DustCenterWorking` | dock auto-empty running | 1169 |

At start-up the robot asks for this state with `EID_I_GET_CLEANPACK_STATUS` (0x455). On failure
it logs "set to idle by default". Each change logs `"New CPS:%s"` (`FUN_00445628`). The docked,
charging, full and returning states all come directly from `mode`.

### 3.2 `subMode` — `CleanSubModeStr[9]`

`""`, `--total`, `--point`, `--area`, `--curpoint`, `--updating`, `--room`, `--smart`,
`--depthTotal`. The wire builder `FUN_0044d270` passes the provider's string straight to
`Json::Value(string)` with no stripping [static]. The code that sets the `subMode` string
(devattr `+0x1d0`) was **not traced**, so whether it stores the table entry with the `--` is
unknown. Strip a leading `--` defensively.

### 3.3 `cleanMode` — `CleanModeStr`

0 `sweepOnly`, 1 `mopOnly`, 2 `sweepMop`.

### 3.4 `workNoisy` — `WorkFanLevelStr`

`mop`, `quiet`, `auto`, `strong`, `max`.

Related tables (see `MAP.md` / `FUNC_MAP.md` §4): `ForbidTypeStr` `all|sweep|mop`;
`CoverModeStr` `normal|depth|forbid`.

---

## 4. R: errors and events

### 4.1 `errorState` codes (`ReportError`, `FUN_0044d160`) [static]

The robot's small error index (devattr `errorState[0]`) is mapped to a cloud code through a
39-entry `std::map<int,int>`. The map is built from rodata `0x48e2f0` in `_INIT_24`:

```
0→0  1→-2101  2→-2102  3→-2201  4→-2302  5→-2304  6→-2308  7→-2309  8→-2401  9→-2401
10→-2402 11→-2403 12→-2404 13→-2405 14→-2406 15→-2406 16→-2407 17→-2501 18→-2501
19→-2502 20→-2601 21→-2602 22→-2603 23→-2605 24→-2606 26→-2607 27→-2700 28→5058
29→-2306 30→-2305 31→-2604 33→-2701 34→-2702 35→-2703 36→-2704 37→-2705 38→-2706
39→-2707 45→6126
```

* Indices that are not in the map are sent as `0`.
* **Where the index comes from (resolved 2026-10-10):** network_proxy keeps its own
  **EID→index table** (file offset `0x89bf0`, stored unsorted — read each qword as an
  `(EID, index)` pair), then maps index→cloud code with the rodata tuples above
  (`0x8e2f0` / a variant at `0x91120`). The tempting "index = `EID_E_*` − 4000" alignment is a
  **coincidence** — an earlier revision of this section fell for it; the real table below
  disproves it (e.g. `EID_E_SKID` 4045 → 19, not 45; `EID_E_CLEAN_LOST_POSE_DONE` 4024 → 45).
* **Joined fault table** (event → index → cloud code; indices without a code report `0`):

  ```
  1128 EID_I_LOW_BATTERY_FIND_CHARGER →  1 → −2101   4020 EID_E_LIDAR_PROTECTIVE_COVER → 12 → −2404
  1086 EID_I_FIND_CHARGER_TASK_FAILED → 28 →  5058   4018 EID_E_LIDAR_SPEED_ERROR      → 29 → −2306
  1130 EID_I_LOW_BAT_NEED_POWEROFF    →  2 → −2102   4019 EID_E_LIDAR_POINT_ERR        → 30 → −2305
  4004 EID_E_WHEEL                    →  3 → −2201   4011 EID_E_FAN_SPEED              → 11 → −2403
  4014 EID_E_COLLISION                →  4 → −2302   4031 EID_E_GARBAGE_BOX_OUT        → 14 → −2406
  4005 EID_E_LEFT_WHEEL               → 40 →  –      4032 EID_E_GARBAGE_BOX_FULL       → 13 → −2405
  4006 EID_E_RIGHT_WHEEL              → 41 →  –      4033 EID_E_GARBAGE_BOX_FULL_OUT   → 16 → −2407
  4008 EID_E_SIDE_BRUSH               → 10 → −2402   4036 EID_E_WATER_BOX_EMPTY        → 22 → −2603
  4009 EID_E_MIDDLE_BRUSH             →  8 → −2401   4037 EID_E_PHYSICAL_TRAPPED       → 17 → −2501
  4015 EID_E_DROP                     → 26 → −2607   4038 EID_E_PLAN_TRAPPED           → 18 → −2501
  4016 EID_E_FRONT_WALL_DIRTY         →  6 → −2308   4040 EID_E_PICK_UP_DO_TASK        → 20 → −2601
  4017 EID_E_PSD_DIRTY                →  7 → −2309   4041 EID_E_TILE_DO_TASK           →  5 → −2304
  4024 EID_E_CLEAN_LOST_POSE_DONE     → 45 →  6126   4042 EID_E_NO_DUST_BOX_DO_TASK    → 15 → −2406
  4051 EID_E_CLEAN_CANNOT_ARRIVE      → 23 → −2605   4043 EID_E_NO_WATER_BOX_DO_TASK   → 21 → −2602
  4052 EID_E_START_FROM_FORBID_AREA   → 24 → −2606   4045 EID_E_SKID                   → 19 → −2502
  4048 EID_E_CANNOT_UPGRADE           → 31 → −2604   4049 EID_E_OPT_DURING_UPGRADE    → 32 →  –
  4050 EID_E_BATTERY_DISCONNECT       → 33 → −2701   4053 EID_E_KIT_FAN_ERR            → 37 → −2705
  4055 EID_E_KIT_MID_BRUSH_ERR        → 34 → −2702   4054 EID_E_KIT_SIDE_BRUSH_ERR     → 36 → −2704
  4056 EID_E_KIT_MOTOR_ERR            → 35 → −2703   4057 EID_E_KIT_WATER_PUMP_ERR     → 39 → −2707
  4058 EID_E_KIT_LIDAR_ERR            → 38 → −2706   4061 EID_E_START_FROM_MAGNETIC_WALL→ 25 →  –
  4065 EID_E_SEWAGE_TANK_FULL         → 44 →  –      4066 EID_E_SEWAGE_BUFFER_TANK_FULL→ 43 →  –
  4067 EID_E_DUST_COVER_OPEN          → 42 →  –      3001 EID_W_ULTRASONIC             →  6 → −2308
  1026 EID_I_CHARGE                   →  1 → −2101   1011 EID_I_PICK_UP_RESUME          → 20 → −2601
  1012 EID_I_GARBAGE_BOX_IN           → 13/14/16/15 → −2405/−2406/−2407/−2406  (four entries)
  ```
* **Not in the table = never reported**: `EID_E_CLEAN_LOST_POSE` (4023), `…_CANNOT`-families and the
  find-charger pose events (4025–4030) have **no index** — e.g. a clean_task "lost pose/map"
  abort does not surface as any `errorState` code. Only the events above can appear in the app.

### 4.2 Event notifications [static]

**`infoType` 20003 (`msg_report.cpp`, `FUN_0045b8b8`, "Event = %d")**

* The caller `FUN_0045c030` logs `"FUCK .... reason is not IER_ERROR_REPORT !!"` when the reason
  is not 7, but then calls `FUN_0045b8b8` **unconditionally** (the *call* is unconditional; the
  function itself still returns without posting when the index misses the 27-entry map below).
* The function looks up a 27-entry `short→int` map (rodata `0x491120`). It is **not**
  identical to the §4.1 status map (`0x48e2f0`):

  | index | event map (20003) | status map (`errorState`) |
  |---|---|---|
  | 12 | −2305 | −2404 |
  | 29 | — | −2306 |
  | 30 | −2306 | −2305 |
  | 0, 19, 31, 33–39, 45 | — (absent, so no 20003 is sent for them) | present (see §4.1; 0→0) |

  The event map's 27 keys are 1–18, 20–24, 26–28 and 30. Apart from the rows above, the indices
  the two maps share map to the same code. The same fault can therefore carry different codes in
  the status record and in the event. It then POSTs to `uploadEvents`:

```
devType=3&sn=<sn>&taskid=<GetCurrentTickMs>&event=4&createtime=<unix>&data=
{"data":{"level":1,"code":<code>,"title":"unkonw",
         "msg":{"code":<code>,"message":"unkonw"}},
 "infoType":20003,"connectionType":1}
```

Evidence for the nesting [static], all in `FUN_0045b8b8`:

* Object A (`x29+0x70`) receives `level`, `code` and `title`.
* Object B (`x29+0x98`) receives `code` and `message`.
* `0x45ba80–90`: `A["msg"] = B`.
* `0x45ba94–a8`: `root["data"] = A`.
* Then `root.infoType = 20003` and `root.connectionType = 1`.

Key literals: `level` `0x4912a0`, `code` `0x48d2d8`, `title` `0x4912a8`, `"unkonw"` `0x4912b0`,
`message` `0x48d3d8`, `msg` `0x4912b8`, `data` `0x488658`, `connectionType` `0x4912c0`.

`"unkonw"` (sic) means the robot never sends error text. Text is supplied by the cloud (§5.6).

**LD events (`msg_report_ld.cpp`, `FUN_0045c120`, "ReportEvent %d" / "LD ReportEvent %d")**

* An 18-entry map (`0x4914d0`) translates the index to LD codes 6012–6136:
  1→6105, 2→6052, 3→6012, 4→6034, 5→6097, 6→6096, 7→6023, 8→6126, 9→6127, 10→6128,
  11→6129, 13→6130, 18→6131, 19→6132, 20→6133, 21→6134, 22→6135, 27→6136.
* The code is stored with a pending flag for `ReportPush`. That record is **not decoded**.

`uploadEvents` therefore carries at least 20001 (status), 20003 (events) and 20004 (clean
records, `task_manager`; see `FUNC_MAP.md` §8.4).

---

## 5. A: how the app receives and decodes status

### 5.1 Push vs poll [static; screen start-up timing inferred]

| Mechanism | Class / method | Trigger | Reads |
|---|---|---|---|
| **Unsolicited push**: any imsocket frame with cmd > 0 whose seq is not pending. `WIRE_PROTOCOL.md` names the push command PUSHMSG = **251** — use it; cmd **4135** is KICK and logs the user out (`BaoLeApplication` ~476ff), and cmd **-2** is special-cased too | `NetOKioWorker.pushMessage` → `PushReceiveImp` → `IMService.onReceived` → LocalBroadcast `YRBoadcast` → `YRBroadcastReceiver` → `pushMethod` → every `NoticeListening` | server | `ImMessage<String>`. `value` is a JSON string parsed into `MapDetailInfo` / `IMValue` and dispatched on `noteCmd` (§5.2) |
| Global push handler | `BaoLeApplication.pushListener()` | every push | Rebroadcasts the Android intents `UPDATA_DEVICE_NAME`, `UPDATE_CLEARA_REA`, `UPDATA_MY_DEFAULT_ROBOT`, `UPDATA_APPOINTMENT_TIME`, `UPDATE_VERSION_PERCENT`, `UPDATE_VERSION_CODE` |
| Main-screen push handler | `MainFragment.onReceive` (`NoticeListening` registered in `onCreateView` ~l.251; ~l.940–1000) | every push while the main screen is alive | parses `value` (fastjson) into `IMValue`; **noteCmd 102 sets the row's `workState` unconditionally** (`robotInfoData.setWorkState(value.getWorkState())`, ~l.976); `setRobotWorkState` (~l.457–462) maps empty/null `workState` → `"0"` → the row shows **Offline** with the battery hidden. `battery` only when non-empty; noteCmd 103 handled too |
| Map-screen push handler | `MapLaserActivity.onReceive(ImMessage)` | push for this `targetId`. `MapLaserActivity.onCreate` is native and `initView()` has no Java caller; `MapLaserActivity` is never registered as `NoticeListening` in Java (only `BaoLeApplication`:465 and `MainFragment`:251 call `addNoticeLing`), so **whether/when `onReceive` runs at all is [inferred]** | §5.2 |
| Map/track poll | `getIMMapDate` (LASTCLEAR 36) and `getIncrementMap` (TRANSIT 133, every 5 s / 2 s on LAN, only while `isStart`) | start **[inferred]**: scheduled from `MapLaserActivity.initView` (~l.430, ~l.447–448), which has no Java caller; parsing and intervals are [static] (`FUNC_MAP.md` §9.1) | Status fields in the reply: `clearArea`, `clearTime` (plus the map fields in `FUNC_MAP.md` §9.1) |
| Settings echo poll | `OptionsActivity.getIMRobotState` (ROBOTSTATE 34), `getRobotState` (TRANSIT "98", the status/settings query; see `FUNC_COMMANDS.md` §0), `getDisturbTime("211")` | settings screen opens [inferred: the methods have no Java callers; `onCreate` is native] | `workMode`, `waterTank`, `fan`, `carpetColor`, `voice`; DND `isOpen`/`sTime`/`eTime` |
| REST robot list | `robot/getMyRobotList.do` (`cuPage`, `pageSize`). Main screen: `MainFragment.initNetWork` → `ConnectApi.getMyRobotList` (Retrofit, `HttpResult`, pageSize "20"). Device list (`MyDeviceActivity`, ~l.284–292): `cuPage` = `this.page` (starts 1, increments as more pages load), pageSize 20 (field ~l.105). Only `CorrectTimeUtils.setCorrectTime()` sends cuPage "1", pageSize "50" (CorrectTimeUtils:25) | main screen load [static]; device list [inferred] | `List<RobotInfoData{authCode, battery, deviceId, deviceIp, deviceName, devicePort, isBind, robotId, robotImg, robotModel, robotName, suportBattery, workState, cameraUid, cameraPwd}>`. `MyDeviceAdapter` and `CorrectTimeUtils` call `getWorkState().equals(…)` with no null check, so `workState` is **required**. `MainFragment$14` → `CorrectTimeUtils.setCorrectTime(List)` → `BaoLeApplication.addRobotInfoList` clears and refills the cache used by the fault flow (§5.2), the OTA result dialogs (`getUpdateDeviceInfo`, `workState` 31/32) and the clock sync. `robotModel` and `authCode` must be non-null for the fault dialog (§5.2) |
| REST snapshot | `DeviceInforApi.getRobotInfo` → `robot/getRobotInfo.do` (`deviceId`) | tapping a robot on the main screen (`MainFragment.getRobotInfo` ~l.315, ~406ff) [static]; screen opens [inferred for native-`onCreate` activities]; notification taps and the fault-flow fallback [static] | `RobotInfo{workState, battery, firmwareVer, hadNoReadError, authCode, deviceIp, devicePort, carpetColor, update{…}, robot{robotModel, robotId, …, modules{radar, map, suportBattery, suportRecharge, suportVoice, suportErrorRecord, clearModel…}}}` |

**Push de-duplication:** `IMService.onReceived` drops a push whose seq is below 10001 if the
same seq arrived less than 2000 ms ago (`TIMEOUT_SEQ` = `bigkoo…c.b` = 2000). A server must
vary `seq32`.

**Heartbeat ack:** cmd **273 (0x111)** (`NetOKioWorker.pushMessage`). This extends
`WIRE_PROTOCOL.md` §4.

### 5.2 Push payload dispatch on `value.noteCmd` [static]

The push value parses into `MapDetailInfo`. Every field is a string:

`battery, brush, carpetColor, chargerPos, cleanArea, clearArea, clearId, clearModule,
clearModuleName, clearSTime, clearTime, deviceIp, devicePort, direction, errRecordId, error,
errorCode, errorDesc, extParam, fan, firmwareVer, forbiddenArea, map, noteCmd, noticeInApp,
pushMsg, pushType, result, track, updateMode, updateModule, updatePro, userId, version, voice,
waterTank, workMode, workState`

| `noteCmd` | Meaning | `BaoLeApplication.pushListener` (global) | `MapLaserActivity.onReceive` (map screen) |
|---|---|---|---|
| **102** | status changed (main telemetry) | Reads `workState`, `battery`, `workMode`, `waterTank`, `fan`, `voice`, `carpetColor`, `deviceIp`/`devicePort` (stored for LAN), `firmwareVer`, and OTA `updateMode`/`updateModule`/`updatePro` while `workState` is 30. `result:"1004"` opens dialog type 6; `result:"1005"` shows the toast "Robot navigate map failed…". `workState=="100"` triggers a time sync | Reads `battery`. If `battery` is present and `result=="1005"`, it **resets the map only** (`setMapEmpty`). Also reads `workState`, **`clearArea` and `clearTime`** (`MapLaserActivity.java` ~1519–1545) |
| **100** | fault | Always rebroadcasts `UPDATA_DEVICE_NAME` and `UPDATE_CLEARA_REA` (type 2). The fault dialog flow is described below this table | Forces `workState="7"` |
| **101** | clean finished / area update | `clearArea`, `clearTime` → `UPDATE_CLEARA_REA` | — |
| **103** | reset / unbound | broadcast | Returns to the main screen |
| **109** | server notification | Acts only if `pushMsg` is non-empty, `userId` is absent or equals the logged-in user, and the app is not in "egg" mode. Uses `pushMsg`, `noticeInApp` and `userId`; `pushType` selects the action. `pushType 100` (started) and `101` (finished, needs `clearId`) raise a system notification when backgrounded and the `clean` pref is "1". `noticeInApp=="1"` with `pushType 101` shows an in-app dialog instead. `pushType 102` (fault) calls the same `getDeviceInfo(…)` with type "1" (a system notification), only in the background and only if both the `errorMsg` and `clean` prefs are "1". `noticeInApp=="1"` **requires a non-null `pushType`** — `BaoLeApplication` ~503 calls `getPushType().equals("101")` and NPEs otherwise | — |
| 200 | user agreement changed | dialog | — |

**Main screen consumer** (`MainFragment.onReceive`, registered statically in `onCreateView`
~l.251; body ~l.940–1000): parses `value` into `IMValue`. A **102** push updates the robot's
row — `battery` only when non-empty, `workState` **unconditionally** (~l.976). `setRobotWorkState`
(~l.457–462) maps an empty/null `workState` to `"0"`, which the row renders as **Offline** with
the battery hidden. Any 102 push that omits `workState` (e.g. a battery-only update) therefore
shows the robot offline on the main screen. `noteCmd` 103 is handled here too.

**Fault flow** (`noteCmd 100` → `BaoLeApplication.getDeviceInfo(targetId, errorCode, errRecordId, "2", …)`).
Source: `tmp/review_r4/BLA.java` ~483ff (recovered with jadx `-m` fallback) and
`DialogActivity.onClick` ~289–297 [static].

1. **Gates:** runs only in the foreground, not in "egg" (mini-game) mode, and with the user pref
   `getUserInfo().getErrorMsg()=="1"`.
2. **Robot lookup:** finds the robot in the cached `robotInfoList`. That list is filled from
   `getMyRobotList.do` (§5.1). If the robot is not cached, the app calls `getRobotInfo.do` and
   continues in `BaoLeApplication$6`.
3. **Device address:** for a cache-found robot, nothing happens if `deviceIp` **or**
   `devicePort` is null. The `$6` fallback substitutes `""`. Send **`""`, not dummy values**.
   `""` passes the null check, and `TextUtils.isEmpty` then skips the LAN path. A non-empty
   value has side effects: noteCmd 102 stores it (`storeDeviceInfo`), `IMSocket.addSendQueue`
   copies it into every command, and `RobotSocketManager.sendRobotData` then opens a LAN
   `RobotSocket` to it.
4. **Both IDs present:** if `errorCode` **and** `errRecordId` are non-empty, `DialogActivity`
   type "1" (fault) opens via the 10-argument `launch`. Its text is "XXXX robotic vacuum cleaner
   malfunction…" with `XXXX` replaced by the `content` extra (`initMyView` ~231). That extra is
   **`robotModel`**: `RobotInfoData.getRobotModel()` on the cached path, `robot.robotModel` on the
   `$6` path. Neither is null-checked, so a **null `robotModel` crashes the app** (NPE in
   `replace`) — assuming the native `onCreate` calls `initMyView`, which is not visible in the
   dex [inferred]. On OK with a non-empty `authCode`, it opens `GetRobotErrorInfoActivity`.
5. **Either ID missing:** `DialogActivity` type "2" opens via the 6-argument `launch`
   (`DialogActivity.java` ~169–177). That overload sets no `authCode` extra, and `initMyView`
   reads `authCode` only for types 3 and 4 (the field defaults to `""`). So OK **always shows
   the misleading toast "Connection timed out, please check the network"** (string 2131296557)
   and never opens `GetRobotErrorsListActivity`, unless the native `onCreate` does something
   unseen. **A bridge must always send both `errorCode` and `errRecordId`.**

`errorCode` is therefore a required gate, even though no REST call uses it:
`LoginApi.getRobotErrorInfo(errorCode)` has no caller (reviewer-confirmed with an androguard
xref). `GetRobotErrorInfoActivity` fetches text only via
`getRobotErrorInfoById.do?errorRecodeId=<errRecordId>`, so `errRecordId` must match a record
from `getRobotErrorsList.do`. Use the §4.1 cloud code as `errorCode` [inferred].

`IMValue` and `IMResult.ValueEntity` (`WIRE_PROTOCOL.md` §6) are subsets used by individual screens.

### 5.3 `workState` codes and settings echoes [static]

Strings are resolved with `res.py`. Sources: `MapLaserActivity.getWorkState` and `MyDeviceAdapter`.

| `workState` | EN / ZH | Behaviour |
|---|---|---|
| `0` or empty | Offline / 离线 | Battery hidden; control blocked ("Robot currently is offline…") |
| `1` | Working / 工作中 | `isStart=true`, so map polling runs |
| `2` | Standby / 待机 | |
| `3` | Complete cleaning / 完成工作 | |
| `4` | Recharging (returning) / 回充中 | `isStart=true` |
| `5` | Charging / 充电中 | Toast "Please start the Robot", and the action is refused for **transitCmd** (not `workState`) values other than `"100"` (start), i.e. 102/104 (`setWorkState(String)`), and for transitCmd 108 area actions (`setWorkState(String,String,String)`). **Not uniform:** the steering overload `setWorkState(String,String)` (`MapLaserActivity.java` ~1371) shows the toast but does not return, so transitCmd 108 + direction is **still sent** while charging. OTA is allowed (`isCharge`) |
| `6` | Charge completed / 充电完成 | Same as 5 |
| `7` | Error / 故障 | Settings blocked |
| `8` | Low battery / 电量不足 | Does **not** change `isStart`; it keeps its previous value. So map polling continues if the robot was working |
| `30` | Updating / 更新中 | OTA in progress; all control blocked |
| `31` | Updating (OTA success) | Info refresh / success dialog |
| `32` | Updating (OTA failure) | Failure dialog |
| `100` | Online / 在线 | Time-sync trigger |

**Battery** (`getElectricity`): an integer string 0..100 shown as `N%`. Icon thresholds are
≤20, ≤40, ≤60, ≤80 and above. It is hidden when `workState` is 0 or `modules.suportBattery=="0"`.

**Settings echoes** (`OptionsActivity`):

* **Water level.** Two decoders exist:
  * `setWaterModeInfo` (text label): `"20"`=High, `"40"`=Middle, `"60"`=Low, anything else = blank.
  * `waterChange()` (dialog tick mark, ~line 1249) uses ranges: 1–39 = High, 40 = Middle,
    41–199 = Low.
  * `255` leaves the previous tick unchanged. 0 and ≥200 (other than 255) clear all ticks.
  * So the app accepts a 0..255 scale. Lower = more water. 255 is a "no change / not
    applicable" sentinel, perhaps "pump off / no tank" [inferred, low–medium confidence].
  * This answers the "0..255, 255=off" note in `COMMANDS.md` from the app side. The robot's own
    `water` scale is still unverified.
* **`fan`:** `"0"`/`"2"` = Standard, `"1"` = Close (off), `"3"` = Strong. `"0"` appears only in the
  echo decoder; the setter sends 1/2/3 (`FUNC_COMMANDS.md` §4.2).
* **`carpetColor`:** `"0"` = Deep, `"1"` = Mild. Setter: `FUNC_COMMANDS.md` §5. The robot has no
  carpet-*colour* field (`autoBoost` is the only carpet-related robot field, §2), so this echo
  can only come from the cloud.
* **`voice`:** a decimal string from 1.0 to 2.0. The UI volume is `voice*100-100`, and 0
  selects the mute icon (`OptionsActivity.setVolume`). Setters: volume (123) and mute (125) in
  `FUNC_COMMANDS.md` §6.1–6.2. The robot reports `vol` 0..10 and `mute` separately (§2).
* **`workMode`:** matched against server-supplied `modules.clearModel` (`clearModel_<n>`).
  Mode names come from the server.

### 5.4 Robot `mode` → app `workState` (the cloud bridge) [inferred, medium confidence]

| robot `mode` (§3.1) | app `workState` |
|---|---|
| `sweep`, `rfctrl` | 1 |
| `idle`, `pause`, `dormant`, `findchargerpause` | 2 |
| `backcharge`, `FindChargerAndWash` | 4 |
| `charge`, `DustCenterWorking` | 5 |
| `fullcharge` | 6 |
| `fault`, or `errorState[0]`≠0 | 7, plus a `noteCmd 100` push |
| `shutdown` | 0 (the robot is about to go offline) [inferred, low confidence] |
| low battery | 8. **No robot trigger exists**: no low-battery `mode`, and no threshold in the status. A bridge must choose one, e.g. `elec` < 15 while not charging, mirroring the OTA guard. [inferred, low confidence] |
| no Channel-B link | 0 |
| Channel-B link (re)established (`10001` handshake) | push `100` (Online) once, then the mapped state [inferred, medium confidence]. `workState "100"` is one of several app clock-sync triggers; `FUNC_COMMANDS.md` §9 owns the list |

Use 3 and `noteCmd 101` (`clearArea`, `clearTime`) when a 20004 record or a clean-done event
arrives. The app shows `clearTime` through `DateUtils.toMinute` (seconds → minutes, rounded up)
and `clearArea` unconverted.

### 5.5 Pose in the app [static]

The app has no pose message. The robot is drawn at the **last point of the `track`** returned
by TRANSIT 133 / LASTCLEAR (`MapLaserActivity.analysis_bytes` → `lastX`/`lastY`). The track
format and the coordinate space (0..1000 map units) are in `FUNC_MAP.md` §9.2–9.3. There is no
heading.

The robot-side sources are `pos`/`phi` (§2) and the 21011 path (in mm; a cloud request answered
via `cleanPack/response`, `FUNC_MAP.md` §3). How the cloud converted mm to the app's map units
is **unknown**.

### 5.6 Error text comes from the server [static]

The app has no fault-text table. Of the 546 `<string>` entries in `values/strings.xml`, none are
fault descriptions. The fault UI uses REST through `YouRenSdkUtil.getUrl` (`<bl-app host>/robot/<name>.do`):

* `getRobotErrorsList.do` (`cuPage`, `pageSize`, `deviceId`) → `List<ErrorCode{errorCode, errorId, errorDesc, errorRecodeId, errorTime, isRead}>`.
  * `errorTime` = **epoch seconds** as a string; the app multiplies by 1000 in `DateUtils`.
  * `isRead` "1" = read (rendering colour).
* `getRobotErrorInfoById.do` (`errorRecodeId`) → `ErrorInfo{errorCode, errorId, errorTitle, errorDesc, solution, customerPhone, errorRecodeId}`.
  `GetRobotErrorInfoActivity` ~154 checks `getErrorDesc()!=null` and then calls
  `getErrorTitle().isEmpty()`, so `errorTitle` is **required** whenever `errorDesc` is present.
  The `getRobotErrorInfo.do` (`errorCode`) variant has no caller.
* `delRobotErrorById.do` (`errorRecodeId`) and `delAllRobotError.do` (`deviceId`)

`RobotInfo.hadNoReadError=="1"` shows a badge. A replacement backend must supply its own text
for the §4.1 codes.

The "search" button on the error page (`GetRobotErrorInfoActivity`) sends **transitCmd 143 =
locate robot**, the same command as `OptionsActivity` `tv_search_device`. Robot side: rc
3010/3011, see `FUNC_COMMANDS.md` §6.3.

### 5.7 Cleaning history [static]

* `robot/getRobotClearList.do` (`deviceId`, `cuPage`, `pageSize`) → `List<CleanRecord{clearId, clearArea, clearTime, clearKeepTime, clearModule, clearSTime, createTime}>` (`CleanRecordActivity`).
  * `clearTime` must be an integer string of seconds (`Long.parseLong`, `CleanRecordAdapter:51`).
  * `createTime` = epoch seconds as a string.
* `getRobotClearRecordInfo.do` (`clearId`) → `MapDetailInfo` (record map: `FUNC_MAP.md` §9.5)
* `delRobotClearRecord.do` (`clearId`; `DeviceInforApi`:80) and `delRobotAllClearRecord.do`
  (`deviceId`; `DeviceInforApi`:111). Request envelope and signing: `FUNC_MAP.md` §9.5

The list is hidden when `modules.map=="0"`.

### 5.8 Firmware version, online state, network [static]

* `robot/getRobotOnLineState.do`:
  * Request: `deviceId`, `authCode`, `deviceType "1"`, `appKey`, `robotLastCode` (`ConnectApi` ~134–140). Both overloads put the app's `appKey` value into **both** `appKey` and `robotLastCode` (~l.139–140, ~164–165); the 4th (error-code) parameter is unused. Only `data[0]` of the reply list is read (~l.144, 154). Callers: `ConnectingActivity` ~419, `NormalSoftConnectClient` ~200.
  * Reply: `List<RobotState{deviceId, state, workState, battery, firmwareVer, voice}>` (`OnLineStateDate` has the same fields in `DeviceInforApi`).
  * `state=="1"` = online.
  * The add-device flow retries it up to **30×** until the robot is online
    (`RetryWithDelay(30, …)`, `ConnectApi` ~143–150; `NormalSoftConnectClient` ~200).
* `robot/getRobotVersionInfoList.do` (`deviceId`, `robotId`) → `List<VersionData{deviceVer,
  isForce, isHideUpdate, softId, softName, softVer, updateDesc, updateUrls[], url}>`.
  `isHideUpdate` is a Java `boolean` (Gson parses `"1"` as `false` — send a JSON boolean);
  `updateUrls` is `List<UpdateUrl>` (`UpdateUrl{url}`). The semantics of each field beyond
  the names are not analysed here.
* `OptionsActivity` shows `deviceIp` and `firmwareVer`.

The app has **no** Wi-Fi signal field.

---

## 6. R: other status queries over Channel B [static]

The handlers come from the dispatch map in §2.2. Replies go through the same generic callback,
i.e. HTTP `cleanPack/response` (§2.2).

| `infoType` | Handler | Reply |
|---|---|---|
| 20001 | `FUN_0046ebc8` | `{"data":{record §2},"infoType":20001}` (no `message` key) |
| **21010** | `FUN_0046e8f8` | **Stub.** `conmunicationState`, `chargeState`, `motorState`, `sensorState` and `cleanModuleState` are all created as empty `Json::arrayValue` (`mov w1,#0x6` before each `Value(ValueType)`, `0x46e954–0x46ea28`). The reply is `{"data":{…:[] ×5},"message":"ok","infoType":21010}`. Nothing to decode |
| **21015** | `FUN_0046f128` → `GetGoodsStatistics` | consumables (§7), `"message":"ok"` |
| 21016 | `FUN_0046f2a8` → `SetGoodsStatistics` | `{"data":{…},"message":"ok","infoType":21016}` on every path (`0x46f318`/`0x46f32c`, `0x46f498`/`0x46f4ac`). No `"fail"` path was found, so "ok" does **not** prove the reset took effect. Re-read with 21015 to confirm (§7.2) |
| **21019** | `FUN_00472758` | `"message":"ok"`. `data` holds `apId`, `apIp`, `staId`, `staIp`, **`staSignal`**, `staMac`, `compileVer`, `mcuVer`. `staSignal` is a **string**, unit unstated (probably RSSI in dBm [inferred, low confidence]). `compileVer` = `GetGitCnt()`, an integer (1241 for this build). `mcuVer` is the **MCU firmware string**. At `0x472830` the handler calls `GetVersion` (`FUN_00415240`, `cp_function.cpp`), which builds `{version:"0.7.1", fullversion:1241, hasUpdateFile:0, mcu:FUN_00415118()}`. `FUN_00415118` queries the MCU and formats it as `"h%dv%d_%d"` (log `"MCU version:%s"`; `"Request mcu version failed"` on error). `0x472838–0x472b4c` copies `["mcu"].asString()` into `mcuVer` when it is a string. `compileVer` therefore equals `fullversion` |
| **21026** | `FUN_0046f6d0` | `data` = `{ip:<str>, port:<int>, protocol:"udp"}`, plus a top-level `code` (= the `GetNetInfo` return value) and `infoType:21026`. This is the local UDP control endpoint (`PROTOCOL.md` §C) |
| 21011 | `FUN_0046ee48` | path. Cloud request answered via `cleanPack/response` (`FUNC_MAP.md` §3, which corrects `MAP.md`) |
| 21021 | `FUN_00472038` | OTA (`version`, `downUrl`, `process`): see `COMMANDS.md` (OTA); `FUNC_COMMANDS.md` §0 only indexes it |

The firmware version is also reported in `cleanPack/sync` (`PROTOCOL.md` §A).

---

## 7. R: consumables and lifetime statistics (`/tmp/Run/LastRecord/CleanInfo.json`)

### 7.1 Accounting (`task_manager` `clean_recorder_utils.cpp`, `FUN_0041c8b0`) [static]

After each valid clean (≥4 path points and `cleanTime` ≥ 60 s, `FUN_0041c808`),
`FUN_0041c8b0(path, record, 1)` updates both `CleanInfo.json` and `CleanInfoBak.json`
(`task_manager` call sites around `0x40e7b4`/`0x40e7dc`). When `CleanInfo.json` is missing it is
created with `param_3=0` and a zeroed record, with no increments (two call sites). With T = the
job's time and A = its area:

```json
{"SingleClean":{"CleanArea":A,"CleanTime":T},
 "AllClean":{"SweepCounts","MopCounts","CleanCounts","SweepArea","MopArea","AllTime","AllArea"},
 "Goods":{"motor","sideBrush","mainBrush","filter","sensors","battery","mop","dustBox"}}
```

* `CleanCounts` += 1. `SweepCounts` or `MopCounts` += 1, by job type (record field +0xc: 0/1).
* `SweepArea` or `MopArea` += A. `AllTime` += T. `AllArea` += A.
* **Goods are cumulative usage in the units of T** (seconds [inferred], §2):
  * `motor`, `sideBrush`, `mainBrush`, `filter`, `sensors`, `battery` and `dustBox` each += T.
  * `mop` += T × mopArea / A (mopArea = record field +0x34).
* There are no limits, percentages or "replace" thresholds in the firmware. The app or server
  must define each part's rated life and compute `remaining% = max(0, 1 − used/limit)`.
  The limits are unknown.

### 7.2 Query and reset (`network_proxy` `cp_function.cpp`) [static]

* **21015 `GetGoodsStatistics`** (`FUN_004115e0`) returns `data` = `{battery, filter,
  mainBrush, sideBrush, sensors}` (uint; 0 if the file is missing). It does **not** return
  `motor`, `mop` or `dustBox`.
* **21016 `SetGoodsStatistics`** (`FUN_00411a70`) takes any of `battery`, `filter`,
  `mainBrush`, `sideBrush`, `sensors` and `mop` given as a **uint**. It writes that absolute
  value (log `"set mainBrush, %u"`) and saves the file.
  * **Reset = 0**, e.g. `{"encrypt":0,"data":{"infoType":21016,"data":{"sideBrush":0}}}`.
  * The reply is always `"message":"ok"` (§6). Verify with 21015.
  * `motor` and `dustBox` cannot be reset.
  * Command catalog entry: `FUNC_COMMANDS.md` §8. That section defers to this one for the details.

### 7.3 A: the app

The analysed dex contains **no consumables screen, bean or string**. Every `com.baole.blap`
class and the resource strings were searched for brush, filter, consumable and 耗材; the only
hits are the `brush` echo field, which appears in `IMValue`, `MapDetailInfo` and
`IMResult.ValueEntity` — and no `getBrush()` caller exists outside those beans.

`APP_COVERAGE.md` shows the dex dump is complete at class level (9072 classes) and the app has no
dynamic-code mechanism. The resource-string evidence (no consumables text in any language) is
unaffected by the native `onCreate` caveat (header). The code evidence is weaker, because 75
activity `onCreate` bodies are native. Overall confidence that 1.5.11 had **no consumables UI**:
high, based mainly on the strings.

---

## 8. R: what the robot needs from a server

1. Reply `{"code":0}` to every `uploadEvents` POST. Parse both body templates leniently (§2.1, §4.2).
2. Pong with `{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}#\t#` to enable status
   pushes (§2.3). Otherwise poll 20001 (§2.2).
3. Implement `cleanPack/response` and reply **2xx with `{"code":0}`** — a non-2xx or empty body
   re-queues and resends the reply, and `code` **102** re-queues and forces a re-register
   (`FUN_0045ee18`, `np_all.c` ~77455–77520). All Channel-B request replies arrive there over
   HTTP, not on the socket (§2.2).
4. Optionally poll 21015 (consumables), 21019 (Wi-Fi and version) and 21026 (local endpoint).
   Skip 21010; it is a stub.

## 9. A: what the vendor app needs from a server (the bridge)

0. **REST envelope.** Every `bl-app` REST reply is parsed into `ResultCall<T>` / `HttpResult<T>`
   with fields `code`, `data`, `errorDetail`, `msg`, `page`, `result` and `version` (all strings
   except `data` and `page`).
   * The app treats a reply as success **only if `result == "0"`** (e.g. `DeviceInforApi.getRobotInfo`
     around line 183; `LoginApi.getRobotErrorsList` around line 985). Anything else goes to
     `onError` with `msg`.
   * The payload goes in `data`.
   * Paged lists also need `page{count, cuPage, pageSize}` (ints). **`count` means different
     things per screen:**
     * `getRobotErrorsList.do` (`GetRobotErrorsListActivity` ~180, ~230, ~264): `count` = **total
       items**; divides by the reply's `page.pageSize` (`pageCount = count/pageSize + 1`), so
       **that field must be non-zero**.
     * `getRobotClearList.do` (`CleanRecordActivity` ~220): same meaning, but divides by its own
       constant `pageSize=20` (~l.77), not the reply's.
     * `getMyRobotList.do` in `MyDeviceActivity` (~214, ~313): `count` = **number of pages**
       (loads more while `page < count`).
     * A non-empty reply **without `page`** NPEs on `getPage().getCount()` in all three screens
       (e.g. `MyDeviceActivity` ~313).
   * On a non-"0" `result`, `YouRenSdkUtil.getYRErrorCode` calls `Integer.parseInt(code)` (~487).
     `code` must therefore be **numeric** (or absent) on error replies.
   * Example: `{"result":"0","code":"0","msg":"","data":{…}}`.
   * Requests are form-encoded and signed (`nonce_str`, `rep_check`, `sign`); see `FUNC_MAP.md` §9.5.
1. `robot/getMyRobotList.do` must list the robot (§5.1) with:
   * non-null `workState` (it is dereferenced unconditionally);
   * non-null `robotModel` (the fault dialog crashes otherwise, §5.2);
   * non-empty `authCode` (needed to open the fault details).
   * `deviceIp` / `devicePort` should be `""`, not null and not dummy values (§5.2 step 3).
     `CorrectTimeUtils.setCorrectTime(List)` substitutes `""` for null, but a cached null makes
     the fault flow and the `workState "100"` `setCorrecTime` path do nothing.
1a. `robot/getRobotInfo.do` must return:
   * `workState`, `battery`, `firmwareVer` and `authCode`;
   * `robot.robotModel` non-null (used by the fault-flow fallback `$6`); `robot` and
     `robot.modules` are dereferenced without null checks;
   * `robot.modules.radar="1"`, which selects `MapLaserActivity`;
   * `modules.map` neither null nor `"0"` — null or `"0"` opens `EmptyMapRobotActivity`
     instead of the map screen;
   * a **non-null `update{…}`** and a **non-null `update.isNeedUpdate`** — `OptionsActivity.setData`
     (~l.573–576) dereferences it (`.equals` NPEs on null). Fields:
     `update{desc, descUrl, deviceType, isForce, isNeedUpdate, title, updateUrl, version}`.
     When `isNeedUpdate=="1"` and `isForce=="1"`, forced-update dialogs gate entry depending on
     `workState` (5/6 charging, 30–32 updating).
2. Imsocket pushes (cmd **251**, §5.1) with `value.noteCmd` 102, 100 and 101, a unique `seq32` and
   `control.targetId` set to the robot's deviceId. **Every 102 push must carry `workState`** —
   the main screen copies it unconditionally and renders the robot as offline (battery hidden)
   when it is absent or empty (§5.1).
3. Replies to ROBOTSTATE 34, LASTCLEAR 36 and TRANSIT 133/98/211 with `result:"0"`.
4. REST endpoints for error records and text, and for clean records (§5.6, §5.7), each in the item 0
   envelope with `result:"0"`.

---

## Open questions

1. **Unit of `cleanArea`/`allArea`**, and how the cloud converted `pos` (mm) and `phi` (mrad)
   into the app's 0..1000 map units. The pose encoding itself is resolved (§2).
2. *(resolved: `elec` and `elecReal` are identical on this firmware, §2.)*
3. **Meaning of the 0..45 error index** — *resolved 2026-10-10: the joined EID→index→code
   table in §4.1 (e.g. −2605 = `EID_E_CLEAN_CANNOT_ARRIVE`).* Remaining: the human-readable
   text of each −2xxx / 5058 / 6126 cloud code (lives server-side).
4. The **ReportPush** (LD 6xxx) record layout. *(The 20003 nesting is resolved, §4.2.)*
5. *(resolved: replies go out via HTTP `cleanPack/response`, §2.2.)*
6. **Initial value of the app-online flag**, and the untraced link from the change callback to the
   event-send dirty flag. Pose and battery changes do fire the callback (§2.1, static).
7. **Robot-side `water` scale** versus the app's 0..255 interpretation, and the **`staSignal` unit**.
8. Whether `subMode` keeps the `--` prefix on the wire (the setter of devattr `+0x1d0` was not traced).
9. **Consumable rated lifetimes**: not present in the firmware or the dex.
10. The shipping `6716` firmware (`FIELD_NOTES.md`) may differ. App coverage is settled by
    `APP_COVERAGE.md`: the dex dump is complete at class level, but 75 com.baole activities
    (77 total) have native (Jiagu-protected) `onCreate` bodies (header), so screen start-up
    behaviour is inferred. The only missing library that matters is `libtoBitmap.so` (map rendering). The other, `jgbEC`, is the packer's and is dead for interop.
11. *(resolved: `ts` is monotonic ms from `steady_clock`, §2.1.)*
