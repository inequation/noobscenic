# Field Notes — live re-home attempt vs. the analyzed firmware

**⚠ Applicability:** all of `PROTOCOL.md`, `REPORT.md`, `MAP.md`, and `schemas/` were
reverse-engineered from the **2020 `LS_S6` CleanPack firmware sample** (`firmware.bin`,
app version 0.7.1, `versionCode`/`GetGitCnt` = 1241). The first attempt to re-home a
**physical** Proscenic unit showed that unit runs **different firmware** whose local
pairing protocol does **not** match the sample. This file records what was observed on
the wire so the handoff isn't misread as applying verbatim to every unit.

Marking: **[field]** = observed from the live capture (`robot.pcapng`, a NIC-level
pktmon capture reframed from 802.11 → L3); **[static]** = from the firmware analysis.

## The physical unit is a different variant
| Property | Analyzed `LS_S6` sample **[static]** | Physical unit **[field]** |
|---|---|---|
| Pairing SSID | `LDRobot` (hardcoded in `apDemo`/`RkLunch.sh`) | **`Proscenic-6716_20403551`** (model id `6716`, not referenced anywhere in the sample) |
| Soft-AP gateway IP | `192.168.78.1` | `192.168.78.1` — **matches** |
| dnsmasq on the AP | yes (`apDemo`) | yes — **DNS on UDP+TCP :53 confirmed** |
| Channel-C UDP `{"cmd":...}` listener | present on random `9000–9999` (`OpenUdpRemoteCtrl`, `rand()%1000+9000`) | **present on fixed UDP `7319`** (owner-confirmed; the sweep below missed it — see correction) |

> **CORRECTION (owner report):** Channel C **is** present on the real M7 Pro unit — on
> a **fixed UDP port `7319`**. The sweep below only probed `9000–9407` (the `LS_S6`
> sample's port range) and therefore missed `7319`; its "absent" conclusion was a
> wrong-port false negative. The capture facts below are accurate for the range that was
> scanned; the conclusion that Channel C is absent is retracted.

## What the capture proves [field]
Capture = 3375 frames, 12 s, during `rehome.py discover/info` against `192.168.78.1`.
- **The `getID` probe was correct:** 3048 UDP datagrams with payload
  `7b22636d64223a20226765744944227d` = `{"cmd": "getID"}` were sent — matching the
  documented Channel-C probe byte-for-byte.
- **They were unanswered at the application layer, because the ports are closed:** the
  robot returned **ICMP type 3 / code 3 (destination port unreachable)** for probes
  across the `9000–9407` range (75 such replies; the rest silently dropped by Linux
  ICMP rate-limiting, not by an application). ICMP port-unreachable is the robot's own
  network stack saying **nothing is bound to those UDP ports**. This is *not* a
  capture-side firewall/OS artifact (the capture is below the OS stack, and the
  rejections come *from* `192.168.78.1`).
- **No other robot listener was reached:** the only TCP the robot accepted was `:53`
  (dnsmasq TCP-DNS). The robot emitted **no** discovery beacon, mDNS, or broadcast of
  its own. The DNS traffic seen is the Windows capture host's background name lookups
  (office365/azure/teams/slack/…), which dnsmasq **REFUSED** (rcode 5) — irrelevant to
  pairing.
- **Coverage gap in the attempt:** probes were sent **unicast to `192.168.78.1` only**;
  **no broadcast** (`192.168.78.255`) was tried. (Given the ports are closed on `.1`,
  a broadcast to the same ports would not have helped, but a different port/scheme was
  never exercised.)

## Implications for the RE documentation
1. **Channel C is present on the `6716`/M7 Pro unit — on fixed UDP `7319`** (owner-confirmed),
   not the `LS_S6` sample's random `9000–9999`. The earlier "not present" finding was a
   wrong-port sweep (`9000–9407` only). Target **UDP `7319`** on shipping units; still
   re-verify the port on any given unit.
2. **Channels A and B (cloud REST + push gateway) are UNTESTED on the `6716` unit.**
   They are cloud-facing and *may* still match (the platform is the same LDRobot base,
   and `192.168.78.1`/dnsmasq behavior matches), but this has not been observed. Treat
   the A/B contract as validated only for the `LS_S6`/2020 firmware until a `6716`
   capture confirms it.
3. **The firmware-signing/format analysis and `proscenic_fw.py` are unaffected** by this
   — they concern the update container, not the live protocol. But note the `6716`
   unit's actual on-flash version was not obtained; the analyzed image may be older.

## Recommended next steps (to actually re-home a `6716` unit)
In priority order, because the local protocol must be observed or extracted, not assumed:
1. **Capture the real Proscenic app pairing this unit.** Join a phone + a sniffing host
   to the `Proscenic-6716_…` AP (it is open/`192.168.78.1`), run the vendor app's
   "add device" flow, and capture on the AP subnet. This directly reveals the real
   local protocol (port, transport, JSON/other, discovery — broadcast vs unicast).
   *(If the app is fully delisted/dead, this may require the APK's pairing flow, which
   is Jiagu-packed — see `APK_PACKING.md`.)*
2. **Port-map the unit in AP mode** beyond the narrow UDP sweep: full `nmap -sU`/`-sT`
   of `192.168.78.1` (and a broadcast `getID`/`getSn` to `192.168.78.255`) to find the
   actual listener, if any.
3. **Dump this unit's firmware** (model `6716`) and RE its provisioning — the 2020
   `LS_S6` analysis is the map, not the territory, for this variant. Root via UART is
   the usual entry (see `REPORT.md` §5); the debug surfaces there may also differ.
4. **If root is obtained**, the config-file shortcut in `PROTOCOL.md` §C still applies
   in principle (write the cloud URL / STA creds directly) — but verify the paths
   (`/data/bin/Run/Config/url`, `ip_port.json`, `/data/cfg/wpa_supplicant.conf`) exist
   on the `6716` firmware; they may have moved.

## Raw evidence
`/media/sf_reshell-shared/robot.pcapng` (802.11, Ethernet-mislabeled by `etl2pcap`;
reframe with the LLC/SNAP `aa aa 03 00 00 00` marker → L3, as in
`tmp/proscenic/pcap/reframe.py`), converted from `robot.etl`. Findings summary in
`/media/sf_reshell-shared/findings.txt`.
