# Field notes — observations from physical hardware

Everything else in this directory is derived from the **`LS_S6` firmware sample**.
This file is derived from **a physical unit on the bench**, and the two do not always
agree. Findings here are dated and attributed to a specific unit, because "the M7 Pro"
is evidently not one thing.

---

## Unit A — `Proscenic-6716_20403551`

| | Documented (`LS_S6` sample) | This unit |
|---|---|---|
| Pairing SSID | `LDRobot` (hardcoded in `apDemo`) | **`Proscenic-6716_20403551`** |
| AP address | 192.168.78.1 | 192.168.78.1 — matches |
| AP DHCP pool | 192.168.78.50-150 | station got .111 — consistent |
| ICMP | — | responds |
| Channel-C `getID` port | `rand()%1000 + 9000`, per boot | **UDP 7913**, fixed across a power cycle |
| Serial (`getSn`) | — | `LSLDSM7PRO20403551` — LDRobot LS/LDS lineage |
| `setSta` SSID field | `ssid` | **`staName`** — `ssid` is refused with `fail,-1` |
| TCP surface | — | only 53; no TCP/HTTP pairing service |
| BSSID | — | `ae:1d:df:67:16:f4` (locally administered bit set, so no OUI lookup) |

`6716` appears nowhere in the firmware-derived documents.

### 2026-09-10 — channel-C discovery does not work on this unit

**Method.** A Windows 11 station joined the (open) AP and ran
`tools/rehome.py discover`, while `pktmon --capture --comp nic` recorded at the NIC,
below the OS network stack, so local firewall or AV behaviour cannot explain a
negative. 3375 frames, converted to pcapng with `etl2pcap`.

#### ⚠ The `etl2pcap` link-type trap — read this before analysing such a capture

The converted file declares link-type **1 (EN10MB / Ethernet)** in its interface
description block, but the frames are **raw 802.11 data frames** with LLC/SNAP
encapsulation. Every IP-level filter therefore matches nothing at all, silently — the
first pass over this capture concluded "zero replies from the robot" purely because of
this, when in fact there are 214.

You can spot it by eye: `aa aa 03 00 00 00 08 00` (LLC/SNAP + ethertype IPv4) sits at
offset 24 in a normal data frame, or 26 in a QoS data frame (subtype 8, `0x88` in the
first byte), right after the 802.11 header. Re-stamp the link type to **105
(`LINKTYPE_IEEE802_11`)** and every ordinary tool works again:

```python
import struct
data = open('robot.pcapng','rb').read()
off, pkts = 0, []
while off + 12 <= len(data):                       # walk the pcapng blocks
    btype, blen = struct.unpack_from('<II', data, off)
    if blen < 12 or off + blen > len(data): break
    if btype == 0x6:                               # Enhanced Packet Block
        body = data[off+8:off+blen-4]
        _, tsh, tsl, cap, _ = struct.unpack_from('<IIIII', body, 0)
        pkts.append((((tsh<<32)|tsl), body[20:20+cap]))
    off += blen
with open('out.pcap','wb') as f:
    f.write(struct.pack('<IHHiIII', 0xa1b2c3d4, 2, 4, 0, 0, 262144, 105))
    for ts, raw in pkts:
        f.write(struct.pack('<IIII', ts//1000000, ts%1000000, len(raw), len(raw)) + raw)
```

Note also that a pktmon NIC capture on a station is **not** monitor mode: it sees only
frames addressed to that station plus broadcast. It carries 802.11 headers because it
is a Native WiFi capture, which is what makes the mislabelling so easy to miss.

#### What the capture actually shows

1. **UDP 9000-9999 is closed.** The device sent **76 ICMP "udp port unreachable"**
   messages naming ports 9000-9005 and 9407. That is a normal Linux stack actively
   refusing, not a filter and not a dropped burst. Only a handful of the ~3000 probes
   drew an ICMP error because Linux rate-limits ICMP error generation to roughly one
   per second; the sweep itself was delivered and processed.
2. **No data reply on any port except 53.** A real listener's reply would be a data
   packet and is not rate-limited, so it would have appeared. None did.
3. **UDP *and* TCP 53 are open** — the device runs a DNS server for its AP clients,
   answering `Refused` to the station's queries.
4. **No broadcast or multicast traffic exists in the capture at all.** The device does
   not announce itself, and — since a station capture does see broadcast — this is
   meaningful evidence, not an artefact.
5. **The device is trying to phone home from inside pairing mode.** Once a second:

   ```
   ARP, Request who-has 47.254.140.182 tell 169.254.110.128   (sender MAC = the robot)
   ```

   `47.254.140.182` is Alibaba Cloud, the same `47.254.0.0/16` as the OSS firmware host
   in `REPORT.md` §2.3. The sender address is APIPA, so the device's **station**
   interface is up but got no DHCP lease. It has no default route, so it ARPs directly
   for an off-subnet address.

#### Conclusions

* **Brute-forcing the port range was not the mistake, and broadcast is not the fix.**
  The sweep covered every port the documented RNG (`rand()%1000 + 9000`) can produce and
  was answered honestly. Broadcast solves *unknown IP*, and the IP was never in doubt;
  finding 4 rules out a device-initiated announcement as well.
* **The documented discovery model is incomplete even for `LS_S6`.** A broadcast
  `getID` cannot locate a *randomly chosen* port — it still has to be addressed
  somewhere. So either a **fixed** discovery port exists that the recovered notes miss
  (and 9000-9999 is the control port handed out afterwards), or the app learns the port
  some other way. Note the reply type is literally `ipfromapp`, which reads like the
  *device* learning the app's address.
* **This unit may simply not run `apDemo`.** A different pairing SSID means a different
  provisioning binary, and `PROTOCOL.md` §C may not describe it at all.
* Not yet excluded: that the provisioning service only listens during a limited window
  or after a specific button sequence, rather than for as long as the AP is up.

### 2026-09-10 (later) — resolved: the service is on **UDP 7913**

A full nmap sweep with the real probe as the payload found it immediately:

```
sudo nmap -Pn -n -sU -p- -T4 --defeat-icmp-ratelimit \
     --data-string '{"cmd":"getID"}' --open --reason -oA robot-udp 192.168.78.1
   53/udp   open  domain     udp-response ttl 63
   7913/udp open  qo-secure  udp-response ttl 63
```

`--data-string` is what made this work: the service answers a *valid* command and
ignores everything else, so nmap's default empty UDP probes would have found nothing.
A companion TCP scan found **only 53/tcp** (65534 resets), so there is no TCP or HTTP
pairing surface on this unit — channel C really is the only local control path.

**The command set is the documented one.** Against port 7913:

```
getSn     {"cmd": "getSn", "result": "ok", "sn": "LSLDSM7PRO20403551"}
getCfg    {"cmd": "getCfg", "result": "ok"}
checkPwd  {"cmd": "checkPwd", "result": "ok", "code": 0}
```

#### Corrections to the earlier entry

* **The SSID divergence was a red herring as far as the protocol goes.** The serial
  `LSLDSM7PRO20403551` reads as LDRobot LS / LDS / M7 Pro, and its tail matches the
  `…_20403551` in the AP SSID. This *is* the platform the firmware sample came from;
  `6716` is a model or batch marker, not a different stack. `PROTOCOL.md` §C's command
  set applies as written.
* **Only the port range was wrong.** `rand()%1000 + 9000` does not describe this
  firmware — 7913 is outside it entirely. **7913 survived a power cycle**, so on this
  unit it is a *fixed* port, not a per-boot random pick; the RNG behaviour in the
  firmware notes does not apply here either. `tools/rehome.py` still sweeps
  `7000-9999` by default rather than hardcoding it, since one unit is not a rule, and
  takes `--discover-ports` for anything wider.
* The "provisioning service only listens in a window" hypothesis is dead; it was
  listening the whole time, just not where anyone was looking.

#### `getWifi` answers with an **empty datagram**

```
tools/rehome.py -p 7913 scan
  error: non-JSON reply from 192.168.78.1:7913: ''      # a zero-length UDP packet
  no reply, retrying (1/2) ... (2/2)
```

The device *answers* — a zero-length datagram is a real packet, not silence — and then
goes quiet. The likely mechanism: `getWifi` starts a real scan, which takes the radio
off the channel it is currently serving the AP on. So the empty datagram is an
immediate acknowledgement, the result follows seconds later, and anything sent into
that window (our retries) is lost while the radio is away.

`tools/rehome.py scan` therefore waits `--scan-timeout` (20s default) and does **not**
retry. Whether the late result actually arrives is unconfirmed. `getWifi` is a
convenience only — `setSta` takes the SSID as a string — so this does not block a
re-home.

#### `getCfg` returned nothing

No `staName`, `staPwd`, `staIp` or `staMac` — just `{"cmd","result"}`. Either this unit
holds no station credentials (consistent with `checkPwd` code `0`, and with the ARP from
an APIPA address in the earlier entry), or this firmware omits the fields. If it is the
former, **the DNS-override shortcut below is not available on this unit**: it has no
network to rejoin, so it has to be told one over channel C first. Worth re-checking
`getCfg` after a successful `setSta`, which also settles which reading is right.

#### Consequence for noobscenic

Nothing here contradicts the **channel A/B** contract, and finding 5 is positive
evidence that this unit talks to the same vendor infrastructure the firmware sample
does. Only **channel C** — the convenience of setting the cloud URL over the air — is
in question.

With 7913 found, **channel C is the route for this unit** and the normal
`rehome` sequence applies — pass `-p 7913`, or let `discover` find it.

The **DNS-override route** (`REPORT.md` §6.1) remains the fallback, and is still the
better answer for a unit already living on the owner's LAN: point
`mobile.proscenic.cn` at the noobscenic host from the router or a local resolver. The
device does no TLS validation (`REPORT.md` §3) and `getSockAddr` is answered by us, so
it yields full control with no pairing protocol at all. It needs the robot to already
have credentials, which — see `getCfg` above — this one may not.

### 2026-09-11 — `setUrl` works, `setSta` is refused

```
-> {"cmd":"setUrl","url":"http://192.168.1.208:8080/"}   <- {"result":"ok"}
-> {"cmd":"setUrl","ip":"192.168.1.208","port":8081}     <- {"result":"ok"}
-> {"cmd":"getCfg"}                                      <- {"result":"ok"}      (still no fields)
-> {"cmd":"setSta","ssid":"<ssid>","staPwd":"<pass>"}    <- {"result":"fail","code":-1}
```

**Both `setUrl` forms are accepted and persist.** The channel-A base URL and the
channel-B gateway address are already set on this unit; only getting it onto a network
remains.

**`setSta` is refused in ~25 ms.** That timing is the useful part: `PROTOCOL.md` §C says
the handler checks the passphrase length and then shells out to
`cleanpack_mode -m sta -s … -p …`. Twenty-five milliseconds is too quick to have run
that script, so this is the **validation** rejecting the request, not the Wi-Fi join
failing. Candidates, in order:

1. ~~**Wrong passphrase field.**~~ **Confirmed, but it was the *SSID* field, not the
   passphrase** — see the next entry.
2. **A successful `getWifi` may be a precondition.** The app's flow is scan → user picks
   → `setSta`, so the firmware may require the SSID to appear in the results of its last
   scan. `getWifi` does not currently complete on this unit (above), which would
   explain both failures with one cause.
3. **Band.** These units are typically 2.4 GHz only; a 5 GHz-only or band-steered SSID
   would not be in a scan list even if the scan worked.
4. **Passphrase characters.** The handler interpolates the passphrase into a shell
   command (`-p "<password>"`), so `"`, `$`, `` ` `` or `\` could break it — though that
   would fail *after* running the script, not in 25 ms.

### 2026-09-12 — **`setSta` wants `staName`, not `ssid`** ✅

Probing all 31 plausible field-name combinations settled it in a quarter of a second:

```
ssid    + staPwd|pwd|password|passwd|psk|key   ->  {"result":"fail","code":-1}
staName + staPwd                               ->  {"result":"ok","code":2}
```

So the correct request for this firmware is:

```json
{"cmd":"setSta","staName":"<ssid>","staPwd":"<8-64 char passphrase>"}
```

`PROTOCOL.md` §C documents `ssid` + `staPwd`, taken from the `LS_S6` firmware's JSON
templates and flagged there as needing confirmation against a live capture. For this
unit the passphrase key is right and **the SSID key is wrong**. It is also the
self-consistent answer: `getCfg` is documented to *return* `staName`/`staPwd`, so the
setter using the same pair is what you would expect — the notes' `ssid` looks like the
odd one out.

`tools/rehome.py` therefore defaults to `--ssid-key staName`, with `ssid` still
selectable. `tools/fakerobot.py` mirrors the real unit and refuses `ssid`, so a
regression fails on the desk rather than on the bench.

**Do not read anything into `applyCfg`.** Probing it reported `ssid + staPwd`
"accepted" with `code:1` on the first try, but a generic config-apply that returns
`ok` regardless of which fields it understands will look exactly like that. It proves
nothing about field names.

#### ⚠ Windows: `SIO_UDP_CONNRESET` silently breaks UDP sweeps

`discover` found nothing from a Windows host while `-p 7913 info` worked perfectly
against the same device seconds later. The cause is not the robot: on Windows, an
**unconnected** UDP socket that draws an ICMP port-unreachable fails the *next*
`recvfrom` with `WSAECONNRESET`. A sweep across ~3000 closed ports produces a stream of
those errors and the one genuine reply is lost behind them; a single unicast to an open
port generates no ICMP and works. This also explains the very first capture, where the
probes were provably on the wire and the application saw nothing.

`tools/rehome.py` now calls `sock.ioctl(socket.SIO_UDP_CONNRESET, False)` (a no-op off
Windows) and drains replies *during* the sweep rather than only after it. Anyone
writing another scanner against this device from Windows needs the same.

### 2026-09-11 — serving the robot **over its own soft-AP**

The re-home does not actually require `setSta`. The robot reaches the vendor cloud
through whatever `setUrl` holds, and `192.168.78.0/24` is **directly connected on its
AP interface**, so a server on that subnet is reachable with no Wi-Fi join, no default
route, and no DHCP. The evidence that it will try is in the first entry above: while
sitting in AP mode it ARPs once a second for an Alibaba address, i.e. it is already
attempting to talk to the cloud from that state.

So: join the robot's AP, point it at *this machine's* `192.168.78.x` address, and run
the server there. `tools/rehome.py point-here` does exactly that — it detects the local
address on the route to the robot and issues both `setUrl` forms, touching nothing
else.

Caveats worth knowing before trying it:

* `network_proxy` may only read `/data/bin/Run/Config/url` at start-up, so a
  power-cycle may be needed for the new URL to take effect. (The `ip`/`port` form is
  documented to trigger a gateway reconnect, so that half may apply immediately.)
* Give the machine a **static** address on `192.168.78.0/24` before power-cycling, or
  the DHCP pool (`.50-.150`) may hand it a different one while the robot is rebooting,
  and the URL will point at nothing.
* The machine serving the robot has no internet while joined to that AP. noobscenic
  needs none.

#### Still open

* **Did the accepted `setSta` actually make it join?** `result:ok, code:2` is the
  handler accepting the request, not proof the association succeeded. Check `checkPwd`
  (a non-zero `code` should mean connected), `getCfg`, the router's client list, and
  the server's traces.
* **Does the late `getWifi` result ever arrive?** Re-run `scan` now that it waits 20s
  without retrying.
* **`getCfg` after `setSta`** — settles whether the empty reply means "no credentials"
  or "firmware omits the fields".
* Capturing the real app's pairing session is no longer needed to make progress, but it
  would still confirm the field names flagged as unverified in `PROTOCOL.md` §C
  (`staPwd` vs `pwd`). The AP is open, so an over-the-air monitor-mode capture needs no
  key. On Windows, Npcap's monitor mode depends on the NDIS driver and most Intel parts
  do not support it; a cheap USB adapter (AR9271, RT5372, MT7612U, RTL8812AU) with
  Linux `iw dev … set type monitor` is dependable. Capturing on the phone instead —
  PCAPdroid on Android, `rvictl` from a Mac for iOS — avoids the hardware entirely.
