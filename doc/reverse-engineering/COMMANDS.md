# Proscenic M7 Pro — command vocabulary

Traced from the robot firmware `network_proxy` (`protocol_ld.cpp` + per-group handlers)
and the decrypted app (`com.baole.blap.module.imsocket.*`, `module/laser/*`).

## Three namespaces — do not conflate

A control command passes through three representations:

1. **App → cloud (Channel A)** — the app sends `ImMessage<ImRequestValue>` in a
   `TRANSIT(250)` frame (`WIRE_PROTOCOL.md`). The command selector is the
   **numeric string `value.transitCmd`** (e.g. `"110"`, `"145"`); params are other
   `value.*` fields.
2. **Cloud → robot (Channel B)** — the robot receives
   `{"infoType":<N>,"encrypt":0|1,"data":{…}}` (`#\t#`-framed; `PROTOCOL.md` §B).
   **This is the only channel your rehome server speaks to the robot.**
3. **Firmware-internal handler names** — inside Channel-B `data`, the robot dispatches
   on a **command-name string** (`setVolume`, `cleanMode`, `reboot`, …) plus an int
   value (`protocol_ld.cpp` `FUN_00472f20`: reads a string field, `compare`s it to the
   names below, then reads an int).

The **app-numeric `transitCmd` ↔ Channel-B `infoType`/`data`** mapping is performed by
Proscenic's cloud and is **not fully recoverable from the client**. See "Driving a
rehomed robot" at the bottom.

## App-side command codes (`value.transitCmd`, Channel A) — verified literals

`setTransitCmd("N")` values found in the app, with the calling context:

| transitCmd | app site / apparent purpose | params seen (`value.*`) |
|-----------:|------------------------------|--------------------------|
| 106 | `setWorkMode(cmd, mode, "")` — **work mode** selector | `mode` = `"1".."10"` |
| 110 | `setWorkMode(cmd, "", fan)` — **fan level** only | `fan` = `"1"/"2"/"3"` |
| 145 | `setWaterMode` — mop water level | `waterTank` = `"20"/"40"/"60"` |
| 98  | Options/OptionOrdinary — a device setting | |
| 143 | Options / `GetRobotErrorInfo` — setting / error info | |
| 210 | Options/OptionOrdinary — a device setting | |
| 127 | Dialog/MainFragment/UpdateVersion/UpdateModelState — **firmware update** | |
| 139 | `CorrectTimeUtils` — time sync | |
| 131 / 133 | Map(Ordinary/Laser) — map ops | |
| 213 | `DisturbTimeActivity` — do-not-disturb window | |
| 113 / 115 / 117 / 149 / 200 / 202 / 204 | appointment/schedule activities | schedule fields |

(Labels for codes without a verified setter are inferred from the activity name;
treat those as approximate.)

### Verified value encodings
- **mode** (`value.mode`, transitCmd 106): `"1".."10"` (`setWorkMode` call sites in
  `OptionOrdinaryActivity`/`OptionsActivity`). Exact label-per-int (which int = which
  named mode) is not proven from the client; confirm against a status report.
- **fan** (`value.fan`, transitCmd 110): `"1"` / `"2"` / `"3"` (`fanModule`).
- **water** (`value.waterTank`, transitCmd 145): `"20"` / `"40"` / `"60"` (three levels).
  Status `waterTank` reports use a 0..255 scale (255 = off, `waterChange`).
- **volume** (`setVolume`, firmware side): int **0..10**, applied as value×10 →
  0..100 (`protocol_ld.cpp`: `if (value < 0xb) …*10`).

## Firmware-internal command names (Channel-B `data` dispatch)

These are the action names the robot recognizes (from `protocol_ld.cpp` and grouped
handlers). Inside a Channel-B command, the robot matches `data`'s command-name string
against these:

**Settings / actuators** (`FUN_00472f20`/`FUN_00474930`/`FUN_0043fd38`/`FUN_004408d0`):
`setVolume` (0..10), `setWaterPump`, `setLidarCollision`, `setledswitch`,
`startDustCenter`, `dustCenterFreq`, `reboot`, `cancleMap`, `delCurMap`.

**Clean / navigation** (`cp_function.cpp`, clean handlers) — firmware string literals:
`cleanMode`, `cleanArea`, `cleanId` (+ `extraAreas`, `segmentId`, `default`),
`updateRegionParam`. (Go-home is a firmware `ChargerControl`/`GetChargePos` symbol, not
a JSON literal; forbidden zones and schedules ride the app-side `value.forbiddenArea`
and `value.orders[]` fields — these are app/ImRequestValue concepts, not robot
command-name strings.)

**Map / files** (`FUN_0044ab18`): `backupMap`, `backupMapMd5`, `cleanFile`.

**OTA** (`FUN_00414af0`/`FUN_00414e38`): `updateMode` (+`version`,`downUrl`,`fileSize`,
`charge`,`fullcharge`).

**Local SoftAP / on-device JSON API** (`wifi_config.cpp`/`bind_user.cpp`,
`FUN_00469498`) — these are the `{"cmd":"<name>",…}` commands used during pairing and
by `rehome.py`, **not** transit: `setUrl`, `setSta`, `setAp`, `getWifi`, `resetWifi`,
`getCfg`/`applyCfg`, `getSn`/`getID`/`setID`, `checkPwd`, `getLog`/`rmLog`, `bindOk`.
(See `PROTOCOL.md` for `setUrl`'s two modes.)

## Robot → server reports (Channel B, device→cloud, always plaintext)

Your rehome server **will** receive these:

- **`infoType 21006`** — keepalive ping `{"data":{}}` (**must pong**).
- **`infoType 20002`** — occupancy-grid map upload (LZ4; see `MAP.md`).
- **`infoType 21011`** — clean-path stream: `{userId,pathID,startPos,totalPoints,posArray,pointCounts}`.
- **`infoType 21020`** — chunked pack transfer (`packId`; error `reason:"invalid json …"`).
- **`infoType 10001`** — connect handshake (`connectionType`, empty `token`, `sn`).
- **Status frame** (`event_send.cpp`): `reliable, elecReal(battery), subMode,
  isInForbidMode, cleanArea, allArea, cleanTime, allTime, workNoisy, errorState,
  timeStamp, water, autoBoost, cleanMode, backWashArea, backupMapSwitch,
  cleanComponents, dustCenterFreq, workstationType, ldAvoidColli`.
- **Device attrs** (`devattr_data.cpp`): `conmunicationState, chargeState, motorState,
  sensorState, cleanModuleState`.

App-side status bean the app parses (`IMValue`/`IMResult.ValueEntity`): `battery,
brush, clearArea, clearId, clearModule, clearTime, direction, error, errorCode, fan,
map, noteCmd, track, voice, waterTank, workMode, workState`.

## Driving a rehomed robot (important limitation)

Once the robot is pointed at your server (`ip_port.json`), the real cloud never sends
it commands again — so you **cannot capture** a cloud→device command from the live
device; you must **synthesize** it in Channel-B form. What is proven:

- Transport/format: `{"infoType":<N>,"encrypt":0,"data":{…}}#\t#` (`PROTOCOL.md` §B;
  `encrypt:0` skips AES).
- Inside `data`, the robot dispatches on a **command-name string** (list above) + an
  int value (e.g. `setVolume` value 0..10).

Not fully proven from the client (both cross Proscenic's cloud):
- the exact `data` key names carrying the command-name string vs. the int value, and
- the routing `infoType N` for each command group.

Recommended: start with the device→cloud direction (fully specced; your server logs
real pings/map/path/status), and validate one control command shape by trying the
proven candidates (`data.cmd`/`data.value`) against a single command and watching the
robot's `result`/`reason` reply, since the robot echoes `{"message":"ok"|"fail",
"reason":…}`. Treat the app-numeric `transitCmd` table as a cross-reference for which
actions exist, not as the on-wire Channel-B format.
