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

## Raw evidence

* `/media/sf_reshell-shared/device-log.bin` — robot's `/tmp/WifiConfLog`, 2026-10-04
  (decoded in `PAIRING_LOG_ANALYSIS.md`).
* `/media/sf_reshell-shared/robot.pcapng` — 802.11 capture from 2026-10-03 (mislabeled as
  Ethernet by `etl2pcap`; reframe with the LLC/SNAP `aa aa 03 00 00 00` marker → L3, as in
  `tmp/proscenic/pcap/reframe.py`), converted from `robot.etl`.
* `/media/sf_reshell-shared/findings.txt` — summary of the 2026-10-03 sweep.
