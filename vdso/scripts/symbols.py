#!/usr/bin/env python3
import subprocess
import re
import argparse
import sys


def find_symbol_address_streaming(elf_file, symbols_to_found):
    """
    使用流式处理的方式，在找到符号后立即终止 readelf 进程。

    :param elf_file: ELF 文件的路径
    :param symbol_name: 要查找的符号名称
    """
    command = ['readelf', '-sW', elf_file]
    process = None
    
    to_found = len(symbols_to_found)
    found = 0

    try:
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1 
        )

        line_regex = re.compile(r"^\s*\d+:\s+([0-9a-fA-F]{8,16})\s+(\d+)\s+(\w+)\s+\w+\s+\w+\s+\S+\s+(.+)$")

        for line in process.stdout:
            match = line_regex.match(line.strip())
            if not match:
                continue

            address = match.group(1)
            found_symbol_name = match.group(4).strip()
            clean_symbol_name = found_symbol_name.split('@@')[0]

            if clean_symbol_name in symbols_to_found and int(address, 16) != 0:
                symbols_to_found[clean_symbol_name] = address
                found += 1

                if found == to_found:
                    process.terminate()
                    break

        
        _, stderr_output = process.communicate()
        
        if not found:
            if process.returncode != 0 and stderr_output:
                print("\nreadelf 报告了错误:", file=sys.stderr)
                print(stderr_output, file=sys.stderr)


    except FileNotFoundError:
        print(f"错误: 'readelf' 命令未找到。请确保它已安装并且在你的 PATH 中。", file=sys.stderr)
    except Exception as e:
        print(f"发生未知错误: {e}", file=sys.stderr)
    finally:
        if process and process.poll() is None:
            process.kill()


def write_result_to_file(output_file, symbols):
    with open(output_file, 'w') as f:
        for symbol, address in symbols.items():
            if address:
                f.write(f"pub const {symbol}: usize = 0x{address};\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        formatter_class=argparse.RawTextHelpFormatter
    )
    parser.add_argument("--input")
    parser.add_argument("--output")

    args = parser.parse_args()
    
    symbols_to_found = {
        "__vdso_sigreturn_trampoline": "",
    }
    
    find_symbol_address_streaming(args.input, symbols_to_found)
    write_result_to_file(args.output, symbols_to_found)
