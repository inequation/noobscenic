# RE request — on-demand map backup download and restore

**From:** the noobscenic implementation side (clean room: our code derives from
`doc/reverse-engineering/` only; this request asks for more of that kind of evidence).
**Date:** 2026-10-10 · **Unit:** SN `LSLDSM7PRO20403551`, model `6716` · **Server:** this
workstation, `192.168.1.208`, ports 8080/8081 · **Robot:** `192.168.1.243`.

## Why we are asking (the model we want)

The robot multipart-posts its map backup to the cloud when a clean ends, and the vendor
cloud archived those per clean — that is how the app's "map history" exists. We would
rather **not** archive every clean's snapshot on the server. The model we want is:

1. **Download on demand:** at the user's request, the robot produces/uploads the current
   map backup and the user stores the file wherever they wish (their own disk, NAS, …).
2. **Restore on demand:** later, the user picks one of those files; our server hosts it
   and asks the robot to restore it.

Half two is documented (`21025`, below) but has open details. Half one — *asking the
robot for a backup at will* — is not documented at all: the only trigger we have seen is
a clean ending. Everything below is what we would need to build that model.

## What we know / observed (evidence)

* **Upload shape.** On clean end the robot multipart-POSTs to `cleanPack/uploadSingle`
  (144 KB in our capture) with parts `backupMapMd5`, `data`, `sn`, `ts`, `cleanFile`
  (a `…_<mapId>_<pathId>_<start>_<end>_1_0.txt` record) and `backupMap`
  (a `….bkmap`). Raw bodies: `var/traces/raw/2026-10-08/442.req.bin`,
  `2026-10-09/603.req.bin`, `…/1387.req.bin`; `Content-Disposition: form-data;
  name="backupMap"` etc. are visible verbatim.
* **When it fires.** We have four uploads (10-07 15:11:00; 10-08 20:05:23;
  10-09 19:41:41 and 19:44:06 — the last two ~7 s after `21017 stop` on runs that had
  cleaned for only ~35 s). Nothing was sent by us to trigger any of them.
* **`COMMANDS.md` ambiguity.** Its "Firmware-internal command names (Channel-B `data`
  dispatch)" section lists `backupMap`, `backupMapMd5`, `cleanFile` under `FUN_0044ab18`
  as action names the robot matches. Our capture shows those exact strings as multipart
  **part names** instead, and the upload happens with no such command sent. One of the
  two readings is wrong; please settle it.
* **Restore** (FUNC_MAP §8.2, static): `21025 {"downUrl":"<url>","md5":"<hex>"}`
  (`FUN_00472da0` → `FUN_00446938`/`ce0`): download to `/tmp/DownloadBackupMap`, check
  md5, apply (`load_backup_map.sh`, `EID_C_RELOAD_BACKUPMAP` 2032), then report
  "BackupMap apply ok" or "down fail" via `cleanPack/response`.
* **`backupMapSwitch`** exists as a status attribute; its setter `FUN_0041750c`
  (SetCollectBackupMapSwitch) has no found caller, and it is absent from the wire status
  builder (FUNC_STATUS §4).
* One live map plus **one** backup slot; no multi-map (FUNC_MAP §8.3).

## Questions

### A. Producing a backup on demand

1. Is there **any** inbound cloud→robot request (infoType + `data` shape) that runs the
   record/backup upload (`FUN_0044ab18`) without a clean ending? If the answer is "no,
   only a clean end triggers it", that is a useful answer — please state it explicitly.
2. What exactly triggers it today: which EID/task_manager site, and which CPS state(s)
   must the robot be in? Is it gated on record validity (FUNC_STATUS §7.1 suggests
   ≥4 path points and `cleanTime` ≥60 s), or does any clean end upload — we saw uploads
   after 35-second stopped runs, which seems to contradict the ≥60 s rule?
3. Are `backupMap`/`backupMapMd5`/`cleanFile` dispatchable command-name strings (if so:
   which infoType carries them, and what does each value make the handler do?), or are
   they only multipart part names? A correction line for `COMMANDS.md` would be ideal.
4. Is there a "send the backup/record **now**" force-flag path, analogous to the
   20002/21014 "send map now" request that sets the MapSend force flag
   (`FUN_004570d8`, FUNC_MAP §2.1)? If such a flag exists for the record upload, which
   request sets it and what does the upload look like then?
5. Retry/failure semantics of the upload: does it use the same "3 consecutive failures →
   drop" bookkeeping as the map upload, or is it fire-and-forget? What does the robot do
   with our reply — does `{"code":0}` matter, is a non-2xx retried?

### B. The `backupMapSwitch`

1. What calls `FUN_0041750c`? Any inbound command / `data` key that toggles it (a
   settings command, 21024-family string, schedule entry, …)?
2. What does it actually gate: creating the `.bkmap`, uploading it, or both? What is its
   default state on a docked unit?
3. Why is it missing from the wire status builder when `/tmp/devattr` carries it — is
   there any way for a server to read it remotely?

### C. The artifact

1. Exact contents of the `.bkmap` tarball (`save_backup_map.sh`): MAP.md says "map +
   `CleanInfo.json` + `MapList.json`". Does it also contain `AreaSetting` (zones),
   `CleanInfoBak.json`, schedules, DevAttr, or anything else? This decides whether a
   restore brings back zones/settings or only the map.
2. Are the multipart parts independent — could the robot send only
   `backupMap`+`backupMapMd5` (no `cleanFile`)? Does part order matter, and are the
   `data`/`ts` part values meaningful or placeholders?
3. Any size or time limit on the POST, and does the reply body/code change what the
   robot does next (e.g. mark the record as delivered)?

### D. Restoring (`21025`)

1. Transport for `downUrl`: plain `http://` only, or is `https://` accepted? Redirects?
   Does the robot send any headers/cookies/auth with the download? (We would host the
   file ourselves, on the LAN.)
2. md5 semantics: is it the md5 of the raw downloaded body, and in what format/case?
   What happens on mismatch — is the live map left untouched, or is anything cleared?
3. State preconditions: does restore work while docked/charging (`charge`/`fullcharge`),
   or does it need `idle`? Does it interrupt a running clean or manual control? What
   status change (if any) can a server watch to know it worked?
4. What exactly does `load_backup_map.sh` replace: the live map and `MapList.json`, or
   the map only (FUNC_MAP §8.3 says `MapList.json` survives restores)? Does `mapId`
   change? Is a reboot/reconnect needed (does channel B drop)? If the archive carries
   zones/records/consumables, are they applied too?
5. Failure semantics and exact reply strings: 404, timeout, bad md5 — what is posted
   back (`"down fail"`?), is it retried, and what state does the robot end in?
6. Is there any way to ask the robot **which** backup it currently holds (its md5 or
   record id), so a server can verify what is live before replacing it?
7. Interaction with map deletion: does `21024 {"cmd":"delCurMap"}` or `"cancleMap"`
   touch the backup slot, and is deleting the current map a precondition for a clean
   restore? What is the difference between the two spellings?

### E. The vendor model (mirroring it, if visible)

1. Does the app ever receive the `.bkmap` itself (a "backup to phone" feature), or only
   record maps through `getRobotClearRecordInfo`? If the app has a **restore** action,
   which REST call does it make, and what does the cloud then send to the robot — the
   exact `21025` shape (`downUrl` signed? `md5`? additional fields?).
2. Is there a REST/imsocket endpoint that hands the app a download URL for a record's
   backup (name, path, parameters)?

## What would settle these

* Static answers from `network_proxy` / `task_manager` / `cp_function.cpp` /
  `protocol_ld.cpp` / the shell scripts (`save_backup_map.sh`, `load_backup_map.sh`),
  or
* a capture of the vendor cloud performing a restore, and of the app asking for a
  backup download, or
* the app-side REST calls for history/backup (dex).

**Our evidence:** `doc/reverse-engineering/FIELD_NOTES.md`, the raw request bodies under
`var/traces/raw/2026-10-0*/` (the multipart uploads), and `FUNC_MAP.md` §8.
