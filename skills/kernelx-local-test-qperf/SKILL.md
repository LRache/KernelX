---
name: kernelx-local-test-qperf
description: Use when working in the KernelX repository on local kernel configuration, common testsuit run configs, boot/QEMU parameters, qperf performance profiling, or large qperf report analysis through summary.md and profile.sqlite. Covers the repo-specific Kconfig to Make to Cargo feature pipeline, common run-test configs, the RISC-V qperf workflow, compact report generation, bounded hotspot queries, and separation of real kernel hotspots from unresolved symbols or sampling noise.
---

# KernelX Local Test And Qperf Workflow

Use this skill inside `/home/rache/code/KernelX` when the task involves local kernel configuration, running the common testsuit image setup, explaining boot/QEMU parameters, or using/analyzing qperf artifacts.

## Configuration Model

KernelX configuration flows through:

```text
config/Kconfig
  -> config/.config
  -> config/config.mk
  -> build.mk
  -> Cargo.toml features / Rust #[cfg(feature = "...")]
```

When adding or editing a `CONFIG_*` feature, check every relevant layer:

- `config/Kconfig` declares the option, default, dependency, and help text.
- `config/config.mk` passes generated `.config` values into the build.
- `build.mk` maps `CONFIG_*` to `RUST_FEATURES`, `RUSTFLAGS`, and build env.
- `Cargo.toml` declares the feature if Rust code uses `#[cfg(feature = "...")]`.
- Rust modules, stubs, and macro-generated types compile in both enabled and disabled states.
- `.github/workflows/ci.yml` needs explicit `CONFIG_*` flags only when CI should exercise that option.

For macro-generated feature surfaces such as `bitflags!`, gate the whole macro/module if the type must disappear when the feature is disabled.

## Common Config Commands

Use Kconfig helpers:

```bash
make defconfig
make menuconfig
make savedefconfig
make exportconfig
make importconfig
```

The common local run-test configs live in `config/riscv` and `config/loongarch`. Prefer importing them directly before running testsuit images:

```bash
make importconfig ARCH=riscv
make importconfig ARCH=loongarch
```

`make importconfig` rewrites `config/.config`. For the common run-test workflow, do not preserve the old `.config` unless the user explicitly cares about a temporary custom configuration. If they do care, save it manually first:

```bash
make exportconfig EXPORT_CONFIG=config/<name>
```

For one-off checks, prefer make variable overrides when possible so `config/.config` is not rewritten:

```bash
make check CONFIG_FANOTIFY=n
make check CONFIG_LOCKDEP=y CONFIG_SPINLOCK_CHECK=y
make run QEMU_ARGS='CONFIG_QEMU_SNAPSHOT=y CONFIG_QEMU_MEMORY=1G'
```

For QEMU-only config changes, inspect the generated command before launching QEMU:

```bash
make -f scripts/qemu.mk -n qemu-run CONFIG_QEMU_SNAPSHOT=y
```

## Important Config Families

Platform/toolchain:

- `CONFIG_RISCV64`, `CONFIG_LOONGARCH64`
- `CONFIG_ARCH`, `CONFIG_ARCH_BITS`
- `CONFIG_RUST_TARGET`
- `CONFIG_SYSROOT`
- `CONFIG_OBJCOPY`, `CONFIG_AR`, `CONFIG_READELF`

Build mode:

- `CONFIG_COMPILE_MODE_DEBUG`
- `CONFIG_COMPILE_MODE_RELEASE`
- `CONFIG_COMPILE_MODE`
- `CONFIG_NO_SMP`
- `CONFIG_NOLOCK`

Debug and observability:

- `CONFIG_LOG_LEVEL_*`
- `CONFIG_LOG_SYSCALL`
- `CONFIG_LOG_SYSCALL_CPU_TIME`
- `CONFIG_WARN_UNIMPLEMENTED_SYSCALL`
- `CONFIG_BACKTRACE`
- `CONFIG_DWARF`
- `CONFIG_LOCKDEP`
- `CONFIG_SPINLOCK_CHECK`
- `CONFIG_ENABLE_WATCHDOG`

Experimental/runtime features:

- `CONFIG_ENABLE_SWAP_MEMORY`
- `CONFIG_KVM`
- `CONFIG_FANOTIFY`
- `CONFIG_VIRTIO_BLOCK_PAGE_CACHE`

QEMU and boot:

- `CONFIG_QEMU_MACHINE`
- `CONFIG_QEMU_BIOS`
- `CONFIG_QEMU_MEMORY`
- `CONFIG_QEMU_CPUS`
- `CONFIG_QEMU_DEBUG_CONSOLE_DEVICE`
- `CONFIG_QEMU_DEBUG_CONSOLE_LOG`
- `CONFIG_DISK_IMAGE`
- `CONFIG_QEMU_SNAPSHOT`
- `CONFIG_SECOND_DISK_IMAGE`
- `CONFIG_SECOND_DEVICE`
- `CONFIG_SECOND_FSTYPE`
- `CONFIG_SECOND_MOUNTPOINT`
- `CONFIG_DEFAULT_BOOTARGS`
- `CONFIG_BOOTARGS`
- `CONFIG_INITPATH`
- `CONFIG_INITARGS`
- `CONFIG_INITCWD`
- `CONFIG_ROOT_DEVICE`
- `CONFIG_ROOT_FSTYPE`

## Boot Arguments

`scripts/qemu.mk` converts QEMU config into `-append` bootargs. Common parameters:

- `kdebug_console=` selects the kernel debug-console device, often `/dev/hvc0`.
- `root=` selects the root block device, such as `virtio_block0` or `virtio_mmio@10001000`.
- `rootfstype=` selects the root filesystem type, usually `ext4`.
- `init=` selects the init program, often `/init` or `/testcode/runtest.sh`.
- `initargs=` passes init arguments.
- `initcwd=` sets init's working directory.
- `tty=` selects the user terminal device.
- `rtc=` selects the RTC device.

Remember that `CONFIG_BOOTARGS` is only the extra raw bootarg string. `CONFIG_INITPATH`, `CONFIG_INITARGS`, `CONFIG_INITCWD`, `CONFIG_ROOT_DEVICE`, and `CONFIG_ROOT_FSTYPE` are appended separately by `scripts/qemu.mk`.

## Running The Common Testsuit Setup

For the common local testsuit image run, import the arch config and run:

```bash
make importconfig ARCH=riscv
make run
```

or:

```bash
make importconfig ARCH=loongarch
make run
```

The local `config/riscv` and `config/loongarch` configs are intended for repeated testsuit runs. They set release-oriented build options, QEMU memory, testsuit `sdcard-*.img`, `/testcode/runtest.sh`, and `CONFIG_QEMU_SNAPSHOT=y`.

`CONFIG_QEMU_SNAPSHOT=y` means guest writes go to temporary QEMU overlays and are discarded when QEMU exits. This is the preferred mode for repeated tests because it reduces the chance of dirtying shared base images.

## Qperf Workflow

LoongArch qperf is unstable in this repo. For qperf profiling, use the RISC-V config and RISC-V QEMU path unless the user explicitly asks to investigate LoongArch qperf itself:

```bash
make importconfig ARCH=riscv
make run-qperf
```

Use the top-level wrapper:

```bash
make run-qperf
```

It rebuilds the kernel with `CONFIG_BACKTRACE=y CONFIG_DWARF=y`, builds `tools/qperf`, runs QEMU with the TCG plugin, runs the analyzer, emits FlameGraph output, and runs `scripts/qperf_report.py` to build an LLM-oriented report and SQLite query index.

Default outputs:

- Raw samples: `build/<arch>64/qperf.bin`
- Folded stacks: `output/qperf/kernelx-qperf-<timestamp>.folded`
- FlameGraph SVG: `output/qperf/kernelx-qperf-<timestamp>.svg`
- Console log: `output/qperf/kernelx-qperf-<timestamp>.console.log`
- LLM summary: `output/qperf/kernelx-qperf-<timestamp>.report/summary.md`
- Query index: `output/qperf/kernelx-qperf-<timestamp>.report/profile.sqlite`
- Bounded TSV/folded views and the lossless aggregate: other files under the matching `.report/` directory

Useful overrides:

```bash
make run-qperf QEMU_ARGS='CONFIG_QEMU_SNAPSHOT=y'
make run-qperf QEMU_ARGS='QPERF_FREQ=101'
make run-qperf QEMU_ARGS='QPERF_FOLDED=output/qperf/my-run.folded QPERF_SVG=output/qperf/my-run.svg'
```

Read the active `QPERF_FREQ` from `scripts/qemu.mk` or the Make override. Keep it away from the 100Hz timer cadence when possible; `137` is a useful example. Treat a trap-heavy profile as suspicious even with a non-aligned frequency and verify it against raw samples before assigning blame.

Analyze qperf artifacts progressively:

1. Start with the exact timestamped `.report/summary.md`. Confirm its input SHA-256, total samples, unique stacks, top-stack coverage, CPU distribution, and unresolved-symbol percentage.
2. If the matching report does not exist, generate it without running QEMU again:

   ```bash
   python3 scripts/qperf_report.py build output/qperf/<name>.folded
   ```

3. Use bounded SQLite queries before reading large folded or TSV files:

   ```bash
   python3 scripts/qperf_report.py query output/qperf/<name>.report/profile.sqlite stats
   python3 scripts/qperf_report.py query output/qperf/<name>.report/profile.sqlite top --metric inclusive --limit 50
   python3 scripts/qperf_report.py query output/qperf/<name>.report/profile.sqlite top --metric self --limit 50
   python3 scripts/qperf_report.py query output/qperf/<name>.report/profile.sqlite stacks --contains <symbol> --limit 20
   python3 scripts/qperf_report.py query output/qperf/<name>.report/profile.sqlite callers --symbol <symbol> --limit 20
   python3 scripts/qperf_report.py query output/qperf/<name>.report/profile.sqlite callees --symbol <symbol> --limit 20
   ```

4. Treat `subsystems.tsv` as overlapping stack-presence classification. One sample may contribute to trap, memory, VFS, and ext4_native simultaneously, so subsystem percentages do not sum to 100%.
5. Read `top-stacks.folded` when the bounded queries need exact stack evidence. Read `full-aggregated.folded` only when the top view is insufficient.
6. If `summary.md` reports substantial `??` or `main;??` coverage, inspect raw `qperf.bin` and map top IPs with the matching ELF before assigning blame. The report database cannot recover instruction pointers already discarded by folded symbolization.
7. Cross-check selected hotspots against source paths, commonly ext4 metadata/checksum/allocation paths, syscall completion/timestamp paths, tmpfs/memtreefs paths, MM fault/TLB paths, or workload startup/lookup/stat/fsync behavior.

The SQLite database stores normalized symbol names, weighted unique stacks, ordered frames, sample-weighted caller/callee edges, CPU totals, subsystem totals, and profile metadata. It does not store raw IPs, sample chronology, source lines, or per-stack CPU identity. Do not load the database directly into model context; use the bounded query commands and consume their text output.

Do not treat disabling semantic filesystem features such as metadata checksums as the default fix. Use those changes as benchmark controls unless the user explicitly asks for such a tradeoff.

## Validation Rules

Respect the repository's local instruction: do not run tests or runtime commands unless the user explicitly asks. For code changes, still run the required static checks when allowed:

```bash
cargo fmt
make check
```

For feature-gate changes, validate both sides when applicable:

```bash
make check CONFIG_<SYMBOL>=y
make check CONFIG_<SYMBOL>=n
```

For lock/scheduler/watchdog-related config, expand the matrix only when relevant:

```bash
make check CONFIG_LOCKDEP=y
make check CONFIG_SPINLOCK_CHECK=y
make check CONFIG_LOCKDEP=y CONFIG_SPINLOCK_CHECK=y
make check CONFIG_NO_SMP=y CONFIG_LOCKDEP=y CONFIG_SPINLOCK_CHECK=y
```
