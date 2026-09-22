# Qihoo 360 "Jiagu" packer internals — as used by the Proscenic app

Target: `assets/libjiagu_a64.so` (arm64) inside `com.proscenic.psnic` 1.5.11.
Reversed with Ghidra + a Unicorn emulation harness (`emu_extract_core.py`).
**[proven]** = executed/emulated or byte-verified; **[static]** = from decompilation.

## Top-level shape [proven]
- `classes.dex` (4.67 MB) is a **stub** — 12 loader classes (`com.stub.StubApp`,
  `com.qihoo.util.*`). The real DEX is **not** appended in the SafaSafari layout
  (last 4 bytes are not a shell length), and is **not** a separate asset. It is
  produced at runtime by a native core.
- `StubApp` (the manifest `<application>`) loads `libjiagu` and calls native
  `interface5/7/8(Application,Context)` to unpack. `JNI_OnLoad` bootstraps everything.

## libjiagu_a64.so is a STAGE-1 loader for an embedded STAGE-2 core [proven]
`JNI_OnLoad → __arm_a_1`:
1. `FUN_00106854()` anti-debug check → `raise(9)` (SIGKILL) on failure.
2. `FUN_00106688()` decrypts & **custom in-memory-loads** a second ELF (the strings
   call it `libmono.so`) — a userspace `dlopen` (`DT_INIT_ARRAY` handling, no file
   touches disk; segment loader `FUN_001031b4`).
3. `FUN_001067c0()` finds the symbol **`makekey`** in the loaded core, `mprotect`s it
   RWX, and **overwrites it with a pointer to libjiagu's `__arm_a_2`** — i.e. the core
   calls back into libjiagu for its key routine.
4. Resolves and calls the **core's** `JNI_OnLoad` → the core registers the real
   `interfaceN` natives and performs the DEX unpacking.

So the DEX-decryption logic lives in the **runtime-only core** (`libmono`), which is
itself encrypted inside libjiagu and never on disk.

## The two embedded blobs [proven]
Custom (camouflaged) sections in libjiagu_a64.so:
| Section | File off | Size | Role |
|---|---|---|---|
| `.mips` | `0x1b430` | `0x85e6c` (548,460) | encrypted+compressed **stage-2 core** (libmono) |
| `.bmp`  | `0x1b008` | `0x426` (1062)     | encrypted **key material** for the core |

Both are high-entropy (≈7.9); no relocations; raw bytes at those offsets.

## The core (`.mips`) decryption pipeline — RC4 + zlib [proven]
`__arm_c_1::__arm_c_0` (`0x104fb0`) does, verbatim:
```
dest = calloc(len); memcpy(dest, .mips, len=0x85e6c)
FUN_00104ef4(this, sbox[264])            # build RC4 state from .bmp key material
FUN_00104cb0(dest, len, sbox)            # RC4 PRGA, decrypt in place
outlen = *(u32*)dest                     # first 4 bytes = uncompressed length
uncompress(out, &outlen, dest+4, len-4)  # zlib inflate  -> the real ELF (libmono)
```
- **`FUN_00104cb0` is textbook RC4 PRGA** (S at `sbox`, i@`0x100`, j@`0x101`;
  `i++; j+=S[i]; swap S[i],S[j]; out^=S[(S[i]+S[j])&0xff]`). [proven]
- **`FUN_00104a50` is textbook RC4 KSA**: S initialised to the identity permutation
  from a constant table at `0x4bb0` (verified `00 01 02 … ff`), then
  `j=(j+S[i]+key[i mod keylen])&0xff; swap`. [proven]
- So the ONLY unknown is the **RC4 key**, derived from `.bmp`.

## The RC4 key derivation from `.bmp` — SOLVED [proven]
`FUN_00104ef4 → FUN_00105244 → FUN_001050c8` parse `.bmp` as a bespoke fake-ELF
structure (14-byte header + body with a `1<<(field&0x1f)` table size and `0x10`-byte
entries), and `FUN_001052e4` (a GF(2) bit-parity transform) + `FUN_00104a50` (the
RC4 KSA) turn the extracted bytes into the RC4 key.

**Two things had hidden this:**
1. **`.bmp` is a real BMP *image*** (it starts with `42 4D` = "BM"; the section is even
   named `.bmp`) — the RC4 key material is **steganographically embedded in a bitmap**,
   which is why it looked "high-entropy/encrypted" and why the parser's fields only make
   sense once read from the true bytes.
2. **A 0x10000 vaddr↔file-offset skew.** libjiagu's data segment is mapped with
   `p_vaddr - p_offset = 0x10000`, so `.bmp`/`.mips` (vaddr `0x1b008`/`0x1b430`) actually
   live at **file offsets `0xb008`/`0xb430`**. Reading them at the vaddr-as-file-offset
   gave garbage. Honour the program headers and the bytes are correct.

**Recovery (proven, reproducible — `extract_libmono.py`):** map libjiagu by program
headers, apply `R_AARCH64_RELATIVE`, then **emulate the native key schedule
`FUN_00104ef4` with Unicorn** (hooking the allocator/`memcpy` PLT stubs) to obtain the
exact 256-byte RC4 S-box. RC4-decrypting `.mips` then yields
`a8 89 14 00 | 78 9c …` — i.e. `len=0x1489a8` followed by the **zlib magic** — and it
inflates **cleanly to exactly 1,345,960 bytes** (a clean inflate to the declared length
is proof the key is correct). That blob is a **custom core container** whose embedded
**`libmono.so` ELF is carved at its `7f 45 4c 46` magic (offset `0x120a0`)** →
`libmono.so`, 1,272,072 bytes, valid AArch64 ELF. **Extracted.**

## The final layer — libmono decrypts the app DEX (remaining)
`libmono.so` (stage-2 engine, statically extracted) registers the real
`interface1..14` natives and, per its strings (`InMemoryDexClassLoader`,
`/dev/ashmem/dalvik-classes.dex`, `%s/classes%d.dex`, `File check or assets decrypt`),
decrypts the **encrypted TAIL of `classes.dex`** — the ~4.66 MB after the 12-class stub
(from ~offset `0xb000`, entropy 7.96) — into the real `com.baole.blap` app DEX and
defines it in memory. The tail does **not** reuse the `.mips` RC4 key (verified), so
this last cipher/key must be reversed inside `libmono.so`. That is the one remaining
step to a fully static DEX; alternatively a dynamic dump yields it directly.

## Anti-analysis [proven]
- All libjiagu strings are **XOR-`0xa5`** obfuscated (e.g. `__arm_a_21` builds
  `/system/bin/linker` and `rtld_db_dlactivity` to detect linker breakpoints).
- `__fun_a_18` is a **bytecode/VM interpreter** (opcode switch 0x06–0x1a) — an
  additional obfuscation layer.
- Anti-debug via `/proc/self/maps`, linker inspection, `raise(9)`.

## `__arm_a_2` ("makekey") [proven]
A custom 16-byte KDF: rolling hash (`h = (in[i] + h*0x1f) & 0xff`) into a 16-byte
buffer + a keystream mix updating an int state. Patched into the core as `makekey`.
Likely the seed generator for one of the RC4 keys.

## Status & how to finish (UPDATED)
Fully characterised the core crypto (**RC4→len→zlib**) and built a working arm64
emulation harness (`emu_extract_core.py`) that runs the packer's *own* KSA+PRGA+inflate
— it will emit `libmono.so` the moment the `.bmp` key bytes are correct. Two ways to
close the last gap:
1. **Static:** locate/emulate the `.bmp` pre-decrypt (search callers of `.bmp` /
   `__arm_a_2`, or emulate from `__arm_a_1` with a stub JNIEnv and the anti-debug
   forced true), then the harness yields `libmono`; then reverse `libmono`'s DEX
   decryption (a further, separate layer).
2. **Dynamic (pragmatic, industry-standard for Jiagu):** run the APK on a real arm64
   device and dump the DEX from memory (FRIDA-DEXDump / hook libart `OpenMemory`).
   This sidesteps all of the above and is far cheaper than peeling every layer.

---

## Final layer — status (app DEX in the classes.dex tail)

Progress from reversing the extracted `libmono.so` and the stub `classes.dex`:

* **Stub `classes.dex` layout:** 12-class loader DEX ends at the map (`~0x0a70c`);
  everything after is Jiagu's appended container.
* **Config manifest (SOLVED):** the bytes right after the stub are Jiagu's config,
  **XOR-`0x52` encoded** (decoder shipped: `jiagu360_core.decode_dex_tail_config`).
  It yields `APPKEY=7bd3e651dc89db26`, `appName=com.baole.blap.app.BaoLeApplication`,
  `pkg=com.proscenic.psnic`, `stubAppName=com/stub/StubApp`, `jiaguVersion=1.3.8.7`,
  `activityName`, `versionName=1.5.11`, `apk-md5`, `checkSum`, `sig`, `protect-time`, …
* **The encrypted app DEX** is the remaining ~4.6 MB (from `~0x0b000`). Its entropy
  profile is **striped**: `~0x000000–0x0c0000` ≈ 8.0 (encrypted), `~0x100000–0x340000`
  ≈ 5.6–6.0 (DEX-bytecode-like, zero-heavy, no ASCII pool), `~0x380000+` ≈ 8.0. So the
  string pool / header regions are encrypted while code sections are lighter — a
  partial-encryption scheme.
* **`libmono.so` decrypts it** (strings: `apk@classes.dex`, `a@assets/.appkey`,
  `getAppkey`, `File check or assets decrypt`, `%s/classes%d.dex`,
  `/dev/ashmem/dalvik-classes.dex`, `InMemoryDexClassLoader`), keyed off `APPKEY`
  (via the `makekey` KDF). The exact `makekey(APPKEY)` = `07db52d90b8a5b8e59dc5880a3dba18b`
  (emulated) but a straight RC4(that) over the tail does **not** produce the DEX — the
  derivation combines more inputs and/or the partial-encryption offsets.

**Blocker on the pure-static finish:** `libmono.so`'s own **program and section headers
are encrypted** (it is loaded by libjiagu's custom in-memory ELF loader, which decrypts
the phdr/shdr records via the `FUN_0010368c`/`FUN_00103738` helpers). With no valid
section layout, Ghidra auto-analysis does not recover the string cross-references and
ADRP+ADD scans don't resolve, making the exact tail-cipher hard to pin statically
without first emulating libjiagu's ELF loader over `libmono`.

**Fastest finish:** a dynamic dump (once KVM/an arm64 emulator is available) —
FRIDA-DEXDump captures the plaintext DEX `libmono` hands to `InMemoryDexClassLoader`,
bypassing this entire layer. Static completion is possible but requires emulating the
custom ELF loader to un-encrypt `libmono`'s headers, then reversing its tail-cipher.

---

## libmono internals — app-DEX cipher FORMAT fully mapped (key = last unknown)

**Analysis unblocker:** `libmono.so`'s program & section headers are encrypted, so
Ghidra loaded it wrong (garbage layout, no real code/strings). Fix: rewrite it with a
single clean identity `PT_LOAD` (RWX, off=0, vaddr=0, filesz=whole) and zeroed section
headers → `libmono_fixed.so`. Ghidra then fully analyses it (2122 strings, real funcs).

**libmono RC4 (same primitive as libjiagu):**
* `FUN_0018fc00` = RC4 **KSA** — copies the identity table at `0x8fd50` into the S-box,
  then standard `j=(j+S[i]+key[i%keylen])&0xff; swap`.
* `FUN_0018fe50` = RC4 **PRGA** (`out ^= S[(S[i]+S[j])&0xff]`).
* `FUN_0018f0b4(out,in,len,keyObj)` = RC4-decrypt `len` bytes with `keyObj` (a
  std::string: SSO if `*keyObj&1`==0 → bytes at `keyObj+1` len `*keyObj>>1`, else heap
  ptr `keyObj+0x10` len `keyObj+8`).

**App-DEX decryptor `FUN_0018f53c(self, &out, &outlen)` — the exact format [proven-static]:**
1. `fstat(self.fd)` → file size; `mmap` the encrypted-dex file.
2. `FUN_0018f0b4(p, p, 0x20, self.key)` — RC4-decrypt the **first 32 bytes** (header).
3. Check **magic `0xFEEDDEEF`** (`*p == -0x1122111`) at offset 0; **payload length at
   offset 0x14** (`p[5]`).
4. `FUN_0018f0b4(p+0x20, p+0x20, len, self.key)` — RC4-decrypt `len` bytes from **offset
   0x20** → the real DEX; copied out.
   ⇒ container = `RC4_key( [0xFEEDDEEF ‖ … ‖ len@0x14 (32B header)] ‖ [DEX payload] )`.

**The key** (`self.key`, a std::string) is derived from **APPKEY** (`7bd3e651dc89db26`,
from the XOR-0x52 config) — but it is **not** the plain APPKEY, its hex form, nor
`makekey(APPKEY)`=`07db52d9…` (all ruled out by the `FEEDDEEF`/`dex\n` oracle over the
tail). The exact derivation is set in the decryptor object's constructor (not yet
traced) and/or the encrypted-dex is first extracted from the classes.dex tail to a file
(the `FEEDDEEF` header is not at any raw-tail offset under those keys). Also present: a
`/proc/self/maps` scanner (`FUN_0014eecc`) that locates the *already-decrypted*
in-memory dex (`/dev/ashmem/dalvik-classes.dex`) for OAT/hotfix — not the decrypt path.

**Remaining to a static DEX:** (1) trace the decryptor-object constructor to get the
exact `self.key` derivation from APPKEY, and (2) the encrypted-dex file provenance
(extraction from the tail). With the key, decryption is a ~10-line RC4 per the format
above. The `FEEDDEEF` magic is a strong known-plaintext oracle for validating any
candidate key. Dynamic dump remains the trivial alternative.

---

## libmono key-derivation chain — traced to APPKEY (exact transform = last mile)

Full call chain for the app-DEX/cache cipher, from `libmono_fixed.so` (Ghidra):

* **RC4 primitive:** `FUN_0018fc00` (KSA, identity-init at `0x8fd50`) + `FUN_0018fe50`
  (PRGA). Wrapper `FUN_0018f0b4(out,in,len,keyStr)` = RC4 with a std::string key.
* **Encrypted-file object** (`0x70`-byte class):
  - ctor `FUN_0018f980(self, path, keyStr, 0xc2)` → `open(path, O_RDWR|O_CREAT)` into
    `self.fd`, and **`self.key = keyStr`** (`FUN_00122d18(self+2, keyStr)`); on create it
    writes an RC4-encrypted 32-byte header `[0xFEEDDEEF | 12B | len@0x14]`.
  - reader/decryptor `FUN_0018f53c(self,&out,&outlen)`: mmap → RC4 first `0x20` (check
    magic `0xFEEDDEEF`, len at `0x14`) → RC4 `len` bytes at `0x20` → DEX. (§ above.)
* **Key string = base ‖ suffix.** `FUN_0018e894` builds the key as
  `FUN_00128344( FUN_0018e548(base), "pk_auto" )` (std::string concat; `pk_auto` @
  `0x20f838`). Different consumers use different suffixes.
* **Base key = the APPKEY config value.** `FUN_0018e548` lazily caches a global
  (`_DAT_002465e8`) set from the `appkey`/`APPKEY` value read out of the XOR-0x52 config
  (`FUN_00186f70`=get-config, `FUN_001873ac`=map-lookup; keys `appkey`@`0x212970`,
  `APPKEY`@`0x212978`). `FUN_001f5058` populates a config/state struct with several keys
  derived from it via `FUN_001f4ee0` / `FUN_001f4de4` (per-purpose transforms).

**Last mile (open):** the exact **base-key transform** (`FUN_0018e548`/`FUN_001f4ee0`/
`FUN_001f4de4` — is the base the raw APPKEY string, or a transform of it?) and the
**suffix used for the primary app DEX** were not fully pinned; and the primary app DEX
is decrypted from the APK's `classes.dex` tail on first run and **re-cached** to a
private FEEDDEEF file (`FUN_0018f980` is `O_CREAT`), so the on-disk cache differs from
the tail. Recovering the exact key is now a bounded job: **emulate the key-derivation
chain** (`FUN_0018e548`→`FUN_001f4ee0/de4`, with the APPKEY value as input) the same way
`makekey`/RC4 were emulated, giving the concrete key string; then RC4-decrypt per the
FEEDDEEF format. The `0xFEEDDEEF` magic is a definitive oracle to confirm the key.

---

## DEFINITIVE conclusion — the app DEX is a RUNTIME-ONLY artifact (static extraction not tractable)

This section supersedes the "last mile / bounded job" optimism above. After tracing the
actual load mechanism in `libmono_fixed.so`, the FEEDDEEF/RC4 cipher documented earlier
turns out to be a **cache side-path, not the primary decryptor**, and the real app DEX
never exists as a static, key-decryptable blob in the APK.

### What actually loads the DEX
* The engine loads classes via **`dalvik/system/InMemoryDexClassLoader`** and the native
  **`art::DexFile::OpenMemory(const u1* base, size_t size, …)`** (both strings resolved by
  `dlsym` in `FUN_001953dc`; loader wrapper `FUN_0012713c`; orchestrator `FUN_00130210`).
  The DEX is handed to ART **as an in-memory buffer** — it is never written to disk in
  plaintext.
* `FUN_00130210` (the ~2700-line orchestrator) calls `OpenMemory` via `FUN_001959d4` with a
  base/length taken from a **runtime-populated structure**, then builds an in-memory class
  loader. It also manages OAT/odex caching (`/system/bin/dex2oat`, `%s/classes.oat`),
  hotfix (`/data/data/%s/.jiagu/hotfix/…`) and jiaguVersion gates — all runtime paths.

### The packer finds the DEX by scanning its own memory map
* `FUN_0014eecc` is **not** a decryptor. It builds `"/proc/%d/maps"`, `fopen`s it, and
  `fgets`/`strstr`-scans each line for **`/dev/ashmem/dalvik-classes.dex`**,
  `apk@classes.dex`, or `.odex`; on a match it parses the `START-END` hex range and invokes
  a callback over that **live-mapped region**. This is pure runtime memory introspection
  (used for integrity/anti-tamper and to relocate the ART-mapped image). It cannot be
  reproduced offline because it reads memory that exists only in a running process.

### Why the FEEDDEEF/RC4 path does not yield the APK's DEX
* `FUN_0018f980` opens its cache with **`O_RDWR|O_CREAT|O_EXCL` (mode `0xc2`)** and, on a
  freshly created (empty) file, **writes** a newly RC4-encrypted `0xFEEDDEEF` header
  (`local_a8 = 0xfeeddeef; FUN_0018f14c(fd,&hdr,0x20,key)`). So `FUN_0018f980` (writer) /
  `FUN_0018f53c` (reader) are a **private on-device cache** at `/data/data/<pkg>/.jiagu/…`,
  produced from the already-decrypted DEX on first run. That file **is not in the APK**, so
  there is nothing on disk for the FEEDDEEF oracle to match.
* Empirical confirmation: RC4 (and AES/zlib/gzip) sweeps over the APK's `classes.dex`
  high-entropy blobs (`0x10000–0x120000` and `0x370000–EOF`) with **every** key candidate
  from the APPKEY family (raw APPKEY, `makekey(APPKEY)` as hex and as raw 16 bytes,
  `…+"pk_auto"`, `…+versionName+versionCode`, package name, and their MD5s) produced **no**
  DEX/zlib/gzip/FEEDDEEF header at any of ~180 offsets. The primary decrypt is interwoven
  with the anti-debug'd native mono engine and JNI-read runtime values
  (`versionName`=`1.5.11`, `versionCode`=`24`, and — per `getPackageInfo`/`GET_SIGNATURES`
  strings — very likely the APK signing certificate), so it is not a standalone offline
  cipher.

### The `classes.dex` in the APK
* It is a **single valid 4.67 MB DEX** whose header `file_size` (`0x4746a8`) equals the whole
  file, so Android's verifier accepts it. Only `0x0–~0x10000` is the real stub
  (`com.stub.StubApp` + loader); the two high-entropy spans are the embedded encrypted
  payload carried as DEX "data", and the mid-entropy span is stub code/strings.

### The only tractable extraction: dynamic dump (needs a running instance)
Because the plaintext DEX materialises solely in process memory, recover it at runtime:
1. **Frida hook on `art::DexFile::OpenMemory`** — dump `[base, base+size)` at entry. This is
   the canonical Jiagu unpack point and yields the DEX bytes directly.
   ```js
   const sym = Module.findExportByName("libart.so", "_ZN3art7DexFile10OpenMemoryEPKhm...");
   Interceptor.attach(sym, { onEnter(a){ const base=a[0], size=a[1].toInt32();
     Memory.protect(base,size,'r--'); const f=new File("/sdcard/dump.dex","wb");
     f.write(Memory.readByteArray(base,size)); f.close(); }});
   ```
   (Alternatively hook the `InMemoryDexClassLoader(ByteBuffer)` ctor and dump the buffer.)
2. **/proc/pid/maps dump** — copy the `/dev/ashmem/dalvik-classes.dex` region straight from
   the running process (the same region `FUN_0014eecc` itself locates).

Both require a live process: the **KVM-accelerated AOSP emulator** the user is enabling, or
a rooted device (unavailable). Static analysis has fully characterised the *packing routine*
(stage-1 → libmono extraction, all crypto primitives, key chain, cache format, and the
InMemoryDexClassLoader/OpenMemory load path); the byte-exact app DEX is the one artifact
that is, by construction, runtime-only.
