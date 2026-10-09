# RE request — how does a *zone clean* actually reach the robot?

**From:** the noobscenic implementation side (clean room: our code derives from
`doc/reverse-engineering/` only; this request asks for more of that kind of evidence).
**Date:** 2026-10-09 · **Unit:** SN `LSLDSM7PRO20403551`, model `6716` · **Server:**
this workstation, `192.168.1.208`, ports 8080/8081 · **Robot:** `192.168.1.243`.

## TL;DR

The zone *editor* works end to end (we write the robot's `AreaSetting` with `21003` and
read it back verbatim with `21004` — a stored region round-trips exactly). But no
variant we can derive of `21023` produces a *zone-restricted* clean. We need to know
what the vendor stack actually sends for "clean this zone/area" (`transitCmd 164` +
`cleanArea` app-side), or what precondition the robot's `task_manager` needs for the
active regions (`EID 1051`) to take effect.

## What we tried (three runs over two sessions, all with the robot docked first)

1. Write a cleanable region (`active:"normal"`, `id:1`, a 0.8×0.8 m rectangle in the
   right-hand room at world (4.8–5.6 m, −2.2–−1.4 m)) with `21003`, `mapId` preserved
   from the robot's own list (`-1`). `21004` echoed it verbatim.
2. `21023 {"cleanId":[1]}` → **ACKed ("ok"), no start**: mode stayed `fullcharge` for
   12 s.
3. Followed with `21005 {"mode":"smartClean"}` → the job started within ~5 s, but the
   robot cleaned the corridor **next to its dock** (x ≈ −5.4 → −3.1 m), meandering with
   `cleanArea` ~1 m², never travelling to the stored region ~10 m east.
4. Repeated with `mapId` set to the **current map id** (`1791385091`) — identical
   outcome, so the `mapId` hypothesis is disproven.
5. Sent `21023` with the region **and** an inline `extraAreas:[{…,"mode":"area",…}]`
   placed at a *different* spot (west of the dock, x ≈ −6.7…−6.1 m) plus `cleanId:[1]`,
   then started with `smartClean`: the robot cleaned near the dock again — neither the
   stored region nor the extra area was visited. (If it had gone west, `extraAreas`
   worked; east, `cleanId` worked.)

In every run the robot behaved as if it were doing an ordinary whole-home clean
starting from the dock. `21023` is listed as "Index only" in `FUNC_COMMANDS.md`, so the
gap is expected — we just cannot close it from the outside.

## What would settle it

* **The app→cloud half of `transitCmd 164`:** what exactly does the app put in
  `cleanArea` (a `MapDetailInfo` area? a region polygon? an id?), and does the app
  follow it with a start (`transitCmd 100`) or is the start implied?
* **The robot-side reception:** trace `task_manager`'s handling of `EID_I_ACTIVE_REGIONS`
  (1051) on an **idle/docked** robot — the docs say it calls a virtual with argument 2
  when idle and 4 when working. Does argument 2 start a job? If yes, what CPS state
  must the robot be in (docked `charge`/`fullcharge` vs `idle`)? Our robot was docked in
  every run; maybe `21023` only "arms" a job that is started by a *different* cloud
  message than `21005 {"mode":"smartClean"}`.
* **Any other field `21005` reads** for a targeted clean (the handler passes the whole
  `data` object on), and whether `21029`/`21030` (auto-area map / segmentation) must be
  in play for `segmentId` targets.

## What we need from you (RE agent)

1. The exact robot-side frames a vendor **zone/area clean** produces (from a cloud-side
   or app-side capture, or from tracing `task_manager` around `EID_I_ACTIVE_REGIONS`).
2. If the answer is "the cloud sends 21023 then starts it", the precise order, timing
   and start message — including whether the robot must be undocked/idle first.
3. The `cleanArea` schema the app builds, so we can mirror the same region descriptor.

**Evidence on our side:** `doc/reverse-engineering/FIELD_NOTES.md` ("Zone editing over
channel B", 2026-10-08/09) and the wire traces in `var/traces/wire-2026-10-0*.jsonl`
(commands 50/52, 69/71, and the `21003`/`21004` pairs).
