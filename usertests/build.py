#!/usr/bin/env python3

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


BASE_DIR = Path(__file__).resolve().parent
TEST_ROOT_DIR = "tests"
INIT_RUNNER_SOURCE = BASE_DIR / "support" / "autorun-init.c"
INIT_RUNNER_LIST = Path("etc/kx-tests.list")

DEFAULT_TESTS = [
    # "basic-ulib",
    # "basic-musl",
    # "basic-glibc",
    # "filesystem",
    # "basic-glibc-static",
    # "os-func",
    # "pthread-musl",
    # "stdio",
    "pthread-musl",
    "kmodule",
]

COMMON_GLIBC_RUNTIME_FILES = [
    "libc.so.6",
    "libm.so.6",
    "libpthread.so.0",
    "libdl.so.2",
    "librt.so.1",
    "libgcc_s.so.1",
]

ARCH_CONFIGS = {
    "riscv64": {
        "aliases": ("riscv", "rv64"),
        "gnu_prefix": "riscv64-unknown-linux-gnu-",
        "elf_prefix": "riscv64-unknown-elf-",
        "musl_prefix": "riscv64-linux-musl-",
        "musl_libc": "/opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libc.so",
        "musl_loader": "ld-musl-riscv64.so.1",
        "glibc_cc": "riscv64-unknown-linux-gnu-gcc",
        "glibc_loader": "ld-linux-riscv64-lp64d.so.1",
        "glibc_loader_path": "lib/ld-linux-riscv64-lp64d.so.1",
        "glibc_search_dirs": (
            "/usr/riscv64-linux-gnu/lib",
            "/lib/riscv64-linux-gnu",
            "/usr/lib/riscv64-linux-gnu",
        ),
        "cargo_target": "riscv64gc-unknown-linux-gnu",
        "cargo_linker": "riscv64-unknown-linux-gnu-gcc",
        "sysroot": "/usr/riscv64-linux-gnu",
        "unsupported_tests": (),
    },
    "loongarch64": {
        "aliases": ("loongarch", "la64"),
        "gnu_prefix": "loongarch64-linux-gnu-",
        "elf_prefix": "loongarch64-unknown-elf-",
        "musl_prefix": "loongarch64-linux-musl-",
        "musl_libc": "/opt/loongarch64-linux-musl-cross/loongarch64-linux-musl/lib/libc.so",
        "musl_loader": "ld-musl-loongarch64-lp64d.so.1",
        "glibc_cc": "loongarch64-linux-gnu-gcc",
        "glibc_loader": "ld-linux-loongarch-lp64d.so.1",
        "glibc_loader_path": "lib64/ld-linux-loongarch-lp64d.so.1",
        "glibc_search_dirs": (
            "/usr/loongarch64-linux-gnu/lib",
            "/lib/loongarch64-linux-gnu",
            "/usr/lib/loongarch64-linux-gnu",
            "/lib64",
            "/usr/lib64",
        ),
        "cargo_target": "loongarch64-unknown-linux-gnu",
        "cargo_linker": "loongarch64-linux-gnu-gcc",
        "sysroot": "/usr/loongarch64-linux-gnu",
        "unsupported_tests": ("basic-ulib", "helloworld"),
    },
}

MUSL_TESTS = {"basic-musl", "pthread-musl"}


class Color:
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    NC = "\033[0m"


def log_info(message):
    print(f"{Color.GREEN}[INFO]{Color.NC} {message}")


def log_warn(message):
    print(f"{Color.YELLOW}[WARN]{Color.NC} {message}")


def log_error(message):
    print(f"{Color.RED}[ERROR]{Color.NC} {message}")


def run(command, **kwargs):
    return subprocess.run(command, check=True, **kwargs)


def normalize_isa(isa):
    for name, config in ARCH_CONFIGS.items():
        if isa == name or isa in config["aliases"]:
            return name
    supported = ", ".join(sorted(ARCH_CONFIGS))
    raise ValueError(f"Unsupported ISA {isa}, supported: {supported}")


def available_tests():
    tests = []
    for test_dir in sorted(BASE_DIR.iterdir()):
        if not test_dir.is_dir() or test_dir.name == "build":
            continue
        if (test_dir / "Makefile").is_file() or (test_dir / "makefile").is_file():
            tests.append(test_dir.name)
    return tests


def split_test_args(args):
    tests = []
    for arg in args:
        tests.extend(test for test in (part.strip() for part in arg.split(",")) if test)
    return tests


def dedup(items):
    result = []
    seen = set()
    for item in items:
        if item in seen:
            continue
        seen.add(item)
        result.append(item)
    return result


def dedup_paths(paths):
    result = []
    seen = set()
    for path in paths:
        path = Path(path)
        key = str(path)
        if key in seen:
            continue
        seen.add(key)
        result.append(path)
    return result


def parse_args(argv):
    parser = argparse.ArgumentParser(description="Build selected usertests into an ext4 image")
    parser.add_argument("isa", nargs="?", default="riscv64", help="target ISA, e.g. riscv64 or loongarch64")
    parser.add_argument("tests", nargs="*", help="test suites to import; use 'all' for every suite with a Makefile")
    parser.add_argument(
        "--tests",
        dest="tests_option",
        action="append",
        default=[],
        metavar="LIST",
        help="comma-separated test suites to import",
    )
    parser.add_argument("--list-tests", action="store_true", help="list available test suites and exit")
    autorun_group = parser.add_mutually_exclusive_group()
    autorun_group.add_argument("--autorun", dest="autorun", action="store_true", help="generate autorun /init")
    autorun_group.add_argument("--no-autorun", dest="autorun", action="store_false", help="do not generate autorun /init")
    parser.set_defaults(autorun=True)
    return parser.parse_args(argv[1:])


def select_tests(positional_tests, option_tests):
    selected = split_test_args(option_tests) + split_test_args(positional_tests)
    if not selected:
        return list(DEFAULT_TESTS)

    if selected == ["all"]:
        return available_tests()
    if "all" in selected:
        raise ValueError("'all' cannot be combined with other test suites")

    available = set(available_tests())
    unknown = [test for test in selected if test not in available]
    if unknown:
        known = ", ".join(available_tests())
        raise ValueError(f"Unknown test suite(s): {', '.join(unknown)}. Available: {known}")

    return dedup(selected)


def env_or_config(env_name, config, config_name):
    return os.environ.get(env_name, config[config_name])


def env_path(env_name):
    value = os.environ.get(env_name)
    if value:
        return Path(value)
    return None


def libc_dirs_from_env(*env_names):
    dirs = []
    for env_name in env_names:
        path = env_path(env_name)
        if path:
            dirs.append(path)
    return dirs


def find_file_in_dirs(filename, dirs):
    for directory in dirs:
        path = directory / filename
        if path.is_file():
            return path
    return None


def compiler_file(gcc, filename):
    if not shutil.which(gcc):
        return None

    result = subprocess.run(
        [gcc, f"-print-file-name={filename}"],
        check=False,
        capture_output=True,
        text=True,
    )
    path = Path(result.stdout.strip())
    if path.is_file():
        return path
    return None


def compiler_output(gcc, *args):
    if not shutil.which(gcc):
        return None

    result = subprocess.run(
        [gcc, *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None

    output = result.stdout.strip()
    return output or None


def glibc_compiler(config):
    if "GLIBC_CC" in os.environ:
        return os.environ["GLIBC_CC"]

    cross_compile = os.environ.get("CROSS_COMPILE")
    if cross_compile:
        return f"{cross_compile}gcc"

    return config["glibc_cc"]


def compiler_sysroot(gcc):
    sysroot = compiler_output(gcc, "--print-sysroot")
    if sysroot is None:
        return None

    path = Path(sysroot)
    if path == Path("/"):
        return None
    return path


def compiler_multiarch(gcc):
    return compiler_output(gcc, "-print-multiarch")


def glibc_sysroot_dirs(gcc, config):
    sysroot = env_path("SYSROOT") or compiler_sysroot(gcc)
    if sysroot is None:
        return []

    loader_dir = Path(config["glibc_loader_path"]).parent
    dirs = [
        sysroot / loader_dir,
        sysroot / "lib",
        sysroot / "lib64",
        sysroot / "usr" / "lib",
        sysroot / "usr" / "lib64",
    ]

    multiarch = compiler_multiarch(gcc)
    if multiarch:
        dirs.extend(
            [
                sysroot / "lib" / multiarch,
                sysroot / "usr" / "lib" / multiarch,
            ]
        )

    return dedup_paths(dirs)


def test_command_path(suite, test):
    return f"/{TEST_ROOT_DIR}/{suite}/{test}"


def commands_from_run_list(suite):
    run_list = Path(suite) / "run.list"
    if not run_list.is_file():
        return None

    commands = []
    for raw_line in run_list.read_text().splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("/"):
            commands.append(line)
        else:
            commands.append(f"/{TEST_ROOT_DIR}/{suite}/{line}")

    return commands


def commands_from_outputs(suite, output_dir):
    commands = []
    entries = sorted(entry for entry in output_dir.iterdir() if entry.is_dir())
    for entry in entries:
        candidate = entry / entry.name
        if candidate.is_file():
            commands.append(test_command_path(suite, entry.name))

    if commands:
        return commands

    for entry in sorted(output_dir.iterdir()):
        if entry.is_file() and os.access(entry, os.X_OK):
            commands.append(test_command_path(suite, entry.name))

    return commands


def copy_test_outputs(output_dir, target_dir, test):
    copied = 0

    for entry in sorted(output_dir.iterdir()):
        if entry.is_dir():
            for item in sorted(entry.iterdir()):
                if item.is_file():
                    shutil.copy2(item, target_dir / item.name)
                    copied += 1

    if copied == 0:
        for entry in sorted(output_dir.iterdir()):
            if entry.is_file():
                shutil.copy2(entry, target_dir / entry.name)
                copied += 1

    if copied > 0:
        log_info(f"Files copied successfully for {test} ({copied} files)")
        return

    log_warn(f"No files found in {output_dir} for {test}")


def copy_runtime_file(src, dest, desc):
    if src.is_file():
        log_info(f"Copying {desc} to build directory...")
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dest)
    else:
        log_warn(f"{desc} not found at {src}, skipping copy")


def find_glibc_runtime_file(filename, config):
    gcc = glibc_compiler(config)
    search_dirs = libc_dirs_from_env("GLIBC_LIB_DIR", "LIBC_DIR")

    glibc_libc = env_path("GLIBC_LIBC")
    if glibc_libc:
        if filename == glibc_libc.name:
            return glibc_libc
        search_dirs.append(glibc_libc.parent)

    search_dirs.extend(glibc_sysroot_dirs(gcc, config))
    search_dirs.extend(Path(directory) for directory in config["glibc_search_dirs"])
    search_dirs = dedup_paths(search_dirs)

    return find_file_in_dirs(filename, search_dirs) or compiler_file(gcc, filename)


def find_musl_libc(config):
    musl_libc = env_path("MUSL_LIBC")
    if musl_libc:
        return musl_libc

    libc = find_file_in_dirs("libc.so", libc_dirs_from_env("MUSL_LIB_DIR", "LIBC_DIR"))
    if libc:
        return libc

    gcc = os.environ.get("MUSL_CC")
    if gcc:
        libc = compiler_file(gcc, "libc.so")
        if libc:
            return libc

    return Path(config["musl_libc"])


def copy_musl_runtime(rootfs_dir, config):
    copy_runtime_file(find_musl_libc(config), rootfs_dir / "lib" / config["musl_loader"], "musl libc")


def copy_glibc_runtime(rootfs_dir, config):
    loader = config["glibc_loader"]
    src = find_glibc_runtime_file(loader, config)
    if src:
        copy_runtime_file(src, rootfs_dir / config["glibc_loader_path"], f"glibc runtime {loader}")
    else:
        log_warn(f"glibc runtime {loader} not found, skipping copy")

    for filename in COMMON_GLIBC_RUNTIME_FILES:
        src = find_glibc_runtime_file(filename, config)
        if src:
            copy_runtime_file(src, rootfs_dir / "lib" / filename, f"glibc runtime {filename}")
        else:
            log_warn(f"glibc runtime {filename} not found, skipping copy")


def make_vars_for_test(test, isa, config):
    make_vars = {
        "ISA": isa,
        "ARCH": isa,
    }

    if test in MUSL_TESTS:
        make_vars["CROSS_COMPILE"] = env_or_config("MUSL_CROSS_COMPILE", config, "musl_prefix")
        if "MUSL_CC" in os.environ:
            make_vars["CC"] = os.environ["MUSL_CC"]
    elif test == "helloworld":
        make_vars["CROSS_COMPILER"] = env_or_config("ELF_CROSS_COMPILER", config, "elf_prefix")
    else:
        make_vars["CROSS_COMPILE"] = env_or_config("CROSS_COMPILE", config, "gnu_prefix")
        if "GLIBC_CC" in os.environ:
            make_vars["CC"] = os.environ["GLIBC_CC"]

    if test == "tokio":
        linker = os.environ.get("LINKER")
        if linker is None:
            linker = os.environ.get("GLIBC_CC", config["cargo_linker"])
        make_vars.update(
            {
                "CARGO_TARGET": env_or_config("CARGO_TARGET", config, "cargo_target"),
                "LINKER": linker,
                "SYSROOT": env_or_config("SYSROOT", config, "sysroot"),
            }
        )

    if test == "basic-glibc-static":
        gcc = glibc_compiler(config)
        glibc_libc = env_path("GLIBC_LIBC")
        glibc_dir = os.environ.get("GLIBC_DIR") or os.environ.get("GLIBC_LIB_DIR") or os.environ.get("LIBC_DIR")
        if not glibc_dir and glibc_libc:
            glibc_dir = str(glibc_libc.parent)
        if not glibc_dir:
            sysroot_dirs = glibc_sysroot_dirs(gcc, config)
            libc = find_file_in_dirs("libc.a", sysroot_dirs) or find_file_in_dirs("libc.so", sysroot_dirs)
            if libc:
                glibc_dir = str(libc.parent)
        libc = compiler_file(gcc, "libc.a") or compiler_file(gcc, "libc.so")
        if not glibc_dir and libc:
            glibc_dir = str(libc.parent)
        if glibc_dir:
            make_vars["GLIBC_DIR"] = glibc_dir

        crt_dirs = libc_dirs_from_env("GLIBC_CRT_DIR", "GLIBC_DIR", "GLIBC_LIB_DIR", "LIBC_DIR")
        if glibc_libc:
            crt_dirs.append(glibc_libc.parent)
        crt_dirs.extend(glibc_sysroot_dirs(gcc, config))
        crt_dirs = dedup_paths(crt_dirs)
        crt1 = find_file_in_dirs("crt1.o", crt_dirs) or compiler_file(gcc, "crt1.o")
        crti = find_file_in_dirs("crti.o", crt_dirs) or compiler_file(gcc, "crti.o")
        crtn = find_file_in_dirs("crtn.o", crt_dirs) or compiler_file(gcc, "crtn.o")
        if crt1 and crti and crtn:
            make_vars["CRT_FILES_BEGIN"] = f"{crt1} {crti}"
            make_vars["CRT_FILES_END"] = str(crtn)

    return make_vars


def build_test(test, isa, rootfs_dir, config):
    if test in config["unsupported_tests"]:
        log_warn(f"{test} does not support {isa}, skipping...")
        return []

    test_dir = Path(test)
    output_dir = test_dir / "build" / isa / "output"
    target_dir = rootfs_dir / TEST_ROOT_DIR / test

    target_dir.mkdir(parents=True, exist_ok=True)

    if not test_dir.is_dir():
        log_warn(f"Test directory {test_dir} does not exist, skipping...")
        return []

    log_info(f"Building {test} for {isa}...")
    shutil.rmtree(output_dir, ignore_errors=True)

    if (test_dir / "Makefile").is_file() or (test_dir / "makefile").is_file():
        log_info(f"Running 'make all' in {test} directory...")
        make_vars = make_vars_for_test(test, isa, config)
        make_args = ["make", "all", *[f"{key}={value}" for key, value in make_vars.items()]]
        result = subprocess.run(make_args, cwd=test_dir)
        if result.returncode != 0:
            log_error(f"Failed to build {test}")
            return []
        log_info(f"{test} build completed successfully")
    else:
        log_warn(f"No Makefile found in {test}, skipping build...")

    if not output_dir.is_dir():
        log_warn(f"Output directory {output_dir} does not exist for {test}")
        return []

    log_info(f"Collecting files from {output_dir}...")
    copy_test_outputs(output_dir, target_dir, test)
    return commands_from_run_list(test) or commands_from_outputs(test, output_dir)


def build_autorun_init(rootfs_dir, config, commands):
    list_file = rootfs_dir / INIT_RUNNER_LIST
    list_file.parent.mkdir(parents=True, exist_ok=True)
    list_file.write_text("".join(f"{command}\n" for command in commands))

    (rootfs_dir / "tmp").mkdir(parents=True, exist_ok=True)

    gcc = glibc_compiler(config)
    log_info(f"Building autorun init with {gcc}...")
    try:
        run([gcc, "-Wall", "-Wextra", "-O2", str(INIT_RUNNER_SOURCE), "-o", str(rootfs_dir / "init")])
    except subprocess.CalledProcessError:
        log_error("Failed to build autorun init")
        return False

    return True


def create_image(build_dir, rootfs_dir, isa, img_size):
    img_file = build_dir / f"{isa}.ext4"
    log_info(f"Creating ext4 image: {img_file}")

    img_file.unlink(missing_ok=True)
    mkfs = shutil.which("mke2fs") or shutil.which("mkfs.ext4")
    if mkfs is None:
        for path in (Path("/sbin/mke2fs"), Path("/sbin/mkfs.ext4")):
            if path.is_file():
                mkfs = str(path)
                break
    if mkfs is None:
        log_error("mke2fs or mkfs.ext4 not found")
        return None

    try:
        run(
            ["dd", "if=/dev/zero", f"of={img_file}", "bs=1M", f"count={img_size}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        run([mkfs, "-q", "-t", "ext4", "-b", "4096", "-F", "-d", str(rootfs_dir), str(img_file)])
    except subprocess.CalledProcessError:
        log_error("Failed to create ext4 image")
        return None

    return img_file


def human_size(path):
    size = path.stat().st_size
    units = ["B", "K", "M", "G", "T"]
    value = float(size)

    for unit in units:
        if value < 1024 or unit == units[-1]:
            return f"{value:.1f}{unit}" if unit != "B" else f"{size}B"
        value /= 1024

    return f"{size}B"


def print_contents(rootfs_dir, tests):
    print(f"Contents of {rootfs_dir}/:")
    tests_root = rootfs_dir / TEST_ROOT_DIR
    for test in tests:
        test_dir = tests_root / test
        if not test_dir.is_dir():
            continue

        print(f"  {TEST_ROOT_DIR}/{test}/:")
        result = subprocess.run(["ls", "-lai", str(test_dir)], capture_output=True, text=True)
        for line in result.stdout.splitlines():
            print(f"    {line}")


def main(argv):
    os.chdir(BASE_DIR)

    args = parse_args(argv)
    if args.list_tests:
        for test in available_tests():
            print(test)
        return 0

    try:
        isa = normalize_isa(args.isa)
    except ValueError as err:
        log_error(str(err))
        return 1
    try:
        tests = select_tests(args.tests, args.tests_option)
    except ValueError as err:
        log_error(str(err))
        return 1

    config = ARCH_CONFIGS[isa]
    img_size = os.environ.get("IMG_SIZE", "1024")
    build_dir = Path(os.environ.get("BUILD_DIR", "build"))
    rootfs_dir = build_dir / isa

    log_info("Build configuration:")
    print(f"  ISA: {isa}")
    print(f"  Image size: {img_size}MB")
    print(f"  Build directory: {build_dir}")
    print(f"  Tests: {' '.join(tests)}")
    print(f"  Autorun init: {'yes' if args.autorun else 'no'}")

    log_info("Creating directory structure...")
    shutil.rmtree(rootfs_dir, ignore_errors=True)
    rootfs_dir.mkdir(parents=True, exist_ok=True)

    test_commands = []
    for test in tests:
        test_commands.extend(build_test(test, isa, rootfs_dir, config))

    if args.autorun and not build_autorun_init(rootfs_dir, config, test_commands):
        return 1

    copy_musl_runtime(rootfs_dir, config)
    copy_glibc_runtime(rootfs_dir, config)

    img_file = create_image(build_dir, rootfs_dir, isa, img_size)
    if img_file is None:
        return 1

    log_info("Build completed successfully!")
    print(f"Generated image: {img_file}")
    print(f"Image size: {human_size(img_file)}")
    print()
    print_contents(rootfs_dir, tests)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
