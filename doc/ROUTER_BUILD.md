# Building noobscenic for the ASUS RT-AC68U (FreshTomato, USB stick)

This is the always-on deployment target from PLAN §18: the router runs the server
and the robot talks to it on the LAN, with the binary and all state on a USB stick.
Nothing here needs a toolchain *on* the router.

Status (2026-10-12): the cross-build is verified from a Windows host — all four
feature combinations build, the deployment binary is 3.2 MB after the size pass
below, the artifact is a statically linked soft-float ARMv7 ELF, and its syscalls
have been audited against the router's 2.6.36 kernel. The on-router smoke test is
still **pending** (see the checklist at the end).

## The device

FreshTomato's [hardware compatibility table][compat] lists every RT-AC68U revision
(A1/A2/B1 at 800 MHz, B2 at 1.4 GHz, C1/E1/V3 at 1 GHz) as:

* ARM, 2 cores, 1 GbE ×4, USB 2.0 + USB 3.0
* 128 MB flash, 64 KB NVRAM, 256 MB RAM
* build **K26ARM (SDK6.37)** — supported since long before the current releases

[compat]: https://wiki.freshtomato.org/doku.php/hardware_compatibility

The FreshTomato ARM sources make K26ARM concrete:

* kernel `release/src-rt-6.x.4708/linux/linux-2.6.36` → **Linux 2.6.36**
* toolchains `hndtools-arm-linux-2.6.36-uclibc-4.5.3` and
  `hndtools-arm-uclibc-7.3`/`-7.5` → the firmware userland is **uClibc**, there is
  no glibc on the box (Entware, if installed, carries its own glibc 2.23 for the
  `armv7sf-k2.6` feed — still below what current Rust glibc targets require)
* the tree's `config_base` has `# CONFIG_VFP is not set` (with
  `CONFIG_FPE_FASTFPE=y`): the kernel does not enable the FPU for user mode, so
  **any VFP or NEON instruction traps**. "Hard-float for speed" is not an option
  on this firmware.

## The target: `armv7-unknown-linux-musleabi`

| target | what it is | verdict |
|---|---|---|
| `armv7-unknown-linux-musleabi` | static musl, soft-float EABI | ✅ what we build |
| `armv7-unknown-linux-musleabihf` | musl, hard-float ABI | ❌ SIGILL: the kernel has no VFP |
| `armv7-unknown-linux-gnueabi`/`hf` | glibc | ❌ Rust documents kernel 4.15+/glibc 2.27; the router has 2.6.36 and uClibc |
| `armv7-unknown-linux-uclibceabi` | uClibc | ⚠️ Tier 3, no prebuilt `std`: needs `-Z build-std` plus a uClibc sysroot — last resort only |

Rust's own target spec is exactly what this firmware needs (rustc 1.98.1):

```rust
llvm_floatabi: Some(FloatAbi::Soft),
features: "+v7,+thumb2,+soft-float,-neon",
crt_static_default: true,
```

That is a fully static ARMv7 binary whose prebuilt `std` emits no VFP/NEON, with
no libc dependency on the router. It is Tier 2 with `std`, so `rustup target add`
is all the Rust side needs.

## Toolchain

Verified 2026-10-12 on x86_64 Windows with:

* rustc/cargo **1.98.1** — the floor is **1.94** (`sqlx 0.9.0` declares
  `rust-version = "1.94.0"`); edition 2024 alone would need 1.85
* `rustup target add armv7-unknown-linux-musleabi`
* **cargo-zigbuild 0.23.4** (`cargo install --locked cargo-zigbuild`)
* **zig 0.17.0** (`winget install --id zig.zig -e`)

A C cross-compiler is mandatory, and this is the part people miss: `sqlx`'s
`sqlite` feature enables `sqlite-bundled`, so `libsqlite3-sys 0.37` compiles
`sqlite3.c` with `cc-rs`. Zig is the entire C toolchain here; on Linux the
`musl.cc` `arm-linux-musleabi-cross` GCC or the `cross` Docker image work the
same way.

Windows:

```powershell
rustup target add armv7-unknown-linux-musleabi
cargo install --locked cargo-zigbuild
winget install --id zig.zig -e
cargo zigbuild --profile router --target armv7-unknown-linux-musleabi `
    --no-default-features --features console
```

If `zig` is not on PATH in the current shell, point cargo-zigbuild at the
executable instead of copying it: `$env:CARGO_ZIGBUILD_ZIG_PATH = "…\zig.exe"`.

Linux/CI equivalent (musl.cc GCC instead of zig):

```sh
rustup target add armv7-unknown-linux-musleabi
curl -sSfLO https://musl.cc/arm-linux-musleabi-cross.tgz
sudo tar -C /opt -xf arm-linux-musleabi-cross.tgz
export PATH=/opt/arm-linux-musleabi-cross/bin:$PATH
export CC_armv7_unknown_linux_musleabi=arm-linux-musleabi-gcc
export AR_armv7_unknown_linux_musleabi=arm-linux-musleabi-ar
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABI_LINKER=arm-linux-musleabi-gcc
cargo build --profile router --target armv7-unknown-linux-musleabi \
    --no-default-features --features console
```

The build prints two harmless linker warnings from rustc's link line
("ignoring deprecated linker optimization setting" and zig's unused
`-nostartfiles`).

## The `router` profile

`Cargo.toml` defines:

```toml
[profile.router]
inherits = "release"
panic = "abort"
lto = "fat"
codegen-units = 1
opt-level = "z"
```

`panic = "abort"` is not a size optimization first (though it saves ~1.2 MB):
libunwind's ARM register-save stubs use VFP, so a panic that unwinds would fault
on this kernel. With abort a panic never enters the unwinder. Do **not** set
`RUST_BACKTRACE=1` on the router: a captured backtrace calls those stubs.

`lto = "fat"`, `codegen-units = 1` and `opt-level = "z"` are the 2026-10-12 size
pass: 6.03 MB → 3.23 MB on the deployment build, with the artifact checks below
re-run afterwards. `-Oz` also reaches the bundled SQLite C code, because cc-rs
follows the profile's optimization level. The workload is I/O-bound, so the
codegen quality given up here does not matter.

`--no-default-features --features console` is the deployment shape: no JSONL/raw
wire tracing (which would write continuously to the USB stick), but `serve`/
`migrate` plus the one-shot CLI verbs stay available over telnet.

## What was verified (2026-10-12)

All four configurations cross-build (`cargo zigbuild`, zig 0.17.0):

| build | bytes |
|---|---|
| `--profile router --no-default-features --features console` | 3,232,224 |
| `--release --no-default-features --features console` | 6,969,592 |
| `--release --no-default-features` | 6,721,936 |
| `--release` (full server) | 7,006,056 |

After the size pass: the router profile adds fat LTO, one codegen unit and `-Oz`;
every build dropped `tracing-subscriber`'s `env-filter` feature (~0.5 MB from the
plain `--release` builds, which were pulling in `regex-syntax`, `regex-automata`
and `matchers`); and the `--no-default-features` builds leave out the new
`cli-ux` feature (clap's colour and typo suggestions — 30 KB on plain release,
13 KB on the router binary). The filter is now
`tracing_subscriber::filter::Targets` with a small parser in `src/main.rs`:
levels, `target=level` and bare targets still work; regex targets no longer do.
These move with new query shapes: the same day's history work added
180–260 KB to the plain `--release` builds (sqlx's generic query code duplicates
across codegen units without LTO) but only ~6 KB to the LTO router build.

The artifact (checked with the `llvm-tools` component):

* `elf32-littlearm`, `ET_EXEC`, `EM_ARM`, header flags `0x5000200` — EABI5 plus
  `EF_ARM_ABI_FLOAT_SOFT`
* no `PT_INTERP` and no `DT_NEEDED`: statically linked, nothing to install on the
  router
* the ARM attributes have no `Tag_ABI_VFP_args` (soft-float calling convention);
  `FP_arch: VFPv3-D16` is only the register-set tag the objects were built with

Syscall audit against the actual 2.6.36 ARM table: the disassembly has 96 `svc`
sites; 66 distinct numbers could be read from `r7` immediates. All of them are
2.6-era except two, and both have their legacy fallback present in the same
binary:

* `403 clock_gettime64` (5.1) next to `263 clock_gettime` and `78 gettimeofday`
  — musl's explicit `ENOSYS` fallback, verified in its `clock_gettime.c`
* `397 statx` (4.11) next to `327 fstatat64`

Everything else is old: `futex` 240, `epoll_ctl` 251, `eventfd2` 356,
`epoll_create1` 357, `prlimit64` 369 (added *in* 2.6.36), `utimensat` 348,
`set_robust_list` 338, `sched_getaffinity` 242, `set_tid_address` 256, `readv`/
`writev` 145/146, `stat64`/`lstat64`/`fstat64` 195–197, `getdents64` 217,
`fcntl64` 221, `mmap2` 192, `rt_sigaction` 174, … There is no `clone3`,
`epoll_pwait2` or `getrandom` (384) in the binary; the `/dev/urandom` and
legacy-syscall paths are what got linked.

Command to reproduce the artifact checks (after `rustup component add llvm-tools`):

```powershell
$bin = "$(rustc --print sysroot)\lib\rustlib\x86_64-pc-windows-msvc\bin"
& $bin\llvm-readobj --file-headers   target\armv7-unknown-linux-musleabi\router\noobscenic
& $bin\llvm-readobj --arch-specific  target\armv7-unknown-linux-musleabi\router\noobscenic
& $bin\llvm-readobj --program-headers --dynamic-table target\armv7-unknown-linux-musleabi\router\noobscenic
& $bin\llvm-size      target\armv7-unknown-linux-musleabi\router\noobscenic
& $bin\llvm-objdump -d --no-show-raw-insn target\armv7-unknown-linux-musleabi\router\noobscenic |
    Select-String -Pattern '\bvldr|\bvstr|\bvmov|\bvpush|\bvpop|\bvldmia|\bvstmia|\bvadd|\bvsub'
```

The opcode scan prints the six libunwind stubs (`vldmia`/`vstmia` of `d0–d15`,
`d16–d31`) **plus** literal-pool bytes that happen to decode as VFP instructions:
13 matches in the `-Oz` build, of which only six are real code. They come straight
from Rust's prebuilt `libunwind.a` and no linker flag removes them
(`panic = "abort"` keeps them only for explicit backtraces); they are not reached
in normal operation. To tell code from data, check whether anything branches to
each hit: the six stubs are call targets, the literal pools are not, and their
surroundings decode as `<unknown>`/`mcr p14` noise.

## Where the remaining bytes are

`cargo bloat` (0.12.1, on a build with `strip = "none"`, since the shipped binary
has no symbol table) attributed the pre-size-pass `.text` as: C code 27.9%
(essentially all of `sqlite3.c` — `sqlite3VdbeExec` alone is 47 KB, plus
FTS3/FTS5/RTREE), `noobscenic` 19.9%, `std` 12.7%, `sqlx_sqlite` 8.3%,
`clap_builder` 4.4%, `tokio` 3.8%, `regex_*` ~5.5% (now removed), then `hyper`,
`sqlx_core`, `libsqlite3_sys`, `time`, `axum` and `serde_json` at 1–2% each.
Options, roughly in order of payoff per unit of risk:

* **Done:** fat LTO + one codegen unit + `-Oz` (−42% measured) and dropping
  `env-filter` (−0.27 MB on the router build, −0.5 MB on plain `--release`), plus
  the router-only `cli-ux` slim (−13 KB here, −30 KB on plain release).
* **clap** (`clap_builder` alone 232 KB): the cheap half is done — `cli-ux` is in
  `default` and out of every `--no-default-features` build, so the router flags
  keep help/usage/error context but drop `color`/`suggestions` and their
  `anstream`/`strsim` chain. The remaining ~200 KB needs hand-rolling the five
  global flags and `serve`/`migrate` (plus the console verbs, or moving them to
  their own feature so a router build can leave clap out entirely); that costs
  help/error-message quality and interface churn.
* **sqlx → rusqlite** (measured 2026-10-12: net ~185–210 KB, ~6%): a probe pair
  running the same workload through both stacks (`router` profile, ARM) puts
  sqlx's wrapper at 367 KB over the bundled SQLite C and rusqlite's at 156 KB,
  so the saving is far smaller than the crate-level bloat numbers suggest —
  rusqlite brings back its own wrapper, and libsqlite3-sys 0.38's amalgamation is
  26 KB larger than 0.37's. Pros — no async pool machinery around a synchronous C
  library, and the `url`/`idna`/ICU chain goes away. Cons — rusqlite is blocking,
  so the pool becomes `spawn_blocking`/a connection thread (a few KB more);
  `sqlx::migrate!`'s embedded, tracked migrations have to be redone
  (`rusqlite_migration` or a `PRAGMA user_version` check); error and row-mapping
  code grows. The queries live in `src/db/queries.rs`, so the blast radius is
  bounded, but a lot of churn for ~6% unless the smaller dependency surface
  matters on its own.
* **Build `std` from source** with `-Z build-std=std,panic_abort
  -Z build-std-features=panic_immediate_abort`: drops the gimli/addr2line/object
  backtrace machinery that prebuilt `std` carries even with `panic = "abort"`,
  ~0.3–0.6 MB. Nightly-only, because rebuilding `std` is an unstable
  cargo/rustc feature, and it makes panics print nothing at all — no message, no
  location, no backtrace.
* **Linker:** `-Wl,--icf=all` is worth trying for a percent or two.

## RAM, not file size

File size is a poor RAM proxy: `.text`/`.rodata` are demand-paged in from the USB
stick, and the writable static footprint is small — ~21 KB `.bss` plus ~84 KB
`.data`/`.data.rel.ro` in the size-first build. The RAM that matters is allocated
at runtime:

* **SQLite**: the default pool is two connections (`database.max_connections`),
  each with a 1 MiB page cache (`PRAGMA cache_size = -1000`), so the engine's
  cache stays around 2 MiB instead of the 2 MiB-per-connection × 5-connection
  default (~10 MiB). `PRAGMA journal_size_limit = 4194304` caps the `-wal` file
  after a checkpoint so it cannot grow between checkpoints on the stick.
* **Transient buffers**: `gateway.max_frame_bytes` (8 MiB) is an upper bound, not
  a reservation — the channel-B decoder grows its buffer as bytes arrive and
  errors past the cap. A `/api/robot/{id}/map` fetch materialises the grid plus
  its base64 and JSON (~130 KB for the current 318×169 map) for the duration of
  that request; the UI's ~1.5 s poll is the much smaller summary.
* **Threads**: tokio uses the core count (two here); stacks are virtual until
  touched.

Measure it on the device: `grep VmRSS /proc/$(pidof noobscenic)/status` and
`free`. Expect a few tens of MB, dominated by SQLite and the allocator rather
than by the executable.

## Deploying from the USB stick

The binary is self-contained: the web UI is `include_str!`-ed and the migrations
are embedded, so only `noobscenic` plus a writable data directory go on the stick.
Use **ext2/ext3/ext4** (SQLite wants POSIX locking and the binary needs exec
permission; FAT/NTFS mounts are for sharing).

FreshTomato mounts USB drives under `/tmp/mnt/<label>`. Autostart with one of its
rc hooks — `router/rc/misc.c` runs every file in `/jffs/etc/config/` whose name
*ends* with the event name, and `usbmount` fires after an auto-mounted drive is
mounted (`init` runs before USB exists; `usbhotplug`, `wanup`, `usbumount` and
`autorun` are the other hooks):

```sh
# /jffs/etc/config/noobscenic.usbmount   (chmod +x)
#!/bin/sh
cd /tmp/mnt/DATA/noobscenic || exit 1
./noobscenic --data-dir ./var >>run.log 2>&1 &
```

`cd` matters: `config.json` is read from the working directory, and only
`--data-dir` has a CLI override. The server binds `0.0.0.0:8080` (web UI + channel
A) and `0.0.0.0:8081` (channel B); Tomato's own UI is on 80/443, so neither
conflicts, but check with `netstat -lnt`. The operator console and the one-shot
CLI verbs work over telnet too.

## Still to do on the device

- [ ] `uname -a` shows the expected 2.6.36.x `brcmarm` kernel, and
      `dmesg | grep -i vfp` prints nothing (the kernel has no VFP support)
- [ ] the binary starts from the stick and stays up (no SIGILL, no missing
      syscall); `var/logs/noobscenic.log` grows
- [ ] after re-homing, the log shows `channel-B handshake; device online` and
      `devices` lists the robot
- [ ] the web UI loads at `http://<router>:8080/` and the map updates
- [ ] memory headroom (`free`) and USB write behaviour over a full clean cycle
- [ ] optionally `opkg install strace` from Entware's `armv7sf-k2.6` feed if
      something needs tracing

## Known unknowns

* **2.6.36 is older than what musl is normally exercised on.** The one hazard
  that matters — musl 1.2's 64-bit-time syscalls, which do not exist before 5.1 —
  has an explicit fallback that the audit found in the binary. A first boot on
  the router is still the proof.
* **32-bit musl ≤ 1.2.6 and CVE-2026-40200** (stack corruption in `qsort`): Rust
  1.98 ships musl 1.2.5. This server does not use the FTS3 `qsort` path, and the
  threat model is a trusted LAN, but revisit when Rust bumps musl.
* **The six libunwind VFP stubs** above are the only VFP/NEON instructions in the
  binary. Removing them entirely would mean building `std` with a VFP-free
  libunwind (nightly `-Z build-std` with `panic_immediate_abort`, or patched
  libunwind) — not worth it unless the device test says otherwise.
