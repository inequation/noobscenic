# Cross-room zone clean — mechanism (2026-10-10)

**Their question (#2, still open):** what does the vendor stack do for a zone in **another
room** — is there a move-to-area step, or a start that travels there?

**Answer:** **No move-to-area step exists anywhere** (a sweep of the binaries' command/EID space found no
zone-clean command that drives to an area; the `goto`-ish strings that *do* exist are internal
task states/logs — navigator `0xd9000`/`0xdde58`, clean_task `0x1b650`, remote_control_task
`0xff58`, find_charger_task `0x322c0`/`0x328e0` — none reachable as such a command; the
trace-location start event has no poster in this build; the only clean-free motion reachable
from a message is manual remote control, EID 0x466). The **designed
cross-room mechanism is the segmented-room path**: `21023 {"cleanId":[…],"segmentId":[<label>]}`
(the handler still requires the `cleanId` key) builds **kind-6 “smart” regions** →
task_manager **submode 7 (`--smart`)**. Mode 7 targets by the **requested** labels (the label
list is built from the region records' ids; ids outside the 1..50 validity range fall back to
*all* labels 1..49 = whole home) and its **transit is label-agnostic A\*** — the robot drives
across any labels to the room; the robot's current label is used only as the **arrival**
predicate (checked ×5).

**Shape of the machinery (all [static], see `tmp/review_room/ROOM_TRAVEL.md`):**

| | submode | mask on the target search | arrival |
|---|---|---|---|
| point / **area** / curpoint / room (and updating / depthTotal) | 2 / **3** / 4 / 6 (5, 8) | target **polygon** on the planner's cleanable grid — **no label restriction** | reaching the polygon |
| total | 1 | own path (`4b44d0` → `FUN_00499230`); never reports cannot-arrive | shared reached-count arrival, not separately traced |
| smart (kind-6, `segmentId`) | 7 | **target label's component** (BFS cluster mask + label-filtered grid) | `labelGrid[robotCell] == labelVec[labelIdx]` ×5 |

Priority total > smart > room > curpoint > point > area. Transit in both classes is
label-agnostic A\* (`0x36a "Go to Target Point First !!!"`, `0x37f "----- point target -----"`,
`0x4ad "try trans success, go to true target pt"`; mode-7 retry `0x4a6 "Try to update slam
here 2.5 second"`). Exhaustion with nothing reached → state 10 → EID 4051 (`-2605`) **unless**
submode is 1 (total) or 4 (curpoint — it navigates to the stored Pose2D at `+0x140`, state 12;
plausibly a go-home, but the writers of `+0x140` are untraced).

## The honest conflict with the live “label” conclusion (their Q1)

The live cross-boundary test (robot parked inside label `0x02`, target 2.6 m away in `0x01`
aborts; same-label target cleans) **cannot distinguish**:

* **(a) a current-label mask** — their reading; or
* **(b) a boundary/grid effect** — in this map the label boundary sits at the mapped passage
  between the rooms (x≈2400) *[inference]*, so “target in the other label” also means
  “path crosses the doorway”; the failing state-10 tail fires when nothing is
  reachable/arrival-satisfiable, which a pruned/blocked passage at the doorway produces
  identically (and the whole-home run that never headed east to the zone — it did move east,
  −5.4→−3.1 m — is consistent with it too).

**Static analysis does not support (a) for the area mode**: the only current-label use in the
entire task is mode 7's arrival check; the area-search mask is the target polygon, with no
label filtering (`ZONE_CLEAN_ROUTE.md`'s implementor-facing note should therefore be treated
as *one of two readings*, not settled). Note an area zone in another room **should** be able
to travel per this analysis — so the far-zone aborts are more likely the cleanable-grid/pruning
side of state 10, unless the end-to-end test below contradicts it.

## Discriminating tests (in order)

1. **Room path from the dock (both the recommended mechanism and a decisive probe):**
   `21023 {"cleanId":[…], "segmentId":[2]}` then `21005 {"mode":"appointClean"}` while docked.
   Expect the robot to leave the dock, transit through the corridor into the right-hand room
   and clean it. Watch the status `subMode` string for the run (expect a different value than
   `"area"`). If this succeeds, the door/passage is fine on the label-agnostic transit, and
   the vendor answer is “send rooms as `segmentId`”. If it fails identically at ~2 s, passage
   / cleanable-grid is the blocker, not labels.
2. **Sharpened cross-boundary pair (their original design, not yet run exactly):** park at
   ≈(2800,−80) (label `0x02`); target the small 160×200 mm zone at (2200…2360, −280…−80)
   (label `0x01`, ~0.44 m from the park point), plus a **distance-matched** same-label (`0x02`)
   control within ~0.5 m of the park point. A cross-boundary abort at this distance while the
   zone's cells are free **and labelled** on the newest 20002 raster is the strongest evidence
   for a mask (a <0.5 m free transit failing on the planner alone would be surprising).
3. **Reachability falsifier for every failed target (one-directional):** inspect the 20002
   raster bytes at the target cells. The 20002 raster is **not** the planner's cleanable/label
   grid (its cell semantics are [inferred] in `FUNC_MAP.md` §2.3), so: not-free/not-labelled
   in the raster ⇒ the target is definitely pruned (state 10 explained, no mask); free in the
   raster does **not** prove it is cleanable in the planner's grids.

## What to watch in a device log

`0x36a`/`0x37f`/`0x4a6`/`0x4ad`/`0x4ed "Reached User Region"` (progress) vs
`0x36e "…SMART  No target Point"`/`0x4b3` (smart no-target, advances label),
`0x2af "mCurPolygonRegionId = %d, mCleanAreas.size() = %d"` + state 10 → `0x0fd3`
(cannot arrive). Landing on `0x4b3` in **smart** mode means the smart path tried labels and
found none; landing straight in state 10 from an **area** job means the polygon search found
no cleanable target cell.

## Gaps

Mode 5/8 (updating/depthTotal) share the generic polygon path (behavior untraced beyond the
dispatch); the fallback seed-cell fields (+0x1c0/+0x1c4) and the EID-0x466 poster at navigator
`0x483554` not chased; untraced whether remote_control_task's internal "goto location" state is
reachable from any app command (if it were, it would qualify "the only clean-free motion is
manual RC" — no command found that reaches it); no live verification of either test. Full
evidence: `tmp/review_room/ROOM_TRAVEL.md` (§§1–4, disassembly-cited).
