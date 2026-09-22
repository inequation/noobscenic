# Proscenic M7 Pro — Firmware & Update-Protocol Reverse Engineering Report

**Target:** Proscenic M7 Pro robot vacuum (vendor platform: **LDRobot**, internal
model **`LS_S6`**, SoC **Rockchip RK3308**, AArch64/ARM64 Linux).
**Sample:** `proscenic/firmware.bin` — SHA‑256 `de751f0580a48ea602f343aa9db0dc085d2df74587567c98b936f1a2890f27d8`
(3,476,576 bytes). Build user `jenkins`, build date **2020‑06‑30**, package id
**`rk3308-LS_S6-0_7_1-NormalServer`** (application version **0.7.1**).
**Device on LAN:** `192.168.1.243` (reachable; all probed TCP ports *filtered* — no inbound services exposed in station mode).

This document is self-contained. Everything marked **[proven]** was demonstrated
by executing the code path or by cryptographic verification; **[static]** was read
from decompiled/disassembled code; **[live]** was confirmed against the running
cloud/device.

> **⚠ Scope:** the analysis is of the **2020 `LS_S6` sample**. A physical unit tested in
> the field runs **different firmware** (SSID `Proscenic-6716_…`, model `6716`) whose
> **local pairing protocol differs** — the documented Channel-C UDP listener is absent
> on it. See **`FIELD_NOTES.md`**. The firmware-format/signature analysis is unaffected;
> the live cloud protocol (Channels A/B) is untested on that unit.

---

## 0. TL;DR

* `firmware.bin` is **not** the full system image. It is the **application-layer
  “CleanPack” package**: a **128-byte RSA‑1024 signature** followed by a
  **gzip’d GNU tar** of the robot’s userspace (`/data/bin` overlay). The full OS
  (`ota-sys.img`, kernel/rootfs) is delivered by a *separate* Rockchip
  `recoverySystem` OTA that is **not** present in this sample. **[proven]**
* **Signature scheme is genuine and enforced:** `sig = RSA_priv_encrypt( PKCS#1‑v1.5 ‖
  ascii_hex(md5(body)) )`; the device verifies with a **public key embedded in
  `bin/updater`**, and a failure aborts the update (`RsaDec fail`). The private key
  is **not** on the device, so images **cannot be forged** without it. **[proven]**
* Weaknesses: **RSA‑1024 + MD5** (both below modern standards), firmware is served
  over **plain HTTP** with **no TLS validation**, and integrity of the *download*
  is only an MD5 in a server-supplied JSON. None of these let you install
  arbitrary code by themselves, because the RSA signature still gates extraction.
* **Continued usability without breaking crypto is achievable:** the device stores
  its cloud URL in `/data/bin/Run/Config/url` and honours a local **`setUrl`**
  provisioning command; standing up a replacement cloud + `setUrl`/`setSta` keeps
  the vacuum working **without touching signed firmware**. Root-level custom code
  needs one of the debug surfaces below (UART/adbd/dropbear/USB), not a forged OTA.
* A ready-to-use tool — **`proscenic_fw.py`** — packs, unpacks, verifies, and
  (with a private key) signs these images. It verifies the real firmware as
  `SIGNATURE: VALID`.
* **Newer firmware?** The exact URL from the RE repo
  (`http://47.254.154.181/robotDrivers/1593772189366`) is **still live** and serves
  a **byte-identical** copy of this 0.7.1 image. The cloud OTA/version endpoints are
  alive but **session-gated** (`code:102 "your sid expired"`), so discovering a
  newer build requires a valid device session (account login), which we do not have.

---

## 1. Firmware container format  *(Goal: update format)*

```
firmware.bin
┌───────────────────────────────────────────────────────────────────────────┐
│ 0x0000 .. 0x007F   (128 bytes)   RSA-1024 signature block                   │
│                                   RSA_public_decrypt(sig) =                  │
│                                   00 01 FF … FF 00 ‖ ascii_hex(md5(body))    │
│                                   (PKCS#1 v1.5, type-1 padding; 32 ASCII B)  │
├───────────────────────────────────────────────────────────────────────────┤
│ 0x0080 .. EOF      body =        gzip( GNU tar( ./<rootfs overlay> ) )       │
│                                   gzip mtime 2020-07-03; tar owner jenkins   │
└───────────────────────────────────────────────────────────────────────────┘
```

**How it was proven.** Raw `RSA^e mod n` of the first 128 bytes with the embedded
public key yields textbook PKCS#1 v1.5 type‑1 padding whose 32‑byte payload is the
ASCII string `142afe5db6be8d1087c96bd58a87bfe0`, which equals `md5(firmware.bin[128:])`
exactly. `firmware.bin[128:]` is a valid gzip stream decompressing to a 6,133,760‑byte
GNU tar. No trailing bytes. **[proven]**

**Two MD5 values exist — don’t confuse them:**
| MD5 | Value | Role |
|-----|-------|------|
| `md5(body)` = `md5(firmware.bin[128:])` | `142afe5db6be8d1087c96bd58a87bfe0` | **signed** payload digest (inside the RSA block) |
| `md5(whole firmware.bin)` | `b139850207a7d5b218e928b6e67bc98f` | **transport** digest the cloud puts in the update JSON |

### 1.1 Verifier internals (`libcpc.so :: RsaEncDec::DecFile`, `bin/updater`) **[static/proven]**
`DecFile(pubkeyPEM, inPath, outPath)`:
1. Read exactly **128** bytes; abort if not (`DecFile 0xbb`).
2. `InitPubKeyFormString(pubkeyPEM)` → `DoDecryptWithPubKey` = `RSA_public_decrypt`
   with PKCS#1 padding; result **must be 0x20 bytes**, else abort (`DecFile 0xc5`).
3. Stream the remaining bytes (the gzip body) to `outPath`.
4. Recompute `md5(outPath)`, format as 32 hex chars, `memcmp` against the RSA‑recovered
   32 bytes → equal ⇒ `DecFile 0xde` **true**; else `DecFile 0xe1` **false**.

Caller `updater` (`updater.cpp`): on **true** → status 4 (proceed to extract); on
**false** → `PostEvent("RsaDec fail")`, delete the output, status 2 / error −5, **abort**.
So the signature check is a hard gate. **[static]**

### 1.2 Update package contents (the tar) **[proven]**
Userspace overlay copied to the A/B slot `/data/bin/CleanPackApp{,2}`:
`bin/` daemons (`navigator`, `slam_pose_provider`, `clean_task`, `charge_task`,
`find_charger_task`, `task_manager`, `carrier`, `network_proxy`, `event_hub`,
`updater`, `config_node`, `time_server_tactics`, `remote_control_task`,
`onboard_ui`, `cpm`, `idle_task`, libs `libcpc.so`/`libalong_wall.so`), boot/OTA
scripts (`RkLunch.sh`, `run.sh`, `final.sh`, `user_start.sh`, `factory_reset.sh`,
`save/load_all_from_flash.sh`, …), the **MCU motor-controller image**
`cleaner_driver_h185v70.bin` (own 12‑byte length/type header, encrypted body,
hardware-version gated), configs (`configs.json.default`, `node_config.json`,
`shm.json`), voice prompts (`music/`), and Wi‑Fi provisioning helpers
(`apDemo`, `cleanpack_mode`, `ap_client`, `connect2ap`).

---

## 2. Update protocol & delivery  *(Goal: update protocol + the URL)*

### 2.1 Two independent update layers **[static/proven]**
1. **Application layer (this file).** `updater` fetches the package (libcurl),
   validates it (§1.1), extracts to `/tmp/CleanPackTmp/`, checks
   `/tmp/CleanPackTmp/dev_name == "LS_S6"` (hardware gate), copies binaries to the
   *other* A/B slot (`AppRom` flips 1↔2), then `final.sh` optionally flashes the MCU
   and, **if** the package contained `ota-sys.img`, hands the full-system image to
   `recoverySystem ota …` (recovery-partition reflash). This sample has **no**
   `ota-sys.img`, so it only updates the app layer.
2. **System layer.** Full kernel+rootfs image `ota-sys.img`, flashed via the
   Rockchip **recovery** partition (or `fw_setenv swu_* ; reboot -f boot-recovery`
   on the `mr112` variant). Its authenticity mechanism is *outside* this sample.

### 2.2 Cloud/OTA control flow (`network_proxy`, `ota.cpp`/`get_sync.cpp`) **[static/live]**
* Default cloud host **`https://mobile.proscenic.cn/`** (also `…com.de`); base URL
  overridable and persisted to `/data/bin/Run/Config/url`; TCP push-gateway in
  `/data/bin/Run/Config/ip_port.json`.
* Device→cloud REST surface (form-urlencoded):
  `cleanPack/{register, binding, unbinding, getSockAddr, sync, response,
  uploadEvents, uploadLogs, uploadSingle, uploadStats}`.
* **Version/OTA check:** `GET <base>?version=1&sn=<SN>&companyId=<id>` and the device-attr
  sync `POST cleanPack/sync` with
  `sn&companyId&mcuVer&version&versionCode&gitSha&cloud=psnk`. Response JSON drives OTA
  (`code`, `hasUpdateFile`, `downUrl`, `fullversion`, transport `md5`). `code:0`=ok,
  `code:102`/`212`=session/sid expired. **[live: `sync` returns `code:102 "你的sid过期啦"`]**
* **File download** (firmware/OSS) uses a *signed* query:
  `itemId=5&clientId=11&fileTypeId=<n>&fileName=<name>&from=mpc_clean_firmware&time=<t>&_taskid=<tid>&_sign=<sig>`
  served from an Alibaba OSS/openresty host (`47.254.154.181`, path `robotDrivers/<ms-timestamp>`).
* `DownLoadAndCheck`: download → compare `md5` to the JSON’s md5 (**transport
  integrity only**) → pass to `updater` → RSA gate (§1.1).

### 2.3 The firmware URL & “is there a newer build?” **[live]**
* RE-repo URL `http://47.254.154.181/robotDrivers/1593772189366` → **HTTP 200**,
  `Content-Length: 3476576`, `Last-Modified: 2020-07-03`; downloaded copy is
  **byte-identical** (same SHA‑256) to `firmware.bin`. Unchanged since 2020.
* Both cloud hosts and the OSS host are **alive** in 2026, but:
  * `cleanPack/sync` requires a valid **`sid`** (session token) — returns `code:102`
    without one. The `sid` is minted by device registration / app login, which needs
    the owner’s Proscenic account credentials (not available here).
  * OSS directory listing is `403`; only exact `downUrl`s (revealed by an
    authenticated sync) are fetchable.
* **Conclusion:** No newer firmware is discoverable through unauthenticated means;
  the only published URL still serves 0.7.1 verbatim. A definitive check requires a
  valid session — either the owner runs `getSockAddr.sh` with their credentials and
  queries `cleanPack/sync`, or the live device is allowed to perform its own OTA
  check. If such a build surfaces, `proscenic_fw.py verify`/`unpack` will analyse it
  the same way. *(No newer image was downloaded because none was reachable.)*

---

## 3. Security properties  *(Goal: checksums, signatures, secure boot)*

| Property | Finding | Severity |
|---|---|---|
| **App-firmware authenticity** | RSA‑1024 PKCS#1‑v1.5 signature over `ascii_hex(md5(body))`, embedded public key, **enforced** (`RsaDec fail` aborts). Forgery needs the vendor private key, which is **not on the device**. | Baseline OK, weak primitives |
| **Signature key strength** | **RSA‑1024** — below the 2048‑bit/112‑bit floor; factorable only by very high-resource adversaries, not casually. | Medium |
| **Digest** | **MD5**. Collisions are trivial but do **not** break this scheme: an attacker cannot retro-fit a second image to an already-signed digest (that is a *second‑preimage*, still ~2¹²⁸). MD5 would only matter if the vendor could be induced to sign attacker-chosen input. | Low-Medium |
| **Download transport** | Firmware fetched over **plain HTTP**; robot does **no TLS validation** (RE-repo Finding 3, consistent with `curl` opts). MITM can swap the image/JSON, but the RSA gate still rejects unsigned/modified images. | Medium (defense-in-depth gap) |
| **Download integrity** | Only an **MD5 in a server-supplied JSON** — attacker-controlled under MITM; not a security control. | Info |
| **Secure boot (SoC)** | RK3308 supports it, but this app-layer sample cannot confirm whether it is fused/enabled. The `ota-sys.img`/recovery path (not in sample) is where SoC secure-boot would apply. **Unknown from this sample.** | Unknown |
| **MCU image** | `cleaner_driver_h185v70.bin`: 12‑byte header (len/ver/type) + encrypted body, hardware-version gated (`h185v70`). No RSA over it in the app layer. | Info |
| **Cloud auth** | Session `sid`/`token`; RE-repo notes tokens don’t expire and some endpoints need only a username — server-side authz weaknesses (out of firmware scope). | Medium (cloud) |

**Bottom line:** you **cannot** ship a forged *signed* OTA without the vendor’s
RSA‑1024 private key. Custom code must come via a device-local trust bypass
(§5), or you sidestep firmware entirely by re-homing the device to your own cloud (§6).

---

## 4. Protocol surfaces & functionality  *(Goal: enumerate everything)*

### 4.1 Process/IPC map (`node_config.json`) **[proven]**
ZeroMQ over Unix-domain IPC, localhost ids 7801–7821:
`carrier(7801/2)`, `slam_pose_provider(7803)`, `event_hub(7804/5)`,
`navigator(7806)`, `task_manager(7807)`, **`debug_proxy` — TCP 7808/7809/7813**,
`network_proxy(7810)`, `time_server_tactics(7811)`, `config_server(7812)`,
`clean_task(7814)`, `find_charger_task(7815)`, `remote_control_task(7816)`,
`updater(7819)`, `av_task(7821)`. **All are 127.0.0.1** (station mode exposes none
externally — matches the live scan: every probed port *filtered*). The **only TCP**
node is `debug_proxy` (see §5).

### 4.2 Local Wi-Fi/provisioning command set (`network_proxy` local_debugger/zmq/udp) **[static]**
JSON `{"cmd": …}` handled locally (used by the app during AP-mode pairing):
`getID, getSn, getWifi, getCfg, applyCfg, checkPwd, setAp, setSta, resetWifi, setUrl`
and log ops `{"req":"getLog"/"rmLog"}`. **`setSta`** (join a Wi-Fi network) and
**`setUrl`** (set the cloud base URL) together let you fully re-home the device.

### 4.3 Cloud REST surface — see §2.2.

### 4.4 Internal event/command catalog **[static]**
**355 `EID_*` identifiers** (full list in `eid_catalog.tsv`), grouped:
`C` command (46, e.g. `EID_C_APP_REQUEST_FACTORY_RESET`), `E` error (73),
`I` info (211), `N` clean-notify (12), `W` warning, `R` report, `D` **debug**
(`EID_D_DEBUG_EVENT_BEGIN`). This is the authoritative functional map of the robot
(cleaning modes, docking, mapping, forbidden zones, water/fan levels, errors, etc.).
Numeric `infoType`s seen as literals: `10001` (login/token), `20002` (map upload),
`20004`, `21006`, `21011` (path), `21020`; app→device control is relayed as
`POST /instructions/{sn}/{cmdCode}` (cloud), e.g. `21012` charge start (RE-repo Finding 2).

### 4.5 Privacy note (confirms RE-repo Finding 4) **[static]**
`network_proxy` uploads home maps/paths (`cleanPack/uploadEvents`, `map_send.cpp`,
`path_send.cpp`, LZ4-compressed occupancy grids). The device is a mapping/telemetry
client of the vendor cloud by design.

---

## 5. Hidden / debug functionality & access  *(Goal: hidden/debug)*

| Surface | What it is | How it’s gated / accessed |
|---|---|---|
| **`debug_proxy`** | Debug daemon on **TCP 7808/7809/7813** (only TCP node). Ships as a *debug* add-on, **not** in the release tarball; `factory_reset.sh` deletes it from both A/B slots. | Present only when a debug/data-collect package was installed; bound to localhost. |
| **`adbd`** | Android Debug Bridge daemon, enabled by an init script `/userdata/cfg/init.d/adbd`. `factory_reset.sh` removes it. | Root shell if that init file exists → strong local-root vector. |
| **`dropbear`** | SSH server via `/userdata/cfg/init.d/dropbear`; also removed by factory reset. | Root SSH if the init file exists. |
| **`data_collect` + `upload_file.sh` + gdb** | `user_start.sh` auto-starts `data_collect` and `upload_file.sh` (log/coredump upload) when `upload_file.sh` is present; comment literally says “start gdb”. | Enabled by dropping `upload_file.sh` into the app dir. |
| **USB-stick update** (`data/cfg/scripts/udisk_mount.sh`) | On USB insert, runs `udisk_export` on a **magic path** `/<mnt>/CleanPackDir_System_XX_f12f2c909e2a4d67576b85344aee66e2/CleanPackImg.scv`; produces `/tmp/CleanPackDecDir` and runs `UdiskRemove.sh`. A **local, physical** update/recovery channel. | `udisk_export` lives in the base rootfs (not this sample); its validation of `.scv` is the thing to audit next — potential offline code-exec. |
| **Factory test mode** (`data/cfg/scripts/automount_tty.sh`) | Inserting a serial/tty device triggers `/data/bin/Factory/BeforeCheckAll.sh` / `AfterCheckAll.sh` (production test jig). | Physical (USB-serial); `Factory/` dir not in this sample. |
| **Wi-Fi monitor mode** (`cleanpack_mode -m mon`) | Puts `wlan0` into 802.11 monitor mode. | Local command. |
| **`setUrl` / `setSta`** | Re-point cloud + Wi-Fi (see §6). | Local provisioning command (AP pairing) or app. |
| **UART console** | RK3308 boards expose a serial console; `RkLunch.sh`/U-Boot are the natural root entry if adbd/dropbear absent. | Physical UART (typical first step). |

The RSA public key, the crypto, and no private key are confirmed to exist **only**
in `updater`/`libcpc.so`; nothing sensitive is in the rootfs payload.

---

## 6. Keeping the M7 Pro usable (recommended, no crypto break needed)

Because the app firmware is properly signed but the device **trusts a configurable
cloud URL over plain HTTP**, the sustainable path is to **replace the cloud, not the
firmware**:

1. **Re-home the device** with the local provisioning commands: `setSta` (join your
   Wi-Fi) and **`setUrl`** (point `/data/bin/Run/Config/url` at your server), or via
   DNS override of `mobile.proscenic.cn` + `ip_port.json` for the push gateway.
   **⚠ Field caveat:** this Channel-C path worked in the `LS_S6` firmware but was
   confirmed **absent** on a physical `Proscenic-6716` unit (UDP `9000–9999` closed —
   `FIELD_NOTES.md`). On such a unit, first capture the vendor app's pairing to learn
   the real local protocol, or get root (UART) and write the config files directly.
2. **Re-implement the LDRobot cloud** endpoints you actually need
   (`cleanPack/register`, `getSockAddr`, `sync`, `response`, `uploadEvents`) and the
   Channel-B gateway framing. **See `PROTOCOL.md` for the exact, corrected contract** —
   in particular: `register` must return `data.session` (its first 16 bytes are the
   AES-128-ECB key) **and** `data.cookies` (the sid, echoed as `Cookie: cookies=…`);
   device→cloud `data` is **plaintext** JSON, only cloud→device commands are
   AES-128-ECB; and the server **must answer the `21006` Ping** keepalive or the robot
   reconnects. The `EID_*`/`infoType` catalog (`eid_catalog.tsv`) is the command map.
3. **Optional root** for deeper control (custom firmware without the vendor key):
   obtain a shell via UART or by enabling `adbd`/`dropbear` (or the USB `.scv`
   path), then either run your software directly or **replace the embedded public key
   in `updater`** so your own RSA key signs future packages (use
   `proscenic_fw.py genkey` + `pack --key`).

None of steps 1–2 modify signed firmware or require breaking RSA/MD5.

---

## 7. Deliverables in this folder

| File | Purpose |
|---|---|
| `REPORT.md` | This report — firmware format, update protocol, security, hidden/debug, live version check. |
| `PROTOCOL.md` | Device↔cloud + local protocol: 3 channels, endpoints, framing, **Channel-B AES/token**, re-home recipe. |
| `FIELD_NOTES.md` | Live re-home attempt vs. the sample: a physical `Proscenic-6716` unit **does not** expose Channel C (packet-proven); variant caveats + next steps. |
| `MAP.md` | Mapping / room-segmentation feature: occupancy grid, map-upload/region/path JSON, LZ4. |
| `APK_PACKING.md` | Why the Android app won't statically decompile (Qihoo 360 Jiagu packer) + component map. |
| `schemas/` | JSON Schema (Draft 2020-12) for every JSON API — `channelA_rest`, `channelB_gateway`, `channelC_local` (+ `README.md`). |
| `eid_catalog.tsv` | All 355 internal `EID_*` events **with numeric IDs** (recovered from `EventId2String::Init`). |
| `proscenic_fw.py` | Pack / unpack / **verify** / sign tool (see `README.md`). Verifies the real image as `SIGNATURE: VALID`. |
| `vendor_public_key.pem` | RSA‑1024 **firmware-signing** public key from `bin/updater` (device’s sole firmware trust anchor). |
| `README.md` | `proscenic_fw.py` usage & quick reference. |

Note: there are **three distinct RSA-1024 keys**, don't conflate them (verified by
`strings` — each binary holds exactly one PEM):
(1) `vendor_public_key.pem` — **firmware signing** (modulus starts `…CoPHLq…`);
embedded in **`updater` only** (not libcpc.so); the device's sole firmware trust anchor.
(2) `/data/bin/sys_data/public.key` — an on-device key file used to build the
Channel-A `register` `sig`.
(3) a separate public key embedded in **`libcpc.so` only** (modulus starts `…Czow…`);
`libcpc.so` provides the `RsaEncDec` primitives but does **not** carry the CoPHLq
signing key — purpose of the Czow key unconfirmed, not load-bearing for a replacement
server. See `PROTOCOL.md` §A/§B.

## 8. Cross-validation summary
* RSA verify (manual bignum) ⟷ `RsaEncDec::DecFile` decompile ⟷ `proscenic_fw.py verify`: **all agree** the signed value is `ascii_hex(md5(body))`.
* gzip/tar structure ⟷ `updater` strings (`CleanPack.tar.gz`, `tar zxf`) ⟷ `final.sh` deploy logic: **consistent**.
* Hardware gate `"LS_S6"` in `updater` ⟷ `dev_name` file in tar ⟷ `package_info`: **consistent**.
* Cloud OTA strings ⟷ live `cleanPack/sync` response (`code:102`) ⟷ `get_sync.cpp` code handling: **consistent**.
* Old firmware URL live download ⟷ local `firmware.bin`: **byte-identical (SHA‑256 match)**.

---

## 9. Live device version cross-check (device @ 192.168.1.243)

The owner's surviving app reports **compiled version `1241`** and **MCU `h185_v60`**.

* **`1241` = this firmware.** `libcpc.so :: GetGitCnt()` and `GetCleanPackGitCnt()`
  both return `mov w0, 0x4d9` = **1241**; `network_proxy` reports this as
  `versionCode` (`get_sync.cpp: versionCode = GetGitCnt()`). Git-count is
  build-unique ⇒ the device is running **exactly this 0.7.1 image**. **[proven]**
* **MCU `h185_v60` is one rev behind this package.** `mcuVer` is read live from the
  MCU (`carrier -mv`, cached `/tmp/AppRom/mcuversion`), not from the bundled file.
  This package carries `cleaner_driver_h185v70.bin`. Updater `FUN_004059a8` parses
  the bundled version (`70`) and flashes only if `current_mcu_ver < bundled`
  (`60 < 70` ⇒ true, non-fatal `"MCU upgrade fail?"`/`"Request mcu version failed"`
  paths let the app update finish anyway). So the unit has a **pending, un-applied
  v60→v70 MCU upgrade** that this same image would perform. **[static]**
