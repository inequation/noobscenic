# Field Notes — live re-home attempt vs. the analyzed firmware

> **CORRECTION (2026-10-04, late) — supersedes the original premise of this file.**
> The original version concluded the physical unit ran *different firmware* because a UDP
> sweep found Channel C closed. **That conclusion is retracted.** The unit runs the analyzed
> `network_proxy` image (LDRobot CleanPack / `LS_S6` base, app 0.7.1, `versionCode` 1241):
> * the live unit *served* Channel C end-to-end (`getSn`, `setUrl`, `setSta`, `applyCfg`,
>   `setID`, `getLog`) on **UDP 7913** — the port hardcoded in the analyzed image
>   (`FUN_00468a70`, `sin_port = 0xe91e`);
> * the log file it produced (`/tmp/WifiConfLog`) carries the analyzed build's exact log
>   literals (`StartBind`, `SetCusDomain`, `StartRecord wifi_info`, `p_s:%d->%d`,
>   `w_c(%s):%d->%d`, `buf2Json Fail`, `invalue cmd`);
> * the earlier sweep only probed `9000–9407` (the *sample's* random RemoteCtrl range) and
>   missed 7913 — a wrong-port false negative.
>
> The full live session is decoded in **`PAIRING_LOG_ANALYSIS.md`** (log timeline, the bind
> state machine, the `bindOk` fork-storm bug, and the corrected rehome order). Read that
> first; this file keeps only the LAN-capture facts that remain true.

Marking: **[field]** = observed from the live capture (`robot.pcapng`, a NIC-level pktmon
capture reframed from 802.11 → L3); **[static]** = from the firmware analysis.

## Corrected unit profile (2026-10-04)

| Property | Analyzed `LS_S6` image **[static]** | Physical `6716` unit **[field]** |
|---|---|---|
| Firmware | `network_proxy` 0.7.1 / `versionCode` 1241 | **same build** — identical log literals, identical 7913 constant, all commands matched |
| Pairing SSID | `LDRobot` (sample value) | vendor-branded, `Proscenic-6716_<serial>` |
| Soft-AP gateway IP | `192.168.78.1` | `192.168.78.1` — matches |
| dnsmasq on the AP | yes (`apDemo`) | yes — TCP/UDP :53 confirmed |
| Channel-C UDP `{"cmd":...}` listener | **UDP 7913** hardcoded (`sin_port = 0xe91e`) | **UDP 7913** — verified end-to-end (the `9000–9999` range is a *different*, cloud-triggered RemoteCtrl socket) |
| Channels A/B | REST + framed-TCP push gateway | target URL/ip:port were set successfully (`:8080` / `:8081`) and the bind was armed with `setID`; **end-to-end connect still unconfirmed** — see `PAIRING_LOG_ANALYSIS.md` §7 |

## What the 2026-10-03 capture proved (and what it didn't) **[field]**

Capture = 3375 frames, 12 s, during `rehome.py discover/info` against `192.168.78.1`.

* The `{"cmd": "getID"}` probe was sent 3048× and unanswered — because **`getID` is not a
  real command** on this firmware (it replies `{"result":"invalue cmd"}` when it is reached
  at all; the `ok/type:ipfromapp` reply belongs to `setID`). Use `getSn` to identify a unit.
* The ICMP port-unreachables returned for `9000–9407` mean only that nothing is bound in
  *that* range. Since the real listener is on 7913, the sweep's "Channel C absent" conclusion
  was a wrong-port false negative.
* The only TCP the robot accepted was `:53` (dnsmasq TCP-DNS). It emitted no discovery
  beacon or mDNS.
* Coverage gap in that attempt (still worth knowing): probes were unicast to `192.168.78.1`
  only; no broadcast was tried.

## What the 2026-10-04 live session added **[field]**

A complete provisioning + bind session against the owner's servers, captured via the robot's
own log (`device-log.bin`, 1.4 MB):

* `setUrl` (both forms), `setSta` + `applyCfg` succeeded — the robot left its AP, joined the
  owner Wi-Fi (`wlan0 = 192.168.1.243`), and remained reachable on 7913 from the LAN
  (the listener survives the switch to STA until `network_proxy` restarts).
* `setID` armed the bind; the robot's reachability recorder confirmed baidu + google + the
  **owner's server host** all reachable at that moment (`p_s:0->7`).
* 19 s later the toolkit sent **`bindOk`**, which closed the local socket *without stopping
  the listener* — see `PAIRING_LOG_ANALYSIS.md` §5 — spawning the `buf2Json`/`invalue cmd`
  storm (~80 forks/s) and ending the local channel for the rest of the process's life.
* The bind never completed (pairing LED stayed on); the WifiConfLog cannot show the
  cloud-side attempt — that evidence lives in the daemon's **run log** (see §7 of the
  analysis file for the exact strings to grep).

## What the 2026-10-05 re-run added **[field]**

After a power-cycle, with the replacement servers up on the target network throughout, a
second `device-log.bin` (2,856 B) repeats the same picture — and this time **without a
`bindOk`**, so no storm:

* Provisioning again succeeded end to end: `setUrl url` + `setUrl ip/port` → ok, `setID`
  → `StartBind` (bind armed), `setSta`+`applyCfg` → robot joined (wlan0 = 192.168.1.243),
  and its ICMP recorder confirmed **baidu ✔ google ✔ 192.168.1.208 ✔** (`p_s:0->7`,
  `ip:->192.168.1.208`).
* Yet **zero TCP/HTTP at either server** — same as 2026-10-04.
* Static analysis since (2026-10-05) pinned the boot→dial chain (`PAIRING_LOG_ANALYSIS.md`
  §10): the heartbeat thread starts at boot and its *first* possible packet is the TCP SYN
  to `ip_port.json`'s target, which must follow within ~1 s of that file parsing; if the
  file is rejected it instead polls `GET <base>cleanPack/getSockAddr…` every ~1.2 s (visible
  HTTP), and if the base URL hasn't been rebuilt since boot those polls are silent.

## The resolution (2026-10-05, evening) — two independent causes **[field]**

**Cause #1 — the server host's firewall was silently dropping the robot's inbound TCP.**
Everything on the robot side was correct — and, as it turned out, it *was* dialing: its
SYNs to `:8081` left the robot every ~6 s the whole time. The Windows host had an inbound
allow rule for `noobscenic.exe` (Channel A) but **none for the Python process on :8081**,
so those SYNs — and anything Channel A would have seen before the link came up — vanished
with **no RST and no server-side log**. ICMP was allowed, which is why the robot's own
recorder happily showed `192.168.1.208 ✔` while every TCP attempt died en route.
Once the rule was added, the robot connected within seconds.

* **Lesson:** "the server heard nothing, but the robot is reachable and pings fine" is
  the signature of a silent inbound drop — check the host firewall *before* suspecting
  the robot (a host-side capture would show the SYNs arriving and being dropped).
* Note the asymmetry that hid it: probes *from the VM* reach both servers (VirtualBox NAT
  delivers them as local traffic, which inbound rules don't filter) — so the servers
  looked healthy from every vantage except the robot's.

**Cause #2 — the robot's own pairing log froze at 13:53** (its runtime filesystem
filled). `/tmp/WifiConfLog` stopped accepting lines mid-session; writes only succeed into
space that was just freed — `{"req":"rmLog"}` deletes it, the next `getLog` re-creates it
with `No Cmd Before` + the identity bundle, and nothing else appends (not even a 2-line
command reply). **A frozen pairing log does NOT mean "nothing happened"**: during the
whole silent period the heartbeat was dialing every ~6 s — it just couldn't narrate it.
(Also: `getLog`/`rmLog` are the `{"req":…}` command style, *not* `{"cmd":…}`.)

**What the first successful Channel-A capture looked like** (19:56:14Z, within ~100 ms of
the Channel-B session coming up; exact bodies in `PROTOCOL.md` §A): `POST
/cleanPack/binding` (the preBind — `sn&ts&userId`, with `ts` repeating the SN) →
`{"code":0}` → *directly bind success* → EID `0x460` → **pairing indication ended**; plus
`POST /cleanPack/register` (with the RSA `sig`) and `POST /cleanPack/sync`
(`companyId=48 mcuVer=S6 version 0.7.1/1241 cloud=psnk`) — all answered by a plain
catch-all `{"code":0}`. No re-provisioning was needed — the robot had been ready all
along; only the firewall rule changed.

## Recommended next steps (2026-10-05 afternoon; superseded by the resolution above — kept for the next reset)

1. **On the server host, during a fresh attempt** — `ss -ltnp | grep -E ':8080|:8081'`
   (is `:8081` really bound, by the right pid? `rehome_server.py` defaults to **:20009**
   without an argv!) and `sudo tcpdump -ni any 'host 192.168.1.243'`. Interpret:
   * SYNs to `:8081` → the robot dials; the listener is the problem.
   * ~1.2 s-spaced `GET /cleanPack/getSockAddr?version=1&sn=…&companyId=…` → heartbeat
     alive but `ip_port.json` rejected; serve
     `{"code":0,"data":{"addr_list":[{"ip":"192.168.1.208","port":8081}]}}` (bypasses the
     local file entirely).
   * **Nothing at all** → the heartbeat produces no traffic; get a shell (§3) and read
     `/data/bin/Run/Config/ip_port.json` + `/tmp/Run/Log`.
2. If the LED still doesn't settle once traffic flows, pull the **run log**
   (`/tmp/Run/Log` on the device — *not* a greppable file elsewhere; the connect lines are
   `HBLog …` ZMQ events materialized there by `event_hub`) with the strings `mIpAndPort`,
   `TcpClient connect …`, `ip_port member is error`, `Send_Ping`, `no session` to see
   exactly where the chain stalled.
3. Getting a shell (UART; `REPORT.md` §5) remains the highest-value step: it unlocks the
   run log, `/data/bin/Run/Config/ip_port.json` inspection, and
   `echo ap > /data/cfg/wifi_mode` to re-enter AP mode without a factory reset
   (`PAIRING_LOG_ANALYSIS.md` §6).

## Steering directions for `21020` 3007/3008 (2026-10-08, [field])

FUNC_COMMANDS §2.1 lists 3007 as "rotate left" and 3008 as "rotate right", both with
the direction marked *[inferred]*. Driving the robot from the web UI's steering pad
shows they are the other way round on this build: **3007 turns it clockwise (right)
and 3008 counter-clockwise (left)**. The pad binds the glyphs accordingly — `↪️` →
3007, `↩️` → 3008 — and the buttons then turn the robot the way the arrows point.
Treat the §2.1 labels as swapped until someone re-checks them.

## Zone editing over channel B (2026-10-08, [field])

* `21004 {}` returns the stored list verbatim — on this unit `{"mapId":-1,"value":[]}`
  when empty. `21003 {"mapId":…,"value":[…]}` is accepted and ACKed ("ok"), and a
  written rectangle reads back **exactly** as sent, unsnapped.
* The robot's vertex snap rule could **not** be observed: 21004 echoes the stored JSON
  verbatim (as FUNC_MAP §5.2 warns), and a forced map upload (`21014 {}` → fresh 20002)
  still carried `"area": []` even with a region saved. The web UI therefore snaps
  nothing and enforces a two-cell (100 mm) minimum thickness instead, so no half-cell
  offset in either direction can collapse a zone.
* `21023 {"cleanId":[N]}` is ACKed but does **not** start a job on this build: mode
  stayed `fullcharge` for 12 s after the robot acknowledged it. Following it with
  `21005 {"mode":"smartClean"}` did start a clean within ~5 s.
* **But the one observation cleaned near the robot's starting position (~1 m² at
  x≈−3.7 m), not the 0.8×0.8 m rectangle drawn at (4.8–5.6 m, −2.2–−1.4 m)**, so
  whether `cleanId` resolved to the stored region is unverified. Hypothesis for a
  later session: the stored list's `mapId` (we preserved the robot's own `-1`) may
  have to be the current map id (`1791385091` here) for `cleanId` to resolve.
* **Follow-up (2026-10-09): the `mapId` hypothesis is disproven, and `extraAreas` does
  not work either.** With the region written under `mapId:1791385091`, `21023
  {"cleanId":[1]}` + `smartClean` again cleaned the corridor near the dock
  (x −5.4 → −3.1 m over 70 s, `cleanArea` ~1 m²), never heading east to the zone.
  A third run sent `21023` with both `cleanId:[1]` *and* an inline
  `extraAreas:[{vertexs:…,"mode":"area"}]` at a deliberately different spot (west of
  the dock, x≈−6.4 m) — neither spot was visited. In all three runs the robot behaved
  as if starting an ordinary whole-home clean from the dock, and `21023` itself is
  always ACKed without starting anything. See `RE_REQUEST_ZONE_CLEAN.md` for the
  question passed to the RE agent.

  **Resolved 2026-10-09 (RE side):** the correct sequence is `21023 {"cleanId":[…]}`
  followed by **`21005 {"mode":"appointClean"}`** (EID 0x410) — `smartClean` synthesizes
  a whole-map total region and *overwrites* the clean-area selection before the job
  starts, so it can never honour zones; `21023` alone only ever live-updates a *running*
  job and is a no-op when idle. The web UI's ▶️ should therefore send `appointClean`
  for a zone clean. Evidence, test matrix and state coverage: `ZONE_CLEAN.md` (see also
  `FUNC_MAP.md` §6.2).

## Zone clean: live run of the corrected sequence (2026-10-10, [field])

The corrected sequence works — with one unexplained limit. All runs were from the dock
with a single stored region (`active:"normal"`, world-frame mm), `21023 {"cleanId":[N]}`
then `21005 {"mode":"appointClean"}`, both ACKed "ok":

| Zone (mm) | Distance from dock | Result |
|---|---|---|
| (−5800…−4800, 800…1800) — contains the dock area | ~0 m | **cleaned**, robot returned and docked on its own |
| (−4600…−3600, 700…1700) — corridor east of the dock | ~1.5 m | **cleaned**, robot returned and docked on its own |
| (4800…5600, −2200…−1400) — right-hand room | ~10 m | **abort**: `sweep`/`subMode:"area"` then `backcharge` ~2 s later, `errorState:[-2605]`, never left the dock area |
| (3800…4600, −2400…−1600) — right-hand room, clear of the room's obstacle blob | ~9 m | **same abort, `-2605` within ~2 s** |

Details that constrain the explanation:

* `subMode` is `"area"` throughout the successful runs — the zone selection reaches the
  task (the plain-zones → `--area` mapping in `ZONE_CLEAN.md` is live-confirmed).
* The abort is instant (undock, ~0.15 m, fault, return to dock) and identical for two
  different rectangles in the right-hand room. Both rectangles are 100 % free cells in
  the newest 20002 map.
* A 4-connected flood fill over the map's free cells (`0xFF`) from the dock's cell
  **reaches both far-zone rectangles** (the dock's free component is 19 582 cells and
  contains the corridor and the right-hand room). So a naive "no free path" explanation
  does not hold; if the fault is a planner failure it is a stricter internal rule
  (inflation, door width, a cleaning graph separate from the transit graph, or an
  area/room association).
* `-2605` is fault index **23** in the status map (`FUNC_STATUS.md` §4.1) — **resolved below**:
  index 23 = `EID_E_CLEAN_CANNOT_ARRIVE`.
* LD event **6023** (index 7 in the `msg_report_ld` map) fired once during the
  *successful* near-zone clean (14:17:34), so it is not the abort signal. It also fired
  on 2026-10-07 15:10:30 during a normal clean. 6052 fired once on 10-07 14:58:12.
  (*Resolved:* 6023 = `EID_I_CLEAN_TASK_FINISHED` — informational; this matches.)
* The robot's stored zone list was restored to empty afterwards (version 39), and the
  robot is docked.

Open: what actually raises fault index 23, and whether a *distant* area clean needs
something the vendor cloud does that we have not done. Follow-up request:
`RE_REQUEST_ZONE_CLEAN_ROUTE.md`.

**Resolved 2026-10-10 (RE side):** the fault is `EID_E_CLEAN_CANNOT_ARRIVE` — the navigator's
area target search found no target for the far room during its ~2.5 s map-update window and
gave up (stage 10). It is a map/label-coverage condition, not a distance limit. Log fragments
to confirm on a failing run, ranked causes and next experiments (label check in the 20002
raster, `21030 reset`, `extraAreas` variant): `ZONE_CLEAN_ROUTE.md`. The `FUNC_STATUS.md` §4.1
error-index table is now resolved — `−2605` = cannot-arrive (index 23), and
`EID_E_CLEAN_LOST_POSE` is never reported as a code. Wire traces: `wire-2026-10-10.jsonl`,
commands 101/102 (far, aborted), 105/106 (far, aborted), 107/108 and 111/112 (near, cleaned),
117/118 (far, aborted), 120/121 (mid, cleaned).

## Zone clean: the far-zone abort is about *rooms*, not distance (2026-10-10, [field])

`ZONE_CLEAN_ROUTE.md` identified `-2605` as `EID_E_CLEAN_CANNOT_ARRIVE` (the navigator's
target search found no cell). We ran its suggested experiments and then bisected the
condition:

* **`21030 {"autoAreaId":0,"operate":"reset"}` works** and the forced 20002 upload then
  carries segmentation labels for the first time (previously **0 labelled** cells; after
  the reset 19 390 cells carry labels `0x01`/`0x02`). It did **not** fix the far zone:
  the same rectangle still started `sweep`/`area` and aborted ~2 s later.
* **`extraAreas` (inline region, `mode:"area"`) also aborted** — so the stored-`cleanId`
  mapping is not the difference.
* **Bisecting by distance** from the dock (all zones 0.8–1.6 m wide, all in free,
  labelled cells, all started with `21023 {"cleanId":[N]}` + `appointClean` from the
  dock): **3.2 m ✓** (operator's `Biurko` zone), **5.6 m ✓**, **6.6 m ✓** — each drove
  out, cleaned and docked itself; **9.6 m ✗** and **10.4 m ✗** aborted as before.
* **The discriminator is the segmentation label**: every success sits in label
  `0x01` — the same label as the dock cell — and every failure sits in label `0x02`
  (the right-hand room). The corridor's label boundary is at x≈2400, **7.8 m** from the
  dock (the nearest label-`0x02` cell), while label-`0x01` free space ends at ≈7.5 m —
  so in this map "another room" and "beyond ~7.5 m" are confounded and cannot be
  separated from the dock.

Working hypothesis (matches the RE mechanism — the target search runs over the
segmentation-label grid): **an area clean can only target cells inside the label the
robot is currently in**. The decisive test starts the clean with the robot *inside*
the other room: clean (a) a zone there (same label → expect success) and (b) a zone
back in the dock's room (different label → fails if label-bound, succeeds if it is
only distance). Follow-up request: `RE_REQUEST_ZONE_CLEAN_ROOM.md`.

> Correction (same day): that two-sided test is **not** decisive — both hypotheses
> predict the same outcome. The discriminating test is a target that is *close but in
> the other label*: the labels touch at x≈2400, y≈−80 (label `0x01` at (2370,−80) next
> to `0x02` at (2420,−80)), so a ~0.5 m zone across the boundary settles it.
> An attempt to drive the robot there with the RC pad (`3005`/`3007`/`3008` closed loop
> on `pos`/`phi`) failed: the heading offset calibrated to ~177° (phi points backwards
> relative to travel) but the controller then oscillated and netted only ~0.2 m of
> travel. The robot was sent home and docked; no further driving without an operator.

Wire traces: `wire-2026-10-10.jsonl`, commands 176/177 (21030 reset), 184/185
(extraAreas, aborted), 188/189 (5.6 m, cleaned), 191/192 (6.6 m, cleaned). The robot's
stored zone list was restored to the operator's `Biurko` zone afterwards.

## Raw evidence

* `/media/sf_reshell-shared/device-log.bin` — robot's `/tmp/WifiConfLog`, 2026-10-04
  (decoded in `PAIRING_LOG_ANALYSIS.md`).
* `/media/sf_reshell-shared/robot.pcapng` — 802.11 capture from 2026-10-03 (mislabeled as
  Ethernet by `etl2pcap`; reframe with the LLC/SNAP `aa aa 03 00 00 00` marker → L3, as in
  `tmp/proscenic/pcap/reframe.py`), converted from `robot.etl`.
* `/media/sf_reshell-shared/findings.txt` — summary of the 2026-10-03 sweep.
