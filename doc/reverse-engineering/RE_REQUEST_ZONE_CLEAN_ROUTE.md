# RE request — why does a *distant* zone clean abort with fault -2605?

**From:** the noobscenic implementation side (clean room: our code derives from
`doc/reverse-engineering/` only). **Date:** 2026-10-10 · **Unit:** SN
`LSLDSM7PRO20403551`, model `6716` · **Server:** this workstation, `192.168.1.208`,
ports 8080/8081 · **Robot:** `192.168.1.243`.

## TL;DR

`ZONE_CLEAN.md`'s corrected sequence (`21023 {"cleanId":[N]}` then
`21005 {"mode":"appointClean"}`) is live-confirmed — `subMode` becomes `"area"` and the
robot travels to and cleans a stored zone, then docks itself. It works for zones at the
dock and ~1.5 m away. For two different rectangles in the right-hand room (~9–10 m
away) the robot starts `sweep`/`area`, undocks ~0.15 m, **aborts within ~2 s and docks
again**, with `errorState:[-2605]`. We need the robot-side reason.

## Exactly what we sent (all from the dock, all ACKed "ok")

```
{"encrypt":0,"data":{"infoType":21023,"data":{"cleanId":[N]},"dInfo":{...}}}
{"encrypt":0,"data":{"infoType":21005,"data":{"mode":"appointClean"},"dInfo":{...}}}
```

with `N` the id of the only stored region (`21003`/`21004` round-trip verified). Zone
rectangles (world mm): near (−5800…−4800, 800…1800) → **cleaned**; mid (−4600…−3600,
700…1700) → **cleaned**; far (4800…5600, −2200…−1400) and (3800…4600, −2400…−1600) →
**abort**. Both far rectangles are 100 % `0xFF` free cells in the newest 20002 map, and
a 4-connected free-cell flood fill from the dock reaches them, so the abort is not
explained by unknown cells blocking the route.

## What we need

1. **Who raises fault index 23** (the status `errorState` value `-2605`,
   `FUNC_STATUS.md` §4.1) and under what condition? Is it a carrier/navigation
   ("no route to the target area"), a task/bin/dock fault, or something else? The
   family guess in §4.1 is low confidence and this fault is the whole question.
2. If it is a route/planning failure: which graph does an **area** clean plan over —
   the same transit graph a whole-home clean uses, or the cleaning graph / the map's
   segmented areas? Is there a distance limit or a "target must be in a known
   cleanable area" precondition for `appointClean` after `21023`?
3. Does the vendor cloud do anything extra for a **distant** zone clean that we have
   not sent (e.g. it first makes the robot travel/dock somewhere, sends the area as
   `extraAreas` instead of a stored `cleanId`, or primes `21029`/`21030`)?
4. For completeness: is LD event 6023 (index 7 in the `msg_report_ld` map) related to
   cleaning areas? It fires during successful cleans, so we assume not.

**Evidence on our side:** `FIELD_NOTES.md` ("Zone clean: live run of the corrected
sequence", 2026-10-10) and `var/traces/wire-2026-10-10.jsonl` (19003 command
pairs). The robot is docked and its zone list is empty.
