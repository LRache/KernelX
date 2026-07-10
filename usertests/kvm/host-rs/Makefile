ARCH ?= riscv64
ISA ?= $(ARCH)
CARGO ?= cargo
PROFILE ?= release

ifeq ($(ARCH),riscv64)
RUST_TARGET ?= riscv64gc-unknown-linux-gnu
CROSS_COMPILE ?= riscv64-unknown-linux-gnu-
else
$(error unsupported ARCH for host-rs: $(ARCH))
endif

ifeq ($(origin CC),default)
CC = $(CROSS_COMPILE)gcc
endif
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
BIN_NAME = kxemu-kvm-rs
OUTPUT_BIN = $(OUTPUT_DIR)/kxemu-kvm

ifeq ($(PROFILE),release)
CARGO_PROFILE_ARG = --release
CARGO_PROFILE_DIR = release
else
CARGO_PROFILE_ARG =
CARGO_PROFILE_DIR = debug
endif

CARGO_BUILD_ENV = CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=$(CC)
CARGO_BUILD_ARGS = --manifest-path Cargo.toml --target $(RUST_TARGET) $(CARGO_PROFILE_ARG)
CARGO_BIN = target/$(RUST_TARGET)/$(CARGO_PROFILE_DIR)/$(BIN_NAME)

.PHONY: all check clean

all:
	@ $(CARGO_BUILD_ENV) $(CARGO) build $(CARGO_BUILD_ARGS)
	@ mkdir -p $(OUTPUT_DIR)
	@ cp $(CARGO_BIN) $(OUTPUT_BIN)

check:
	@ $(CARGO_BUILD_ENV) $(CARGO) check $(CARGO_BUILD_ARGS)

clean:
	@ rm -rf $(BUILD_DIR)
	@ $(CARGO) clean --manifest-path Cargo.toml
