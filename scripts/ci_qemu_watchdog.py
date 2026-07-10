#!/usr/bin/env python3
"""
Run QEMU for CI while failing if it stops producing output.

On idle timeout, this captures a small QEMU monitor snapshot before stopping the
guest, so the workflow artifact has enough state for post-mortem debugging.
"""

import argparse
import os
import selectors
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path


IDLE_EXIT_CODE = 124
FAILURE_EXIT_CODE = 1
READ_SIZE = 4096
MONITOR_COMMANDS = [
    "info status",
    "info version",
    "info cpus",
    "info registers",
    "info block",
    "info chardev",
    "info network",
    "info mtree",
    "info qtree",
]


def hmp_prompt_seen(data: bytes) -> bool:
    return data.rstrip().endswith(b"(qemu)")


def write_all(log, data: bytes) -> None:
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
    log.write(data)
    log.flush()


def write_line(log, message: str) -> None:
    write_all(log, f"{message}\n".encode())


def capture_failure(args: argparse.Namespace, reason: str) -> None:
    if args.monitor_socket and args.monitor_log:
        capture_monitor(
            args.monitor_socket,
            Path(args.monitor_log),
            Path(args.memory_dump) if args.memory_dump else None,
            reason,
        )


def read_monitor_response(
    sock: socket.socket,
    timeout: float = 5.0,
    idle_timeout: float = 0.3,
    wait_for_prompt: bool = False,
) -> bytes:
    deadline = time.monotonic() + timeout
    idle_deadline = deadline if wait_for_prompt else time.monotonic() + idle_timeout
    chunks: list[bytes] = []

    sock.setblocking(False)
    while time.monotonic() < deadline and time.monotonic() < idle_deadline:
        try:
            data = sock.recv(65536)
        except BlockingIOError:
            time.sleep(0.05)
            continue

        if not data:
            break

        chunks.append(data)
        response = b"".join(chunks)
        if wait_for_prompt and hmp_prompt_seen(response):
            break
        if not wait_for_prompt:
            idle_deadline = time.monotonic() + idle_timeout

    return response if chunks else b""


def run_monitor_command(
    sock: socket.socket,
    log,
    command: str,
    timeout: float = 5.0,
    wait_for_prompt: bool = False,
) -> None:
    log.write(f"\n(qemu) {command}\n".encode())
    log.flush()
    sock.sendall(f"{command}\n".encode())
    log.write(read_monitor_response(sock, timeout=timeout, wait_for_prompt=wait_for_prompt))
    log.flush()


def connect_monitor(path: str) -> socket.socket:
    deadline = time.monotonic() + 2.0
    last_error: OSError | None = None

    while time.monotonic() < deadline:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.connect(path)
            return sock
        except OSError as err:
            last_error = err
            sock.close()
            time.sleep(0.1)

    raise RuntimeError(f"failed to connect to QEMU monitor {path}: {last_error}")


def capture_monitor(
    monitor_socket: str,
    monitor_log: Path,
    memory_dump: Path | None,
    reason: str,
) -> None:
    monitor_log.parent.mkdir(parents=True, exist_ok=True)
    with monitor_log.open("ab") as log:
        log.write(f"### QEMU monitor snapshot: {reason}\n".encode())
        log.write(f"### captured_at={time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\n\n".encode())

        try:
            with connect_monitor(monitor_socket) as sock:
                greeting = read_monitor_response(sock)
                if greeting:
                    log.write(greeting)

                run_monitor_command(sock, log, "stop", wait_for_prompt=True)
                if memory_dump is not None:
                    memory_dump.parent.mkdir(parents=True, exist_ok=True)
                    run_monitor_command(
                        sock,
                        log,
                        f"dump-guest-memory {memory_dump}",
                        timeout=600.0,
                        wait_for_prompt=True,
                    )

                for command in MONITOR_COMMANDS:
                    run_monitor_command(sock, log, command)
        except Exception as err:
            log.write(f"\nfailed to capture QEMU monitor: {err}\n".encode())


def stop_qemu(proc: subprocess.Popen[bytes]) -> None:
    if proc.poll() is not None:
        return

    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def remove_monitor_socket(path: str | None) -> None:
    if path is None:
        return

    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def run_qemu(args: argparse.Namespace) -> int:
    log_path = Path(args.log)
    log_path.parent.mkdir(parents=True, exist_ok=True)

    remove_monitor_socket(args.monitor_socket)

    with log_path.open("ab") as log:
        write_line(log, f"[ci-qemu-watchdog] idle timeout: {args.idle_timeout}s")
        write_line(log, f"[ci-qemu-watchdog] command: {' '.join(args.cmd)}")

        proc = subprocess.Popen(
            args.cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )
        assert proc.stdout is not None
        os.set_blocking(proc.stdout.fileno(), False)

        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ)
        last_output = time.monotonic()
        idle_timed_out = False
        stopped_after_success = False
        stopping = False
        success_marker = args.success_marker.encode() if args.success_marker else None
        success_seen = False
        marker_tail = b""

        def observe_output(data: bytes) -> None:
            nonlocal marker_tail, success_seen
            if success_marker is None or success_seen:
                return

            combined = marker_tail + data
            if success_marker in combined:
                success_seen = True

            marker_tail_len = max(0, len(success_marker) - 1)
            marker_tail = combined[-marker_tail_len:] if marker_tail_len else b""

        def on_signal(signum, _frame) -> None:
            nonlocal stopping
            if stopping:
                return
            stopping = True
            write_line(log, f"[ci-qemu-watchdog] received signal {signum}, stopping QEMU")
            capture_failure(args, f"signal {signum}")
            stop_qemu(proc)

        signal.signal(signal.SIGTERM, on_signal)
        signal.signal(signal.SIGINT, on_signal)

        while proc.poll() is None:
            for key, _events in selector.select(timeout=1.0):
                data = key.fileobj.read(READ_SIZE)
                if data:
                    write_all(log, data)
                    observe_output(data)
                    last_output = time.monotonic()

            if args.exit_on_success and success_seen:
                stopped_after_success = True
                write_line(log, "[ci-qemu-watchdog] success marker seen; stopping QEMU")
                stop_qemu(proc)
                break

            if time.monotonic() - last_output >= args.idle_timeout:
                idle_timed_out = True
                write_line(
                    log,
                    f"[ci-qemu-watchdog] no QEMU output for {args.idle_timeout}s; capturing monitor state",
                )
                capture_failure(args, "idle timeout")
                stop_qemu(proc)
                break

        for key, _events in selector.select(timeout=0):
            data = key.fileobj.read()
            if data:
                write_all(log, data)
                observe_output(data)

        selector.close()
        returncode = proc.wait()

        if idle_timed_out:
            write_line(log, f"[ci-qemu-watchdog] exiting with {IDLE_EXIT_CODE}")
            remove_monitor_socket(args.monitor_socket)
            return IDLE_EXIT_CODE

        if stopped_after_success:
            write_line(log, "[ci-qemu-watchdog] exiting with 0")
            remove_monitor_socket(args.monitor_socket)
            return 0

        write_line(log, f"[ci-qemu-watchdog] QEMU exited with {returncode}")
        if returncode != 0:
            write_line(log, "[ci-qemu-watchdog] capturing monitor state after non-zero QEMU exit")
            capture_failure(args, f"qemu exited with {returncode}")
            remove_monitor_socket(args.monitor_socket)
            return returncode

        if success_marker is not None and not success_seen:
            write_line(log, "[ci-qemu-watchdog] capturing monitor state after missing success marker")
            capture_failure(args, "missing success marker")
            remove_monitor_socket(args.monitor_socket)
            return FAILURE_EXIT_CODE

        remove_monitor_socket(args.monitor_socket)
        return returncode


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run QEMU with CI idle-output watchdog.")
    parser.add_argument("--idle-timeout", type=int, default=300, help="Seconds without output before failing.")
    parser.add_argument("--log", required=True, help="File that receives mirrored QEMU output.")
    parser.add_argument("--monitor-socket", help="QEMU HMP monitor UNIX socket path.")
    parser.add_argument("--monitor-log", help="File that receives QEMU monitor state on timeout.")
    parser.add_argument("--memory-dump", help="Guest physical memory dump path captured on timeout.")
    parser.add_argument("--success-marker", help="Output marker required for a successful run.")
    parser.add_argument(
        "--exit-on-success",
        action="store_true",
        help="Stop QEMU and return success as soon as --success-marker is seen.",
    )
    parser.add_argument("cmd", nargs=argparse.REMAINDER, help="QEMU command and arguments.")

    args = parser.parse_args()
    if args.cmd and args.cmd[0] == "--":
        args.cmd = args.cmd[1:]
    if not args.cmd:
        parser.error("No QEMU command specified after --")
    if bool(args.monitor_socket) != bool(args.monitor_log):
        parser.error("--monitor-socket and --monitor-log must be used together")
    if args.memory_dump and not args.monitor_socket:
        parser.error("--memory-dump requires --monitor-socket and --monitor-log")
    if args.exit_on_success and not args.success_marker:
        parser.error("--exit-on-success requires --success-marker")
    return args


if __name__ == "__main__":
    sys.exit(run_qemu(parse_args()))
