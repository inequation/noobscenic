# RE request — can Y-shaped mopping be restricted to a selected zone/room?

**From:** the noobscenic implementation side (clean room: our code derives from
`doc/reverse-engineering/` only; this request asks for more of that kind of evidence).
**Date:** 2026-10-18 · **Unit:** SN `LSLDSM7PRO20403551`, model `6716`.
**Current deployment:** the robot is re-homed to the router-hosted instance
(`192.168.1.1:8080`); the old workstation server is retired and nothing here talks to
the robot. Live probing is therefore a manual, operator-gated step.

## Why we are asking

The web UI exposes Y-shaped mopping as a whole-home start:
`21005 {"mode":"smartClean","pathType":"y_word"}`. We would like to offer the same
pattern for a **selected stored zone or room** (and, if the model allows it, an inline
`extraAreas` region), without falling back to "clean the whole home in a Y".

The static evidence says the two start paths are mutually exclusive: `smartClean`
carries and honours `pathType`, but it synthesizes a whole-map total region and
overwrites any prior `21023` selection; `appointClean` starts from the selected
regions, but posts EID `0x410` with **no payload**, so an extra `pathType` is dropped.
We need to know whether that conclusion is complete, or whether a hidden seam exists.

## What we know (evidence)

* **`21005` dispatch** (`FUNC_COMMANDS.md` §1.1): `"smartClean"` = whole-home total
  clean (`FUN_0041b6f8`), synthesizes the total region into the CleanArea shm
  (**overwrites any `21023` selection**) and posts EID `0x413`; the whole `data`
  object is passed on, so `pathType:"y_word"` is honoured. `"appointClean"` =
  start a clean from the current CleanArea region set (`FUN_00411150`, EID `0x410`,
  no payload); `"reAppointClean"` is the same via `0x411`. `"depthTotalClean"` builds
  `{"cover_mode":1}` and copies `pathType` — again whole-home.
* **Zone selection** (`21023`, `FUNC_MAP.md` §6 / `ZONE_CLEAN.md`): `cleanId` (stored
  regions), optional `extraAreas` (inline `mode:"area"` / `"point"`) and
  `segmentId` are folded into the shm. The subsequent `appointClean` derives the
  sub-mode (`--area`, `--room`, …) from those region records.
* **Region data model** (`FUNC_MAP.md` §4): `vertexs`, `active`
  (`normal|depth|forbid`), `forbidType`, `cleanType`, `workNoisy`, `name`, `tag`,
  `id`, `mode` (`default|area|room|point|curpoint`). **No path-pattern field is
  documented.**
* **rc `3024`** (`FUNC_COMMANDS.md` §4.4): with a mop fitted it posts
  `{"mode":"smartClean","pathType":"y_word","cleanMode":…,"from":"carrier"}` — the
  same whole-home start. Without a mop it raises event 4043 and does not start.
* **Event consumers** (`ZONE_CLEAN.md` §"What we now know"): the `0x413` consumer
  understands `cover_mode` / `pathType` / `cleanMode` but **no regions**; the `0x410`
  start block reads the shm region set and starts `clean_task -t <submode>`.
* **`smartAreaClean`** (`21005 {"mode":"smartAreaClean"}`, `FUN_0041afb0`) runs
  auto-segmentation and then cleans room by room. We have not checked whether it
  reads or forwards `pathType`.
* **Mid-job retarget** (`ZONE_CLEAN.md` test #5): while a clean runs, a new `21023`
  causes navigator pause / re-target / resume. Whether that preserves a pattern
  already baked into the running task is unknown.

## Questions

Please answer each with a yes/no and the function/struct evidence; correct any of the
"what we know" bullets above if they are wrong.

1. **Is there any path-pattern input on the `appointClean` path?** Does `FUN_00411150`
   / the `0x410` start block / `clean_task` consult a path-pattern source *other* than
   the `0x413` event — for example a field in the CleanArea shm region records, a
   task_manager config file, or a global (`PathType`, `path_type`, …) — when it starts
   an `--area` or `--room` job? If yes, what stores it and how can an inbound
   cloud→robot message set it?
2. **Does the region parser accept a path-pattern key?**
   `AreaProcess::CreateRegionPolygonFromJson` (or the `21023` handler `FUN_00473528`
   and its `0x41b` writer): are there any additional per-region keys parsed beyond the
   documented ones — e.g. `pathType`, `path`, `workMode`, or `mode` values beyond
   `default|area|room|point|curpoint`? Are unknown keys ignored, preserved, or
   rejected? Could an inline `extraAreas` region carry `pathType:"y_word"` and have
   the task manager honour it?
3. **Does `clean_task` accept a Y-pattern flag on its command line?** The `0x410`
   block invokes `clean_task` with `-t <CleanSubModeStr[submode]>`. Is there another
   argv flag (or an environment/config input) that selects the Y pattern, and is the
   argv built anywhere that could include it for an area/room start? If the pattern is
   hard-wired per sub-mode, say which sub-modes imply Y.
4. **Does `smartAreaClean` honour `pathType`?** Could
   `21005 {"mode":"smartAreaClean","pathType":"y_word"}` produce room-by-room Y
   mopping? If yes: how does it choose rooms (segmentation labels, stored regions,
   `extraAreas`), and does it still clean the whole home room-by-room, or can the
   set be restricted?
5. **Does a mid-job `21023` retarget preserve the Y pattern?** What exactly does the
   navigator re-target change — the CleanArea shm only, or the already-built
   `clean_task` state? Could the sequence "start
   `smartClean` + `pathType:"y_word"`, then `21023` with one zone" yield a Y pattern
   confined to that zone, or does the retarget either restart a plain area job or
   keep the original total region?
6. **The internal schedule start (`0x420`)** takes
   `{"workNoisy":…,"cleanId":[…],"cleanNum":N}`. Does its consumer read `pathType`
   (or any other extra key), and is there any inbound Channel-B message that can
   inject that event/list? We understand the server cannot send `0x420` directly;
   this is to rule out a schedule-shaped route.
7. **If no zone-restricted Y exists, please state it explicitly** and name the layer
   that would have to change (region parser / `clean_task` / task_manager) so we can
   record the limitation as settled rather than "unexplored".

## Optional live probes (only if static analysis cannot settle it)

Both start the robot and are noisy, so they are operator-gated. The current router
deployment means we will not run these from here.

* **A — pattern on the zone starter:** `21023 {"cleanId":[<one stored clean zone>]}`
  then `21005 {"mode":"appointClean","pathType":"y_word"}`. Static evidence predicts
  the extra key is dropped and a normal `subMode:"area"` job runs. Useful
  observations: `subMode`, the track's shape, `cleanArea`, and whether any log line
  mentions `pathType`.
* **B — retarget a Y start:** `21005 {"mode":"smartClean","pathType":"y_word"}` and,
  once the job is running, `21023 {"cleanId":[<zone>]}`. Observe whether the track
  stays a Y confined to the zone, falls back to plain area cleaning, or reverts to
  whole-home.

## What we will do with the answer

If a zone/room Y is possible, we will add it as a per-selection option (a Y toggle in
the zone picker/editor or a Y variant of the placed-spot square, depending on which
seam exists). If it is not, we will record the constraint next to the whole-home 🇾
drawer button and stop treating it as an open question.
