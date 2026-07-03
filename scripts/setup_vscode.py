#!/usr/bin/env python3
"""
Read the active Kconfig .config and generate .vscode/settings.json
for rust-analyzer, so the IDE gets the same env vars and cargo flags
as the real build.
"""

import json
import os
import re
import shlex
import sys
from pathlib import Path


def get_project_root() -> Path:
    """Return the project root (where the top-level Makefile lives)."""
    script_dir = Path(__file__).resolve().parent
    return script_dir.parent


def get_config_path(project_root: Path) -> Path:
    """Return the active Kconfig output path."""
    config = Path(os.environ.get("KCONFIG_CONFIG", "config/.config"))
    if not config.is_absolute():
        config = project_root / config
    return config


def decode_config_value(raw: str) -> str:
    """Decode Kconfig assignment values such as y, 64, or "quoted text"."""
    try:
        values = shlex.split(raw)
    except ValueError as err:
        print(f"Invalid Kconfig value {raw!r}: {err}", file=sys.stderr)
        sys.exit(1)

    if not values:
        return ""
    return values[0]


def read_config(config_path: Path) -> dict[str, str]:
    """Read a generated Kconfig .config file."""
    if not config_path.exists():
        print(f"Config file not found: {config_path}", file=sys.stderr)
        print("Run `make defconfig` or set KCONFIG_CONFIG to an existing file.", file=sys.stderr)
        sys.exit(1)

    config = {}
    set_pattern = re.compile(r"^(CONFIG_[A-Za-z0-9_]+)=(.*)$")
    unset_pattern = re.compile(r"^# (CONFIG_[A-Za-z0-9_]+) is not set$")

    with open(config_path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue

            match = set_pattern.match(line)
            if match:
                config[match.group(1)] = decode_config_value(match.group(2))
                continue

            match = unset_pattern.match(line)
            if match:
                config[match.group(1)] = ""

    return config


def config_value(config: dict[str, str], name: str, default: str = "") -> str:
    """Get CONFIG_<name> from parsed Kconfig values."""
    return config.get(f"CONFIG_{name}", default)


def config_enabled(config: dict[str, str], name: str) -> bool:
    """Return whether CONFIG_<name> is enabled."""
    return config_value(config, name) == "y"


def read_arch_rustflags(project_root: Path, arch: str, arch_bits: str) -> list[str]:
    """Read simple RUSTFLAGS appends from scripts/<arch><bits>.mk if present."""
    arch_config = project_root / "scripts" / f"{arch}{arch_bits}.mk"
    if not arch_config.exists():
        return []

    flags = []
    append_pattern = re.compile(r"^RUSTFLAGS\s*\+=\s*(.*)$")
    with open(arch_config) as f:
        for line in f:
            match = append_pattern.match(line.strip())
            if match:
                flags.extend(shlex.split(match.group(1)))

    return flags


def build_env_vars(project_root: Path, config: dict[str, str]) -> dict[str, str]:
    """Build the BUILD_ENV values used by build.mk."""
    arch = config_value(config, "ARCH")
    arch_bits = config_value(config, "ARCH_BITS")

    rustflags = []
    if config_enabled(config, "BACKTRACE"):
        rustflags.extend(["-C", "force-frame-pointers=yes"])
    rustflags.extend(read_arch_rustflags(project_root, arch, arch_bits))

    return {
        "ARCH": arch,
        "ARCH_BITS": arch_bits,
        "CROSS_COMPILE": config_value(config, "CROSS_COMPILE"),
        "AR": config_value(config, "AR", "ar"),
        "RUST_TARGET": config_value(config, "RUST_TARGET", "riscv64gc-unknown-none-elf"),
        "KERNELX_INITPATH": config_value(config, "INITPATH"),
        "KERNELX_INITCWD": config_value(config, "INITCWD"),
        "KERNELX_RELEASE": config_value(config, "KERNELX_RELEASE"),
        "KERNELX_HOME": str(project_root),
        "CONFIG_DEFAULT_BOOT_DEVICE": config_value(config, "DEFAULT_BOOT_ROOT_DEVICE"),
        "CONFIG_SECOND_DEVICE": config_value(config, "SECOND_DEVICE"),
        "CONFIG_SECOND_FSTYPE": config_value(config, "SECOND_FSTYPE") or "ext4",
        "CONFIG_SECOND_MOUNTPOINT": config_value(config, "SECOND_MOUNTPOINT") or "/mnt",
        "CONFIG_DEFAULT_INITPATH": config_value(config, "DEFAULT_INITPATH"),
        "CONFIG_DEFAULT_BOOTARGS": config_value(config, "DEFAULT_BOOTARGS"),
        "SYSROOT": config_value(config, "SYSROOT"),
        "COMPILE_MODE": config_value(config, "COMPILE_MODE", "debug"),
        "RUSTFLAGS": " ".join(rustflags),
    }


def build_cargo_info(config: dict[str, str]) -> dict:
    """Build the cargo settings mirrored from build.mk."""
    log_features = {
        "trace": "log-trace",
        "debug": "log-debug",
        "info": "log-info",
        "warn": "log-warn",
    }
    features = []

    log_level = config_value(config, "LOG_LEVEL")
    if not log_level:
        features.append("log-info")
    elif log_level in log_features:
        features.append(log_features[log_level])
    else:
        print(
            f"Warning: invalid LOG_LEVEL: {log_level}. Valid values: trace, debug, info, warn",
            file=sys.stderr,
        )

    feature_configs = [
        ("LOG_SYSCALL", "log-trace-syscall"),
        ("LOG_SYSCALL_CPU_TIME", "log-syscall-cpu-time"),
        ("ENABLE_SWAP_MEMORY", "swap-memory"),
        ("KVM", "kvm"),
        ("WARN_UNIMPLEMENTED_SYSCALL", "warn-unimplemented-syscall"),
        ("NO_SMP", "no-smp"),
        ("LOCKDEP", "lockdep"),
        ("SPINLOCK_CHECK", "spinlock-check"),
        ("ENABLE_WATCHDOG", "watchdog"),
        ("FANOTIFY", "fanotify"),
        ("VIRTIO_BLOCK_PAGE_CACHE", "virtio-block-page-cache"),
        ("NOLOCK", "nolock"),
        ("BACKTRACE", "backtrace"),
    ]
    for config_name, feature in feature_configs:
        if config_enabled(config, config_name):
            features.append(feature)

    return {
        "target": config_value(config, "RUST_TARGET", "riscv64gc-unknown-none-elf"),
        "features": features,
        "extra_args": ["--no-default-features"],
    }


def build_settings(env_vars: dict[str, str], cargo_info: dict) -> dict:
    """Build the .vscode/settings.json content."""
    extra_env = {}
    for key, value in env_vars.items():
        extra_env[key] = value

    # Use ${workspaceFolder} for portability across machines
    if "KERNELX_HOME" in extra_env:
        extra_env["KERNELX_HOME"] = "${workspaceFolder}"

    settings = {}

    # rust-analyzer settings
    settings["rust-analyzer.server.extraEnv"] = extra_env

    if cargo_info["target"]:
        settings["rust-analyzer.cargo.target"] = cargo_info["target"]
        settings["rust-analyzer.check.extraArgs"] = [
            "--target",
            cargo_info["target"],
        ]

    if cargo_info["features"]:
        settings["rust-analyzer.cargo.features"] = cargo_info["features"]

    if "--no-default-features" in cargo_info["extra_args"]:
        settings["rust-analyzer.cargo.noDefaultFeatures"] = True

    # no_std targets don't have a `test` crate
    settings["rust-analyzer.check.allTargets"] = False

    return settings


def merge_settings(existing: dict, new_settings: dict) -> dict:
    """Merge new rust-analyzer settings into existing settings, preserving other keys."""
    result = {}

    # Copy non-rust-analyzer keys from existing
    for key, value in existing.items():
        if not key.startswith("rust-analyzer."):
            result[key] = value

    # Add all new rust-analyzer settings
    result.update(new_settings)

    return result


def main():
    project_root = get_project_root()
    vscode_dir = project_root / ".vscode"
    settings_path = vscode_dir / "settings.json"
    config_path = get_config_path(project_root)

    print(f"Project root: {project_root}")
    print(f"Reading config: {config_path}")

    config = read_config(config_path)
    env_vars = build_env_vars(project_root, config)
    cargo_info = build_cargo_info(config)

    print("Environment variables:")
    for key, value in env_vars.items():
        print(f"  {key}={value}")

    print(f"\nTarget: {cargo_info['target']}")
    print(f"Features: {cargo_info['features']}")

    new_settings = build_settings(env_vars, cargo_info)

    # Merge with existing settings if present
    existing = {}
    if settings_path.exists():
        try:
            with open(settings_path) as f:
                existing = json.load(f)
            print(f"\nMerging with existing {settings_path}")
        except json.JSONDecodeError:
            print(f"\nWarning: existing {settings_path} is invalid JSON, overwriting")

    final_settings = merge_settings(existing, new_settings)

    # Write
    vscode_dir.mkdir(parents=True, exist_ok=True)
    with open(settings_path, "w") as f:
        json.dump(final_settings, f, indent=4, ensure_ascii=False)
        f.write("\n")

    print(f"\nGenerated {settings_path}")


if __name__ == "__main__":
    main()
