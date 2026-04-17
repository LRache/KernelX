#!/usr/bin/env python3
"""
从 ELF 文件提取文本段符号，生成紧凑二进制符号表供内核运行时查找。

二进制格式：
  [u32 LE: count]
  [count × { u64 LE: addr, u32 LE: name_offset }]
  [string pool: null-terminated UTF-8]

用法：
  python3 scripts/gen_symbols.py <elf> <output.bin>
"""

import os
import struct
import subprocess

TEXT_SYMBOL_TYPES = {"T", "t"}


def should_keep_symbol(sym_type: str, sym_name: str) -> bool:
    if sym_type not in TEXT_SYMBOL_TYPES:
        return False
    if not sym_name:
        return False
    # Keep function-local text symbols emitted by Rust, but still drop
    # assembler/compiler helper labels such as .L0, .Lpcrel_hi*, $x.
    if sym_name.startswith(".") or sym_name.startswith("$"):
        return False
    return True


def run_nm(elf_path: str) -> list[tuple[int, str]]:
    nm = "nm"
    result = subprocess.run(
        [nm, "--demangle", "-n", "--defined-only", elf_path],
        capture_output=True, text=True, check=True,
    )
    symbols: list[tuple[int, str]] = []
    for line in result.stdout.splitlines():
        parts = line.split(None, 2)
        if len(parts) < 3:
            continue
        addr_str, sym_type, name = parts
        sym_name = name.strip()
        if not should_keep_symbol(sym_type, sym_name):
            continue
        try:
            addr = int(addr_str, 16)
        except ValueError:
            continue
        symbols.append((addr, sym_name))
    # nm -n already sorts by address, but be safe
    symbols.sort(key=lambda x: x[0])
    return symbols


def build_binary(symbols: list[tuple[int, str]]) -> bytes:
    count = len(symbols)
    string_pool = bytearray()
    name_offsets: list[int] = []
    for _, name in symbols:
        name_offsets.append(len(string_pool))
        string_pool.extend(name.encode("utf-8"))
        string_pool.append(0)

    header = struct.pack("<I", count)
    entries = bytearray()
    for i, (addr, _) in enumerate(symbols):
        entries += struct.pack("<QI", addr, name_offsets[i])

    return header + bytes(entries) + bytes(string_pool)


def main() -> None:
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("elf", help="Input ELF file")
    parser.add_argument("output", help="Output binary file")
    args = parser.parse_args()

    symbols = run_nm(args.elf)
    data = build_binary(symbols)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "wb") as f:
        f.write(data)

    string_bytes = len(data) - 4 - len(symbols) * 12
    print(f"gen_symbols: {len(symbols)} symbols, "
          f"{string_bytes} B string pool, "
          f"{len(data)} B total → {args.output}")


if __name__ == "__main__":
    main()
