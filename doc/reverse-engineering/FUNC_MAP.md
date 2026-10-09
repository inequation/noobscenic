# Proscenic M7 Pro — Map functions (functional overview for a replacement app/server)

Scope: how the map gets from the robot to the cloud and back, how the app decodes, draws and
edits it, and how cleaning is aimed at zones, rooms or points. Battery, state and pose telemetry
decoding are covered in `FUNC_STATUS.md`. The general command catalog is in `FUNC_COMMANDS.md`.
This file only adds what `MAP.md`, `PROTOCOL.md`, `COMMANDS.md` and `WIRE_PROTOCOL.md` do not
already say, and it marks where those documents are wrong.

Tags: **[static]** read from decompiled code. **[dynamic]** observed at runtime (there are none in
this file). **[inferred]** reasoned from the evidence but not proven.

Evidence sources:
- Firmware: `tmp/proscenic/rootfs/bin/{network_proxy,libcpc.so,slam_pose_provider,task_manager}`.
  Full decompiles are in `tmp/proscenic/funcmap/{np_all.c,libcpc_area.c,libcpc_path.c,spp_all.c,tm_all.c}`.
  `FUN_xxxxxxxx` addresses refer to `network_proxy` unless stated otherwise.
- App: `tmp/proscenic/jadx_app/sources/com/baole/blap/...` (decompiled `app_main.dex`) and
  `tmp/proscenic/jadx_sec/sources/org/k/JNIUtils.java` (decompiled from `app_secondary.dex` for
  this task).
- App coverage: `APP_COVERAGE.md` (dex completeness, Jiagu method-level extraction, and the two
  native libraries referenced by the dex but absent from the APK).

---

## 0. Feature checklist

| Feature | Covered where | Status |
|---|---|---|
| Map upload (robot → cloud): format, transport, trigger, gating | MAP.md §20002; this file §2 (transport and gate corrected) | Fully known (static) |
| Map cell values (free, occupied, unknown, room labels) | this file §2.3 | Partially known (static only, not checked against a capture; label/free encodings can collide) |
| Map origin and resolution maths | MAP.md; this file §2.3–§2.4 | Fully known (static; row/y orientation [inferred]) |
| Cleaning path / trace (format, units, request and reply) | this file §3 (corrects MAP.md) | Partially known (format, units, tags and caps static; `mark`/`startPos` semantics not established) |
| Dock position and heading | this file §2.5 (corrects MAP.md) | Fully known (static) |
| Region / zone data model (no-go, no-mop, deep clean, rooms) | this file §4 (corrects MAP.md) | Fully known (static) |
| Set the zone list (no-go, no-mop, named rooms, per-region settings) | this file §5.1 (21003) | Fully known (static) |
| Read the zone list back | this file §5.2 (21004) | Fully known (static) |
| Virtual walls (line type) | §4 | Not supported as a line. Only as a ≥3-vertex polygon; <50 mm walls can collapse [static/inferred] |
| Zone clean / room clean / point clean (target selection) | this file §6 (21023) | Partially known (whether it auto-starts is unknown) |
| Spot clean at the robot's current position | this file §6.3 (21020 ctrlCode 3001) | Fully known (static; no reply on the cloud path) |
| Go to point without cleaning | — | Not found in firmware |
| Auto room segmentation, reset, merge, split | this file §7 (21030) | Fully known (static) |
| Room rename and room ordering | §7.4 | Rename: via 21003 region `name` only. Ordering: unknown |
| Delete the current map | this file §8.1 (21024 `delCurMap`/`cancleMap`) | Fully known (static) |
| Backup map (save, upload, restore) | MAP.md persistence; this file §8.2 (21025, uploadSingle) | Partially known |
| Multi-map (several floors) | §8.3 | Not supported in this firmware beyond one backup map [static/inferred] |
| Clean-record (history) map | this file §8.4 (20004 file and app REST) | Partially known |
| App: map fetch and polling | this file §9.1 | Fully known (static) |
| App: track binary decode | this file §9.2 | Fully known (static) |
| App: map pixel decode | this file §9.2 | Unknown (decoder lib absent from the APK — APP_COVERAGE.md) |
| App: rendering and screen↔map transform | §9.3 | Fully known (static) for the app's own 1000-unit space only; how those units relate to robot mm is cloud-side and unknown |
| App: zone editing messages (164/166) | §9.4 | Fully known (static) |
| App ↔ firmware translation (cloud) | §1 | Not observable in either binary [inferred]; the firmware contract (§2–§8) is the primary target |

---

## 1. Read this first: one product, two vocabularies, one invisible translation

The Android app analysed here — "Proscenic Robotic" 1.5.11 (`com.baole.blap`, a BaoLe/YouRen
platform build) — **is the app the user used to control this M7 Pro** (first-hand). It still
speaks a different vocabulary from the robot firmware, so the vendor cloud between them must
translate [inferred — the only mechanism that fits]:

| | App 1.5.11 (`MapLaserActivity`) | M7 Pro firmware (`network_proxy`) |
|---|---|---|
| Cloud | `bl-im*.robotbona.com:20008` + `bl-app*.robotbona.com` (`BuildConfig`, `YouRenSdkUtil`) | `mobile.proscenic.cn` / `.com.de` (`cleanPack/*`) plus the TCP gateway |
| Map | base64 binary, 8-byte header, 1000×1000 canvas, native decoder (`libtoBitmap.so`) | LZ4 occupancy grid with metric origin and resolution (§2) |
| Coordinates | integers 0..1000 ("map units"), scaled by `mapDis` | millimetres, world frame |
| Zones | axis-aligned rectangles `"l,t;r,b"`, at most 5 per kind | polygons `vertexs:[[x,y],...]` |
| Commands | `transitCmd` 133/164/166/... (Channel A framing and command table: `WIRE_PROTOCOL.md`) | `infoType` 20002/21003/21011/21023/21030 (Channel B: `PROTOCOL.md` §B) |

The app has no reference to `proscenic.c*`, `cleanPack`, `infoType` or `2100x` (grep of
`jadx_app/sources/com/baole`), and the firmware has no reference to the app's `transitCmd`
vocabulary. **No translation code exists in either binary** — so the translation lives in the
vendor cloud and is not observable here.

**What to build against.** For a replacement **server/app for the M7 Pro**, the firmware
contract (§2–§8) is the primary interface: it is fully specified by the robot's own `infoType`
messages. The 1.5.11 pipeline (§9) is documented as information only — it describes what a
BaoLe/YouRen cloud client sends, in its own units, and no message of it is known to reach this
robot. An earlier revision of this file claimed 1.5.11 was "a different product's app"; that was
wrong (it is the user's app) and has been removed. The claim in `COMMANDS.md`/`WIRE_PROTOCOL.md`
that the cloud translates the app's `transitCmd` into Channel B is consistent with the user's
experience, but its bytes are unrecoverable from either binary — do not design around it.

**App coverage (from `APP_COVERAGE.md`).** The decrypted dex is complete at class level (9072
classes; no dynamic-dex/plugin loading anywhere), but this build adds Jiagu **method-level
extraction**: 77 app activities — including `MapLaserActivity`, `JMapDetailActivity` and
`OptionsActivity` — have their `onCreate` bodies removed and marked `native`. Their helper
methods are present and readable; only the per-screen start-up orchestration is missing. The
signed universal APK provably never contained `libtoBitmap.so` (the laser-map pixel decoder) or
`libjgbEC.so` (Jiagu's optional security SDK, dead for interop). `libtoBitmap.so` is on the M7
Pro's live-map path; in this build its absence would break only live-map rendering, not device
control. Recovering it (if the pixel format is ever needed) means pulling the phone's installed
build over adb or finding another build — not further unpacking (§9.2, open question 8).

---

## 2. Map upload (robot → cloud)

For the payload template, see MAP.md §"Map upload message — infoType 20002". This section adds
the transport, trigger and gating rules, and corrects MAP.md.

### 2.1 Transport — corrects MAP.md
- **[static]** `20002` is sent **only over HTTP**:
  `POST <base>/cleanPack/uploadEvents` with body `sn=%s&ts=%lu&data=<20002 JSON>`.
  - `FUN_0045ad80` "SendMap" picks the URL at struct offset `+0x28`. The URL-table initializer at
    `np_all.c:80140` maps `+0x28` to `cleanPack/uploadEvents`.
  - The body template differs from the `devType=3&…` events template.
- MAP.md says "emits 20002 on both transports". **That is wrong.** No Channel-B send of the map
  buffer exists. The MapSend thread `FUN_0045b1a0` builds the JSON (`FUN_0045a430`) and posts it
  (`FUN_0045ad80`), and nothing else.
- **[static] Inbound `20002` and `21014`** on Channel B are **"send the map now" requests**. The
  dispatch-table slot `0x20` runs callback `FUN_004570d8`, which sets the MapSend force flag
  (`+0x80`). The `data` of these requests is ignored.

### 2.2 When the robot uploads — new, and important for a server
`FUN_0045b1a0` wakes every 200 ms. The loop body runs only while both of these hold:
- pause flag `+9 == 0` — set to 1 when a new app/cloud binding starts (`FUN_00449158`,
  "StartBind"); cleared by the MapSend start/restart `FUN_0045a2f0`, which also zeroes the
  failure counter;
- the "app is online" flag `+0xbc != 0`, written only by the 21006 pong handler
  (`FUN_00457b2c`/`b30`) from `data.isExistConnect` (bool).

While running, it uploads when either of these holds:
- the map ID or path ID changed and at least 3 s have passed since the last upload, or
- the force flag `+0x80` is set.

The force flag is set by:
- an inbound 20002/21014 request (dispatch slot `0x20` → `FUN_004570d8`, §2.1);
- every 21030 operation (§7; `FUN_00473528` invokes the same callback);
- an event callback `FUN_004573d8`, when the event it receives has value 2 (which event that is
  is not established) [static/inferred].

**Failure handling [static]:** each failed POST increments a counter (`+0xb8`). After 3
consecutive failures the pending change is marked as sent and dropped (the sent bookkeeping at
`+0x68`/`+0x70`/`+0x78` is updated and the force flag is cleared). A server outage therefore
**loses the pending map** until the map changes again or a new 20002/21014 request arrives.

**Consequence [static]:** a flat or unenveloped pong keeps the link alive, **but the robot will
never upload maps**. To get maps, pong with the fully enveloped form (corrected 2026-10-07,
`CHANNEL_B_INBOUND.md`):
```
{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}#\t#
```
Both the outer `encrypt`/`data` envelope and the *inner* integer `infoType` are required — the
robot dispatches the contents of the outer `data`.

### 2.3 Raster content and coordinates (extends MAP.md)
- **Source [static]:** `MapDataReal` `FUN_00438fa0` reads the ShowMap shm, then calls
  `FUN_00438660`. That function quantizes each cell:
  - raw `<0x50` → **0x00**
  - raw `<200` → **0x7F (unknown)**
  - otherwise → **0xFF**

  Where segmentation exists **and the frame is wider than 89 cells** (`0x59 < width` gate in
  `FUN_00438660`; narrower maps never carry labels), **0xFF cells are overwritten with the
  non-zero room label byte**. Nothing in the code stops a label from equalling 0x7F or 0xFF; if
  that happened the cell would be indistinguishable from unknown/free (no guard found) [static;
  consequence inferred]. The grid is then cropped to the bounding box of known cells (+10-cell
  margin) and LZ4-compressed. `autoAreaId` is the segmentation ID (`local_4c & 0xffffff`).
- **[inferred]** The room labels are written onto 0xFF cells, so **0xFF = free floor** and
  **0x00 = obstacle/wall** (the MRPT "probability of free" convention). Therefore:

  | Byte | Meaning |
  |---|---|
  | `0x00` | wall / obstacle |
  | `0x7F` | unknown |
  | `0xFF` | free, not segmented |
  | other (`0x01..0xFE`) | free cell in room `label` |

  Confirm this against one captured map. Label values are the IDs used by 21030 merge/split
  (`pixel`, `pixel1`, `pixel2`) and by 21023 `segmentId` [inferred].
- **Origin quirk [static]:**
  - The frame's `x_min`/`y_min` are the **centre** of cropped cell 0 (`origin + (col0+0.5)*res`).
  - `FUN_0045a430` then prints `x_min - 0.05` and `y_min - 0.05` (constant `DAT_00487a88`).
  - So the centre of column `c` is `x_min + 0.05 + c*resolution`, and the same holds for rows/y.
  - MAP.md gives the corner formula `x = x_min + col*resolution` and does not claim a cell
    centre. The two readings differ by at most half a cell (0.025 m at `resolution` 0.05), not a
    whole cell — an earlier revision of this file said "off by one cell", which overstated it.
- Rows are row-major with y increasing with the row index (world y up). Flip vertically for
  screen display [inferred].
- **`area[]`** in the 20002 payload = the current region list (§4), plus any array found in
  `/tmp/Run/LastRecord/AreaExtern.json` [static]. Each element is written from a fixed template
  (`FUN_0045a430`, `np_all.c:73472`):
  `{"vertexs":[[x,y],…],"active":"…","name":"…","tag":"…","id":N,"mode":"…","forbidType":"…"}`
  — with **no `cleanType` and no `workNoisy`**, and with vertices already snapped by the parser
  (§4). Consequence: a read-modify-write that starts from 20002 and posts back via 21003
  silently resets per-region clean type and fan level. Round-trip through 21004 instead (§5.1).
  The writer of `AreaExtern.json` is not in this firmware.

### 2.4 Drawing the map: robot mm ↔ raster ↔ screen — new
Given a decoded 20002 map: `width`, `height` (cells), `resolution` (metres/cell), `x_min`,
`y_min` (metres, as received) and the row-major grid. All of the following is **[static]** unless
tagged otherwise.

- **Cell → world (cell centre, mm):**
  `x_mm = 1000*(x_min + 0.05 + col*resolution)`,
  `y_mm = 1000*(y_min + 0.05 + row*resolution)`.
  The `0.05` is a fixed constant (one cell, 50 mm) — it is not derived from `resolution`.
- **World → cell (for drawing mm points):**
  `col = (x_mm/1000 − x_min − 0.05)/resolution`, `row = (y_mm/1000 − y_min − 0.05)/resolution`.
  Round to the nearest integer for a cell index; keep the fraction for smooth overlay drawing.
- **This firmware's fixed values:** `resolution = 0.05` (`DAT_00487a88`), grid base origin
  −2.2 m (`DAT_00487a8c`). So `x_mm = 1000*x_min + 50*(col+1)`, and cell centres lie at
  −2175 + 50k mm.
- **The 50 mm vertex lattice.** Region vertices are snapped to `(v/50)*50 ± 25` (§4), i.e. every
  sendable vertex is ≡ 25 (mod 50) mm. That is exactly the raster's cell-centre lattice, so a
  snapped vertex lands on a cell centre of the uploaded grid (when inside the crop); a vertex
  outside the crop has no cell in the message.
- **Drawing:**
  - *Zone polygons:* transform each `vertexs` point with the inverse above; the points are
    already on the lattice, so they sit on cell centres.
  - *Path:* strip the low 2-bit tag first (`v & ~3`, §3), then use the same transform.
  - *Dock:* `chargeHandlePos` is already `[x_mm, y_mm]` in the same world frame (§2.5) — same
    transform. Heading = `chargeHandlePhi/1000 − π` rad (CCW from +x) [static units; sign
    convention inferred].
  - *Screen:* pick a cell size `cellPx`; `px = col*cellPx + offX`, and since row 0 is the
    smallest world y while screens grow downward, `py = viewH − (row*cellPx + offY)` (vertical
    flip) [inferred].
- **Screen tap → robot mm** (for issuing 21003 `vertexs`, 21023 `extraAreas`, 21030 `p1`/`p2`):
  with the drawing above, `col = (tapX − offX)/cellPx`, `row = (viewH − tapY − offY)/cellPx`,
  then `x_mm = 1000*(x_min + 0.05 + col*resolution)`.
- **History maps differ:** the 20004 record's `x_min` has no −0.05 shift, so there the centre is
  `x_min + col*resolution` (§8.4). Use the matching formula per message.
- **The app's own transform is different** (its 1000-unit canvas with `mapDis`, §9.3). Its
  units are converted to robot mm by the vendor cloud, and that mapping is not observable
  [inferred] — do not reuse the app's numbers on the robot side.
- **Pose:** heading/position for drawing the robot comes from `FUNC_STATUS.md` §2 (`pos` in mm,
  `phi` in thousandths of a radian, **without** the dock's π offset); this file does not
  re-derive it.

### 2.5 Dock fields — corrects MAP.md
- The 20002 payload ends with `"chargeHandlePos":[x,y],"chargeHandlePhi":<int>` and
  `"chargeHandleState"` (`FUN_0045a430`). The state string is `"find"` on success and
  `"notFind"` when the getter fails, in which case no pos/phi keys are emitted
  (`np_all.c:73365–73378`; MAP.md already documents both).
- **[static] Units** (getter `FUN_0043eda0`): `chargeHandlePos` is **millimetres** world frame:
  `(int)(metres*1000)`. `chargeHandlePhi` is **thousandths of a radian, offset by +π**:
  `(int)((phi + π)*1000)` — constants `0x48c2b8` = 1000.0, `0x48c2c0` = π. It is **not degrees**;
  the range is 0..6283. The heading in radians is `chargeHandlePhi/1000 − π`.
- MAP.md's current revision already carries the corrected units, and so does
  `schemas/channelB_gateway.schema.json`; `FUNC_STATUS.md` §2 documents the same getter. (MAP.md's
  earlier "`<int deg>`" text is gone.) This section is the verification record: the units above
  are read from the getter and its float constants.
- Drawing: as §2.4; the dock shares the path/zone world frame, so one transform covers all.

---

## 3. Cleaning path (trace) — corrects MAP.md §21011

- **Request/response, not a stream [static].** The cloud sends (enveloped; corrected 2026-10-07,
  `CHANNEL_B_INBOUND.md`)
  `{"encrypt":0,"data":{"infoType":21011,"data":{"startPos":<int>,"mask":<int>},"dInfo":{"ts":…,"userId":…}}}`.
  Handler `FUN_0046ee48` starts PathSend (`FUN_0045dea0`) from `startPos`. The robot answers over
  **HTTP** `POST cleanPack/response` (URL table `+0x68`) with body
  `sn=%s&infoType=21011&ts=%s&userId=%s&data=`:
  ```
  {"message":"ok","infoType":21011,"data":{"userId","pathID","startPos","totalPoints",
   "posArray":[[x,y],[x,y],...],"pointCounts":N},"dInfo":{"ts","userId"}}
  ```
- **`posArray` holds `[x,y]` pairs, not a flat list** (`FUN_0045e104` writes `[`, x, `,`, y, `]`).
- **Units [static]:** millimetres in world frame (`PathProcess::UpdatePathPoints` uses pose × 1000).
- **Low 2 bits of x and of y are a point-type tag.** Each value is truncated to a multiple of 4,
  then the tag is added. The classification is ordered — earlier tests win — so the **ShowPoseType
  4** test comes first [static, `libcpc.so` `PathProcess::UpdatePathPoints`]:

  | Condition (tested in this order) | Tag `(x&3, y&3)` |
  |---|---|
  | ShowPoseType 4 **and** status `rfctrl`(7) | (0,2) |
  | ShowPoseType 4, any other status | (1,2) |
  | status `backcharge`(4) or `FindChargerAndWash`(11) | (0,1) |
  | status `rfctrl`(7) | (0,2) |
  | ShowPoseType 0 | (1,0) |
  | ShowPoseType 1 | (0,0) |
  | ShowPoseType 2 | (1,1) |
  | ShowPoseType 3 | (0,3) |
  | otherwise | (3,3) |

  Precedence matters: `backcharge` with ShowPoseType 4 is (1,2), not (0,1). ShowPoseType names
  are unknown. Strip the bits (`v & ~3`) for drawing.
- **`mask` (a.k.a. "mark"), `startPos`, and the reply counters [static]:**
  - The second int of the request is stored on the queue node and logged as `mark`
    (`FUN_0045dea0` stores `startPos` at node `+0x10` and the mark at `+0x14`; the send log at
    `path_send.cpp:0x6d` prints `\tmark:%d`). No firmware behaviour found reads it: queue identity
    is `(startPos, dInfo)` — a new request with the same pair updates the pending node instead of
    adding one — and the mark is only carried along. **A server can send any int (0 is safe); it
    is not needed to obtain a path.**
  - `startPos` is echoed in the reply and is the queue key. Whether it is literally a point index
    into the path is not established [static/inferred].
  - In the reply, header `totalPoints` is the count the path store reports for this request;
    trailer `pointCounts` is the number of points actually written into this `posArray`
    (`FUN_0045e104` logs "SendCleanPathNewTail pointCounts = %d"). The writer stops emitting
    points once the body reaches ~130 KB (length checks `0x1fbc0`/`0x1fbe0`), so for a long path
    `pointCounts < totalPoints`; fetch the remainder by re-requesting with a later `startPos`
    (exact resume semantics untested).
- **[static]** Inside a 30000 batch (`FUN_00470d40`, `{"cmds":[...],"fastCmds":[...]}`), a 21011
  entry returns only the header with `posArray` empty.
- **[static]** Command replies in general go over HTTP `cleanPack/response`, not the TCP socket.
  `FUN_00459100` builds `sn&infoType&ts&userId&data`, echoing `dInfo`, and `FUN_0045ee18`
  "DealResponse" posts it. A server must read results there.
- **21020 correction (corrects PROTOCOL.md, COMMANDS.md, MAP.md and this file's earlier text).**
  [static] 21020 is the **direct remote-control command**, not a chunked pack transfer. It exists
  on two paths, which earlier revisions merged:
  - **Cloud / Channel-B path:** handler `FUN_0046f5b0` (wrapped by `FUN_004703e8`) reads
    `data.ctrlCode` (int, required) and `data.params` (object, optional), dispatches via
    `FUN_00413b20`, and **sends no reply at all**.
  - **LAN UDP path:** `OpenUdpRemoteCtrl` (`FUN_0041366c` → `FUN_004100b8(&DAT_004c42a0,0,FUN_00414140)`)
    registers the UDP handler `FUN_00414140`, whose request shape is
    `{"infoType":21020,"packId":N,"data":{"mode":"rfctrl","ctrlCode":int,"ctrlParams":…}}` (note
    the different member name) and which acks `{"message":"ok","infoType":21020,"packId":N}` (or
    `"fail"` + reason). **That ack exists only on the LAN UDP server**, not on the cloud protocol.
  Details and the whole ctrlCode table are in `FUNC_COMMANDS.md` §2.1.

---

## 4. Region (zone/room) data model — corrects MAP.md "Region descriptor"

Parser: `libcpc.so` `AreaProcess::CreateRegionPolygonFromJson`. Enum tables come from
`CoverModeStr`, `ForbidTypeStr`, `CleanModeStr`, `WorkFanLevelStr`. All [static].

```json
{"vertexs":[[x,y],[x,y],[x,y],...], "active":"normal|depth|forbid",
 "forbidType":"all|sweep|mop", "cleanType":"sweepOnly|mopOnly|sweepMop",
 "workNoisy":"mop|quiet|auto|strong|max", "name":"<str>", "tag":"<str>",
 "id":<int>, "mode":"default|area|room|point|curpoint"}
```

- **`vertexs`**
  - At least 3 integer points, otherwise the region is rejected **by the parser**. A 21003
    payload containing a 1–2 vertex region is stored without complaint (§5.1) and fails only when
    the region is later used. Arbitrary polygons are allowed.
  - Units are **mm, world frame**, the same frame as the map origin.
  - Each vertex is snapped to the centre of a 50 mm cell: `(v/50)*50 ± 25`.
  - A polygon thinner than 50 mm in either axis can **collapse**: vertices at y=0 and y=10 both
    snap to 25, leaving a zero-height region. Minimum useful wall thickness is one cell (both
    edges must land on different centres). How the robot behaves on a collapsed or very thin
    forbid polygon is untested. A virtual wall therefore has to be sent as a thin but
    non-degenerate polygon; there is no line primitive [inferred].
- **`active`** = cover mode: `normal`(0), `depth`(1, deep clean), `forbid`(2).
  **Correction:** a no-go zone is `active:"forbid"`. MAP.md says "any region with a forbidType is
  forbidden", but `forbidType` is always present in the robot's output.
- **`forbidType`** is used only when `active=="forbid"`:
  - `all`(0) = no-go
  - `sweep`(1) = no-sweep
  - `mop`(2) = **no-mop zone**
- **`cleanType`** (per region): `sweepOnly`(0), `mopOnly`(1), and any other *string* →
  `sweepMop`(2). **Default when the member is absent or not a string: `sweepOnly`(0)** — the
  field starts at 0 and only a present-but-unrecognised string sets 2. Earlier revisions said the
  default was `sweepMop`; wrong.
- **`workNoisy`** is the per-region fan level; **default `auto`(2)** when absent or illegal
  (illegal logs "Member workNoisy illegal:%s").
- **`name`, `tag` and `mode` are copied with an unbounded `strcpy` from the incoming JSON —
  there is no length check anywhere.** `CreateRegionPolygonFromJson` `strcpy`s each string into
  a **32-byte stack buffer** (`libcpc_area.c:1524` name, `:1533` tag, `:1547` mode; objdump
  `0x23794`/`0x237ec`/`0x23874`), then block-copies those buffers into the region struct at
  `+0xc` (name, 32 B), `+0x2c` (tag, 32 B) and `+0x4c` (mode, **31 B**, through `+0x6a`).
  Nothing truncates and nothing rejects.
  - **A server must keep `name` and `tag` to at most 31 UTF-8 bytes** (plus NUL) **and `mode`
    to at most 30** (its struct field is 31 bytes). It is bytes, not characters: a Chinese room
    name of ~10+ characters overflows.
  - A longer value is a real buffer overflow in the robot firmware: the `strcpy` itself
    overruns the 32-byte **stack** buffers in the parser frame (the struct stores are
    fixed-size block copies, so the stack is damaged first, not the struct neighbours). The
    parse runs whenever a region is materialised — 21023 `extraAreas` (§6.1) and the stored
    21003 list when it is read back (`GetRegionPolygonFromID`/`GetRegionPolygonFromIndex`) —
    and 21003 stores input verbatim (§5.1), so any client string can reach it. Treat this as a
    memory-safety flaw in the firmware and never send longer values.
- **`mode`** is the **region kind**, not a clean mode (correction to MAP.md):
  - `area` → 2
  - `point` → 3
  - `curpoint` → 4
  - `room` → 5
  - `default` → 2
  - missing or unrecognised `mode` → **0** (no branch matched; the kind keeps its initial 0).
    Earlier revisions said 2; wrong.
  - Segment regions created by the firmware get kind 6.
- **`id`** defaults to **−9** and is stored as a signed **16-bit** field (a 32-bit `asInt` is
  truncated; values outside −32768..32767 wrap).
- Firmware-generated point regions are 1.5 m squares (±750 mm) around the pose, named
  `Point`/`point` or `CurPoint`/`curpoint` (`CreatPointCleanRegion`, `CreatCurPointCleanRegion`).
- **Not present:** no room-order or per-room repeat-count key is parsed.

Persisted file: `/tmp/Run/LastRecord/AreaSetting` = `{"mapId":<int>,"value":[region,...]}`.

---

## 5. Editing zones and rooms

### 5.1 Set the whole list — `21003` SetAreaTactics (cloud → robot)
```
{"encrypt":0,"data":{"infoType":21003,"data":{"mapId":<int>,"value":[region,...]},"dInfo":{"ts":"…","userId":"…"}}}
```
- `FUN_00471758` → `FUN_00410f50` → `AreaProcess::SetAreaData`. This **replaces** the whole
  list, rewrites `AreaSetting`, and posts `EID_I_APP_SET_FORBID_AREA`(1050) and
  `EID_I_APP_SET_CMD`(1042). The reply is "ok" via `cleanPack/response`. [static]
- `mapId` is stored but **not checked** against the current map [static].
- **`SetAreaData` validates nothing.** It serialises the input (`toStyledString`) and writes it
  to `/tmp/Run/LastRecord/AreaSetting` verbatim — unknown fields, wrong types, <3-vertex
  polygons and over-long `name`/`tag`/`mode` are all stored as received (§4 describes where they
  bite). `ResetTotalJson` runs first to drop the old list.
- **Every zone edit is a read–modify–write of the full list.** That covers add, move and delete
  no-go / no-mop / deep-clean zones, renaming rooms, and per-region fan/clean type. MAP.md's
  "payload carries region descriptors" is correct but understates that the list is replaced.
- **Read-modify-write from 21004, not from 20002.** The 20002 `area[]` template omits
  `cleanType` and `workNoisy` and carries already-snapped vertices (§2.3); posting it back as
  21003 silently resets every region's clean type to `sweepOnly` and fan level to `auto`.
  Always fetch with 21004, edit, and send back.

### 5.2 Read the list — `21004` GetAreaTactics
- `{"encrypt":0,"data":{"infoType":21004,"data":{}}}` → the reply `data` is the `AreaSetting` JSON
  (`FUN_00471e28` / `FUN_00411088`) [static].
- The reply is the **stored JSON verbatim** — it echoes exactly what was last posted, unsnapped
  vertices, unknown fields and over-long strings included.
- Also: `21002` = GetTimeTactics (schedules, out of scope).

---

## 6. Cleaning specific targets

### 6.1 `21023` ActiveRegions (`FUN_00471bc0` → `FUN_0041bdf8`) [static]
```
{"encrypt":0,"data":{"infoType":21023,"data":{
   "cleanId":[<int>,...],          // REQUIRED, otherwise "ActiveRegions no cleanId" → fail
   "extraAreas":[region,...],      // optional ad-hoc regions (§4 schema)
   "segmentId":[<label>,...] },    // optional auto-segmented rooms
   "dInfo":{"ts":"…","userId":"…"}}}
```
`cleanId` values:

| Value | Meaning |
|---|---|
| −1 | whole-map "Total" region |
| −2 | every stored region |
| −3 | stored forbid regions only (`active==forbid`) |
| −4 | stored non-forbid regions only |
| N≥0 | the stored region whose `id == N` |

- `extraAreas` items are parsed with the §4 schema (and are one of the paths into the
  unbounded-`strcpy` flaw in §4):
  - **zone clean** = a region with `mode:"area"`
  - **spot / go-to-point clean** = a region with `mode:"point"`, normally a 1.5 m square around
    the target [inferred, by analogy with `CreatPointCleanRegion`]
- `segmentId` = room labels from the segmentation raster (§2.3, §7); each becomes a kind-6 region
  (`AreaProcess::CreatSegmentRegion`).
- The result is written to the region shared memory and `EID_I_ACTIVE_REGIONS`(1051) is posted.
- Practical use: including `-3` alongside the targets keeps stored no-go zones active —
  **recommendation only; no firmware requirement or observed behaviour behind it [inferred]**.

### 6.2 Does 21023 start the job? — resolved 2026-10-09 (`ZONE_CLEAN.md`)
- The 21023 handler posts only 0x41b, never a start event [static] (the start events
  0x410/0x411/0x413 come from the 21005 handlers). `task_manager` handles 0x41b only in the
  sweep (arg 2) and CPS-4/0xb (arg 4) states, via `TaskManager+0x110` (`FUN_00417608`): push
  the shm region set (`0x1389`; sweep additionally pauses/resumes the navigator via `0x7d6`) —
  a **live update of a running job**. On an idle/docked robot the event is dropped: **21023
  alone never starts**.
- **Start with `21005 {"mode":"appointClean"}`** (EID 0x410, no payload): `task_manager` starts
  `clean_task` from the shm region set 21023 wrote (`-t <CleanSubModeStr>`); `reAppointClean`
  (0x411) is equivalent. **Do not use `smartClean`/`depthTotalClean` for a zone clean** — they
  synthesize a whole-map total region and **overwrite the shm**, wiping the selection (log
  "App start total clean"). Full sequence, state coverage and evidence: `ZONE_CLEAN.md`.

### 6.3 Spot clean at the current position [static]
- On the **cloud/Channel-B path**, `21020 {"data":{"ctrlCode":3001}}` → `EID_I_APP_POINT_CLEAN`(1039)
  ("DirectRfCtl start curpoint clean"). `FUN_0046f5b0` (wrapper `FUN_004703e8`) reads
  `data.ctrlCode` and calls `FUN_00413b20`; it **sends no reply** (§3; the `packId` ack belongs
  to the LAN UDP variant).
  The robot uses a 1.5 m square around itself.
- The handlers "App start point clean" (`FUN_0041d090`), "App Start Spot Clean" (`FUN_004138e8`)
  and `UpdateAllForbidRegionsOnly` (`FUN_00419528`) have **no callers** (capstone scan of
  `.text`). They are dead code in this build.

### 6.4 Smart room clean [static]
- `21005 {"mode":"smartAreaClean"}` (`FUN_0041afb0`) runs auto-segmentation and then cleans room
  by room.
- `21005 {"mode":"depthTotalClean"}` sets `cover_mode=1`.

No "go to point without cleaning" command was found.

---

## 7. Room segmentation editing — `21030` SetAutoAreaMap (`FUN_00473528`) [static]

```
{"encrypt":0,"data":{"infoType":21030,"data":{"autoAreaId":<int>,"operate":"<op>","extra":{...}},"dInfo":{"ts":"…","userId":"…"}}}
```

| operate | extra | effect |
|---|---|---|
| `reset` | – | Re-run segmentation on the plan map (`FUN_0041a288`). New `autoAreaId`. |
| `clear` | – | Clear `SegmentationMapShareMem` (no rooms). |
| `merge` | `{"pixel1":L1,"pixel2":L2}` | Merge two room labels (`FUN_00417d28`). |
| `split` | `{"pixel":L,"p1":[x,y],"p2":[x,y]}` | Split room L along the line p1–p2, given in **mm world**. `FUN_00418088` converts to segmentation pixels as `pixel = (mm/1000 − seg_origin)/seg_res`, where `seg_origin`/`seg_res` come from the **`SegmentationMapShareMem` header** — **not** from the 20002 `x_min` (earlier revisions implied the wrong origin). |
| `debug` | – | Copies debug files to `/tmp/LdRecord`. |

7.1 **`autoAreaId` must match the robot's current value.** The internal operations return
distinct codes (mismatch = −0x6e; `MergeAutoAreaMap` logs "id not the same (…), can not support
!!"); the wire reply reports the pairs in 7.2 — a mismatch is code **−1** with info
`autoAreaId not match to local ..`. (An earlier revision mixed the internal −0x6e with the wire
code −1.)

7.2 **Reply** (via `cleanPack/response`):
- Success: `{"message":"ok","infoType":21030,"data":{"operate":..,"autoAreaId":<new>,"code":0}}`
- Failure: `"fail"` with `info`/`code` (exact strings):

  | Info | Code |
  |---|---|
  | "area not connected" | −3 |
  | "map Empty" | −4 |
  | "map Too Small.." | −2 |
  | "auto area memory is nullptr .." | −1 |
  | "autoAreaId not match to local .." | −1 |
  | "other Error .." | −6 |

  Internally these are −0x72, −0x71, −0x70, −0x6f, −0x6e in the order above; every other internal
  code falls into "other Error .." (−6).

7.3 After every operation the robot force-uploads the map (§2.2), so the new labels and
`autoAreaId` arrive in 20002.

7.4 **Rename and ordering:** segment labels carry no name on the robot. Names exist only as
`name` on 21003 regions, or server-side [inferred]. **Room order: unknown.** There is no order
field; `segmentId` array order may define it [inferred, untested].

---

## 8. Map lifecycle

### 8.1 Delete the map [static]
`21024 {"cmd":"delCurMap"}` or `{"cmd":"cancleMap"}` (both go to the same branch of
`FUN_00472f20`) → `FUN_00413498` → `EID_C_DELETE_MAP`(2033).

### 8.2 Backup map (static, partially)
- **Upload.** When a clean finishes, `OnNewCleanRecord` (`FUN_0044ab18`) multipart-posts the
  following to `cleanPack/uploadSingle` (URL table `+0xc8`):
  - `cleanFile` (the record)
  - `backupMap` (`*.bkmap` = tar.gz of `/tmp/Run/LastRecord`; see MAP.md and `save_backup_map.sh`)
  - `backupMapMd5`
- **Restore.** `21025 {"downUrl":"<url>","md5":"<hex>"}` (`FUN_00472da0` → `FUN_00446938`/`ce0`):
  1. download to `/tmp/DownloadBackupMap`
  2. check the md5
  3. apply (`load_backup_map.sh`, `EID_C_RELOAD_BACKUPMAP` 2032)
  4. report "BackupMap apply ok" or "down fail" via `cleanPack/response`
- **Enable switch.** The `backupMapSwitch` status attribute exists. Its setter
  `FUN_0041750c` (SetCollectBackupMapSwitch) has no direct caller. How it is set is unknown.

### 8.3 Multi-map
There is one live map plus one backup slot. `MapList.json` is kept across restores, but nothing
in the firmware selects between several maps. This means multi-floor maps are not supported by
this build [inferred].

### 8.4 Clean-record (history) map [static, partial]
- `task_manager` `FUN_0041d9b0` writes `/tmp/Run/BackUpMap/<…>.txt` containing:
  - `{"infoType":20004,"data":{"events","mapID","width","height","resolution","x_min","y_min","lz4_len","map", base64_len, …}`
  - the record fields `sn, mac, start, end, sweep, mop, sweepOnly, cleanTime, cleanMode,
    isDoneNormal, isError, curState`
  - `area:[{vertexs…,active,forbidType,name,tag,id,mode}]`
  - `chargeHandle*`
- **The 20004 origin is not the 20002 formula.** `tm_all.c:22455` builds
  `x_min = origin + (col+0.5)*res` with **no −0.05 shift**: for history maps the wire `x_min` *is*
  the centre of cropped cell 0, so cell centres are `x_min + col*res` — one 50 mm cell off from
  what §2.3/§2.4 give for a 20002 map. A viewer must use the matching formula per message.
- **`events`** is `EventId2String::ShowString` names concatenated with **no separator** into a
  1 KiB zeroed buffer (cap `0x3ff`), so it is partly decodable from the event-name table;
  exact truncation behaviour not tested.
- The robot uploads this file as `cleanFile` (§8.2). The exact field order and the remaining
  `events` details are not fully decoded. The app side of history maps is §9.5.

---

## 9. App 1.5.11 map pipeline (BaoLe/YouRen vocabulary, see §1)

Coverage caveat: the dex is complete at class level, but 77 activity `onCreate` bodies
(including `MapLaserActivity`'s) are Jiagu-extracted stubs — helper logic used below is present
and readable, but the exact per-screen startup conditions come from bodies we cannot see
(`APP_COVERAGE.md`).

### 9.1 Fetch and polling [static]
- On open, `getIMMapDate` sends **LASTCLEAR** (the Channel-A command with code 36 — the command
  table is in `WIRE_PROTOCOL.md` and is not repeated here) with body
  `{appKey, authCode, deviceId, deviceType:"1"}` and `targetType "6"`, `targetId "0"`.
- Then `getIncrementMap` sends **TRANSIT `transitCmd:"133"`** with `trackNum` and `mapSign`
  every 5 s, or every 2 s when the LAN flag `isLAN` is set. Polling only runs while the screen's
  `isStart` flag is set, is suspended while the activity is paused or `isEnd` (map/track
  processing/animating is in progress), and is suppressed while `isStartForbidden`/
  `isStartPrecincts` are set (`MapLaserActivity.java` runnables around lines 431–450).
  - All traffic still goes through the cloud IM socket (`TransitCmdManager`).
  - Identical JSON within 600 ms is dropped.
- Reply `value` fields:

  | Field | Meaning |
  |---|---|
  | `result:"0"` | success |
  | `map` | base64 map binary |
  | `track` | base64 track binary |
  | `leftMaxPoint`, `rightMaxPoint`, `centerPoint` | `"x,y"` in map units |
  | `chargerPos` | `"x,y"` |
  | `forbiddenArea`, `cleanArea` | JSON list of `{"area":"l,t;r,b",…}` |
  | `clearArea`, `clearTime` | cleaned area and time |
  | `clearSign` | changes per cleaning session; a change resets the map |
  | `isMove` | `"1"` = relocalized; resets the map |

- A push with `noteCmd 102` and `result "1005"` also resets the map.
- Non-laser robots use `MapOrdinaryActivity` with `transitCmd "131"`. LiDAR robots are selected by
  `robot.modules.radar=="1"`.

### 9.2 Binary formats
- **Track [static]:**
  - Header bytes: `[1]`=bytes per point (4), `[2..3]` u16LE cleaned area, `[4..5]` u16LE start
    index, `[6..7]` u16LE end index.
  - After the header come points as u16LE x, u16LE y, in map units 0..1000.
  - The next request's `trackNum` = base64 of the end index as **2 bytes big-endian**
    (`intToByteArrayLittle` actually produces big-endian). The initial value is `"AAA="`.
  - If the chunk start ≠ the previous end, the app restarts the path.
- **Map:**
  - Header is 8 bytes; `[6..7]` u16LE is echoed back as `mapSign`, base64 and big-endian [static].
  - The pixel decode is native: `org.k.JNIUtils.getMapBitmap` → `ModifyBitmapMapData(bitmap,
    history_id[100], colourWall #666666, colourFloor #ffffff, bytes)`. **The header is not
    actually stripped:** `MapLaserActivity` copies `mapLaser[8:]` into a new array but discards
    the result and passes the **full** buffer (header included) to `getMapBitmap`
    (`MapLaserActivity.java:870-872`), so whatever the native decoder does with those 8 bytes is
    its own business [static].
  - **`libtoBitmap.so` is absent from this APK** — provably never in the signed universal APK and
    not downloaded at runtime (`APP_COVERAGE.md`) — so the pixel format stays unknown. The
    decoder is real and on the M7 Pro path; in *this* build its absence would break only the
    live-map view, not device control. Recovering the format needs the lib from the phone's
    installed build (adb pull) or another APK.
  - [inferred] It is a block map: 10×10 blocks with per-block `history_id`, as in the
    `BlockMap`/`MapHistory` beans. The unused Java `__uncompress` decodes an RLE with
    `0xC0|count` prefix into a 2500-byte block (= 100×100 cells at 2 bpp).

### 9.3 Rendering and coordinate transforms [static, `LaserMapView`, `PartitionView`]
This is the app's **own** 1000-unit space; how those units map to the robot's millimetres is
done by the vendor cloud and is unknown (§1, §2.4). For a replacement that draws the firmware's
grid directly, use §2.4 (and §8.4 for history maps).

- The map canvas is 1000×1000 units. `widthCoef = viewW/1000` and `heightCoef = viewH/1000`
  (non-uniform). The bitmap is 1000×1000 ARGB scaled by that matrix.
- Drawing order:
  1. `canvas.scale(s, about view centre)` (s = 1..5, pinch)
  2. `translate(dx,dy)`
  3. bitmap
  4. track path (#bbbbbb)
  5. dock circle + bolt at `chargerPos`
  6. robot
  7. forbidden rectangles (#cccccc fill, #333333 edge)
  8. clean rectangles (#ffd5de, multiply)
- **Robot** = last track point, drawn as a circle (radius 10/16 px) with no heading. It is
  animated along the newest ≤50-unit segment. Pose/heading decoding is in `FUNC_STATUS.md` §2
  (`pos` mm, `phi` thousandths of a radian, **no** π offset there — the dock's own π offset is a
  different field, §2.5).
- Auto-fit: centre on `centerPoint`; scale so the `left/rightMaxPoint` box fills 80% (`setScaleToBig`).
- Rotation is in 90° steps via `View.setRotation` and is reset to 0 while editing.
- **Screen ↔ map while editing:** `setTouchClose()` forces scale 1 and offset 0, so
  `map = px / (viewSize/1000)`.
  - `getMapcoordinate()` returns integer `"l,t;r,b"`.
  - left and top are clamped to 0..990; right and bottom to ≤1000.
- The size label in metres = units × `mapDis`. `mapDis` = `robotInfo.param.mapDis` in **mm per
  unit** ÷1000; the default is 0.03 m.

### 9.4 Edit commands [static, `MapLaserActivity.onClick`/`setAreaClean`]
- **Zone clean:** `transitCmd "164"`, `value.cleanArea = [{"area":"l,t;r,b","sign":"<ms timestamp>"},…]`.
  At most 5 rectangles. [inferred] It also starts cleaning, because the button is labelled "clean".
- **No-go zones:** `transitCmd "166"`, `value.forbiddenArea = [same objects]`.
  - At most 5. This is a full replace: an empty list clears all zones.
  - There is no no-mop or virtual-wall type in this app.
- After a successful reply the app re-polls with 133.
- Editing is refused when `workState` is 0 (offline), `"1"` (busy), or 30–32 (error/upgrade).
- **Not in the app:** room split, merge or rename, go-to-point, per-room settings, multi-map.

### 9.5 History maps (REST) [static]
Calls go to `https://bl-app[-region].robotbona.com/baole-web/robot/<name>.do` (note the
`/baole-web` path segment), form-encoded and signed with
`nonce_str`, `rep_check` and `sign` (`YouRenSdkUtil.getRequestBody`). The signing algorithm was
not analysed.

| Endpoint | Request | Response |
|---|---|---|
| `getRobotClearList` | `{deviceId, cuPage, pageSize}` | record list |
| `getRobotClearRecordInfo` | `{clearId}` | `{map, track, extParam:"{centerPoint,leftMaxPoint,rightMaxPoint}", forbiddenArea, cleanArea, chargerPos, clearArea, clearTime, clearModule, clearModuleName, clearSTime}`, rendered by `JMapDetailActivity` |
| `delRobotClearRecord` | `{clearId}` | — |
| `delRobotAllClearRecord` | `{deviceId}` | — |

---

## 10. Corrections to existing documents (summary)

| Doc | Claim | Correction (evidence) |
|---|---|---|
| MAP.md | 20002 sent on Channel B and on Channel A | HTTP `uploadEvents` only. Inbound 20002/21014 = "send map now" (§2.1). |
| PROTOCOL.md | Pong `{"data":{}}` is enough | Keeps the link up, but map upload needs `data.isExistConnect:true` and the MapSend pause flag clear (§2.2). |
| MAP.md | Cell semantics "higher = more likely occupied" | 0x00 wall, 0x7F unknown, 0xFF/labels free [inferred] (§2.3); MAP.md now carries this table (corrected 2026-10-05). |
| MAP.md | World mapping `x = x_min + col*res` | The firmware prints `x_min` as stored centre minus 0.05 m, so cell centres are `x_min + 0.05 + col*res`; MAP.md's corner reading differs by half a cell, not a whole one (§2.3). |
| MAP.md | `chargeHandlePhi` (earlier: `<int deg>`) | Already corrected in MAP.md's current revision: `(int)((phi+π)*1000)` — thousandths of a radian, +π offset; `chargeHandlePos` in mm (§2.5, verified from `FUN_0043eda0`). |
| MAP.md | 21011 is a device stream with flat `posArray` | Cloud request → HTTP response; `[[x,y]…]` in mm with 2-bit tags (§3). |
| MAP.md, PROTOCOL.md, COMMANDS.md | 21020 = chunked pack transfer | Remote-control `ctrlCode` command; the `packId` ack is LAN-UDP-only, the cloud path sends no reply (§3, §6.3; `FUNC_COMMANDS.md` §2.1). |
| MAP.md | Region `mode` = per-room clean mode; `forbidType` ⇒ forbidden | `mode` = region kind; forbidden ⇔ `active:"forbid"`; clean type is `cleanType`; fan is `workNoisy` (§4). |
| COMMANDS.md, WIRE_PROTOCOL.md | App `transitCmd` ↔ robot via cloud translation | The app is the user's app for this robot; the translation lives in the vendor cloud and is not visible in either binary. A replacement should target the firmware contract (§1). |
| COMMANDS.md | 131/133 "map ops" | 133 = laser map/track poll, 131 = non-laser poll, 164 = zone clean, 166 = no-go list (§9). |

---

## Open questions
1. Exact cell semantics and label range of the 20002 raster, and whether a room label can
   collide with 0x7F/0xFF. Capture one map (needs the `isExistConnect:true` pong).
2. Whether 21023 alone starts a zone/room/point clean, or a 21005 start must follow (§6.2), and in
   which order rooms are cleaned.
3. How the cloud/app is meant to issue go-to-point. Only `extraAreas` with `mode:"point"` was
   found; it is untested and may still clean rather than just navigate.
4. ShowPoseType names behind the path tag bits (§3).
5. Full `20004` record layout and the `events` truncation behaviour. Also how `backupMapSwitch` is
   set, since its setter has no caller.
6. Which writer (if any) produces `AreaExtern.json`.
7. **The cloud translation.** How the vendor cloud maps the app's `transitCmd` vocabulary and
   1000-unit coordinates to/from the robot's `infoType` messages is not in either binary (§1).
   Comparing what the app sends (readable) with what the robot's server-side traffic does
   (Channel B, once rehomed) is the route to it; no conclusion about the cloud should be built
   into a replacement.
8. The 1.5.11 map pixel format, because `libtoBitmap.so` is missing from this APK build
   (§9.2). Recoverable from the phone's installed build over adb, or from another APK.
9. `21011` `mask`/"mark" semantics, whether `startPos` is a point index, and the exact
   chunk-resume behaviour at the ~130 KB reply cap (§3).
10. Everything here comes from the 2020 `LS_S6` build. Channel A/B behaviour on the field `6716`
    unit is untested (FIELD_NOTES.md).
