#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

ARCH=${ARCH:-${1:-riscv64}}
ISA=${ISA:-$ARCH}
GUEST_COMPONENTS=${GUEST_COMPONENTS:-"hello_sbi timer_tick"}
DEFAULT_IMG_SIZE_MB=128
LINUX_GUEST_IMG_SIZE_MB=768
IMG_SIZE_MB_EXPLICIT=0
if [[ -n "${IMG_SIZE_MB+x}" ]]; then
    IMG_SIZE_MB_EXPLICIT=1
fi
IMG_SIZE_MB=${IMG_SIZE_MB:-$DEFAULT_IMG_SIZE_MB}
KVM_SKIP_BUILD=${KVM_SKIP_BUILD:-0}

BUILD_DIR=${BUILD_DIR:-"$SCRIPT_DIR/build/$ARCH"}
OUTPUT_DIR=${OUTPUT_DIR:-"$BUILD_DIR/output"}
IMG_FILE=${IMG_FILE:-"$BUILD_DIR/kvm.ext4"}

if [[ -z "${CROSS_COMPILE:-}" && "$ARCH" == "riscv64" ]]; then
    CROSS_COMPILE=riscv64-linux-gnu-
else
    CROSS_COMPILE=${CROSS_COMPILE:-}
fi

CC=${CC:-${CROSS_COMPILE}gcc}
READELF=${READELF:-${CROSS_COMPILE}readelf}
MKFS_EXT4=${MKFS_EXT4:-mkfs.ext4}

declare -A COPIED_PATHS=()
declare -A CREATED_DIRS=()
declare -A PROCESSED_ELFS=()
declare -A STAGED_INTERPRETERS=()

CREATED_DIRS["/"]=1

log() {
    printf '[kvm-package] %s\n' "$*"
}

die() {
    printf '[kvm-package] error: %s\n' "$*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

resolve_realpath() {
    local path=$1
    if command -v realpath >/dev/null 2>&1; then
        realpath "$path"
    else
        readlink -f "$path"
    fi
}

compiler_file_name() {
    local name=$1
    local path=

    if command -v "$CC" >/dev/null 2>&1; then
        path=$("$CC" -print-file-name="$name" 2>/dev/null || true)
        if [[ -n "$path" && "$path" != "$name" && -e "$path" ]]; then
            resolve_realpath "$path"
            return 0
        fi
    fi

    return 1
}

library_search_dirs() {
    local sysroot=

    if command -v "$CC" >/dev/null 2>&1; then
        sysroot=$("$CC" -print-sysroot 2>/dev/null || true)
        if [[ -n "$sysroot" && "$sysroot" != "/" ]]; then
            printf '%s\n' "$sysroot/lib" "$sysroot/usr/lib"
        fi
    fi

    case "$ARCH" in
        riscv64)
            printf '%s\n' \
                /usr/riscv64-linux-gnu/lib \
                /lib/riscv64-linux-gnu \
                /usr/lib/riscv64-linux-gnu
            ;;
        x86_64)
            printf '%s\n' \
                /lib64 \
                /usr/lib64 \
                /lib/x86_64-linux-gnu \
                /usr/lib/x86_64-linux-gnu
            ;;
    esac

    printf '%s\n' /lib /usr/lib /usr/local/lib
}

find_library() {
    local name=$1
    local dir=

    if [[ "$name" == */* && -e "$name" ]]; then
        resolve_realpath "$name"
        return 0
    fi

    if compiler_file_name "$name"; then
        return 0
    fi

    while IFS= read -r dir; do
        [[ -n "$dir" && -e "$dir/$name" ]] || continue
        resolve_realpath "$dir/$name"
        return 0
    done < <(library_search_dirs)

    return 1
}

ensure_image_dir() {
    local dir=$1
    local parent=

    [[ -n "$dir" ]] || dir="/"
    [[ "$dir" == "/" ]] && return 0
    [[ -n "${CREATED_DIRS[$dir]:-}" ]] && return 0

    parent=$(dirname "$dir")
    ensure_image_dir "$parent"

    e2mkdir "$IMG_FILE:$dir"
    CREATED_DIRS[$dir]=1
}

copy_to_image() {
    local src=$1
    local dest_path=$2

    if [[ -n "${COPIED_PATHS[$dest_path]:-}" ]]; then
        return 0
    fi

    ensure_image_dir "$(dirname "$dest_path")"
    e2cp -p "$src" "$IMG_FILE:$dest_path"
    COPIED_PATHS[$dest_path]=1
    log "copy $(basename "$src") -> $dest_path"
}

elf_interpreter() {
    local elf=$1
    "$READELF" -l "$elf" 2>/dev/null |
        sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p' |
        head -n 1
}

elf_needed_libraries() {
    local elf=$1
    "$READELF" -d "$elf" 2>/dev/null |
        sed -n 's/.*Shared library: \[\([^]]*\)\].*/\1/p'
}

stage_library() {
    local name=$1
    local src=

    src=$(find_library "$name") || die "cannot find dynamic library: $name"
    copy_to_image "$src" "/lib/$(basename "$name")"
    stage_elf_deps "$src"
}

stage_elf_deps() {
    local elf=$1
    local elf_key=
    local interpreter=
    local interpreter_name=
    local needed=
    local interpreter_src=

    "$READELF" -h "$elf" >/dev/null 2>&1 || return 0
    elf_key=$(resolve_realpath "$elf")
    if [[ -n "${PROCESSED_ELFS[$elf_key]:-}" ]]; then
        return 0
    fi
    PROCESSED_ELFS[$elf_key]=1

    interpreter=$(elf_interpreter "$elf")
    if [[ -n "$interpreter" ]]; then
        interpreter_name=$(basename "$interpreter")
        interpreter_src=$(find_library "$(basename "$interpreter")") ||
            die "cannot find ELF interpreter: $interpreter"
        copy_to_image "$interpreter_src" "$interpreter"
        STAGED_INTERPRETERS[$interpreter_name]=1
    fi

    while IFS= read -r needed; do
        [[ -n "$needed" ]] || continue
        [[ -n "$interpreter_name" && "$needed" == "$interpreter_name" ]] && continue
        [[ -n "${STAGED_INTERPRETERS[$needed]:-}" ]] && continue
        stage_library "$needed"
    done < <(elf_needed_libraries "$elf")
}

stage_dynamic_libraries() {
    local file=

    while IFS= read -r -d '' file; do
        stage_elf_deps "$file"
    done < <(find "$OUTPUT_DIR" -type f -print0)
}

build_outputs() {
    if [[ "$KVM_SKIP_BUILD" == "1" ]]; then
        return 0
    fi

    log "build kvm usertests for ARCH=$ARCH ISA=$ISA GUEST_COMPONENTS=$GUEST_COMPONENTS"
    make -C "$SCRIPT_DIR" ARCH="$ARCH" ISA="$ISA" GUEST_COMPONENTS="$GUEST_COMPONENTS" all
}

adjust_image_size() {
    if [[ "$IMG_SIZE_MB_EXPLICIT" == "0" && -d "$OUTPUT_DIR/guest/linux5.15" ]]; then
        IMG_SIZE_MB=$LINUX_GUEST_IMG_SIZE_MB
    fi
}

copy_outputs_to_image() {
    local file=
    local rel=
    local dest=

    [[ -d "$OUTPUT_DIR" ]] || die "missing build output directory: $OUTPUT_DIR"
    [[ -n "$(find "$OUTPUT_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
        die "empty build output directory: $OUTPUT_DIR"

    while IFS= read -r -d '' file; do
        rel=${file#"$OUTPUT_DIR"/}
        dest="/$rel"
        copy_to_image "$file" "$dest"
    done < <(find "$OUTPUT_DIR" -type f -print0)
}

create_image() {
    mkdir -p "$(dirname "$IMG_FILE")"
    rm -f "$IMG_FILE"

    log "create ext4 image: $IMG_FILE (${IMG_SIZE_MB}MiB)"
    dd if=/dev/zero of="$IMG_FILE" bs=1M count="$IMG_SIZE_MB" status=none
    "$MKFS_EXT4" -F -b 4096 "$IMG_FILE" >/dev/null
}

main() {
    require_cmd "$READELF"
    require_cmd "$MKFS_EXT4"
    require_cmd e2mkdir
    require_cmd e2cp
    require_cmd dd
    require_cmd make

    build_outputs
    adjust_image_size
    create_image
    copy_outputs_to_image
    stage_dynamic_libraries

    log "image generated: $IMG_FILE"
}

main "$@"
