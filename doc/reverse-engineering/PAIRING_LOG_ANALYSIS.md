# Why the M7 Pro stayed in pairing mode — `device-log.bin` decoded

**Date:** 2026-10-04; updated 2026-10-05 (second capture §10–§11, and the evening resolution) · **Unit:** SN `LSLDSM7PRO20403551`, model `6716` (`Proscenic-6716_20403551`)
**Evidence:** `/media/sf_reshell-shared/device-log.bin` — the 2026-10-04 capture (1,429,504 B, pulled via the `getLog`
local command) plus the 2026-10-05 re-run (2,856 B), both decoded here — and static analysis
of `network_proxy` (`tmp/proscenic/funcmap/np_all.c`).

**The file was truncated at 1.4 MB, but this is not a mystery:** it is `/tmp/WifiConfLog`,
the firmware's own append-only pairing diary. It grows ~2.5 KB/s *forever* once a certain
firmware bug is triggered (see §5), so any `getLog` transfer that lands mid-storm is cut at
whatever byte the transfer happened to reach. The first 46 KB of it — everything before the
storm — is fully readable and answers the question. The captured file covers its whole life,
from its creation at 08:06:50 UTC to the cut at ~18:21 UTC.

---

## TL;DR

1. **Every non-storm line decodes cleanly, and your provisioning worked.** The log shows
   `setUrl url: http://192.168.1.208:8080/` → ok, `setUrl ip: 192.168.1.208, port: 8081` → ok,
   `setSta` → ok, `applyCfg` → ok, and later `setID {id: Foo}` → `StartBind` →
   `{cmd:getID,result:ok,type:ipfromapp}`. At 17:32:05 UTC the robot was on your Wi-Fi
   (`wlan0 = 192.168.1.243`) and its reachability recorder confirmed **baidu ✔ google ✔ and
   your own server host `192.168.1.208` ✔** (`p_s:0->7`, `ip:->192.168.1.208`).
2. **The `buf2Json` / `invalue cmd` spam is a firmware bug triggered by your `bindOk`.**
   It starts at the *exact second* `bindOk` was sent (17:32:24 UTC) and is a
   close-the-socket-without-stopping-the-listener bug that spins at ~28 iterations/s,
   logging 3 lines each and forking ~85 shells/s, indefinitely.
3. **The robot stayed in pairing mode because the *cloud bind* never completed** — and that
   has nothing to do with the local log. Leaving pairing requires the robot to (a) bring up
   its TCP link to your Channel-B server (`:8081`, handshake `10001`, pong `21006`) **and**
   (b) successfully POST its preBind to your Channel-A server (`:8080`) and get `code:0` back.
   Only then does the firmware fire internal event **EID 0x460**, which is what ends the
   pairing indication. None of that evidence is in `WifiConfLog` — the dial/connect/register
   lines go to a **different channel** (ZMQ bus events, `HBLog …` / `TcpClient connect …` /
   `mIpAndPort : [ip:port]`, materialized to `/tmp/Run/Log` by the onboard `event_hub`).
   See §7 for how to get them.
5. **The 2026-10-05 re-run (§11) repeats the same picture with the servers up throughout**:
   provisioning + bind armed + your host pingable, zero packets at the servers, no storm
   (no `bindOk` this time). §10 maps the boot→dial chain that precedes any traffic; §11 lists
   the three network-level observations (`ss`, `tcpdump`, or serving getSockAddr) that pin
   down the remaining failure.
6. **RESOLVED the same evening (§11, "Resolution"):** the robot had been dialing all along
   — the **server host's firewall was silently dropping its inbound TCP** (no allow rule
   for the Python process on :8081; ICMP was allowed, which is why the robot's own checks
   looked green). With the rule added, the full chain completed within seconds —
   `10001` + `21006` pongs, then `binding`/`register`/`sync` to Channel A,
   `code:0` → *directly bind success* → EID `0x460` → **pairing indication ended**.
   Independently, the robot's own log froze at 13:53 (runtime fs full) — a frozen log
   does **not** mean nothing happened. Protocol details now in `PROTOCOL.md` §A;
   field lessons in `FIELD_NOTES.md`.
4. **`bindOk` 19 s after `setID` was premature and cost you the channel.** After it, UDP
   :7913 is dead: the socket is closed, further datagrams get ICMP port-unreachable, and the
   storm consumes the box. Everything you tried *after* 17:32:24 UTC through the local
   channel — including any re-run of the toolkit without a power cycle — went nowhere.
   That alone plausibly explains "it sends no traffic whatsoever": by the time you were
   watching, the robot had already been through its only attempt window, and the channel you
   were talking to was gone.

---

## 1. What this file is

`/tmp/WifiConfLog` is written by the local-channel code with a hilariously direct method:
every log call is `FUN_0047ac10(msg)` →

```c
snprintf(buf, 0x7f, "echo \"[%d,%d]:%s\" >> /tmp/WifiConfLog", GetCurrentTickSec(), time(), msg);
system(buf);
```

That explains three cosmetic oddities:

* **Quote-eating** — the shell strips the inner quotes, so you see `{cmd: getID}` not
  `{"cmd": "getID"}`.
* **Truncation at ~127 bytes** and **newline-splitting**: a message containing `\n` (e.g.
  `Close mSockFd\n\n`) lands as several `echo` lines; you can see a stray one-character
  fragment (`L` at tick 22802) in the middle of the storm.
* **One `system()` fork per line** — which matters for the storm in §5.

What gets logged: every received datagram's raw text, the reply that is sent, and any
side-effect the handler or a recorder chooses to log. It does **not** log the cloud side.

`getLog` (`FUN_0047aca0`) serves the file in offset-paged base64 chunks sized
`(req_len>>2)*3 - 0x23`. The file keeps growing while it is being read, which is why the
transfer ended mid-line at 1,429,504 bytes (`…{result:invalue c`). The easiest robust read is
a shell (`cat /tmp/WifiConfLog`), or `rmLog` first to reset it.

## 2. The decoded timeline

All 38 non-storm lines, in order (ticks are seconds since boot; UTC computed from the
embedded `time()` value). Credential values are masked.

### Session 1 — provisioning (08:06:50–08:07:01 UTC) — the file is *created* here
| tick | UTC | log line | meaning |
|---|---|---|---|
| 88 | 08:06:50 | `{cmd: getID}` | tool's discovery probe (see §3 note — not a real command) |
| 88 | 08:06:50 | `{result:invalue cmd}` | the reply to that probe |
| 90 | 08:06:52 | `{cmd: setUrl, url: http://192.168.1.208:8080/}` then `{cmd:setUrl,result:ok}` | **Channel-A base URL written** (`/data/bin/Run/Config/url`) |
| 90 | 08:06:52 | `{cmd: setUrl, ip: 192.168.1.208, port: 8081}` then `{cmd:setUrl,result:ok}` | **Channel-B target written** (`/data/bin/Run/Config/ip_port.json` = 192.168.1.208:8081) |
| 90 | 08:06:52 | `{cmd: getCfg}` + reply | tool re-reads config (echoes stored SSID/key → masked here) |
| 90 | 08:06:52 | `{cmd: setSta, staName: <SSID>, staPwd: ***}` + `{cmd:setSta,result:ok,code:2}` | Wi-Fi creds **stored** (memory only) |
| 94 | 08:06:56 | `{cmd: getID}` + reply | a second probe |
| 97 | 08:06:59 | `{cmd: applyCfg}` + `{cmd:applyCfg,result:ok,code:1}` | **commit**: robot runs `cleanpack_mode -m sta`, drops the AP, joins your Wi-Fi |

### Session 2 — status check (17:19:36–17:19:39 UTC)
| 21815 | 17:19:36 | `{cmd: getID}` + reply | probe |
| 21817 | 17:19:39 | `{cmd: getSn}` → `ok,sn:LSLDSM7PRO20403551` | identification works |
| 21817 | 17:19:39 | `{cmd: getCfg}` → ok (masked) | config still stored |
| 21818 | 17:19:39 | `{cmd: checkPwd}` → `ok,code:1` | **Wi-Fi join state = 1 (connected)** |

### Session 3 — bind attempt (17:32:02–17:32:24 UTC)
| 22561 | 17:32:02 | `{cmd: getID}` + `{result:invalue cmd}` | probe |
| 22564 | 17:32:05 | `{cmd: getSn}` → ok | identification |
| 22564 | 17:32:05 | `{cmd: setID, id: Foo, deviceSN: LSLDSM7PRO20403551}` | **bind request** — armed (see §4) |
| 22564 | 17:32:05 | `SetCusDomain=` then a second line `http://192.168.1.208:8080/` | handler logs the custom domain it loaded from the `url` file (the split line = the stored file value carries a trailing newline, embedded verbatim) |
| 22564 | 17:32:05 | `StartBind` | the bind recorder/state machine starts |
| 22564 | 17:32:05 | `{cmd:getID,result:ok,type:ipfromapp}` | **setID's reply** (it answers as `getID`; see §3) |
| 22564 | 17:32:05 | `StartRecord wifi_info info no ip` | reachability recorder ticks on (1 Hz) |
| 22564 | 17:32:05 | `p_s:0->7` | reachability bitmask 0→7: **baidu ✔ (bit0), google ✔ (bit1), custom domain ✔ (bit2)** |
| 22564 | 17:32:05 | `ip:->192.168.1.208` | the configured server host resolves (already an IP) |
| 22564 | 17:32:05 | `w_c(192.168.1.243):0->2` | wlan0 has address **192.168.1.243** (2 = `ifconfig` reported an `inet` line) |
| 22580 | 17:32:21 | `{cmd: getID}` + reply | another probe |
| 22583 | 17:32:24 | `{cmd: bindOk}` | **closes the local channel → triggering the storm (identical tick)** |
| 22583… | 17:32:24… | *(storm: `buf2Json Fail` / empty / `{result:invalue cmd}` ≈28×/s)* | §5 |
| 22611 | 17:32:52 | `p_s:7->3` | custom-domain ping missed (robocop: 1 packet, 2 s timeout) |
| 22632 | 17:33:13 | `p_s:3->7` | recovered |
| 22634 | 17:33:15 | `p_s:7->3` | missed again |
| 22636 | 17:33:17 | `p_s:3->7` | recovered |
| 22802 | 17:36:03 | `L` | newline-split fragment of a longer message (benign) |
| 23144 | ~18:21 | `{result:invalue c` | transfer cut mid-storm |

## 3. Small decode notes

* **`getID` is not a command on this firmware.** Probing `{"cmd":"getID"}` gets
  `{"result":"invalue cmd"}` — every time (ticks 88, 94, 22561, 22580). The
  `{"cmd":"getID","result":"ok","type":"ipfromapp"}` you see documented is **setID's reply**.
  Use `getSn` for identification.
* The `p_s:7->3→7→3→7` flaps start **after** the storm begins; they are the robot's own
  quality-of-service degrading under ~85 shell forks/s, not a real network change. At
  17:32:05, before the storm, the custom-domain check to **your host** passed.
* The recorder (`p_s`/`ip:`/`w_c`) is the bind-time telemetry: it starts with `setID`
  (`StartRecord`) and is what the vendor app shows as "connection quality" during pairing.

## 4. What leaving pairing mode actually requires

The chain, from `setID` to "pairing over" (**all of it must succeed**):

1. **`setID` arms the bind** (local command → handled, answered). Firmware-side it sets the
   "need connect" flag (`HeartOnlineLd +9`, the only writer) and stores your `id` as userId.
   This is the moment the robot starts *wanting* to reach your cloud.
2. **Heart loop resolves the Channel-B address** (1 s tick): reads
   `/data/bin/Run/Config/ip_port.json` — your 192.168.1.208:8081 — and hands it to the TCP
   client. (Your earlier `setUrl ip/port` had already stored it; `setID` is what *activates*
   it. No reboot is needed — the earlier "reboot required" note in `PROTOCOL.md` was wrong.)
3. **TCP connect to your :8081 + `{"infoType":10001,…}` handshake + answering the `21006`
   heartbeats.** Only after connect+pong does the firmware set its "online" flag.
4. **BindUser thread** (1 s tick): once the robot is *online*, it POSTs its preBind
   (`sn=…&ts=…&userId=…`) to the **Channel-A** URL (your noobscenic `:8080`). It needs
   `code:0` in the reply (102/0x66 means "re-register first").
5. **On `code:0`** it logs `directly bind success..` and posts **EID 0x460** — the internal
   "bind success" event the LED/UI reacts to. **That** is when pairing ends.

`bindOk` plays no part in this chain. It is the app's "I'm done with the local socket" signal;
sending it early cancels nothing but destroys your ability to talk to the robot.

**So why did nothing reach your servers?** The log proves the *robot-side preconditions*
(armed bind, correct B target stored, LAN + host reachable at 17:32:05). It cannot prove or
disprove the dial — see **§10** (the address-resolution chain that precedes any dial) and
**§11** (the 2026-10-05 re-run with the servers up throughout, and the network-level checks
that discriminate the remaining failure modes).

## 5. The `bindOk` bug — why the log drowns in `buf2Json Fail` / `invalue cmd`

* The local listener runs in `wifi_config/ListenerThreadProcess` (`FUN_00468890`): a
  blocking UDP socket on 7913 (`socket(AF_INET, SOCK_DGRAM|SOCK_CLOEXEC)`, i.e. **blocking**)
  with a 10 ms loop sleep. While the channel is healthy it sits in `recvfrom` — *silent*.
* `bindOk`'s handler (`FUN_00463c60`) does exactly two things:
  `ZmqManager::PostEvent(…,"Close mSockFd\n\n")` and `close(fd)`. It **does not clear the
  listener's running flag and does not set the fd to -1** (correct close paths exist:
  `FUN_00464ac0` closes and sets -1; `FUN_00464b34/b38` clear the flag too — `bindOk` just
  doesn't use them).
* After `close()`, the fd number is stale. The next `recvfrom` returns `-1/EBADF`
  **instantly** (no longer blocking — the kernel rejects the bad fd before any wait). The
  return value is ignored, the zeroed 256-byte buffer is dispatched unchanged, which logs
  **`buf2Json Fail`**, then the (empty) buffer echo as a bare `[tick,time]:` line, then the
  default reply **`{result:invalue cmd}`** is logged before a `sendto` that fails silently.
  sleep 10 ms → repeat. Net: ~28 iterations/s × 3 lines ⇒ **~85 log lines/s, each an
  `echo … >>` `system()` fork ⇒ ~85 shell forks/s, ~2.5 KB/s, ~9 MB/h, forever** (until
  `network_proxy` restarts).
* Statistics from the captured file: **15,352 storm iterations** (15,352 `buf2Json Fail`,
  15,352 empty, 15,352 + 5 `invalue cmd`), from tick 22583 to the cut at tick 23144 —
  the file ends mid-line because the transfer was cut while the file grew.

**Operational rule: don't send `bindOk`.** It serves no purpose for a headless rehome; it
kills the channel and upgrades it into a CPU-chewing fork storm. If you do send it, expect
:7913 to be dead until the process restarts — and note that after a **reboot** the listener
does *not* necessarily come back (§6).

## 6. Recovering the local channel

The listener is started at boot **only when `/data/cfg/wifi_mode` == `"ap"`**
(`interface_obj.cpp`: "wifi is ap mode , start listener …"), or when the robot *enters*
AP mode (callback `FUN_00457850`). It is *not* stopped when the robot later switches to STA —
which is why, in this capture, you could still talk to it at 17:19/17:32 while it sat on your
LAN as `192.168.1.243` (it had booted in AP mode for the 08:06 provisioning, and the
listener survived `applyCfg`).

Consequences:

* A **plain power-cycle in STA mode will NOT bring :7913 back.** (It does end the storm.)
* To get the channel back you need the robot in AP mode again:
  * **Physical/factory reset** — `factory_reset.sh` writes `echo "ap" > /data/cfg/wifi_mode`
    (and hides the AP via `/data/cfg/ap_hid`, and wipes `/data/bin/Run/Config/url`; you'd
    re-provision from scratch).
  * **With a shell** (root via UART/adbd/dropbear, see `REPORT.md` §5):
    `echo ap > /data/cfg/wifi_mode` then reboot — this keeps your `ip_port.json`/URL.
* Once back in AP mode, re-provision exactly as in Session 1.

## 7. How to get the *cloud-side* evidence (corrected 2026-10-05)

The decisive records are **not** in `WifiConfLog`. The connect machinery narrates itself on a
**per-process ZMQ event bus**, not a greppable file:

* The heartbeat builds `[2026/10/4 17:32:24.123 ]Send_Ping`-style lines and pushes them into
  an in-memory ring (`FifoFrame`, 2048 B, `FUN_00479020` = length-prefixed push — *not* a file
  write), then drains it with `ZmqManager::PostEvent(…, "HBLog %s", line)`
  (heart_online_ld.cpp:0xbc). `mIpAndPort`, `TcpClient connect …`, `reconnect to server …`
  are posted to the same bus as plain `PostEvent` calls (`FormatPrint` shares the path).
* What turns bus events into files is the onboard log consumer — the `event_hub` process —
  which writes **`/tmp/Run/Log/`** (the daemon's run log; persisted on demand by
  `save_all_to_flash.sh` → `/data/bin/Run/Log.tar.gz`, wiped by `factory_reset.sh`). So with a
  shell the greppable path is **`/tmp/Run/Log`, not the repo-wide grep that used to be
  suggested here**.

With a shell:

```sh
grep -rE "mIpAndPort|TcpClient|reconnect to server|Send_Ping|no session" /tmp/Run/Log 2>/dev/null
```

What to look for, and what it means:

| run-log line (literal) | meaning |
|---|---|
| `mIpAndPort : [192.168.1.208:8081]` | the exact target the heartbeat was told to dial — proof it used *your* address (or not) |
| `reconnect to server … isOpen=%d, send interval:%d` | a reconnect cycle started |
| `TcpClient connect success %s:%d` / `TcpClient connect false, %s, errno:%d` / `TCP connect failed` | **the connect result — this answers "did it ever knock?"** |
| `Send_Ping` | heartbeat lines; their absence means the loop wasn't running |
| `Get ip and port from %s success, ip: %s, port: %d` / `ip_port member is error` | the ip_port.json read result (see §10) |
| `get tcp addr : %s` / `Code=%d url=%s` | the cloud getSockAddr fallback fired (§10) — if this repeats, the local file was rejected |
| `no session!!!` / `reg response data = %s` | Channel-A registration problems (noobscenic's `register` reply shape) |

Without a shell, the same facts leak through the network (§10–§11) — that is the practical
observation channel this time.

## 10. The address-resolution chain (what runs before any dial)

New static analysis (2026-10-05) of the boot → dial path, all in `network_proxy`:

1. `main()` (`FUN_0040b138`) starts every subsystem: manager `Init` (`FUN_00458790` — starts
   the UDP :7913 listener if `wifi_mode == "ap"`) then manager `Start` (`FUN_00457668`),
   whose first action is `HeartOnlineLd::Start` (`FUN_00454198`, vtable slot +0x18) — it sets
   the run flag `+0xa=1` and spawns the heartbeat thread (`FUN_00455090`). **The thread runs
   from boot, even in AP mode.**
2. The thread begins with **address resolution** (`FUN_00454998`), which loops until it gets a
   non-empty address list (1 s retry). `FUN_00451cd0` (`get_tcp_addr.cpp`) tries, in order:
   * **local file** `/data/bin/Run/Config/ip_port.json` — must be a JSON object with
     `"ip"` (string) + `"port"` (int): `{"ip":"192.168.1.208","port":8081}`. This is exactly
     what the `setUrl ip/port` local command writes (handler `FUN_00466af0` builds that object
     and `WriteStrToFile`s it, then `FUN_00454458` also hands the pending address to the
     heartbeat for the live path). On success → 1-entry list, done — **no HTTP is involved.**
   * **cloud fallback** if the file is missing/unparseable/wrong-typed: HTTP GET
     `<base>cleanPack/getSockAddr?version=1&sn=<SN>&companyId=<id>` (curl, 5 s timeout) and
     parse `{"code":0,"data":{"addr_list":[{"ip":"…","port":N}]}}`. On failure it sleeps
     **0.2 s** and the outer loop waits 1 s → ~1.2 s retry cadence; every retry is a visible
     HTTP request at the base host — *unless the URL itself is malformed* (see below).
3. With an address in hand the thread does `FUN_0046c4c8` → `TcpClientPort::Connect`
   (`FUN_0046a588`: non-blocking `connect()`, 3 attempts, `select`): **the TCP SYN is the very
   first packet the rehome can produce**, then `FUN_0046c950` sends the `10001` handshake and
   `FUN_00455090`'s loop pings `21006` every ~6 s. On `isOpen==0` it closes/redials. Nothing
   in this path consults the servers' responses first: **if ip_port.json parses, a SYN to
   :8081 must appear within ~1 s of the file existing.**

**The Channel-A URLs** are built at `setUrl` time, not at boot: the handler writes
`/data/bin/Run/Config/url`, then `FUN_004650c8` reads it back (or defaults to the vendor
`https://mobile.proscenic.cn/`) and rewrites every endpoint string in the config singleton as
`<base> + cleanPack/<name>` — `register` (→ `FUN_0044ec90`), `binding` (preBind, →
`FUN_004492c0`), `getSockAddr`, `sync`, `uploadLogs`, … So after a successful
`setUrl url` those requests point at the base host — **but only within that boot**: before the
first `setUrl` of a boot the strings are still relative (`cleanPack/…` with no scheme), curl
rejects them locally with zero packets, and the getSockAddr poll is silent.

## 11. Session 3 — 2026-10-05 capture (servers up throughout, still "nothing heard")

`/media/sf_reshell-shared/device-log.bin` (2,856 B, 67 lines) — a clean second attempt on a
power-cycled robot, this time with **no `bindOk` sent** (no storm: 1 `buf2Json Fail`, 1 empty
line, 4 probe `invalue cmd` replies). Decoded:

| tick | UTC | line(s) |
|---|---|---|
| 50–53 | 13:49:57–13:50:00 | boot in AP mode: `getID` probe, `getSn` (SN ok), `getCfg`, `checkPwd` → `code:0` (not joined yet) |
| 161–166 | 13:51:48–13:51:53 | `getID`; **`setUrl url: http://192.168.1.208:8080/` → ok**; **`setUrl ip: 192.168.1.208, port: 8081` → ok**; `getCfg`; `getSn`; **`setID id: Foo`** → `SetCusDomain=` + the URL (clean, no embedded newline this time) + `StartBind` + `StartRecord wifi_info info no ip`; reply `{cmd:getID,result:ok,type:ipfromapp}`; `setSta` (creds) → `code:2`; `applyCfg` → `code:1` |
| 176 | 13:52:02 | `p_s:0->7`, `ip:->192.168.1.208`, `w_c(192.168.1.243):0->2` — **on your Wi-Fi, baidu+google+your host all ping-reachable (ICMP), custom domain resolves to .208** |
| 230 | 13:52:54 | `getID` probe (still alive) |
| — | — | appended `deal_net_log.sh` bundle: identity files, `wpa.log`, `cleanpack_mode` logs (ap @13:49:24Z → sta @13:51:53Z) — all as expected |

So the client-side picture repeats Session 1's: **provisioning fully succeeded, the bind is
armed, L2/L3 to your host is proven by the robot's own ICMP recorder — and still no
TCP/HTTP at the servers.** With §10, that pins the failure to the *first* observable step:
either the heartbeat never resolved an address (and its getSockAddr fallback never emitted a
packet, i.e. the base URL wasn't rebuilt or the thread isn't looping), or it dialed and the
dial didn't reach the server process. Discriminator (cheap, on the server host):

```sh
ss -ltnp | grep -E ':8080|:8081'          # is :8081 really bound, by the right pid?
                                          # (rehome_server.py defaults to :20009 with no argv!)
sudo tcpdump -ni any 'host 192.168.1.243' # watch during a re-provision / setID
```

* SYNs to `:8081` → the robot **is** dialing: the problem is the listener (wrong port/PID).
* Repeating `GET /cleanPack/getSockAddr?version=1&sn=…&companyId=…` at ~1.2 s → the heartbeat
  is alive but **rejects ip_port.json**; answer that route with
  `{"code":0,"data":{"addr_list":[{"ip":"192.168.1.208","port":8081}]}}` and the robot will
  dial without ever needing the local file.
* **Zero packets at all** → the heartbeat thread isn't producing traffic (stuck before
  address resolution or stopped): next step is a shell (§6) to read
  `/data/bin/Run/Config/ip_port.json` and `/tmp/Run/Log`.

### Resolution (2026-10-05, evening) — the mystery is closed **[field]**

Both remaining hypotheses above turned out to be *wrong in the same direction*: the robot
**was** dialing all along (its SYN went out every ~6 s), and nothing was wrong with its
resolve path either — **the server host's firewall was silently dropping the robot's
inbound TCP.** The Windows host had an inbound allow rule for `noobscenic.exe` (Channel A)
but none for the Python process on **:8081**; the robot's SYNs (and any Channel-A traffic
that would have preceded the link) were dropped with *no RST and no server log*, while
**ICMP was allowed** — which is exactly why the robot's own recorder showed
`192.168.1.208 ✔` throughout. Probes from the analysis VM could never reveal this: they
arrive via VirtualBox NAT as local traffic, which inbound rules don't filter. (That's also
why "no traffic whatsoever" was true on both servers' *logs* yet false on the wire.)

Once the rule was added (19:5x UTC), the chain ran end-to-end within seconds, with **no
re-provisioning**:

* Channel B: `[+] robot connected (192.168.1.243)` → `10001` handshake with the real SN →
  `21006` pings every ~6 s, ponged.
* Channel A, all within ~100 ms and all answered by a plain catch-all `{"code":0}`:
  1. `POST /cleanPack/binding` — `sn=LSLDSM7PRO20403551&ts=LSLDSM7PRO20403551&userId=Foo`
     (the preBind; **`ts` repeats the SN** — vendor quirk) → `code:0` →
     *directly bind success* → **EID 0x460 → pairing indication ended**.
  2. `POST /cleanPack/register` — `sn=…&ld_sn=14411442DB220CBE&sig=<b64 RSA>&ts=0`
     (152 B; `ld_sn` = contents of `/data/bin/sys_data/sn`); `data:{}` accepted, no
     session/cookies issued, no re-register loop, and **no `Cookie:` header** on any of
     these requests (there was nothing to send).
  3. `POST /cleanPack/sync` — `sn=…&companyId=48&mcuVer=S6&version=0.7.1&versionCode=1241`
     `&gitSha=NULL&cloud=psnk` (98 B).

Independently, the robot's own log **froze at 13:53** because its runtime filesystem
filled (writes then only succeed into just-freed space — demonstrated with
`{"req":"rmLog"}` → `getLog`). That freeze is why the robot's side of this story was
invisible for hours: the heartbeat was dialing and could not narrate it. **A frozen
pairing log does not mean "nothing happened."** Details and the operational lessons:
`FIELD_NOTES.md` ("The resolution").

## 8. Corrected rehome order (the fix)

1. **Start the servers first.** Channel B (`rehome_server.py`) must be listening on the
   exact ip:port you will put in `ip_port.json` (**192.168.1.208:8081** here) and must
   answer the `10001` handshake and pong every `21006`. Channel A (noobscenic `:8080`) must
   serve `cleanPack/register` returning `data.session` (≥16 B) + `data.cookies`, and must
   answer the robot's preBind POST with integer **`code:0`**.
2. Provision the robot (all of this worked already): `setUrl url`, `setUrl ip/port`,
   `setSta` + `applyCfg`.
3. Only then send **`setID`** (`{"id":…,"deviceSN":"<SN>"}`). Watch for the TCP connection on
   :8081 within seconds.
4. Let the robot walk the chain: B handshake + heartbeats → BindUser preBind → `code:0` →
   EID 0x460 → **LED stops pairing**.
5. **Do NOT send `bindOk`.** Leave the channel open; it costs nothing and lets you retry
   (e.g. re-`setID`, re-`setUrl`) without a reset.
6. If you need a clean slate after a mistake: power-cycle (ends the storm). If :7913 then
   doesn't answer, the robot booted in STA — put it back in AP mode per §6.

## 9. Corrections to earlier artifacts (applied)

* `FIELD_NOTES.md` claimed the physical unit runs *different firmware* from the analyzed
  image. **It doesn't.** The log literals (`StartRecord wifi_info`, `buf2Json Fail`,
  `p_s:%d->%d`, `w_c(%s):%d->%d`, `SetCusDomain`, `StartBind`, `invalue cmd`) all match the
  analyzed `network_proxy` build's strings, and the UDP port constant in the image is exactly
  7913. The "different variant" conclusion came from a wrong-port sweep; corrected there.
* `PROTOCOL.md`: `getID` row (it's not a command), the Channel-C port banner (7913 is
  hardcoded in this image — the `rand()%1000+9000` range belongs to the separate
  cloud-triggered RemoteCtrl socket, not this listener), the `bindOk` row (storm bug), and
  the "setUrl ip/port needs a reboot" claim (it does not — the tick loop applies the new
  address once the bind is armed).

## Evidence index

| Item | Where |
|---|---|
| Raw log | `/media/sf_reshell-shared/device-log.bin` (1,429,504 B) |
| Second capture + resolution evidence | `/media/sf_reshell-shared/var/` (noobscenic tap: `events` table + `traces/wire-*.jsonl` with the robot's exact `binding`/`register`/`sync` bodies) and `/media/sf_reshell-shared/rehome_server.txt` (Channel-B console: `10001`/`21006`) |
| Decompiled `network_proxy` | `tmp/proscenic/funcmap/np_all.c` |
| Extracted firmware rootfs | `tmp/proscenic/rootfs/` |
| Logger | `FUN_0047ac10` (echo-to-file), `FUN_0047aca0` (getLog) |
| Listener | `FUN_00468890` (loop), `FUN_00468a70` (7913 socket), `FUN_00468b98` (start) |
| bindOk handler | `FUN_00463c60` (bug), correct stops `FUN_00464ac0`/`FUN_00464b34` |
| Bind arming | `setID` handler (sets `HeartOnlineLd+9`), `FUN_00454998` (resolve), heart loop `FUN_00455090` |
| Bind completion | `bind_user.cpp` `FUN_004495d4`, EID 0x460 |
