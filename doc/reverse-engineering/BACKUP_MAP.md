# Map backup: on-demand download and restore — resolved 2026-10-10

**Request:** can a replacement server (a) make the robot produce/upload a map backup at will,
and (b) push a stored backup back to the robot on demand? Answers, evidence and the corrected
model below. Underlying traces: `tmp/review_backup/UPLOAD_TRACE.md` /
`RESTORE_TRACE.md` (address-cited); this document is the digest.

## The model, corrected

* **(a) On-demand download does NOT exist.** There is **no inbound request of any kind** that
  makes the robot produce or upload a backup. The upload is driven by a **filesystem poller**
  inside `network_proxy`: every ~10 s it looks for record files in `/tmp/Run/CleanRecord/`,
  matches each to a `.bkmap` in `/tmp/Run/BackUpMap/`, and multipart-POSTs them. Those files
  are only ever created by `task_manager` **at clean end** (and the `.bkmap` only when the
  BKMap switch is on and the map save succeeded). So "produce a backup now" is reachable only
  by *running a clean that passes the record gate* (see A2), or not at all.
* **(b) Restore works, with caveats.** `21025 {"downUrl":…,"md5":…}` is implemented and
  undemanding (no CPS gate; interrupts a running clean; no reboot). But: the `code:0` reply is
  pushed **without waiting for the install** (the ordering is racy), the install's exit status
  is never checked, and there is
  **no way to read back which backup is installed**. A server should confirm by watching the
  post-restore map upload instead (see D5).

## A. Producing a backup (upload path)

**A1 — trigger.** No inbound infoType reaches the upload. Chain (verified):
`InterfaceObj::Init FUN_00457668` starts a never-stopped poller thread
(`FUN_004268a0 → FUN_00427870`, 10 s loop) whose callback `FUN_0044b9b8 → FUN_0044ab18` is
the **sole** way the multipart upload runs. `FUN_00427640` picks the oldest
`/tmp/Run/CleanRecord/*.txt` (by min start-time), prefix-matches
`/tmp/Run/BackUpMap/<name>*.bkmap`, waits 5 s, logs
`"Upload CleanFile %s And Map %s"`, and invokes it. The only gate inside `FUN_0044ab18` is a
latch (`mIsCmdExeFailed`, obj+8) that is set **permanently** if a file is still present after a
"successful" upload — i.e. if the server answered success but the robot couldn't delete the
file. There is **no force flag** (the `+0x80` "send now" pattern is map_send.cpp only) and no
CPS check anywhere in the chain.

**A2 — what creates the files.** `task_manager FUN_0041d9b0` writes
`/tmp/Run/CleanRecord/<sn>_<mapId>_<start>_<end>_<cleanArea>_0.txt` at clean end, and — only
when `tm+0x263 mEnableBKMap` is set **and** the map-save reply (ZMQ `0x7d5`) succeeded — packs
`/tmp/Run/BackUpMap/<same-name>.bkmap` via
`sh /tmp/AppRom/save_backup_map.sh "/tmp/Run/BackUpMap/" "<name>.bkmap"`. The tm gate before
any file is created: **`cleanTime` ≥ 60 s**, path length > 3, start time
sane (> 2018). An `IsRunningMode` early-exit (`/data/cfg/running_mode`, log *"Running mode,
will not save clean record"*) suppresses record creation entirely. (The 1 MiB directory check
is **not** a gate — see A5.) **The 35-second-run uploads**: two code-consistent explanations, not statically
separable — (1) the poller drains the *oldest queued* record and files persist until the server
succeeds (a 0–10 s+5 s poll explains the observed ~7 s); (2) tm's `cleanTime` is a
session-composite counter (per-mode deltas), so a 35 s wall-clock run can present ≥60 s. Either
way, the ≥60 s gate applies to record *creation*, not to the observed upload time.

**A3 — COMMANDS.md correction.** `backupMap`, `backupMapMd5`, `cleanFile` are **outbound
only** — multipart part names / JSON key emitted by `FUN_0044ab18`'s builder (single reference
each in the image; no `isMember`/parse use anywhere; the bytes are absent from task_manager).
They are **not** inbound command names. The real inbound backup keys are **`downUrl` + `md5`**
(21025). `COMMANDS.md` §"Firmware-internal command names" is corrected accordingly.

**A4 — no "send now" analogue.** See A1: no flag, no request; the poller is the only trigger.

**A5 — retry/failure semantics.** Fire-and-forget with auto-redrive: on failure the files are
**kept** and the same record is retried on the next ~10 s poll (`"FileUploadHttps fail … code
= %d"`); HTTP `code 102` in either POST triggers a session reset (`FUN_0044f268`); on success
both files are removed. **A server should answer 2xx + `{"code":0}`** — otherwise the robot
retries every poll. (When `/tmp/Run/CleanRecord` exceeds 1 MiB, task_manager does **not**
block: it logs *"Clean record director full:…remove all files"* and **deletes every queued
record** in that directory, then writes on — unsent backups are silently lost. The record's
pending `.bkmap` in `/tmp/Run/BackUpMap/` is *not* deleted with it — it is orphaned (the
poller can never match it to a record again) until the next save's one-slot wipe clears it.
Two corollaries: if
the session/cookie is not established, the uploader's "No cookie" branch returns *success* and
the files are deleted without any POST; and
the save-time one-slot wipe can strip a still-queued older
record's backup, which then uploads as the 4-part mapless POST.) curl
timeout 30 s. Multipart parts, ascending-key order: `backupMapMd5` (only when a `.bkmap`
exists), `data` (`"{}"`), `sn`, `ts`, `cleanFile` (always), `backupMap` (only with a backup) —
i.e. 6 parts with a backup, 4 without (`cleanFile` is never optional).

## B. `backupMapSwitch`

* `FUN_0041750c` (np `SetCollectBackupMapSwitch`) is **dead in this build** — zero callers or
  references of any kind (branch, data, table, symbol scans). It would have set
  `DevAttrData+0xFD8` via a config ZMQ `0x45c` and broadcast `0x4b7`.
* The **working switch is task_manager's `mEnableBKMap` (ctx+0x263, default 1 = on)**, set by
  the ZMQ `0x4b7` handler and by the config service's "BKMap" JSON parser
  (`TaskLogic` vtable slot `0x424230`). It gates **creating the `.bkmap` at clean end** (and
  thereby whether the upload carries the `backupMap` parts).
* `backupMapSwitch` in `/tmp/devattr` comes from `DevAttrData+0xFD8` (BSS, default 0 — the dead
  setter's field). It is emitted **only** by the devattr dump (`FUN_0041f010`), **never** by
  the wire status builder. **No remote read exists.**

## C. The artifact

* The `.bkmap` = `tar -zcf LastRecord.tar.gz ./*` of `/tmp/Run/LastRecord` top-level entries
  (`save_backup_map.sh`; one slot — task_manager wipes `/tmp/Run/BackUpMap/` before calling it,
  and the script's non-empty refusal is just a backstop). Members
  evidenced by the firmware strings and scripts: **`AreaSetting` (zones), `PathAndPos`,
  `AreaExtern.json`, `CleanInfo.json`, `CleanInfoBak.json`, `LastCleanInfo`, `MapList.json`**.
  So a restore **does bring zones and
  the stored path back** (see D4 for the two files that are protected from replacement).
* Parts and sizes: see A5 (ascending-key order, `cleanFile` mandatory, backup parts conditional).
  `data` is literally `"{}"` and `ts` the request tick — placeholders as far as the code shows.
  Limits: np curl timeout 30 s; the record dir's 1 MiB rule is a **wipe** (A5), not a POST limit.

## D. Restoring (`21025`)

**D1 — transport.** libcurl GET of `downUrl` **verbatim**: `http://` and `https://` both
accepted; **TLS verification disabled**; **redirects NOT followed**; **no headers, cookies or
auth** sent (beyond libcurl's own defaults); connect timeout 10 s, total 100 s; destination
`/tmp/DownloadBackupMap` (pre-existing file removed first). LAN hosting works fine.

**D2 — md5.** md5 of the **raw downloaded body**, formatted **lowercase 32-hex**, compared with
`memcmp` — **case-sensitive, exactly 32 bytes**, no trimming. Mismatch → download deleted,
live map untouched, reply `fail`/`code -3`.

**D3 — state preconditions.** **None.** No CPS/charging/workState gate; the handler is
re-entrancy-guarded only (a second 21025 while one is in flight → `{"message":"ok",…,
"data":{"code":-1}}`). It **interrupts a running clean** (stops the tasks first) and works from
every state loop including dormant (`"Exit dormant, enter idle"`). On success: path id + region
shm rewritten (no zone file → regions cleared), `SCSI_LOAD_MAP`, pose set, internal
`EID 0x477`. **No reboot, no channel-B drop.**

**D4 — what is replaced.** The firmware runs
`sh /tmp/AppRom/load_backup_map.sh "/tmp/DownloadBackupMap" "/tmp/Run/LastRecord/"`
(dir hard-coded; exit status never checked). The script preserves the live **`CleanInfo.json`**
and **`MapList.json`** (re-applied over the archive), clears everything else in `LastRecord`,
extracts the archive, deletes the download. Net: **zones (`AreaSetting`), the path, and the
record files are replaced by the archive; the history list and clean stats survive.**
`mapId` is not touched by the script — but the restored `PathAndPos`/region state is what the
robot then runs on (expect a fresh map upload after install — see D5).

**D5 — reply semantics to code against.**
| Situation | Reply | Note |
|---|---|---|
| malformed / missing fields | `{"message":"fail","infoType":21025,"data":7}` | generic fail builder |
| already downloading | `{"message":"ok","infoType":21025,"data":{"code":-1}}` | busy |
| any download/md5 error (incl. 404/timeout) | `{"message":"fail","infoType":21025,"data":{"code":-3}}` | no retry, no HTTP-status awareness |
| downloaded + md5 ok | `{"message":"ok","infoType":21025,"data":{"code":0}}` | **pushed BEFORE install** (racy — install may already be running — but `code:0` still only guarantees download+md5, not installation) |

The install happens after the reply, in task_manager; its completion is the robot's internal
`EID 0x477` → `OnMapAndRegionChangedEvent` → **"BackUpMap Map Send Start!"** → a fresh **map
upload** (20002) follows. **Watch for that upload (new mapSign/map content) as the real
"restore applied" signal**, and note a corrupt archive can yield `code:0` + an empty map
(tar failures are silent at both levels).

**D6 — no way to query the installed backup.** Nothing reports the current `.bkmap` md5 or
record id (devattr-only field, see B). The only outbound carrier of backup identity is the
clean-record upload's `backupMapMd5` (A5). A server that archives uploads can remember what it
last sent; the robot cannot confirm.

**D7 — `delCurMap` vs `cancleMap`.** Exact aliases — both hit `FUN_00413498` →
`EID_C_DELETE_MAP`; neither touches the backup slot or the download path; **deleting the live
map is not a precondition** for restore.

## E. The vendor model (app side)

App 1.5.11 has **no backup/restore feature**: nothing in the dex downloads or uploads a
`.bkmap`; `DownloadUtil` is used only for My-center images; history maps are served by the
**cloud** from its own archive via the REST records endpoints (`getRobotClearList` /
`getRobotClearRecordInfo` — the record's map/track comes inline from the cloud). So the vendor
moves the backup only robot↔cloud: **robot → cloud uploads on every (qualified) clean end;
cloud → robot restores with 21025**; the app just views cloud-served history. A replacement
server mirroring "user stores the file themselves" must therefore live with (a): no on-demand
upload — archive what arrives (or accept a real clean run per snapshot) — and (b): 21025 for
restore, using the D5 signals.

## Verification status / uncertainty

* All claims above trace to the two reports' cited addresses; the load-bearing ones
  (poller → `FUN_0044ab18` as sole caller; tm gate + `mEnableBKMap`; dead np setter; curl
  option block; md5 compare; script semantics; reply branches) were disassembled, not taken
  from decompiler text alone. No live-robot test yet.
* Not settled: the 35 s-vs-60 s observation (two explanations, see A2); the 6th record-name
  field's meaning (prints a zeroed vararg); the "BKMap" config parser's external caller
  (indirect vtable dispatch); whether any cloud→device message other than 21025 reaches the
  restore path (none found).
