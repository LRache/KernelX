#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
LINUX_DIR=${LINUX_DIR:-"$SCRIPT_DIR/linux"}

LINUX_VERSION=5.15.132
LINUX_SERIES=v5.x
LINUX_ARCHIVE=linux-${LINUX_VERSION}.tar.xz
LINUX_URL=https://cdn.kernel.org/pub/linux/kernel/${LINUX_SERIES}/${LINUX_ARCHIVE}

usage() {
    echo "Usage: $0 linux5.15" >&2
    exit 1
}

download_linux_5_15() {
    mkdir -p "$LINUX_DIR"

    if [[ ! -f "$LINUX_DIR/$LINUX_ARCHIVE" ]]; then
        wget -O "$LINUX_DIR/$LINUX_ARCHIVE" "$LINUX_URL"
    fi

    if [[ ! -d "$LINUX_DIR/linux-$LINUX_VERSION" ]]; then
        tar -C "$LINUX_DIR" -xf "$LINUX_DIR/$LINUX_ARCHIVE"
    fi
}

case "${1:-}" in
    linux5.15)
        download_linux_5_15
        ;;
    *)
        usage
        ;;
esac
