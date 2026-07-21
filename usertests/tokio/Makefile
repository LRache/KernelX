ISA ?= riscv64
ARCH ?= $(ISA)

CARGO ?= cargo
CARGO_TARGET ?= riscv64gc-unknown-linux-gnu
SYSROOT ?= /opt/cross-tools/riscv64-unknown-linux-gnu/sysroot
LINKER ?= /opt/cross-tools/riscv64-unknown-linux-gnu/bin/riscv64-unknown-linux-gnu-gcc

ALL_CASES := async_fd_await stdin_interval tick_interval file_rw
CASE_SOURCES_async_fd_await := src/main.rs Cargo.toml
CASE_SOURCES_stdin_interval := src/stdin_interval.rs Cargo.toml
CASE_SOURCES_tick_interval := src/tick_interval.rs Cargo.toml
CASE_SOURCES_file_rw := src/file_rw.rs Cargo.toml
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(case)))
include ../mk/case-selection.mk

BIN_NAMES = $(CASES)
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
CARGO_TARGET_DIR = $(BUILD_DIR)/cargo-target
TEST_TARGETS = $(foreach bin,$(BIN_NAMES),$(OUTPUT_DIR)/$(bin)/$(bin))
CARGO_TARGET_ENV = $(shell printf '%s' '$(CARGO_TARGET)' | tr '[:lower:]-' '[:upper:]_')
RUSTFLAGS += -C link-arg=--sysroot=$(SYSROOT)

all: $(TEST_TARGETS)

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $$(CASE_SOURCES_$(1))
	$$(info + CARGO $(1))
	@ mkdir -p $$(@D) $(BUILD_DIR)
	@ CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) \
		CARGO_TARGET_$(CARGO_TARGET_ENV)_LINKER=$(LINKER) \
		RUSTFLAGS="$(RUSTFLAGS)" \
		$(CARGO) build --release --target $(CARGO_TARGET) --bin $(1)
	$$(info + CP $$@)
	@ cp $$(CARGO_TARGET_DIR)/$$(CARGO_TARGET)/release/$(1) $$@
endef

$(foreach bin,$(ALL_CASES),$(eval $(call TEST_TARGET_RULE,$(bin))))

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR) target
