#!/usr/bin/env python3
"""
Parse `make -n check` output and generate .vscode/settings.json
for rust-analyzer, so the IDE gets the same env vars and cargo flags
as the real build.
"""

import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path


def get_project_root() -> Path:
    """Return the project root (where the top-level Makefile lives)."""
    script_dir = Path(__file__).resolve().parent
    return script_dir.parent


def run_dry_make(project_root: Path) -> str:
    """Run `make -n check` twice to get the final cargo command line."""
    # First pass: get the inner make command
    result = subprocess.run(
        ["make", "-n", "check"],
        cwd=project_root,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"Error running 'make -n check':\n{result.stderr}", file=sys.stderr)
        sys.exit(1)

    first_output = result.stdout.strip()

    # The first pass outputs: make -f build.mk check KEY=VAL ...
    # We need to run that with -n to get the actual cargo command
    # Extract the inner make command and add -n
    if first_output.startswith("make"):
        # Insert -n after "make"
        inner_cmd = first_output.replace("make ", "make -n ", 1)
        result2 = subprocess.run(
            inner_cmd,
            cwd=project_root,
            capture_output=True,
            text=True,
            shell=True,
        )
        if result2.returncode != 0:
            print(f"Error running inner make:\n{result2.stderr}", file=sys.stderr)
            sys.exit(1)
        return result2.stdout.strip()

    # If it already contains "cargo", use directly
    return first_output


def parse_cargo_command(line: str) -> tuple[dict[str, str], dict]:
    """
    Parse a line like:
      ARCH=riscv ... RUSTFLAGS="..." cargo check --target xxx --features "a b c" --release
    Returns (env_vars, cargo_info).
    """
    env_vars = {}
    cargo_args = []

    # Split carefully: env vars come before "cargo"
    cargo_idx = line.find("cargo ")
    if cargo_idx == -1:
        print(f"Could not find 'cargo' in command:\n{line}", file=sys.stderr)
        sys.exit(1)

    env_part = line[:cargo_idx].strip()
    cargo_part = line[cargo_idx:].strip()

    # Parse env vars: KEY=VALUE pairs
    # Handle quoted values like RUSTFLAGS="-C force-frame-pointers=yes"
    env_pattern = re.compile(r'(\w+)=("(?:[^"\\]|\\.)*"|[^\s]*)')
    for match in env_pattern.finditer(env_part):
        key = match.group(1)
        value = match.group(2)
        # Strip surrounding quotes
        if value.startswith('"') and value.endswith('"'):
            value = value[1:-1]
        env_vars[key] = value

    # Parse cargo arguments
    cargo_tokens = shlex.split(cargo_part)

    cargo_info = {
        "target": None,
        "features": [],
        "extra_args": [],
    }

    i = 1  # skip "cargo" and subcommand will be at index 1
    while i < len(cargo_tokens):
        token = cargo_tokens[i]
        if token == "--target" and i + 1 < len(cargo_tokens):
            cargo_info["target"] = cargo_tokens[i + 1]
            i += 2
        elif token == "--features" and i + 1 < len(cargo_tokens):
            features_str = cargo_tokens[i + 1]
            cargo_info["features"] = features_str.split()
            i += 2
        elif token == "--no-default-features":
            cargo_info["extra_args"].append(token)
            i += 1
        elif token in ("check", "--release"):
            i += 1
        else:
            cargo_info["extra_args"].append(token)
            i += 1

    return env_vars, cargo_info


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

    print(f"Project root: {project_root}")
    print("Running make -n check ...")

    cargo_line = run_dry_make(project_root)
    print(f"Cargo command:\n  {cargo_line}\n")

    env_vars, cargo_info = parse_cargo_command(cargo_line)

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
