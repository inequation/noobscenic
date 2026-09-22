# Proscenic M7 Pro — Android app packing (why static RE of the APK fails)

**APK:** `Proscenic+Robotic_1.5.11_APKPure.apk`, package `com.proscenic.psnic`
(versionCode 24 / 1.5.11). Underlying app SDK namespace is **`com.baole.blap`**
(Baole is the OEM). **[proven]**

## What blocks disassembly: Qihoo 360 "Jiagu" (加固) packer
APKiD verdict:
```
packer    : Jiagu
compiler  : dexlib 2.x
obfuscator: unreadable field names, unreadable method names
```
Evidence in the APK:
* Native packer runtimes in `assets/`: `libjiagu.so`, `libjiagu_a64.so`,
  `libjiagu_x86.so`, plus `assets/.appkey` (16 bytes: `7bd3e651dc89db26`).
* Packer bootstrap classes are the **only** things that decompile:
  `com.stub.StubApp` (the manifest `Application` shim), `com.qihoo.util.*`
  (`DtcLoader`, `LiteApplication`, `GameApplication`, `OverseaApplication`,
  `Configuration`, …), `com.qihoo360.replugin.Entry`.
* `jadx` recovers **12** classes total; the real `com.baole.blap.*` code is absent
  from `classes.dex`.

### Mechanism
`classes.dex` (~4.6 MB) is a **loader stub**, not the app. At launch, `com.stub.StubApp`
(installed as the `<application>` in the manifest) initialises the Jiagu runtime from
`assets/libjiagu*.so`, which **decrypts/decompresses the real DEX in memory** and
hands it to a custom `ClassLoader`. The genuine classes are therefore never present
on disk in cleartext, so `apktool`/`jadx`/`dex2jar` see only the stub. Jiagu builds
also carry anti-debug / anti-emulator / integrity checks in the native layer, plus
name obfuscation on whatever *is* visible.

### What is still readable without unpacking (useful for clean-room)
The **AndroidManifest** is not encrypted, so the component class names survive and
map out the app’s features (a client-side confirmation of the device protocol):
* Pairing / provisioning (Channel C): `…adddevice.activity.SelectDeviceActivity`,
  `SearchResultsActivity`, `ConnectRobotActivity`, `ConnectingActivity`,
  `WIFIloginActivity`, `ResetDeviceActivity`, `ConfirmResetActivity`,
  `CustomCaptureActivity` (QR).
* OTA (Channel A): `dialog.UpdateFirmwareDialogActivity`,
  `deviceinfor.activity.DownloadFirmwareActivity`.
* Mapping: `deviceinfor.activity.MapDetailActivity`, `MapOrdinaryActivity`,
  `devicecontrol.activity.CameraActivity`.
* Records / scheduling: `CleanRecordActivity`, `AppointmentTimeActivity`,
  `BatchReservationActivity`, `DisturbTimeActivity`.
Launcher entry is `com.baole.blap.module.login.activity.LogoActivity`.

### How to recover the app code (dynamic unpacking) — if ever needed
Static tools cannot; you must dump the decrypted DEX at runtime:
1. Run the APK on a **real ARM device / rooted phone** (Jiagu resists emulators),
   Android 8-era (targetSdk 26).
2. Attach Frida and use an in-memory DEX dumper (e.g. FRIDA-DEXDump) to snapshot the
   DEX images the custom ClassLoader maps, or hook `DexFile`/`ClassLoader` /
   libart `OpenMemory`.
3. Reassemble the dumped DEX, then `jadx` the result; expect obfuscated names.

**UPDATE (superseded):** the app DEX was **successfully extracted** later by a
different route than the two below — an API-22 x86 emulator + `packages.xml`
certificate restore + `/proc/mem` dump. See **`UNPACKING.md`** for the full method and
`dex/app_main.dex` (com.baole.blap, 5933 classes) for the result. The
static/`FRIDA-DEXDump`-on-real-device recommendation in this section did not pan out
and was not the winning path; it is kept for historical record. The device-side
binaries remain the authoritative protocol source, but the decrypted app was needed
and obtained (it supplies Channel-A/`WIRE_PROTOCOL.md` and the command vocabulary in
`COMMANDS.md`).

---

## Unpacking attempt: `SafaSafari/jiagu_unpacker` (static) — did NOT work here

Per request, evaluated and run `github.com/SafaSafari/jiagu_unpacker`
(commit cloned 2026-09). It is a legitimate, self-contained **static** unpacker
(Python + pycryptodome; no device/Frida needed) for **one** Jiagu variant, using
hardcoded constants:

* container model: packed `classes.dex` = `[shell_dex][encrypted_payload][4-byte
  big-endian shell_dex_length trailer]`;
* payload crypto: first `512+16` bytes AES-CBC (`key="bajk3b4j3bvuoa3h"`,
  `iv="mers46ha35ga23hn"`, PKCS5) then plaintext; secondary DEXes XOR `0x66` over
  first 112 bytes; then length-prefixed original DEX records.

**Result on this APK: fails — variant mismatch** (the tool's README warns of exactly
this). Evidence gathered while adapting it:

* `classes.dex` (4,671,144 B) is a **valid standalone stub DEX** with
  **`class_defs_size = 12`** (the Jiagu loader classes only), `method_ids = 9580`,
  DEX structures ending at the map_list (`map_off = 0xa63c`, ends `0xa70c`).
* Everything from ~`0xa70c`→EOF is an **appended high-entropy blob** (entropy ≈ 7.9)
  — the encrypted real DEX — but it is **not** framed as `[shell][enc][len-trailer]`:
  the last 4 bytes decode to a nonsense shell length (`0x1a302e00` = 439,365,120 ≫
  file size), so the tool aborts immediately.
* The APK has **no trailing data after the ZIP EOCD** (0 bytes) and **no encrypted
  DEX asset** — the payload lives only inside `classes.dex`.
* Brute-forced the payload start (`0xa000`–`0xc000`, 16-byte steps) against the
  tool's AES-CBC **and** AES-ECB, with both the tool's key and the APK's
  `assets/.appkey` (`7bd3e651dc89db26`, 16 B) as key, and zero/echo/tool IVs:
  **no combination yields a valid app-name/DEX** → this build uses a **different key
  and/or cipher** than the tool supports. Plain XOR-`0x66` also does not reveal a
  DEX/zip/gzip/ELF magic.
* `assets/libjiagu_a64.so` (the native unpacker) is **stripped**; it pulls in
  `libmono.so`/`libz.so`. Recovering the scheme would mean reversing this obfuscated
  native lib.

**Conclusion for the handoff (updated):** the static SafaSafari tool does not cover
this Jiagu variant. The app DEX was nonetheless **later extracted successfully** — not
by either option below, but by the x86-emulator dynamic route documented in
**`UNPACKING.md`** (defeat the `JNI_OnLoad` anti-emulator gate, run on API 22 where the
older ART OAT version selects the signature-keyed decrypt path, restore the genuine
signing cert via the emulator's `packages.xml`, then `SIGSTOP` the process and carve
the DEX from `/proc/<pid>/mem`). The historical options were:
1. Dynamic dump on a real ARM device (FRIDA-DEXDump / hook libart) — not used.
2. Reverse `libjiagu_a64.so` to recover the key/framing — not used (the x86 stage-2
   crypto was cracked instead; see `UNPACKING.md` / `jiagu/extract_x86.py`).

The device-side binaries remain the authoritative *protocol* source, but the decrypted
app was in fact required and obtained: it is the source for `WIRE_PROTOCOL.md`
(Channel A) and `COMMANDS.md`.

---

## Deep reverse of the packer (follow-up — `jiagu/` subfolder)

A later effort reversed the native packer itself (`libjiagu_a64.so`, arm64) rather
than relying on the static tool. Full write-up: **`jiagu/JIAGU_INTERNALS.md`**;
tooling: **`jiagu/jiagu360_core.py`** (variant detector + blob carver + the exact
RC4/zlib core cipher) and **`jiagu/emu_extract_core.py`** (Unicorn harness that runs
the packer's own key-schedule/RC4/inflate).

Confirmed (proven by emulation / byte-verification):
* `libjiagu` is a **stage-1 loader**; the real engine (`libmono.so`) is embedded
  **encrypted + zlib'd** in a camouflaged section **`.mips`** (off 0x1b430, 0x85e6c),
  with key material in **`.bmp`** (off 0x1b008, 0x426), custom in-memory-loaded.
* Core payload pipeline = **RC4 (standard KSA/PRGA, reproduced) → 4-byte length →
  zlib inflate**. Anti-debug throughout (XOR-0xa5 strings, linker/breakpoint scans,
  `kill(pid,9)`), plus a bytecode-VM obfuscation layer (`__fun_a_18`).

Not reached (honest boundary): the **`.bmp` key material is itself encrypted** by a
prior step not yet located, so the RC4 key (hence `libmono`, hence the DEX the engine
finally decrypts) was not statically recovered. Completion options, in
`JIAGU_INTERNALS.md` §"Status & how to finish": (1) emulate the native key schedule
from an earlier entry point to recover the `.bmp` key, then `core_decrypt()` yields
`libmono`, then reverse `libmono`'s DEX stage; or (2) the industry-standard **dynamic
dump** (run on a real arm64 device, FRIDA-DEXDump) — not possible in this headless VM.
