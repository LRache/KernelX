---
name: kernelx-configure-build-options
description: Configure and maintain KernelX compile-time kernel options. Use when selecting an architecture or build mode, editing Kconfig values, using an alternate KCONFIG_CONFIG, importing or exporting configuration presets, adding a CONFIG_* option, mapping it into Make and Cargo features, or checking that CI and Rust cfg gates follow the repository's build pipeline.
---

# Configure KernelX Build Options

Treat the current checkout as the source of truth. Inspect the relevant files before changing configuration because local `config/.config` state and available presets may differ between checkouts.

## Identify The Configuration Kind

Classify the requested value before editing anything:

- Put user-selectable kernel and build options in `config/Kconfig`.
- Map compile-time options through `config/config.mk` and `build.mk`.
- Declare matching Cargo features in `Cargo.toml` when Rust uses `#[cfg(feature = "...")]`.
- Treat QEMU launch and boot values as runtime configuration; trace those through `scripts/qemu.mk` instead of inventing a Cargo feature.
- Treat CI command-line overrides as CI policy, not as defaults for local development.

Use the active build chain:

```text
config/Kconfig
  -> KCONFIG_CONFIG (defaults to config/.config)
  -> config/config.mk
  -> build.mk
  -> Cargo.toml feature / Rust #[cfg(feature = "...")]
```

Do not infer the effective build from a preset alone. The resolved file selected by `KCONFIG_CONFIG` is the active configuration.

## Inspect Without Mutating

Read the active configuration and its consumers first:

```bash
rg -n '^(CONFIG_|# CONFIG_)' "${KCONFIG_CONFIG:-config/.config}"
rg -n 'CONFIG_<SYMBOL>|<cargo-feature>' config/Kconfig config/config.mk build.mk Cargo.toml src .github/workflows/ci.yml
```

When only explaining or reviewing configuration, do not run `menuconfig`, `defconfig`, or `importconfig`; they write the file selected by `KCONFIG_CONFIG`.

## Modify An Existing Configuration

Use the interactive editor for persistent local changes:

```bash
make menuconfig
```

Use a separate resolved configuration when the current `config/.config` must remain untouched:

```bash
KCONFIG_CONFIG=config/.config.debug make menuconfig
KCONFIG_CONFIG=config/.config.debug make check
```

Use a command-line override for a one-off build check when the option is already forwarded by `config/config.mk`:

```bash
make check CONFIG_FANOTIFY=n
make check CONFIG_LOCKDEP=y CONFIG_SPINLOCK_CHECK=y
```

Do not switch architecture with only `ARCH=<arch>` while retaining a resolved configuration for another architecture. Architecture selection also controls derived values such as `CONFIG_ARCH_BITS`, `CONFIG_RUST_TARGET`, toolchain settings, and conditional Kconfig dependencies; generate or import a consistent configuration instead.

## Import And Export Presets

Check that a preset exists before using it. This repository ignores most generated and local files under `config/`, so do not assume a host-specific preset is tracked.

Import a preset into the active resolved configuration:

```bash
make importconfig IMPORT_CONFIG=config/<preset>
```

Import into a separate resolved file:

```bash
KCONFIG_CONFIG=config/.config.<name> make importconfig IMPORT_CONFIG=config/<preset>
```

Export the active configuration as a minimal defconfig:

```bash
make exportconfig EXPORT_CONFIG=config/<preset>
```

Use `make defconfig` or `make savedefconfig` only after confirming the selected `DEFCONFIG` exists or is the intended output:

```bash
make defconfig DEFCONFIG=config/<preset>
make savedefconfig DEFCONFIG=config/<preset>
```

Remember that `importconfig`, `defconfig`, and `menuconfig` rewrite `KCONFIG_CONFIG`. Preserve the old file first only when the user asks to keep a custom configuration.

## Add A Compile-Time Option

Wire a new option through every layer that consumes it:

1. Add the symbol to the nearest relevant menu in `config/Kconfig`. Choose `bool`, `string`, `int`, or `choice` deliberately, and state meaningful defaults, dependencies, and help text.
2. Add `CONFIG_<SYMBOL>` to `KERNEL_CONFIG` in `config/config.mk` when `build.mk` must receive it.
3. In `build.mk`, map enabled booleans to `RUST_FEATURES`, or map non-feature values to the appropriate build environment or flags.
4. Add the kebab-case feature to `[features]` in `Cargo.toml` when Rust code uses a Cargo feature gate.
5. Gate the Rust implementation with `#[cfg(feature = "<feature>")]` and ensure both enabled and disabled builds have a coherent API. Gate an entire macro invocation when a macro-generated type should not exist in the disabled build.
6. Add explicit `CONFIG_<SYMBOL>=...` values to `.github/workflows/ci.yml` only when CI should exercise or pin the option.

Prefer the smallest existing integration pattern near a similar option. Do not add a Cargo feature for a value used only by Make, C code, QEMU, or boot arguments.

## Keep Configuration Portable

- Never place host-specific absolute test-image paths in the skill, Kconfig defaults, committed presets, examples, or suggested patches.
- Use repository-relative paths such as `images/test.img` only when the artifact is repository-managed; otherwise use placeholders such as `<path-to-image>`.
- Avoid committing local toolchain and sysroot paths. Prefer portable tool names, environment overrides, or documented placeholders unless the repository intentionally owns the path.
- Distinguish host file paths from guest paths such as `/init` or `/mnt`; guest paths are kernel boot configuration, not locations of host test images.

## Validate Changes

For changes to Rust or build wiring, follow the repository requirements:

```bash
cargo fmt
make check
```

Check both sides of a new boolean feature when applicable:

```bash
make check CONFIG_<SYMBOL>=y
make check CONFIG_<SYMBOL>=n
```

Expand the matrix only when dependencies require it, such as architecture, SMP, or lock-check combinations. Do not run runtime tests, QEMU, or testsuit commands unless the user explicitly requests them.

For skill-only Markdown or metadata changes, run the skill validator instead of rebuilding the kernel.
