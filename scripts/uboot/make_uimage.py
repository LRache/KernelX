#!/usr/bin/env python3
"""Wrap a raw KernelX binary in a U-Boot legacy uImage header.

The default addresses match the VisionFive 2 U-Boot setup used for KernelX:
the payload is loaded and entered at 0x40200000.  U-Boot's RISC-V bootm path
then calls the payload as ``kernel(hartid, fdt)``.
"""

import argparse
import struct
import time
import zlib
from pathlib import Path


HEADER_FORMAT = "!7I4B32s"
HEADER_SIZE = struct.calcsize(HEADER_FORMAT)
UIMAGE_MAGIC = 0x27051956
UIMAGE_OS_LINUX = 5
UIMAGE_ARCH_RISCV = 26
UIMAGE_TYPE_KERNEL = 2
UIMAGE_COMP_NONE = 0
DEFAULT_ADDRESS = 0x40200000
DEFAULT_NAME = "KernelX raw RISC-V"


def parse_int(value):
    return int(value, 0)


def make_header(data, load_address, entry_address, name):
    name_bytes = name.encode("ascii")
    if len(name_bytes) > 32:
        raise ValueError("uImage name must be at most 32 ASCII bytes")

    fields = [
        UIMAGE_MAGIC,
        0,
        int(time.time()),
        len(data),
        load_address,
        entry_address,
        zlib.crc32(data) & 0xFFFFFFFF,
        UIMAGE_OS_LINUX,
        UIMAGE_ARCH_RISCV,
        UIMAGE_TYPE_KERNEL,
        UIMAGE_COMP_NONE,
        name_bytes.ljust(32, b"\0"),
    ]
    header = struct.pack(HEADER_FORMAT, *fields)
    fields[1] = zlib.crc32(header) & 0xFFFFFFFF
    return struct.pack(HEADER_FORMAT, *fields)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "-i",
        "--input",
        type=Path,
        default=Path("build/riscv64/Image"),
        help="raw KernelX binary (default: build/riscv64/Image)",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=Path("build/riscv64/kernelx.uImage"),
        help="output uImage (default: build/riscv64/kernelx.uImage)",
    )
    parser.add_argument(
        "--load-address",
        type=parse_int,
        default=DEFAULT_ADDRESS,
        help="payload load address (default: 0x40200000)",
    )
    parser.add_argument(
        "--entry-address",
        type=parse_int,
        default=None,
        help="payload entry address (default: load address)",
    )
    parser.add_argument(
        "--name",
        default=DEFAULT_NAME,
        help="uImage name stored in the header",
    )
    args = parser.parse_args()

    if not args.input.is_file():
        parser.error(f"input file does not exist: {args.input}")
    if args.input.resolve() == args.output.resolve():
        parser.error("input and output must be different files")

    data = args.input.read_bytes()
    if not data:
        parser.error(f"input file is empty: {args.input}")

    entry_address = args.load_address if args.entry_address is None else args.entry_address
    header = make_header(data, args.load_address, entry_address, args.name)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(header + data)

    print(
        f"wrote {args.output} ({HEADER_SIZE} byte header + "
        f"{len(data)} byte payload, total {HEADER_SIZE + len(data)} bytes)"
    )
    print(f"load/entry address: 0x{args.load_address:x}/0x{entry_address:x}")


if __name__ == "__main__":
    main()
