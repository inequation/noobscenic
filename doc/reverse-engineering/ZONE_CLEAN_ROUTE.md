# Distant zone clean aborts with −2605 — resolved 2026-10-10

**Symptom:** `21023 {"cleanId":[N]}` + `21005 {"mode":"appointClean"}` works for zones near the
dock, but for two rectangles in the right-hand room (~9–10 m) the robot starts `sweep`/`area`,
backs 0.15 m off the dock, and aborts within ~2 s with `errorState:[-2605]`, re-docking.
**Answer: the fault is `EID_E_CLEAN_CANNOT_ARRIVE` — the navigator's target search did not
find the far zone during the area clean's map-update window. It is a map-coverage condition,
not a distance limit.**

## 1. Fault identity (corrected 2026-10-10; supersedes the interim "lost pose" reading)

`errorState` indices come from network_proxy's **EID→index table** (file `0x89bf0`) joined to
the index→cloud-code map (`0x8e2f0`). The verified entry: **`4051 EID_E_CLEAN_CANNOT_ARRIVE`
→ index 23 → −2605** (full table: `FUNC_STATUS.md` §4.1). `EID_E_CLEAN_LOST_POSE` (4023) is
**absent from that table** — clean_task's "Lost position/map, clean done" path exists (four
`0xfb7` sites; live handlers in `CleanTaskLSS7`) but can never produce this status code.
Do not chase it for this symptom; it is a separate abort class (see §5).

## 2. Why it aborts (~2 s, 0.15 m) — the navigator flow, end to end [static]

1. `clean_task` starts the area job and sends navigator **`NCT_INIT`** (parser `FUN_004d0d68` →
   `CCleanTask::Init FUN_004afe40`) carrying `{mCleanSubMode, mIsSecondTotalClean, Pose2D, mMode,
   region records}` — region type 2 ⇒ forbid, else clean; per-region label = the rect id;
   coordinates included.
2. `CCleanTask::Run` stage 2 → `CreateSubRegion FUN_004b3e00` → **target search `FUN_004b0cf0`**
   over the occupancy grid (`FUN_0042f6c0(grid, map, 0x13)`) masked, for the smart submode, by
   the **segmentation-label grid** (`SegmentationMapShareMem`, connected-component flood fill).
3. `DoTransition FUN_004b2668` first runs a map-update wait — log
   **`"Try to update slam here 2.5 second"`** (≈3,000,000 ticks) — then **one** re-search. If it
   still finds nothing: `"CLEAN_SUB_MOD_SMART  No target Point "` (double space after `SMART`,
   trailing space) → label++ → stage 2; when labels
   and regions are exhausted (`reachTimes(+0x15c)==0`, submode ≠ 1) `CreateSubRegion` sets
   **stage 10**, and `CCleanTask::Run` case 10 posts **EID 4051** — the only site in the
   firmware that *posts* 4051 (`bl PostEvent` at navigator `0x4b5118`; libcpc.so merely has a
   `movz #0xfd3` for the name table).
4. np maps 4051 → **index 23** into `devattr errorState[0]`; the app-facing code **−2605** is
   produced from that index by np's ReportError mapping (`FUNC_STATUS.md` §4.1).
5. `task_manager` (FUN_00414418) has a 4051 branch: stop `clean_task` → start
   `find_charger_task` — **exactly the observed "abort and dock again"**. No `-r` retry on
   this branch.

The 0.15 m back-off is a **hard-coded leave-charger step** (`{−0.15, 0, 0.2, 10.0}` sent to
navigator as part of the 0x7d6 step; 5 s timeout). The abort decision follows it — that is why
the failure always looks like "0.15 m, then fault".

**Timing:** the search wait alone is ~2.5 s; the observed abort is the same order (~2 s).
The tick unit is not verified, so treat the match as **approximate, not exact**.

## 3. Why near zones pass and the far room fails

*[Inference, ranked — not yet confirmed by a capture:]* the working zones sit in the
dock/corridor area, which the map and its labels demonstrably cover, and the binary's success
form is `"try trans success, go to true target pt"` (`@0x4dd798`). For the far room the search
finds **no target cell** in its mask during the window. The
implementor's flood-fill test over free cells does not contradict this: the search is not a
reachability test — it looks for a *target cell* in the map/label grids the planner actually
uses, and that state evidently lacks a usable target there.

Candidate causes (ranked, not yet distinguished — all are map-state, none is distance):
1. **No segmentation label over the far room** in the current `SegmentationMapShareMem`
   (the smart-submode search is restricted to the label grid; unlabelled area = no target).
2. The far room is missing/unknown in the robot's **current** map even though the latest
   uploaded 20002 raster shows free cells (the upload can be stale relative to live state, or
   the planner's cleaned/inflated mask differs).
3. A label exists but its connected component is empty in the planner's mask
   (`"cant find clean area!!!"` is the log for a label with no cells).

**Self-serve check #1 (no robot access needed):** inspect the newest 20002 `map` raster at the
far-room cells vs the working near-zone cells — the segmentation pass overwrites free `0xFF`
cells with the non-zero room-label byte. If the near zones sit on **labelled** cells and the far
room is still plain `0xFF` (or a single label with no connected body), hypothesis 1/3 is it.
**Self-serve check #2:** pull the device log during a failing far-zone run and grep the strings
in §4 — one run settles which cause.

## 4. Device-log greps (decisive; ordered)

If the cannot-arrive path fires (expected), grep **short fragments** (the full strings carry
irregular spacing):
`Try to update slam here 2.5 second`, `No target Point`, `cant find clean area`,
`Get mSegmentationMap failed` (the last one ⇒ stage 8 at Init — segmentation map missing
entirely); the trap-deal path (which raises the *search-over* event 6006, not 4051) shows
`Small Area Clean Done` / `Give up try`.

If instead you see (a **different** abort class, no −2605 in that case):
`"Leave charger finished. "` + `"No map data"` + `"Lost map, clean done. CleanSubMode:%d"`, or
`"Location fail, lost pose"` / `"Location time out, lost pose"` + `"Lost position, clean done"`
— that is clean_task's lost-map/lost-pose mechanism (4023), driven by `has-map-data`
(slam snapshot) × `mResumeCleanning` (task_manager resume), **not** by the target.

## 5. The parallel mechanism (clean_task lost-pose/map) — context only

For completeness (from the parallel trace): the live handlers are `CleanTaskLSS7` slots +0x48
(leave-charger, "Lost map") and +0x50 (location, "Lost position"). The live "Lost map" fires iff
`[+0x1e6] has-map-data == 0` **and** `[+0xfc] mResumeCleanning == 1` — if the resume flag is
clear it returns "rebuild map" and does *not* fault. `has-map-data` is fed solely by
slam_pose_provider's snapshot (`[slam+0xE40]`) *per the parallel trace — no direct store exists
in clean_task, so treat as strong inference*; `mResumeCleanning` only by task_manager's resume
message (`0x7d8`, armed by its persisted resume state — self-reinforcing across failures). No
target-coordinate dependence exists in this path.

## 6. Answers to the request's other questions

- **Is there a distance limit?** No branch on dock distance was found in the examined
  functions — targets have `mMaxTime=-1/mMaxReplanTimes=-1`; the only
  budgets are the 2.5 s smart search window, a 120 s point-target/trap budget and a 4.5 m
  bbox shortcut.
- **Does the vendor send anything extra for distant zones?** No robot-side branch found; the
  zone kind only changes the payload shape (region records with rect id/label/coords) and is
  consumed identically. The differentiator is map/label coverage.
- **LD event 6023?** Solved, unrelated: np's LD report table maps `EID 1080
  (EID_I_CLEAN_TASK_FINISHED)` → LD index 7 → 6023 — "clean task finished", informational.
- **`21029`/`21030` priming?** Not required by the code path; but re-running segmentation
  (`21030 reset`) is the natural way to fix a missing-label condition (see below).

## 7. Suggested next experiments (in order)

1. Grep the newest 20002 raster for label bytes at the far room vs the working zones (§3 check 1).
2. Pull one failing-run device log; grep §4 (§3 check 2). This distinguishes causes 1–3.
3. If labels are missing: `21030 {"operate":"reset"}` (re-segment on the plan map), force a map
   upload, then retry the same far-zone clean. If that fixes it, the condition is "zone outside
   the current segmentation".
4. If not: try the same rectangle sent as an inline **`extraAreas`** region (kind 2) — if it
   then cleans, the stored-`cleanId` region's mapping into the label grid is the difference.
5. If the far room is simply not in the current map: run one whole-home or manual clean covering
   that room (so the robot maps/segments it), then retry.

## Evidence & uncertainty

Traces: `tmp/review_route/a2/NAV_TARGET.md` (navigator: target search, stage 10 → 4051, the only
*post* of 0xfd3 — libcpc registers the name, task_manager only compares; LD 6023) and
`tmp/review_route/a1/CLEAN_TASK_LOST.md` (clean_task: live handlers, the
0.15 m step, site-4 condition). Fault table: `FUNC_STATUS.md` §4.1. Uncertain: which of causes
1–3 holds (needs the log/raster check); `GetCurrentTick` unit; the segmentation producer/label
numbering; app-side region-kind→payload construction (clean_task side verified only).
