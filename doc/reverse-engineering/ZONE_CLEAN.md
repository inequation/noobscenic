# Zone clean — how it actually starts (resolved 2026-10-09)

**Why this document exists.** A replacement server could write zones (21003/21004 round-trip
works) but `21023 {"cleanId":[1]}` never started anything, and a following
`21005 {"mode":"smartClean"}` cleaned the whole home, ignoring the zone. Both halves of that
observation are explained below, and the correct sequence is given.

## Root cause — two independent reasons the old sequence cannot work

1. **`21023` never starts a clean.** It writes the selection into the **CleanArea shared
   memory** and posts **EID 0x41b**. In `task_manager`, 0x41b is handled only by the sweep and
   CPS-4/0xb states, where it calls `TaskManager+0x110` (= `FUN_00417608`) to make the
   **navigator's active-region set match the shm** — pause navigator (`0x7d6`), push regions
   (`SendEventAndWaitReply 0x1389`), resume. That is a **live update of a running job**. The
   idle handler never tests 0x41b: while idle/docked/charging the event is dropped and nothing
   starts. (The 0x41b sites are only the sweep/CPS-4 handlers; the vtable slot was verified in
   the binary.)
2. **`21005 {"mode":"smartClean"}` destroys the selection by construction.** Its handler
   synthesizes a whole-map **total** region (`CreatTotalCleanRegion` → `ShmUser::Write`, log
   *"App start total clean"*), i.e. **it overwrites the same CleanArea shm that 21023 wrote**,
   then posts EID 0x413 (which carries no region data). The started clean task therefore
   sweeps the whole map. `depthTotalClean` does the same (plus `--depthTotal`);
   `smartAreaClean` rebuilds from the segmentation map.

So the old sequence had: the zone write influencing nothing (no job running), then a start
that wipes the zone write before the job starts. Either alone makes the clean whole-home.

## The correct cloud→robot sequence for a zone clean

1. **`21023` ActiveRegions with a non-empty `"cleanId"` array** — positive ids resolve against
   the stored area file (the ids `21004`'s `value[].id` shows); optional `"extraAreas"`
   (mm-world regions, `FUNC_MAP.md` §4 — `"mode":"area"` = zone) and `"segmentId"` are folded
   into the same selection. ACKed (`0x521f` ok; fail when `cleanId` is missing). The 0x41b it
   posts is harmless when idle. Do not interleave a `smartClean` after this.
2. **`21005 {"mode":"appointClean"}`** — posts EID 0x410 with **no payload**; `task_manager`
   starts `clean_task` with `-t <CleanSubModeStr[submode]>`, where the sub-mode comes from the
   shm regions 21023 wrote (plain zones → `--area`, room ids → `--room`, …). Despite the name,
   **`appointClean` is the "start from the current region set" path** — it is not only for
   schedules. `"reAppointClean"` (EID 0x411) runs the same start block.

   Equivalently: **`smartClean` = "clean totals"** (it *is* the total-region builder);
   **`appointClean` = "clean the regions currently selected in the shm"**.

State coverage (each start block read): idle, dormant, sweep (restart / update-in-place when
sub-mode is 3/6; 0x420 is the one refused while sweeping), CPS 3/6/10, CPS 4/0xb,
charge/fullcharge. **The robot need not be undocked.** The only guard observed is the
low-battery pause flag (refusals: *"Clean task paused, scheduled task will not executed"*).

Internal-only alternative (cannot be sent by a server, documented for completeness): EID
**0x420** `{"workNoisy":"…","cleanId":[<ids>],"cleanNum":N}` — the schedule path. Its consumer
resolves `cleanId` itself and starts the same way; with a missing/empty `cleanId` it substitutes
**`[-1,-3]`** = the whole-map **total** region plus the stored **forbid** records (kept as
no-go), i.e. whole home by design.

## Test matrix for the implementor (in order)

| # | Send | Expect |
|---|---|---|
| 1 | `21023 {"cleanId":[1]}` **then** `21005 {"mode":"appointClean"}` | robot travels to the stored zone; `cleanArea` grows there |
| 2 | `21023 {"cleanId":[1],"extraAreas":[{"vertexs":…,"mode":"area"}]}` **then** `appointClean` | both stored zone and inline area reachable (this is run 3 from the request, with the correct start) |
| 3 | `21005 {"mode":"appointClean"}` alone | cleans all non-forbid stored regions; an empty (or forbid-only) shm defaults to the total region = whole home |
| 4 | `21005 {"mode":"smartClean"}` | whole-home (control; also overwrites the shm with the total region) |
| 5 | zone clean running, then `21023 {"cleanId":[<other>]}` | navigator pause/re-target/resume (the only thing 0x41b does) |

Note the persistence side-effect: after a zone clean the shm still holds the zone set, so a
later whole-home start must be `smartClean` (it rebuilds the total region) — and a later
"all stored regions" clean can be `21023 {"cleanId":[-2]}` + `appointClean`.

**Distant-zone abort?** If a stored-zone clean for a far room backs ~0.15 m off the dock and
re-docks within ~2 s with `errorState [-2605]`, that is `EID_E_CLEAN_CANNOT_ARRIVE` — the
navigator's target search failed over the current map/segmentation state (no distance branch
was found; ranked coverage causes and next steps in `ZONE_CLEAN_ROUTE.md` §3/§7). See `ZONE_CLEAN_ROUTE.md`.

## The app side (what the vendor app really sends) — verified from the APK

* **`transitCmd "164"`** carries `cleanArea` = JSON list of `MapArea`:
  `{"area":"<left>,<top>;<right>,<bottom>","sign":"<epoch-millis>"}` — four **integer** corners
  in the app's 0–1000 map frame (`PartitionView.getMapcoordinate`, clamped 0–990/1000). No ids,
  no names, no millimetres. `forbiddenArea` (166) uses the same list type.
* The app **does not start anything with 164** — its success callback only re-fetches the map.
  The start is the ordinary start button → `transitCmd "100"` with **no parameters**, a separate
  user action. The vendor cloud therefore converts the rectangles into robot regions and chooses
  the start itself. That the cloud sends `21023` + `appointClean` for this flow is
  **[inferred]** (no vendor capture exists) — but it is the only construction consistent with
  the robot code, since `smartClean` can never honour a selection.
* Zone rectangles return to the app in `MapDetailInfo.cleanArea` pushes in the same
  `"l,t;r,b"` string form.

## Evidence (LS_S6 0.7.1; task_manager = tm, network_proxy = np)

* **Vtable**: `TaskLogic` 0x424218 and `TaskLogicLSS1` 0x4269d0 — `[+0x110] = 0x417608`
  (`tm FUN_00417608`: arg 2 → sweep, arg 4 → CPS 4/0xb; the `0x1389` region push to the
  navigator is unconditional, while the `0x7d6` navigator pause/resume pair is emitted
  **only for arg 2**; log `"Active regions at work(CPS:%s)"`).
* **0x41b sites**: tm_all.c 14086 (sweep → arg 2), 14726 and 15199 (CPS 4/0xb → arg 4). Idle
  handler `tm FUN_00413840` never tests 0x41b. Start blocks from 0x410/0x411: tm_all.c
  13357 (idle), 13539 (dormant), 14017 (sweep), 14241/14277 (CPS 3/6/10), 15557/15638
  (charge/fullcharge).
* **smartClean overwrites**: np `FUN_0041b6f8` — `CreatTotalCleanRegion` (15092) →
  `ShmUser::Write` → log `"App start total clean"` (15364) → `PostEvent(0x413, raw 21005 JSON)`;
  tm's 0x413 consumer understands only `cover_mode`/`pathType`/`cleanMode` — no regions.
* **21023**: np `FUN_0041bdf8` — requires `cleanId` (`"ActiveRegions no cleanId"`), same id
  switch, `ShmUser::Write` (16181), `PostEvent(0x41b)` (16312).
* **appointClean/reAppointClean**: np `FUN_00411150`/`FUN_004111d8` — `PostEvent(0x410)`/`(0x411)`,
  no shm access, no payload. tm's 0x410/0x411 blocks start `clean_task` from the shm.
* **Schedule carrier**: `time_server_tactics` disasm 0x403d90–0x403ed0 builds
  `{"workNoisy":…,"cleanId":[…],"cleanNum":N}` and posts 0x420; tm `FUN_00410590` (= +0x100)
  resolves cleanId (default `[-1,-3]` when empty) into the shm, then the idle handler starts.
* **CleanArea shm**: key 103 (128000 B), bound at tm this+0x490 / np `DAT_004c57e8`; region
  stride 0x88 in memory, 0x74+8n packed. Region type at +0x6E: 1 total, 3 point, 4 curpoint,
  5 room, 6 smart (a type 2 also exists — `libcpc_area.c:1560`; meaning not pinned here);
  cover_mode +0x00, clean_mode +0x04, forbid_type +0x08.

## Uncertainty

* Static only — no live robot test of the corrected sequence yet.
* Positive `cleanId` ids are the stored area-file ids (`21004`); the app's zone ids are a
  different namespace (cloud-side mapping).
* Negative sentinels **confirmed in code** (tm `FUN_00410590` and np's switch agree):
  `-1` = `CreatTotalCleanRegion` (whole map); `-2` = all records, unfiltered; `-3` = records
  whose `cover_mode == 2` (forbid) only; `-4` = all records **except** `cover_mode == 2`.
  (`FUNC_MAP.md` §6.1's "-3 keeps stored no-go zones active" matches this.)
* `FUN_00419528` ("UpdateAllForbidRegionsOnly") rewrites the shm and posts 0x41b, but **no
  caller, data reference or relocation to it exists in this build — dead code** (its banner
  string is unreferenced too; `FUNC_MAP.md` §6.3's assessment stands).
