ISA ?= riscv64
ARCH ?= $(ISA)

CARGO ?= cargo
CARGO_TARGET ?= riscv64gc-unknown-linux-gnu
SYSROOT ?= /opt/cross-tools/riscv64-unknown-linux-gnu/sysroot
LINKER ?= /opt/cross-tools/riscv64-unknown-linux-gnu/bin/riscv64-unknown-linux-gnu-gcc

BIN_NAMES = async_fd_await stdin_interval tick_interval file_rw
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
CARGO_TARGET_DIR = $(BUILD_DIR)/cargo-target
BUILD_STAMP = $(BUILD_DIR)/.cargo-build.stamp
TEST_TARGETS = $(foreach bin,$(BIN_NAMES),$(OUTPUT_DIR)/$(bin)/$(bin))
CARGO_TARGET_ENV = $(shell printf '%s' '$(CARGO_TARGET)' | tr '[:lower:]-' '[:upper:]_')
RUSTFLAGS += -C link-arg=--sysroot=$(SYSROOT)

all: $(TEST_TARGETS)

$(BUILD_STAMP): Cargo.toml src/main.rs src/stdin_interval.rs src/tick_interval.rs src/file_rw.rs
	$(info + CARGO $(BIN_NAMES))
	@ mkdir -p $(OUTPUT_DIR) $(BUILD_DIR)
	@ CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) \
		CARGO_TARGET_$(CARGO_TARGET_ENV)_LINKER=$(LINKER) \
		RUSTFLAGS="$(RUSTFLAGS)" \
		$(CARGO) build --release --target $(CARGO_TARGET)
	@ touch $@

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(BUILD_STAMP)
	$$(info + CP $$@)
	@ mkdir -p $$(@D)
	@ cp $$(CARGO_TARGET_DIR)/$$(CARGO_TARGET)/release/$(1) $$@
endef

$(foreach bin,$(BIN_NAMES),$(eval $(call TEST_TARGET_RULE,$(bin))))

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR) target
