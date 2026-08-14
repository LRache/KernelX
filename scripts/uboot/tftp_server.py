#!/usr/bin/env python3
"""Serve KernelX uImages to U-Boot over TFTP.

The default root is this checkout's ``build/riscv64`` directory.  UDP/69 is
privileged on most hosts, so start this script with sudo when required.
"""

import argparse
import socket
import struct
from pathlib import Path


DEFAULT_ROOT = Path(__file__).resolve().parents[2] / "build/riscv64"
DEFAULT_ALLOWED = ("Image", "kernelx.uImage")
BLOCK_SIZE = 512


def packet_error(code, message):
    return struct.pack("!HH", 5, code) + message.encode() + b"\0"


def transfer(server, request, client, root, allowed):
    fields = request[2:].split(b"\0")
    try:
        filename = fields[0].decode("utf-8", "strict")
    except (IndexError, UnicodeDecodeError):
        server.sendto(packet_error(1, "Invalid filename"), client)
        return

    if filename not in allowed:
        server.sendto(packet_error(1, "File not found"), client)
        return

    path = root / filename
    try:
        data = path.open("rb")
    except OSError:
        server.sendto(packet_error(1, "File not found"), client)
        return

    with data:
        block = 1
        while True:
            chunk = data.read(BLOCK_SIZE)
            packet = struct.pack("!HH", 3, block) + chunk
            acknowledged = False
            for _ in range(5):
                server.sendto(packet, client)
                server.settimeout(2.0)
                try:
                    while True:
                        reply, source = server.recvfrom(2048)
                        if source != client or len(reply) < 4:
                            continue
                        opcode, ack_block = struct.unpack("!HH", reply[:4])
                        if opcode == 4 and ack_block == block:
                            acknowledged = True
                            break
                        if opcode == 5:
                            return
                    if acknowledged:
                        break
                except socket.timeout:
                    continue
            if not acknowledged:
                return
            if len(chunk) < BLOCK_SIZE:
                return
            block = (block + 1) & 0xFFFF


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=DEFAULT_ROOT,
        help=f"TFTP root (default: {DEFAULT_ROOT})",
    )
    parser.add_argument(
        "--bind-address",
        default="192.168.120.1",
        help="local address to bind (default: 192.168.120.1)",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=69,
        help="TFTP UDP port (default: 69)",
    )
    parser.add_argument(
        "--allow",
        dest="allowed",
        action="append",
        default=list(DEFAULT_ALLOWED),
        help="additional file name to serve",
    )
    args = parser.parse_args()

    if not args.root.is_dir():
        parser.error(f"TFTP root does not exist: {args.root}")

    server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind((args.bind_address, args.port))
    print(
        f"TFTP serving {', '.join(args.allowed)} from {args.root} "
        f"on {args.bind_address}:{args.port}",
        flush=True,
    )
    while True:
        server.settimeout(None)
        request, client = server.recvfrom(2048)
        if len(request) >= 4 and struct.unpack("!H", request[:2])[0] == 1:
            try:
                transfer(server, request, client, args.root, set(args.allowed))
            finally:
                server.settimeout(None)


if __name__ == "__main__":
    main()
