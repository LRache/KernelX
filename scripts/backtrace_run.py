#!/usr/bin/env python3
"""
backtrace_run.py — 像 tee 一样运行 QEMU，退出后自动解析 stack backtrace 并输出源码行号。

用法:
    python3 scripts/backtrace_run.py --elf <elf> --cross-compile <prefix> -- <qemu cmd...>
"""

import sys
import os
import re
import pty
import select
import subprocess
import signal
import argparse

try:
    import termios
    import tty
    HAS_TTY = True
except ImportError:
    HAS_TTY = False

BACKTRACE_START = "--- Stack Backtrace ---"
BACKTRACE_END   = "--- End of Backtrace ---"
# 匹配形如: [00] 0xffffffc000xxxxxx: func+0xNN
FRAME_RE = re.compile(r'\[(\d+)\]\s+(0x[0-9a-fA-F]+):')

RESET  = "\033[0m"
BOLD   = "\033[1m"
BLUE   = "\033[1;34m"
YELLOW = "\033[33m"
CYAN   = "\033[36m"


def resolve_address(elf: str, addr: str) -> str:
    """调用 addr2line 将 PC 地址解析为 函数名 + 文件:行号。"""
    try:
        r = subprocess.run(
            ["addr2line", "-e", elf, "-f", "-p", "-i", "-C", addr],
            capture_output=True, text=True, timeout=5,
        )
        out = r.stdout.strip()
        return out if out else "??"
    except Exception as e:
        return str(e)


def print_backtrace_locations(bt_lines: list[str], elf: str, index: int = 0, total: int = 1) -> None:
    label = f"  Backtrace Source Locations" if total == 1 else f"  Backtrace #{index + 1} Source Locations"
    print(f"\n{BLUE}{'─' * 60}{RESET}")
    print(f"{BLUE}{label}{RESET}")
    print(f"{BLUE}{'─' * 60}{RESET}")
    for line in bt_lines:
        m = FRAME_RE.search(line)
        if not m:
            continue
        depth = int(m.group(1))
        addr  = m.group(2)
        loc   = resolve_address(elf, addr)
        print(f"  {YELLOW}[{depth:02d}]{RESET} {CYAN}{addr}{RESET}  {loc}")
    print(f"{BLUE}{'─' * 60}{RESET}\n")


def run_qemu(cmd: list[str]) -> bytes:
    """
    用 PTY 运行 cmd，把输出实时镜像到 stdout，同时收集全部字节返回。
    如果 stdin 是终端则设为 raw mode，以便 Ctrl-A X 等按键透传给 QEMU。
    """
    master_fd, slave_fd = pty.openpty()

    proc = subprocess.Popen(
        cmd,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        close_fds=True,
    )
    os.close(slave_fd)

    buf = b""
    old_settings = None

    stdin_fd = sys.stdin.fileno() if sys.stdin.isatty() else -1
    if HAS_TTY and stdin_fd >= 0:
        old_settings = termios.tcgetattr(stdin_fd)
        tty.setraw(stdin_fd)

    def restore_tty():
        if old_settings is not None:
            try:
                termios.tcsetattr(stdin_fd, termios.TCSADRAIN, old_settings)
            except Exception:
                pass

    def on_signal(_sig, _frame):
        proc.terminate()
        restore_tty()
        sys.exit(0)

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT,  on_signal)

    try:
        fds = [master_fd] + ([stdin_fd] if stdin_fd >= 0 else [])
        while True:
            try:
                r, _, _ = select.select(fds, [], [], 0.05)
            except (ValueError, OSError):
                break

            for fd in r:
                if fd == master_fd:
                    try:
                        data = os.read(master_fd, 4096)
                    except OSError:
                        data = b""
                    if data:
                        sys.stdout.buffer.write(data)
                        sys.stdout.buffer.flush()
                        buf += data
                    else:
                        # master 关闭 → QEMU 已退出
                        proc.wait()
                        return buf
                elif fd == stdin_fd:
                    try:
                        data = os.read(stdin_fd, 256)
                    except OSError:
                        data = b""
                    if data:
                        try:
                            os.write(master_fd, data)
                        except OSError:
                            pass

            if proc.poll() is not None:
                # 排空剩余输出
                while True:
                    try:
                        r2, _, _ = select.select([master_fd], [], [], 0.05)
                    except (ValueError, OSError):
                        break
                    if not r2:
                        break
                    try:
                        data = os.read(master_fd, 4096)
                    except OSError:
                        break
                    if data:
                        sys.stdout.buffer.write(data)
                        sys.stdout.buffer.flush()
                        buf += data
                    else:
                        break
                break
    finally:
        restore_tty()
        try:
            os.close(master_fd)
        except OSError:
            pass

    proc.wait()
    return buf


def extract_backtraces(output: bytes) -> list[list[str]]:
    lines = output.decode("utf-8", errors="replace").splitlines()
    all_bts: list[list[str]] = []
    in_bt = False
    current: list[str] = []
    for line in lines:
        if BACKTRACE_START in line:
            in_bt = True
            current = []
        elif BACKTRACE_END in line:
            in_bt = False
            if current:
                all_bts.append(current)
        elif in_bt:
            current.append(line)
    return all_bts


def main():
    parser = argparse.ArgumentParser(
        description="Run QEMU with live output and resolve panic backtrace to source lines.",
        add_help=True,
    )
    parser.add_argument("--elf",           required=True, help="Kernel ELF file (for addr2line)")
    # parser.add_argument("--cross-compile", default="",   help="Toolchain prefix, e.g. riscv64-unknown-elf-")
    parser.add_argument("cmd", nargs=argparse.REMAINDER, help="QEMU command and arguments")

    args = parser.parse_args()
    cmd = args.cmd
    if cmd and cmd[0] == "--":
        cmd = cmd[1:]

    if not cmd:
        parser.error("No QEMU command specified after --")

    output = run_qemu(cmd)
    all_bts = extract_backtraces(output)

    if all_bts:
        for i, bt_lines in enumerate(all_bts):
            print_backtrace_locations(bt_lines, args.elf, index=i, total=len(all_bts))
    else:
        print(f"\n{CYAN}(no stack backtrace found in output){RESET}\n")


if __name__ == "__main__":
    main()
