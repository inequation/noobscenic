# RE request — is an area clean restricted to the robot's *current* segmentation label?

**From:** the noobscenic implementation side. **Date:** 2026-10-10 · **Unit:** SN
`LSLDSM7PRO20403551` · **Server:** `192.168.1.208`, ports 8080/8081.

## What we now know (follow-up to `ZONE_CLEAN_ROUTE.md`)

The `-2605` = `EID_E_CLEAN_CANNOT_ARRIVE` reading is corroborated by six runs from the
dock with the corrected `21023` + `appointClean` sequence:

| Zone (world mm, 0.8–1.6 m rectangles) | Distance from dock | Label of its cells | Result |
|---|---|---|---|
| operator's `Biurko` (−7373…−5315 × 2677…4093) | 3.2 m | `0x01` | cleaned |
| (−5800…−4800 × 800…1800) | ~0 m | `0x01` | cleaned |
| (−4600…−3600 × 700…1700) | ~1.3 m | `0x01` | cleaned |
| (−400…400 × −1600…−800) | 5.6 m | `0x01` | cleaned |
| (800…1600 × 0…800) | 6.6 m | `0x01` | cleaned |
| (3800…4600 × −2400…−1600) | 9.6 m | `0x02` | aborted, `-2605` |
| (4800…5600 × −2200…−1400) | 10.4 m | `0x02` | aborted, `-2605` (also as inline `extraAreas`) |

Two further data points: `21030 {"autoAreaId":0,"operate":"reset"}` ran fine and the
following 20002 upload carried segmentation labels for the first time (19 390 labelled
cells; before it, **zero**), but the far zone still aborted. The corridor's label
boundary sits at x≈2400, i.e. **7.8 m** from the dock — the nearest `0x02` cell — while
`0x01` free space ends at ≈7.5 m, so from the dock "another room" and "beyond ~7.5 m"
are indistinguishable.

## The question

1. Is the area-clean target search (`CCleanTask::CreateSubRegion` → `FUN_004b0cf0`)
   masked to the **label of the cell the robot starts on** (or to a specific label the
   clean_task was given), rather than to "any labelled cell"? The six runs are
   consistent with that and with nothing else we can vary.
2. What does the vendor stack do for a zone in **another room**? If the robot must be
   in that room first, what makes it travel there — does the app/cloud start a clean
   that naturally reaches it, or is there a move-to-area step we have not found?
3. If it is not the label: what in the target search depends on the robot's position at
   start (a search box around the pose, the "4.5 m bbox" in `ZONE_CLEAN_ROUTE.md` §6,
   the leave-charger step, …)? The 6.6 m success rules out a 4.5 m radius around the
   robot, and an 8.5 m label-`0x02` test would still be confounded with a ~7.5 m
   distance threshold in this map.

**Decisive experiment we can run on request:** drive the robot into the right-hand room
(or stop a whole-home clean once it is there), then clean (a) a zone in that room and
(b) the operator's `Biurko` zone back in room `0x01`. Label-bound ⇒ (a) works and (b)
fails; distance-bound ⇒ both work (the robot would be ~10 m from Biurko).

**Evidence:** `FIELD_NOTES.md` ("Zone clean: the far-zone abort is about *rooms*…",
2026-10-10) and `var/traces/wire-2026-10-10.jsonl`.
