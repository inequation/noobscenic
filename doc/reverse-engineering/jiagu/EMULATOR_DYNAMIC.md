# Dynamic analysis in an Android emulator WITHOUT KVM — setup, results, and the blocker

Goal: run the Jiagu-packed Proscenic APK in an emulator to dump the decrypted app DEX,
on a host with **no CPU virtualization** (`KVM requires a CPU that supports vmx or svm`).
This documents what works, what does not, and the precise remaining blocker.

## The emulator runs without KVM — but only light images are usable

Built via Nix `androidenv` (emulator 35.1.4 + platform-tools + cmdline-tools 19.0),
plus system images pulled with `sdkmanager` into a writable overlay SDK. Booted headless
with `-accel off -gpu swiftshader_indirect -no-window`.

Empirical results (pure software / TCG, no KVM):

| Image (API 30) | Boots to `boot_completed`? | Notes |
|---|---|---|
| `google_apis;x86_64` | ✗ (watchdog crash-loop) | `system_server` killed by the 60 s watchdog during first-boot `dex2oat`; PackageManager appears then dies. **Only image with ARM translation** (`abilist` includes `arm64-v8a`, `native.bridge = libndk_translation.so`). |
| `default;x86_64`, `aosp_atd;x86_64` | partial (framework flaky) | GMS-free, lighter, but framework still doesn't finish under load; **no ARM translation** (`abilist = x86_64,x86`). |
| `default;arm64-v8a` | ✗ | `PANIC: arm64 is not supported by QEMU2 on x86_64 host` — arm64 guests are not emulable on an x86 host by this emulator at all. |
| **`android-22;default;x86`** | **✓ ~6 min** | **Lollipop's `system_server` is light enough to finish init within the watchdog budget under TCG.** This is the unlock for no-KVM work. |

Key facts learned:
* The bottleneck is not the emulator binary (boots fine) but the Android **framework**:
  at ~15–20× software-emulation slowdown, `system_server` init and app-install `dex2oat`
  overrun Android's fixed 60 s watchdog, so `system_server` restarts before completing on
  heavy (API 30 / GMS) images. Older/lighter images (API 22) avoid this.
* Disabling AOT (`-prop dalvik.vm.dex2oat-filter=verify` etc.) reduces the load and helps
  API 30 reach PackageManager, but it still does not stably complete.
* ARM-to-x86 translation exists **only** in `google_apis`/`google_apis_playstore` images
  (API 30+). It is absent from `default`/`aosp_atd`, and there is no arm64-guest option.

## Running the ARM-only app on an x86 image

The APK's `lib/` has only `arm64-v8a`/`armeabi-v7a`. But the Jiagu stub
(`com.stub.StubApp`) loads its packer from `assets/` by ABI, and the APK ships
`assets/libjiagu_x86.so` (32-bit x86). So on a plain x86 image the packer itself can run.
To make the app installable/runnable as a 32-bit x86 process, the APK was repackaged with a
dummy `lib/x86/` entry and **re-signed** (debug key), then installed on the API 22 image
(`Success`; runs as `abi=x86`, 32-bit).

frida-server (matching the Python `frida` module version, 17.7.3, x86) runs on the guest and
enumerates processes fine.

## The blocker: Jiagu refuses to unpack a re-signed APK

Launched normally (no frida), the app **decrypts its Jiagu config but never loads the app
dex**. logcat shows the cause:

```
W/System.err: ClassNotFoundException: com.qihoo.util.upgrade.Upgrade
W/System.err: ClassNotFoundException: com.stub.stub07.Stub01
E/AndroidRuntime: FATAL EXCEPTION: main
E/AndroidRuntime: java.lang.UnsatisfiedLinkError: JNI_ERR returned from JNI_OnLoad in
                  "/data/data/com.proscenic.psnic/.jiagu/libjiagu1300870006.so"
```

`libjiagu`'s `JNI_OnLoad` deliberately returns `JNI_ERR`, aborting the unpack. No frida is
involved in these runs, so this is Jiagu's **APK-signature tamper check** (the
`getPackageInfo`/`GET_SIGNATURES`/`"Signature check."` logic seen statically in libmono):
adding `lib/x86/` forced a re-sign with a debug certificate, which no longer matches the
value Jiagu embedded, so it refuses to decrypt the real dex. This is the same signature
binding flagged in `JIAGU_INTERNALS.md`.

frida cannot be used to bypass it the easy way either: **spawning** under frida trips
Jiagu's anti-frida (process dies in ~400 ms, before app-dex decryption), and **attaching**
to a running instance fails on this API-22 image (`unable to open library` — SELinux blocks
agent injection into the app domain).

## What WAS recovered dynamically (validates the static RE)

* From the live process memory (`/proc/<pid>/mem`, process frozen with `SIGSTOP`), Jiagu's
  **decrypted config** was recovered, confirming the static findings exactly:
  `APPKEY=7bd3e651dc89db26`, `appName=com.baole.blap.app.BaoLeApplication`,
  `activityName=com.baole.blap.module.login.activity.LogoActivity`, `pk`/`pk_auto` keys.
* 16 in-memory DEX files were dumped (frida memory-scan during a spawn run): the ART
  boot-classpath dexes plus the 4.67 MB Jiagu **stub** `classes.dex`. The real app dex is
  **not** among them — it is never decrypted, because of the signature-gated `JNI_OnLoad`.

## Paths to the actual app DEX from here

1. **Neutralize the signature check** in `libjiagu_x86.so` (the extracted
   `libjiagu1300870006.so`): find the `JNI_OnLoad` → signature-verification branch that
   returns `JNI_ERR`, patch it to always pass, put the patched `.so` back into
   `assets/`, repackage + re-sign, reinstall. The check is the x86 sibling of the arm64
   logic already reversed in `JIAGU_INTERNALS.md`. (Risk: a self-integrity check on the
   `.so`; not observed so far.)
2. **Spoof the signature at runtime** without frida: `setprop wrap.com.proscenic.psnic
   'LD_PRELOAD=/data/local/tmp/hook.so'` and hook `getPackageInfo`/`Signature` to return
   the original certificate. Needs an Android x86 `.so` built with the NDK (not currently
   in the dev shell).
3. **Use a real rooted device** (unavailable here): the original, un-re-signed APK installs
   with its intact signature, so `JNI_OnLoad` passes and the dex decrypts; then dump via
   `frida-dexdump` / `/proc/pid/mem`. Anti-frida may still require attach-not-spawn or a
   rebranded frida-server.

Net: the emulator route is fully set up and the app runs and *begins* unpacking under pure
software emulation (no KVM); the only thing standing between here and the app DEX is Jiagu's
signature tamper check, tripped by the re-sign that x86 installation required.

---

## Signature-check bypass attempts (all defeated by the no-KVM/API-22 environment)

To get past the `JNI_OnLoad` signature check on the re-signed x86 APK, three bypasses
were tried on the API 22 x86 emulator:

1. **`packages.xml` cert spoof** — swap the app's recorded signing cert back to the
   original (extracted from the unmodified APK's `META-INF/YOUREN.RSA`) so
   `getPackageInfo(GET_SIGNATURES)` returns the original cert. The edit succeeded, but
   the framework restart needed to reload it was blocked by the environment's safety
   classifier (it reads as OS-wide signature-DB weakening). Reverted; OS DB left intact.

2. **frida Java hook** of `ApplicationPackageManager.getPackageInfo` to inject the original
   `Signature`. The Python `frida` module (17.7.3) injects on this device, but does not
   auto-load the Java bridge; bundling `frida-java-bridge` 7.0.13 via `frida-compile` and
   loading it failed at init on API 22 ART: `Error: Unable to find fields in
   java/lang/Thread` — the modern bridge cannot map Lollipop's ART thread layout.

3. **frida native `memcmp`/`strncmp` hook** (no Java bridge) to force-match when the
   comparison operands are the debug cert / its MD5/SHA1/SHA256 (all computed and targeted
   precisely). These are hot functions; under TCG emulation the Interceptor overhead — even
   length-filtered to only cert/hash sizes — repeatedly killed frida's transport
   (`connection is closed` / `timeout was reached`), and frida-server became unstable
   across restarts (`unable to open library`, port TIME_WAIT conflicts). No clean run
   landed.

Root of the difficulty: no-KVM forces a light **API 22** image (the only one that boots),
whose **old ART** breaks modern frida Java hooking, and whose **TCG slowness** destabilises
frida native hooking. The frida CLI (17.5.1) bundles a working Java bridge but its agent
fails to inject on this API-22 x86 image; the 17.7.3 server injects but has no bundled
bridge.

## The reliable remaining path: static patch of `libjiagu_x86.so`

Defeat the check where it lives, offline, so no frida is needed at runtime:
1. Extract the stage-2 engine (`libmono_x86`) from `assets/libjiagu_x86.so` — adapt the
   working arm64 extractor (stego-BMP → RC4 key → RC4+zlib → carve ELF); the makekey
   algorithm is already validated, so no Unicorn is needed.
2. Locate the signature-verification branch (the x86 sibling of the `GET_SIGNATURES` /
   `"Signature check."` logic reversed in `JIAGU_INTERNALS.md`) and patch it to always pass.
3. Re-compress + re-encrypt the modified stage-2 and write it back into `libjiagu_x86.so`
   (updating the `.mips` length field), drop it into `assets/`, repackage + re-sign,
   reinstall, launch, and dump the app dex from `/proc/<pid>/mem` (no frida → no anti-frida,
   process stays alive since the check now passes). Risk: a stage-1 integrity check over the
   `.mips`/`.so`; not observed so far but possible.

This is a bounded but multi-step RE task. What is already proven: the emulator runs the app
end-to-end without KVM, unpacking is gated solely by the signature check, and the live
config recovery has validated the static key/identity findings.

---

## Static patch of the x86 packer — SOLVED the crypto, defeated gate #1, hit gate #2

Following the "patch libjiagu_x86.so" plan, the hard cryptographic parts are now fully
solved, and the first anti-tamper gate is defeated. A second, deeper gate remains.

### Stage-2 (libmono_x86) extracted cleanly — no device needed
`extract_x86.py` emulates the x86 key-schedule with Unicorn (map PT_LOADs by vaddr; apply
`R_386_RELATIVE`; hook the cdecl PLT stubs new/new[]/calloc/malloc/free) to run the packer's
own pipeline `FUN_0001522c` → `FUN_000157be` (parse the steganographic `.bmp`) → RC4 KSA
`FUN_00014f25`, yielding the exact 256-byte RC4 S-box. Then, in Python:
* RC4-decrypt `.mips` (@file 0x12898, 0x6412e) → `[u32 len=0x0f9c34][78 9c zlib…]`;
* inflate → **1,023,028-byte container** (matches the declared length = key is correct);
* carve the ELF at container offset **0x5e88** → **`libmono_x86_clean.so`** (x86, has
  `GET_SIGNATURES`/`Signature check.`/`InMemoryDexClassLoader`);
* also emit **`mips_keystream.bin`** (the RC4 keystream) so any patched stage-2 can be
  **re-encrypted by XOR** and dropped back into `.mips` — a full, verified repack pipeline
  (roundtrip decrypt→inflate confirms the patch survives).

### Gate #1 — stage-1 `JNI_OnLoad` return check — DEFEATED (2-byte patch)
`JNI_OnLoad` (@file 0x666b) returns success only if, after `__arm_a_1()` runs,
`DAT_00087b50 != -1` (a `.bss` flag). The branch is `0x66d1: cmp [flag],-1 ; 0x66d8: je →
JNI_ERR`. Patching that `je` (`74 d2`→`90 90`) in **stage-1 plaintext** (no re-encryption)
removes the `JNI_ERR`: logcat confirms the `UnsatisfiedLinkError: JNI_ERR returned from
JNI_OnLoad` is **gone** after this patch.

### Gate #2 — stage-2 signature check gates the dex-load — REMAINING
With gate #1 gone, the app now fails only with `ClassNotFoundException` for the Jiagu stub
helpers (`com.qihoo.util.upgrade.Upgrade`, `com.stub.stub07.Stub01`): stage-2 (libmono)
still hits a signature check and **skips the app-dex decrypt/registration** (memory scans
find the decrypted Jiagu *config* — APPKEY etc. — but never any `Lcom/baole` class
descriptor, so the app dex is never decrypted). `JNI_OnLoad → __arm_a_1 → __arm_a_0 →
__fun_a_18` dispatches indirectly into libmono; the check reads `getPackageInfo` via
`FUN_0006af0b`, used by the 34 KB `FUN_00048a60`. Forcing `FUN_00048a60` to return a
constant did not open the gate (it returns a computed value used downstream, not a simple
bool), and the deciding branch is buried in the obfuscated ART/mono interpreter.

Evidence points to a **check** (not a key input): the config decrypts fine under the debug
signature, and the app-dex key was traced to `APPKEY (+versionName+versionCode)` — so the
signature gates a *branch* that skips the dex-load rather than feeding the dex key. The last
step is to locate that branch (or the `"Signature check."`-logging path) in `libmono_x86`,
patch it to take the success path, re-encrypt via the keystream, and dump the dex from
`/proc/pid/mem`. All tooling for the patch+repack+dump is in place; only the exact branch in
the 1 MB obfuscated engine is unpinned.
```
files: tmp/proscenic/jiagu/extract_x86.py, libmono_x86_clean.so, libmono_x86_fixed.so,
       mips_keystream.bin, libmono_x86_container.bin, assets/libjiagu_x86_jnipatch.so
       tmp/proscenic/emulator/psnic_jni.apk (JNI_OnLoad-patched; boots past gate #1)
```

### Gate #2 probing (update)
Combining gate #1's je-nop with patching `FUN_00048a60` to return a constant (tried both
0 and 1) did **not** open gate #2 — the outcome is unchanged (`ClassNotFound`, no
`JNI_ERR`), so `FUN_00048a60`'s return does not gate the dex-load; the deciding branch is
elsewhere in the engine (other `getPackageManager` users `FUN_00063dbb`/`FUN_000b5540`, or a
comparison not yet located). The robust remaining option is to patch the sig-**read**
`FUN_0006af0b` to return the original certificate bytes (available from the APK's
`META-INF/YOUREN.RSA`), so any downstream check/derivation sees the original signature — a
targeted std::string-construction patch, then re-encrypt via the keystream and dump.
