FROM debian:bookworm-slim

# --- Base build tools ---
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl wget gnupg ca-certificates lsb-release software-properties-common \
    make cmake ninja-build \
    gcc g++ vim \
    git python3 python3-pip \
    xz-utils file \
    && rm -rf /var/lib/apt/lists/*

# --- LLVM 21 (provides clang-21, llvm-objcopy-21, etc.) ---
RUN curl -fsSL https://apt.llvm.org/llvm.sh -o /tmp/llvm.sh \
    && chmod +x /tmp/llvm.sh \
    && /tmp/llvm.sh 21 \
    && rm /tmp/llvm.sh \
    && rm -rf /var/lib/apt/lists/* \
    && for tool in clang clang++ ld.lld llvm-objcopy llvm-objdump llvm-ar llvm-nm llvm-strip; do \
         ln -sf /usr/bin/${tool}-21 /usr/local/bin/${tool}; \
       done

# --- RISC-V cross-compiler (riscv64-unknown-linux-gnu-gcc) ---
# Debian ships this as gcc-riscv64-linux-gnu; the binaries are prefixed
# riscv64-linux-gnu-* so we create riscv64-unknown-linux-gnu-* symlinks.
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc-riscv64-linux-gnu g++-riscv64-linux-gnu binutils-riscv64-linux-gnu \
    qemu-system-misc \
    && rm -rf /var/lib/apt/lists/* \
    && for bin in /usr/bin/riscv64-linux-gnu-*; do \
         name=$(basename "$bin"); \
         link="/usr/local/bin/riscv64-unknown-linux-gnu-${name#riscv64-linux-gnu-}"; \
         ln -sf "$bin" "$link"; \
       done

# --- Rust (nightly + riscv64gc-unknown-none-elf target) ---
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --no-modify-path --default-toolchain nightly \
    && rustup target add riscv64gc-unknown-none-elf \
    && rustup component add rust-src

# --- Patch ---
RUN sed -i 's|^# include <gnu/stubs-lp64.h>|//&|' /usr/riscv64-linux-gnu/include/gnu/stubs.h

# --- Working directory ---
WORKDIR /workspace

CMD ["/bin/bash"]
