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
| Channel-C `getID` on UDP 9000-9999 | discovery/identify | **nothing listening** |
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

#### Consequence for noobscenic

Nothing here contradicts the **channel A/B** contract, and finding 5 is positive
evidence that this unit talks to the same vendor infrastructure the firmware sample
does. Only **channel C** — the convenience of setting the cloud URL over the air — is
in question.

That makes the **DNS-override route** (`REPORT.md` §6.1) the preferred path for this
unit: if it still holds the owner's Wi-Fi credentials, take it out of pairing mode, let
it rejoin the LAN, and point `mobile.proscenic.cn` at the noobscenic host from the
router or a local resolver. The device does no TLS validation (`REPORT.md` §3), and
`getSockAddr` is answered by us, so that yields full control with no pairing protocol
needed at all.

#### Next steps

* **Full TCP scan.** Only UDP 9000-9999 has been swept; TCP has never been scanned
  except incidentally on 53. Many units of this class pair over TCP or HTTP.
  `tools/rehome.py portscan --proto tcp` or `nmap -sS -p- 192.168.78.1`.
* **Wider UDP sweep**, paced so the ICMP rate limit does not turn closed ports into
  ambiguous non-answers: `tools/rehome.py portscan --proto udp --udp-ports 1-65535
  --pace-ms 20`.
* **Passive listen** in case the announcement is periodic but rare:
  `tools/rehome.py listen --duration 300`.
* **Capture the real app pairing.** The AP is open, so an over-the-air monitor-mode
  capture needs no key and yields plaintext. The APK is packed (Qihoo 360 Jiagu, see
  `APK_PACKING.md`), so this is the only practical route to the app side. On Windows,
  Npcap's monitor mode depends on the NDIS driver and most Intel parts do not support
  it; a cheap USB adapter (AR9271, RT5372, MT7612U, RTL8812AU) with Linux `iw dev …
  set type monitor` is the dependable option. Capturing on the phone instead —
  PCAPdroid on Android, or `rvictl` from a Mac for iOS — avoids the hardware entirely,
  at the cost of possibly missing inbound broadcast.
