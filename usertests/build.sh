#!/bin/bash

set -e

ISA=${1:-riscv64}
IMG_SIZE=${IMG_SIZE:-256}
BUILD_DIR=${BUILD_DIR:-build}

TESTS=(
    # "basic-ulib"
    # "basic-musl"
    "basic-glibc"
    "filesystem"
    # "basic-glibc-static"
    # "os-func"
)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_test() {
    echo -e "${BLUE}[TEST]${NC} $1"
}

log_info "Build configuration:"
echo "  ISA: $ISA"
echo "  Image size: ${IMG_SIZE}MB"
echo "  Build directory: $BUILD_DIR"
echo "  Tests: ${TESTS[*]}"

log_info "Creating directory structure..."
rm -rf ${BUILD_DIR}/${ISA}
mkdir -p ${BUILD_DIR}/${ISA}
mkdir -p img/${ISA}

for test in "${TESTS[@]}"; do
    TEST_DIR="$test"
    OUTPUT_DIR="$test/build/${ISA}/output"
    TARGET_DIR="${BUILD_DIR}/${ISA}/$test"
    
    mkdir -p "$TARGET_DIR"
    
    if [ ! -d "$TEST_DIR" ]; then
        log_warn "Test directory $TEST_DIR does not exist, skipping..."
        continue
    fi
    
    log_info "Building $test for $ISA..."
    cd "$TEST_DIR"
    
    if [ -f "Makefile" ] || [ -f "makefile" ]; then
        log_info "Running 'make all' in $test directory..."
        if make all ISA=$ISA; then
            log_info "$test build completed successfully"
        else
            log_error "Failed to build $test"
            cd ..
            continue
        fi
    else
        log_warn "No Makefile found in $test, skipping build..."
    fi
    
    cd ..
    
    if [ ! -d "$OUTPUT_DIR" ]; then
        log_warn "Output directory $OUTPUT_DIR does not exist for $test"
        continue
    fi
    
    log_info "Collecting files from $OUTPUT_DIR..."
    if [ "$(ls -A $OUTPUT_DIR 2>/dev/null)" ]; then
        cp -r $OUTPUT_DIR/* "$TARGET_DIR/"
        log_info "Files copied successfully for $test ($(ls -1 $OUTPUT_DIR | wc -l) files)"
    else
        log_warn "No files found in $OUTPUT_DIR for $test"
    fi
done

MUSL_LIBC="/opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libc.so"
MUSL_LIBC_DEST="${BUILD_DIR}/${ISA}/lib/ld-musl-riscv64.so.1"

if [ -f "$MUSL_LIBC" ]; then
    log_info "Copying musl libc to build directory..."
    mkdir -p "${BUILD_DIR}/${ISA}/lib"
    cp "$MUSL_LIBC" "$MUSL_LIBC_DEST"
else
    log_error "Musl libc not found at $MUSL_LIBC, skipping copy"
fi

# GLIBC_LIBC_LD="/home/rache/code/glibc/build/riscv64-linux-gnu/elf/ld.so"
# GLIBC_LIBC_SO="/home/rache/code/glibc/build/riscv64-linux-gnu/libc.so"
GLIBC_LIBC_LD="/usr/riscv64-linux-gnu/lib/ld-linux-riscv64-lp64d.so.1"
GLIBC_LIBC_SO="/usr/riscv64-linux-gnu/lib/libc.so.6"
GLIBC_LIBC_LD_DEST="${BUILD_DIR}/${ISA}/lib/ld-linux-riscv64-lp64d.so.1"
GLIBC_LIBC_SO_DEST="${BUILD_DIR}/${ISA}/lib/libc.so.6"

mkdir -p "${BUILD_DIR}/${ISA}/lib"

if [ -f "$GLIBC_LIBC_LD" ]; then
    log_info "Copying glibc dynamic linker to build directory..."
    cp "$GLIBC_LIBC_LD" "$GLIBC_LIBC_LD_DEST"
else
    log_error "Glibc dynamic linker not found at $GLIBC_LIBC_LD, skipping copy"
fi

if [ -f "$GLIBC_LIBC_SO" ]; then
    log_info "Copying glibc libc.so.6 to build directory..."
    cp "$GLIBC_LIBC_SO" "$GLIBC_LIBC_SO_DEST"
else
    log_error "Glibc libc.so.6 not found at $GLIBC_LIBC_SO, skipping copy"
fi

IMG_FILE="${BUILD_DIR}/${ISA}.ext4"
log_info "Creating ext4 image: $IMG_FILE"

[ -f "$IMG_FILE" ] && rm -f "$IMG_FILE"

dd if=/dev/zero of=$IMG_FILE bs=1M count=$IMG_SIZE 2>/dev/null
/sbin/mkfs.ext4 -b 4096 -F $IMG_FILE >/dev/null 2>&1

if sudo mount -o loop $IMG_FILE img/${ISA}; then
    sudo cp -r ${BUILD_DIR}/${ISA}/* img/${ISA}/ 2>/dev/null || true
    sudo chmod -R 755 img/${ISA}/
    sync
    sudo umount img/${ISA}
    rm -rf img
else
    log_error "Failed to mount image"
    exit 1
fi

log_info "Build completed successfully!"
echo "Generated image: $IMG_FILE"
echo "Image size: $(ls -lh $IMG_FILE | awk '{print $5}')"
echo ""
echo "Contents of ${BUILD_DIR}/${ISA}/:"
for test in "${TESTS[@]}"; do
    if [ -d "${BUILD_DIR}/${ISA}/$test" ]; then
        echo "  $test/:"
        ls -lai ${BUILD_DIR}/${ISA}/$test/ | sed 's/^/    /'
    fi
done
