# Proscenic M7 Pro — Mapping / Room Segmentation

The robot builds a SLAM occupancy grid of the home and uploads it, plus the
traversed path, room partitions, and dock position, to the cloud. This is the
feature behind RE-repo Finding 4 ("uploads a map of your home"). All items below
are from `network_proxy`, `navigator`, `slam_pose_provider`. **[static]** unless noted.

## Stack identification
* **SLAM / occupancy grid:** **MRPT** `COccupancyGridMap2D` (strings
  `[COccupancyGridMap2D::resizeGrid]…`, `BtGridMap2D`, `Binary.map`,
  `UpdateGridMapByBinaryMap`). A width×height grid of cells with a metric origin
  (`x_min`,`y_min`) and `resolution` (metres/cell).
* **Room segmentation:** an OpenCV-style watershed pipeline in a separate module
  built from `…/map_segmentation/src/mapSegmentation/*` (`_LD_watershed.cpp`,
  `_LD_morphologyEx.cpp`, `_LD_cvtcolor.cpp`, `_LD_drawContours.cpp`,
  `segmentation_maps.cpp`). Output is an `autoAreaId` partition; intermediates land
  in `/tmp/segmentationMapSrc` and `/tmp/segmentationMapOutput`.
  `LdRobotCV::SegmentationMaps(uint8_t*, int, int)` is the entry point; results are
  shared to other processes via the `SegmentationMapShareMem` shared-memory segment.
* **Compression:** **LZ4** (`LZ4_compress_default` / `LZ4_decompress_safe`).
* Shared-memory grids between processes (see `shm.json`): `CleanPathShareMem`
  (key 100, 1000 KB), `ShowMap` (key 101, 2000 KB), `SegmentationMapShareMem`.

## Map upload message — `infoType` 20002 (device → cloud)
`map_send.cpp` emits this `20002` payload **only over HTTP**, as Channel-A
`cleanPack/uploadEvents` (`FUNC_MAP.md` §2.1 corrects an earlier claim that it also went out on
Channel B; inbound `20002`/`21014` on Channel B are "send the map now" requests, not uploads).
Exact template (from `network_proxy`):
```json
{"infoType":20002,"data":{
  "SN":"<serial>",
  "mapId":<uint64>, "autoAreaId":<int>, "pathId":<int>,
  "width":<int>, "height":<int>, "resolution":<float, m/cell>,
  "x_min":<float, m>, "y_min":<float, m>,
  "lz4_len":<int>,
  "area":[ <region>, … ],
  "map":"<base64>", "base64_len":<int>,
  "chargeHandlePos":[<int x, mm>,<int y, mm>], "chargeHandlePhi":<int, (phi+π)*1000 mrad>,
  "chargeHandleState":"find"        // or, when unknown: "chargeHandleState":"notFind" (no pos/phi)
}}
```
* **`map`** = `base64( LZ4_compress( occupancy_grid_bytes ) )`.
  `lz4_len` = length of the LZ4 stream, `base64_len` = length of the base64 text.
  Decode: `LZ4_decompress_safe( base64_decode(map), out, lz4_len, width*height )`
  → a **`width*height`** cell buffer, row-major. One byte per cell; `FUN_00438660` quantizes it
  for upload: raw `<200` → **`0x7F` (unknown)**, otherwise → **`0xFF` (free floor)**; on maps
  wide enough for segmentation (>89 cells) `0xFF` cells are overwritten with a non-zero room-label
  byte (`0x00` = wall; the label range is not guarded against `0x7F`/`0xFF`). Full table and
  caveats: `FUNC_MAP.md` §2.3.
* **World mapping:** the wire `x_min`/`y_min` is the **stored cell-0 centre minus 0.05 m**
  (`DAT_00487a88`), so the centre of cell `(col,row)` is `x = x_min + 0.05 + col*resolution`,
  `y = y_min + 0.05 + row*resolution` — half a cell from the corner reading this document used
  before (`FUNC_MAP.md` §2.3).
* **Dock:** `chargeHandlePos` is in **mm** (`(int)(x*1000)`, `(int)(y*1000)`); `chargeHandlePhi` is
  the heading as `(int)((phi+π)*1000)` — **thousandths of a radian** (range 0..6283), **not
  degrees** (`network_proxy` `ChargerControl::ReadCharger` `FUN_0043eda0`; see `FUNC_STATUS.md` §2).

## Region / room descriptor (the `area[]` elements, and `SetAreaTactics`)
Each region:
```json
{ "vertexs":[[x,y],…],
  "active":"<str>", "name":"<room name>", "tag":"<str>",
  "id":<int areaId>, "mode":"<region kind>", "forbidType":"<forbidden-zone type>" }
```
* `name` = user-visible room name; `id` = area id (ties to `autoAreaId` partitions).
* `mode` = **region kind**; a no-go/virtual-wall zone is `active:"forbid"` — `forbidType` is used
  only then (it is always present in the robot's output). Per-room clean mode is `cleanType`,
  per-room fan is `workNoisy`; the corrected field table is `FUNC_MAP.md` §4.
* ⚠ `name`, `tag` and `mode` are copied with no length check (unbounded `strcpy` into 32-byte
  stack buffers; struct fields 32/32/31 bytes) — keep `name`/`tag` ≤31 and `mode` ≤30 UTF-8
  bytes (`FUNC_MAP.md` §4).
* Categories tracked internally: clean areas, forbid areas, `backWashArea`,
  `AutoForbidRegion` (auto-generated no-go around traps).

### `infoType` 21003 — `SetAreaTactics` (cloud → device)
Sets/updates the room/area tactics (per-room clean mode, forbidden zones, etc.).
Payload carries the same region descriptors; the device applies them to the live
partition. `cleanMode` default is `cleanmop`; smart-clean job trigger looks like
`{"mode":"smartClean","pathType":"y_word","cleanMode":"<mode>","from":"carrier"}`.

## Clean-path stream — `infoType` 21011 (device → cloud)
```json
{"message":"ok","infoType":21011,"data":{
  "userId":"<str>", "pathID":<int>, "startPos":<int>, "totalPoints":<int>,
  "posArray":[ <x0>,<y0>, <x1>,<y1>, … ] }}
```
The traversed path as a flat position array, streamed in chunks (`startPos` /
`totalPoints` allow incremental append; large maps/paths use the `packId` chunking of
`infoType` 21020).

## Persistence / backup
* Live maps/records live under `/tmp/Run/…` and are periodically tarred to flash:
  `save_all_to_flash.sh` writes `LastRecord.tar.gz`, `CleanRecord.tar.gz` to
  `/data/bin/Run/`. `save_backup_map.sh` / `load_backup_map.sh` handle a
  `LastRecord` backup map (a `.tar.gz` of the map + `CleanInfo.json` + `MapList.json`).
* `mapId` / `pathId` are the identifiers used to correlate a stored map with uploads.

## Privacy note
The uploaded grid + room names + dock + full traversed path constitute a detailed
floor plan of the home; historically this went to the vendor cloud over the
non-validated HTTP/TLS channels described in `PROTOCOL.md`. A replacement cloud can
simply store these locally (or discard them).
