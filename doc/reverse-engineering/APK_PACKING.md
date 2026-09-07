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

**Recommendation for this project:** don’t bother. The **device-side binaries**
(`network_proxy` et al.) are the authoritative protocol implementation and are
unpacked and fully analysed in `PROTOCOL.md` / `REPORT.md`. The app would only serve
to cross-check field names (e.g. the exact `setSta` request keys), which one live
pairing capture also gives you.

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

**Conclusion for the handoff:** the static tool does not cover this Jiagu variant, so
the app DEX remains packed. Options if the app code is ever actually needed:
1. **Dynamic dump (recommended):** run the APK on a real ARM device, dump the
   decrypted DEX from memory (FRIDA-DEXDump, or hook libart `OpenMemory`/`DexFile`).
2. **Reverse `libjiagu_a64.so`** to recover this variant's key/framing, then extend a
   static unpacker (high effort; stripped, anti-analysis native code).

Neither is necessary for this project: the **device-side binaries are the
authoritative protocol source** and are already fully documented
(`PROTOCOL.md`, `MAP.md`, `schemas/`). The app would only cross-check soft spots
(e.g. exact `setSta` request keys), which a single live pairing capture also settles.
