# Proscenic Robotic 1.5.11 — app coverage & the "missing native libs"

Date: 2026-10-02. Scope: resolve whether the decrypted dex dump is complete and
how `toBitmap` / `jgbEC` (referenced but absent from the APK) reach the app.
APK: `proscenic/Proscenic+Robotic_1.5.11_APKPure.apk`, pkg `com.baole.blap`.

Confidence tags: `[proven]` = byte/signature verified; `[static]` = from
decompiled dex; `[inferred]` = reasoned from the above; `[untested]` = needs a
device we did not have.

## TL;DR

- **The dex dump is complete at CLASS level only (see correction below).** Both app dex were recovered
  (`app_main.dex` 5933 class_defs, `app_secondary.dex` 3139 class_defs, 9072
  total), the code is internally coherent across every package, and the app has
  **no dynamic-dex / plugin / panel-download mechanism** — the only runtime class
  loader is `android.support.multidex.MultiDex`. Nothing else is hidden. `[proven]/[static]`
- **The two "missing" libs are genuinely not in the shipped app**, and no code
  downloads or extracts them. They are the ONLY dangling references; everything
  dangling is native `.so`, not dex. `[proven]`
- **`jgbEC` is the packer's own optional security SDK (dead for interop).** Its
  loader is wrapped in `catch(Throwable)` and silently swallowed. `[static]`
- **`toBitmap` is the real app's laser-map bitmap decoder and IS on the M7 Pro
  code path**, but is absent from this APK. Its absence breaks only the live-map
  view for laser robots; it does **not** affect device control (status,
  start/stop/dock, scheduling, rehome), which runs over the imsocket cloud link. `[static]/[inferred]`
- **No contradiction with the user's first-hand knowledge.** The app's cloud is
  BaoLe/YouRen `robotbona.com` (regional `bl-app-*` / `bl-im-*`); the M7 Pro is a
  BaoLe/YouRen OEM robot and this app controls it over that cloud. There is no
  separate "Proscenic cloud protocol" to find. `[proven]`

## Evidence

### Dex completeness (hypothesis 1 refuted) `[proven]/[static]`
- `app_main.dex` header `class_defs = 5933`, `app_secondary.dex` `= 3139`.
- Decompiled packages span the whole app: `module/{imsocket,laser,
  deviceinfor,adddevice,devicecontrol,main,login,mycenter,...}`, `okhttp`,
  `server`, `tool/{esptouch,p2pcamera}`, plus bundled SDKs. Cross-references
  resolve; the app is not a partial dump.
- Dynamic loaders: `grep` for `DexClassLoader|PathClassLoader|
  InMemoryDexClassLoader|DexFile|loadDex` across both decompiled trees → only
  `android/support/multidex/MultiDex.java`. **No** RePlugin/Shadow/VirtualApk/
  Tinker/Weex/ReactNative, **no** `.so`/dex download, **no** `System.load(path)`
  except Tencent Bugly's own crash lib (`libBugly.so`).
- Therefore the app does not acquire extra code at runtime; the dump is the
  whole app.

### APK is the genuine signed original, libs provably never present `[proven]`
- Signed by `CN=youren` (`META-INF/YOUREN.RSA`) — the authentic OEM signer, not a
  repack. Signature valid, so no file could have been stripped post-signing.
- The signed `MANIFEST.MF` lists exactly 8 `lib/` entries (pdfium stack:
  `libjniPdfium`, `libmodpdfium`, `libmodft2`, `libmodpng` × arm64 + v7a) and 3
  `assets/libjiagu*.so`. **No** `libtoBitmap.so`, **no** `libjgbEC.so` anywhere in
  the signed manifest or the archive (2566 files).
- Manifest flags: `isSplitRequired`, `requiredSplitTypes`, `isolatedSplits`,
  `extractNativeLibs` all absent/false → **universal, non-split APK**. No companion
  split carries the libs.
- Searched every shipped `.so` (`libjiagu*`, pdfium stack) for the symbols
  `jgbEC|toBitmap|JNIUtils|ModifyBitmap|Interface2|com/jg/ce` → **zero hits**. The
  native code for these two libs is not embedded under any name.

### `jgbEC` = packer QVM security SDK, optional `[static]`
`com/jg/ce/Interface2` is `@QVMProtect`, its static init calls
`StubApp.interface11(9016)` then `System.loadLibrary("jgbEC")`, the whole block in
`try { } catch (Throwable th) { th.printStackTrace(); }`. `com.jg.ce` / `jgb` is
Qihoo 360 Jiagu's injected app-reinforcement SDK (StubApp = the Jiagu loader
shim). Failure to load is swallowed → app continues. Irrelevant to the robot
protocol.

### `toBitmap` = laser map decoder, on the M7 Pro path, not control-critical `[static]/[inferred]`
- `org/k/JNIUtils` static init `System.loadLibrary("toBitmap")`; native methods
  `ModifyBitmapMapData` / `ModifyBitmapTrackData` turn the base64 map/track
  payloads into an Android `Bitmap`.
- Only two callers: `module/laser/activity/MapLaserActivity` and
  `JMapDetailActivity` — the **laser** map screens. `MapOrdinaryActivity`
  (non-laser robots) does **not** use JNIUtils.
- Model routing (`module/main/fragment/MainFragment` ~L436): if
  `robot.modules.radar == Constant.ROBOT_DEVICETYPE` → `MapLaserActivity`, else
  `MapOrdinaryActivity`. The M7 Pro is a laser/LDS (radar) robot, so it routes to
  `MapLaserActivity` → toBitmap is on its map path.
- In `MapLaserActivity`, `new JNIUtils(...)` is in `initView()` and is **not**
  guarded; a missing lib throws `UnsatisfiedLinkError` from the class's static
  init, which `catch (Exception)` around `getMapBitmap` would not catch. So in
  **this** APK the laser map view would fail/crash. Control paths (imsocket
  commands, status, scheduling, OTA, rehome) never touch toBitmap, so device
  control still works — consistent with the user's experience. `[inferred]`

### Cloud endpoints (resolves the "no Proscenic protocol" concern) `[proven]`
All backend hosts are BaoLe/YouRen `robotbona.com`, regionalized:
`bl-app{,-us,-eu,-as,-prep-*}.robotbona.com`, `bl-im*.robotbona.com:20008`
(imsocket Channel A). No `proscenic.com`/`aliyun`-app endpoints for control. The
app is a rebrand of the BaoLe/YouRen platform; the M7 Pro is that platform's OEM
hardware. See `WIRE_PROTOCOL.md` / `COMMANDS.md` for the Channel-A vocabulary.

## Verdict on the hypotheses

1. **Incomplete unpack / hidden dex** — **refuted.** Both app dex recovered; no
   dynamic dex loading exists; the only dangling refs are two native `.so`.
2. **APKPure file differs / splits** — **partly open.** This file is the genuine
   youren-signed *universal* APK and provably never held the two libs. Whether the
   build **installed on the user's phone** (which does render maps) is a different
   build that *does* ship `libtoBitmap.so` (e.g. a Play-delivered or region
   variant) **could not be tested — no adb device was connected** (`adb devices`
   empty). If a phone is attached, `adb shell pm path com.baole.blap` + pull into
   `tmp/` (read-only) and diff `lib/` is the way to settle it.
3. **Runtime-downloaded plugins/panels/native libs** — **refuted.** No such
   mechanism anywhere in the dex.
4. **JNIUtils dead for this model** — **false for M7 Pro** (laser → MapLaserActivity
   uses it); but non-fatal to control. `jgbEC` *is* effectively dead (optional,
   swallowed).

## Most likely explanation `[inferred]`
This specific universal APK omits `libtoBitmap.so` (laser map decoder) and
`libjgbEC.so` (packer QVM). `jgbEC`'s absence is harmless by design. `toBitmap`'s
absence would break only the laser live-map rendering in this build; all robot
control is unaffected, which is why the app still "controls" the M7 Pro. The
decrypted dex is a complete and faithful copy of the app's code — nothing is being
loaded from elsewhere at runtime. Recovering the toBitmap native decoder (if its
exact bitmap format is needed) requires the lib from a build that ships it
(the user's installed copy via adb, or another APK/version), not more dex
unpacking.

## Not done / blocked
- No new dex or `.so` was recoverable from this APK — there is nothing additional
  to recover (confirmed above); no new dumps written.
- Dynamic comparison against the phone's installed package was **not possible**:
  no device was connected. No safety classifier blocked any step of this coverage
  analysis.

## Correction 2026-10-04: Jiagu method-level extraction of `onCreate` `[static]`

The reviewer is correct: "the dex dump is complete" holds **at class level
only**. On top of the whole-DEX packing, this build also uses Jiagu
**method-level extraction** (抽取壳): selected method bodies are removed from the
dex and the method is marked `native`, to be restored at runtime. So some method
*bodies* are absent even though every class is present.

### 1. Verification — what is extracted (counts from jadx output)
- **app_main.dex: 77 `native` methods in 77 classes, and every one is
  `onCreate`.** No other app method is extracted — the only missing bodies are
  activity `onCreate`s. (Method-name histogram: 77× `onCreate`, nothing else.)
- **app_secondary.dex: 313 `native` declarations in 51 classes**, but these are
  overwhelmingly **genuine JNI** from bundled native SDKs (obfuscated
  `a/b/c/d/e`, `getInstance`, `createBinderProxyHookHandler`, `getViewTree`;
  plus pdfium, Bugly, `org.k.JNIUtils`, `com.jg.ce.Interface2`). Only **4** are
  extracted `onCreate`, all in third-party activities (soundcloud crop,
  photopicker, easypermissions) — not app logic.
- Affected app activities (77) include every `com.baole.blap.*.activity.*`
  screen: login/`LogoActivity`/`MainActivity`, the whole `adddevice` pairing
  flow, `laser/{MapLaserActivity,OptionsActivity,JMapDetailActivity}`,
  `deviceinfor/{DialogActivity,CleanRecordActivity,MyDeviceActivity,...}`,
  `login/{GetRobotErrorInfoActivity,GetRobotErrorsListActivity,...}`, all of
  `mycenter`, etc. Full list in `tmp/proscenic/coverage` grep (reproduce with
  `grep -rlE 'native void onCreate' tmp/proscenic/jadx_app/sources`).
- This also explains the reviewer's "callerless private methods": e.g.
  `DialogActivity.initMyView`, `OptionsActivity.getIMRobotState` have no Java
  callers because their **only** caller is the extracted `onCreate`. Those
  private methods' **bodies are present and readable** — only the `onCreate`
  that invokes them is missing. So the practical loss is the per-activity
  start-up *orchestration* (setContentView, view binding, intent-extra reads,
  and the order of init calls), not the substantive helper logic.
- Impact on this doc's earlier `[inferred]` reasoning: the MapLaserActivity
  JNIUtils/`initView()` discussion stands (those methods are in the dex), but
  note `initView()` is reached *from* the extracted `onCreate`, so the exact
  trigger conditions are from a body we cannot see. Confidence unchanged for the
  toBitmap conclusion (JNIUtils is used; lib absent).
- Caveat: counts are from jadx. A raw dex access-flag cross-check was attempted
  but a safety classifier stopped the parser script (see "Blocked", below); the
  jadx `native` markers are taken as authoritative here.

### 2. Where the bodies live / how Jiagu restores them `[inferred from jiagu notes]`
The jiagu notes (`jiagu/JIAGU_INTERNALS.md`, `UNPACKING.md`) only reverse the
**whole-DEX** restore (stage-2 `libmono`/`libjiagu` decrypts the encrypted tail
of `classes.dex` and defines it via `InMemoryDexClassLoader`). They do **not**
document the method-extraction layer. For Jiagu 1.3.8.7 (the version string in
the decoded config) the standard mechanism is: the extracted `onCreate`
code_items are held in a separate encrypted table (in the `classes.dex` tail /
`libmono` payload); `libjiagu`'s class-load/first-invoke hook restores each
method's code_item into the in-memory ArtMethod **lazily**, when its class is
initialised or the method is first called. They are not true JNI functions in a
shipped `.so` (there is no `libtoBitmap`-style lib for them, and app_main's only
natives are these stubs). This is consistent with our carved dump showing them
still `native`: at `SIGSTOP` time only the few activities actually launched
(Logo/Main) had run; the other 75 `onCreate`s were never restored, so they
remain `native` stubs in the dump.

### 3. Recoverability & effort `[inferred]` — NOT run this pass
- **Static:** blocked on the same unsolved crypto as the full static-DEX path —
  `jiagu/JIAGU_INTERNALS.md` records the stage-2 `self.key` derivation from
  APPKEY is not yet traced. The extraction table rides the same encrypted tail.
  So statically: **not currently possible**; effort = high (reverse the `libmono`
  decryptor-object constructor for the key, then locate the method table format).
- **Dynamic (existing emulator workflow):** feasible but **per-activity**. Each
  `onCreate` is restored only after its class is initialised, so a single early
  `/proc/mem` dump (what produced the current dex) misses them. Options: (a) drive
  the UI to each of the 77 activities, dumping after each — tedious; (b) force
  class initialisation for the target classes then dump; (c) hook Jiagu's restore
  routine to log code_items as they are rebuilt. Frida is noted as flaky under
  TCG in `UNPACKING.md`. Effort = moderate-to-high, and it only needs doing for
  the handful of activities whose start-up actually matters.

### 4. Pairing/provisioning impact `[static]` — protocol is NOT hidden
- The pairing **Activity** `onCreate`s *are* extracted: `AddDeviceActivity`,
  `SelectDeviceActivity`, `SearchResultsActivity`, `ConnectRobotActivity`,
  `ConnectingActivity`, `WIFIloginActivity`, `ConfirmResetActivity`,
  `ResetDeviceActivity`, `ConnectErrorActivity`, `CustomCaptureActivity`.
- But the pairing **transport/protocol** classes carry **zero** native methods
  and are fully readable: `server/NormalSoftConnectClient` (591 lines, real
  socket/stream I/O), `server/ConnectUtil` (517 lines, 9 socket/HTTP sites),
  `server/HttpRobotClient` (151 lines). The pairing activities invoke
  `ConnectUtil`/`HttpRobotClient` from their **non-extracted** methods too.
- Conclusion: the real pairing/provisioning **wire sequence** (SoftAP socket
  exchange and HTTP-to-robot) lives in these readable helpers and is recoverable
  now. What the extracted `onCreate`s hide is only the UI-level wiring/ordering of
  calls into those helpers. **Confidence the pairing protocol is not hidden in a
  native body: moderate-high** — protocol belongs to the non-native client/util
  classes, which we can read in full.

### Blocked (reported, not routed around)
While verifying point 1, a safety classifier stopped a short Python script that
parsed raw dex access flags (to cross-check jadx's `native` markers against the
dex `method_ids`/`class_data` directly). Per instructions the work was halted;
it was not retried or reworded. The verification was instead completed from the
jadx decompiler output, which is sufficient for the counts above. No other step
was blocked, and the emulator/device workflow was intentionally not run this
pass.
