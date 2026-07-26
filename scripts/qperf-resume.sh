#!/usr/bin/env bash

set -euo pipefail

socket_path=${1:-${QPERF_CONTROL:-/tmp/kernelx-qperf.sock}}

if [[ ! -S ${socket_path} ]]; then
    echo "QPerf control socket is unavailable: ${socket_path}" >&2
    exit 1
fi

response=$(
    python3 - "${socket_path}" <<'PY'
import socket
import sys

with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
    client.settimeout(2)
    client.connect(sys.argv[1])
    client.sendall(b"start\n")
    print(client.makefile(encoding="utf-8").readline().strip())
PY
)
if [[ ${response} != "ok" ]]; then
    echo "Failed to resume qperf: ${response}" >&2
    exit 1
fi

echo "QPerf sampling resumed"
