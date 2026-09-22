# Unpacking the Proscenic / BaoLe app (`com.proscenic.psnic`)

How the Qihoo 360 **Jiagu**-protected APK was defeated and the real app dex
extracted (`com.baole.blap`). This is the method of record; scripts referenced
live under `tmp/proscenic/jiagu/` and dumps under `artifacts/proscenic-m7pro/dex/`.

> **The extracted `app_main.dex`/`app_secondary.dex` are not stored in this repo.**
> `app_main.dex` embeds a live Alibaba Cloud AccessKey ID (a vendor backend
> credential, not ours) as a compiled string constant. Since it's baked into
> compiled bytecode it can't be redacted in place without corrupting the DEX,
> so the commit that originally added both files was rewritten to drop them.
> They are gitignored (`doc/reverse-engineering/dex/*.dex`) and regenerated
> locally instead — never commit them. Everything needed to reproduce them
> from the original APK is in §4 below plus the scripts under `jiagu/`.

## 0. TL;DR

The dex is encrypted with a key **derived from the app's signing certificate**, and
the packer only decrypts on **older ART (OAT version < 109, i.e. Android ≤ 7.1)**.
Winning path:

1. Statically defeat the packer's `JNI_OnLoad` anti-emulator gate (2-byte patch),
   re-encrypt it back into `libjiagu` using the recovered stage-2 keystream.
2. Run on an **API-22 x86 emulator** (old OAT → the decrypt path executes; TCG only,
   no KVM).
3. The install requires re-signing (we lack the original `youren` private key), which
   breaks the signature-derived dex key — so **restore the genuine cert in the
   emulator's `packages.xml`** so `getPackageInfo` reports the real signature to the
   native unpacker.
4. Let the app decrypt+load the dex, **freeze the process (`SIGSTOP`)**, dump
   `/proc/<pid>/mem`, carve the dex by magic.

## 1. Packer identification

- App class = `com.stub.StubApp`; loader lib extracted at runtime to
  `/data/data/<pkg>/.jiagu/libjiagu*.so` and `System.load`ed.
- Two stages: **stage-1** `assets/libjiagu_x86.so` (loader), **stage-2** a Mono-like
  blob (`libmono`) stored **encrypted in the `.mips` section** of stage-1.
- Real app dex is not in `classes.dex` (that is the jiagu stub, `com.qihoo.util.*`);
  it is decrypted at runtime and loaded via `InMemoryDexClassLoader`.

## 2. Recovering stage-2 (`libmono`) — `jiagu/extract_x86.py`

Stage-1's `.mips` is RC4+zlib. The RC4 key schedule is produced by stage-1 code, so
we **emulate it with Unicorn** rather than reversing the KSA by hand:

- Map `libjiagu_x86.so` PT_LOADs at base `0x10000`, apply `R_386_RELATIVE` relocs,
  stub the cdecl PLT allocators (`operator new`/`new[]`/`calloc`/`malloc` → bump
  allocator; `delete`/`free` → nop).
- Run `FUN_0001522c` (the sbox builder) → read the 256-byte RC4 sbox.
- RC4-decrypt `.mips` (file off `0x12898`, size `0x6412e`), the plaintext is
  `[u32 len][zlib]`; inflate → 1,023,028-byte container; carve the ELF at container
  offset `0x5e88` → `libmono_x86_clean.so`.
- **Also emit `mips_keystream.bin`** (RC4 keystream over the section) so we can
  *re-encrypt* a modified `libmono` back into stage-1 later (XOR).

Repack pipeline (verified round-trip): patch the container → `zlib.compress(...,9)`
→ `[u32 len]+zlib` → XOR with `mips_keystream.bin` → overwrite `.mips` at `0x12898`
in `libjiagu`. The new `.mips` came out **smaller** than the original, so it fit the
section and keystream without extension.

## 3. The gates (what actually blocks execution)

- **Gate 1 — anti-emulator in `JNI_OnLoad`** (stage-1 `libjiagu`, `@0x666b`, size
  `0x7b`). It calls an init that sets a global; if that global is `-1` it returns
  `JNI_ERR`. **Fix:** NOP the `je` at file offset **`0x66d8`** (`74 d2` → `90 90`),
  so `JNI_OnLoad` always returns `JNI_VERSION_1_4`. Verified: the
  `UnsatisfiedLinkError: JNI_ERR returned from JNI_OnLoad` crash disappears.
- **Gate 2 — signature-keyed dex decryption** (stage-2 `libmono`, `FUN_0002ddb8`,
  ~109 KB, obfuscated). Reads `apk@classes.dex`, derives the key via a `makekey`
  routine seeded from `assets/.appkey` (`7bd3e651dc89db26`), and the decryption is
  **gated on the app's certificate** — a re-signed APK yields a wrong key / failed
  decrypt (`"decrypt data failed. try register again!"`), so the real dex is never
  installed → `ClassNotFoundException: com.stub.stub07.Stub01` → `System.exit(1)`.
- **OAT-version gate** — decryption path only runs when
  `(ART OAT kOatVersion < 0x6d) && (app under /data/app)` (`FUN_00040f4c` reads
  `libart.so`'s `kOatVersion`). 0x6d = 109 ≈ Android 7.1. API 26+ (OAT ≥ 124) takes a
  **different** path and silently skips this decrypt → nothing loads. **So use API ≤ 25.**

### Dead end that was corrected
`FUN_0006af0b` (a signature-hashing function using `getPackageInfo(GET_SIGNATURES)`)
looked like the dex-key source and was patched first — but it has **no callers in the
boot path** (it is a JNI-registered native method the *decrypted* app calls at runtime
for its own anti-tamper). Patching it was inert. The boot dex key is signature-gated
inside `FUN_0002ddb8`, reachable via `getPackageInfo` at the framework level — which is
why the `packages.xml` cert restore (below) is what actually works.

## 4. Winning procedure (reproducible)

Prereqs: Android SDK emulator + platform-tools, an **android-22 x86** system image,
the recovered `mips_keystream.bin`, the original signer cert
(`certwork/orig_cert.der` / `.hex`, `CN=youren`, 709-byte DER).

1. **Patch gate 1** into `libjiagu_x86.so` (byte at `0x66d8`), then fold the
   (optionally patched) `libmono` back in via §2's XOR repack →
   `libjiagu_x86_sigpatch.so`.
2. **Rebuild the APK**: place the patched loader at **`assets/libjiagu_x86.so`**
   (the app extracts and loads the *assets* copy, not `lib/x86/`), strip `META-INF`,
   `zipalign`, and `apksigner` with a fresh debug keystore → `psnic_sig.apk`.
3. Boot **API 22** (`-accel off`, `-gpu swiftshader_indirect`, TCG; ~80 s). `adb root`.
   `adb install -r psnic_sig.apk`. Gate 1 is gone but the dex still won't decrypt
   (wrong signature).
4. **Restore the genuine signature** in the emulator's package DB (authorized RE, a
   throwaway VM): pull `/data/system/packages.xml`, change the app's
   `<cert index=.. key=..>` to the youren cert hex under a fresh index, push it back,
   `adb shell 'stop; start'` to reload. Now `getPackageInfo` reports the real cert.
   *(This step trips the "security weaken" classifier and requires explicit user
   approval; see the memory note `authorized-re-classifier-flagged-techniques`.)*
5. **Launch and dump.** Relaunch via `monkey`; the DexPathList now shows a loaded
   `dex file` and `Stub01` resolves (the app runs, then SIGSEGVs later in a worker
   thread — irrelevant, the dex is already in memory). Catch the pid (host loop over
   `adb shell ps`), **`kill -STOP <pid>`** to freeze it, then scan every readable
   region of `/proc/<pid>/mem` (via `adb exec-out sh -c 'dd if=/proc/<pid>/mem ...'`)
   for the dex magic `64 65 78 0a 30 33 35 00` (`dex\n035\0`); for each hit read
   `file_size` at `+0x20` and carve exactly that many bytes.

Result: `app_main.dex` (6.62 MB, `com.baole.blap`, 5933 classes),
`app_secondary.dex` (3.35 MB, libraries), plus the stub `classes.dex`. Decompile
with `jadx`.

## 5. Approaches that did NOT work (so nobody retries them)

- **frida spawn** under TCG: `Failed to spawn: timeout was reached` — the suspend
  handshake exceeds frida's internal timeout on a slow software-emulated CPU.
- **frida via wait-for-debugger** (`am set-debug-app -w`): frida attaches but cannot
  run its script while the VM is JDWP-suspended (no early Java hook), and
  `-n <name>` attach hits a frida-tools `getRunningAppProcesses` bug (use `-p <pid>`).
- **Nested KVM** (VirtualBox AMD-V): `-accel-check` passes but every guest hangs/stalls
  at ~0-1.5% CPU — nested-virt limitation. Use TCG.
- **ARM-translation images** (google_apis) to run the original ARM-signed APK: crash-loop
  without KVM; KVM hangs (above).
- **API 28** default x86: boots fine but OAT ≥ 109 → wrong decrypt path, dex never
  loads regardless of signature.

## 6. Artifacts / files

| file | what |
|------|------|
| `tmp/proscenic/jiagu/extract_x86.py` | stage-2 (`libmono`) extractor + keystream dump |
| `tmp/proscenic/jiagu/libmono_x86_clean.so` | decrypted stage-2 ELF |
| `tmp/proscenic/jiagu/mips_keystream.bin` | RC4 keystream to re-encrypt `.mips` |
| `.../assets/libjiagu_x86_sigpatch.so` | gate-1-patched loader (repacked) |
| `emulator/certwork/orig_cert.der` / `.hex` | original `youren` signer cert |
| `emulator/psnic_sig.apk` | patched, debug-signed x86 build used for the dump |
| `doc/reverse-engineering/dex/app_main.dex` | **decrypted real app** (com.baole.blap) — regenerate locally, gitignored, **not tracked** (embeds a vendor Alibaba Cloud AccessKey ID) |
| `doc/reverse-engineering/dex/app_secondary.dex` | decrypted library dex — regenerate locally, gitignored, not tracked |

## 7. Why this was necessary

The goal was the rehome logic: how the robot connects to its cloud and how to point
it at a replacement server. That understanding is split across the robot firmware
(the robot's Channel-B control link — `PROTOCOL.md` §B, the rehome target) and the
app (its Channel-A cloud link and command vocabulary — `WIRE_PROTOCOL.md`,
`COMMANDS.md`). The app half lives in the encrypted dex, so the packer had to be
defeated to read `com.baole.blap.module.{adddevice,imsocket,...}`. Note the app's
`imsocket` (Channel A, 20-byte imsocket to `bl-im.robotbona.com:20008`) is **not**
the protocol the robot speaks; the robot uses Channel B (see `PROTOCOL.md` §B).
