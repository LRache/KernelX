COMPILE_MODE ?= debug

OBJCOPY ?= objcopy

KERNELX_HOME := $(strip $(patsubst %/, %, $(dir $(abspath $(lastword $(MAKEFILE_LIST))))))
LWEXT4_SUBMODULE := $(KERNELX_HOME)/clib/lib/lwext4/lwext4
LWEXT4_PATCHES := $(sort $(abspath $(wildcard $(KERNELX_HOME)/patches/lwext4-*.patch)))

BUILD = $(abspath build/$(ARCH)$(ARCH_BITS))
KERNEL_VM = $(BUILD)/vmkernelx
KERNEL_IMAGE = $(BUILD)/Image

CLIB = clib/build/$(ARCH)$(ARCH_BITS)/libkernelx_clib.a
VDSO = vdso/build/$(ARCH)$(ARCH_BITS)/vdso.o

BUILD_ENV = \
	ARCH=$(ARCH) \
	ARCH_BITS=$(ARCH_BITS) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	KERNELX_INITPATH=$(INITPATH) \
	KERNELX_INITCWD=$(INITCWD) \
	KERNELX_RELEASE=$(KERNELX_RELEASE) \
	KERNELX_HOME=$(KERNELX_HOME) \
	SYSROOT=$(SYSROOT) \
	COMPILE_MODE=$(COMPILE_MODE) \
	RUSTFLAGS="$(RUSTFLAGS)"

RUST_TARGET = riscv64gc-unknown-none-elf
RUST_TARGET_DIR ?= $(abspath target/$(RUST_TARGET)/$(COMPILE_MODE))
RUST_KERNEL ?= $(RUST_TARGET_DIR)/kernelx
RUST_DEPENDENCIES = $(RUST_TARGET_DIR)/kernelx.d

# ------ Configure log level features using a more elegant lookup ------ #
LOG_FEATURES_trace = log-trace
LOG_FEATURES_debug = log-debug
LOG_FEATURES_info = log-info
LOG_FEATURES_warn = log-warn

ifeq ($(CONFIG_LOG_LEVEL),)
RUST_FEATURES += log-info
else ifneq ($(LOG_FEATURES_$(CONFIG_LOG_LEVEL)),)
RUST_FEATURES += $(LOG_FEATURES_$(CONFIG_LOG_LEVEL))
else
$(warning Invalid LOG_LEVEL: $(CONFIG_LOG_LEVEL). Valid values: trace, debug, info, warn)
endif
# ------ Configure log level features using a more elegant lookup ------ #

ifeq ($(CONFIG_LOG_SYSCALL),y)
RUST_FEATURES += log-trace-syscall
endif

ifeq ($(CONFIG_ENABLE_SWAP_MEMORY),y)
RUST_FEATURES += swap-memory
endif

ifeq ($(CONFIG_KVM),y)
RUST_FEATURES += kvm
endif

ifeq ($(CONFIG_WARN_UNIMPLEMENTED_SYSCALL),y)
RUST_FEATURES += warn-unimplemented-syscall
endif

ifeq ($(CONFIG_NO_SMP),y)
RUST_FEATURES += no-smp
endif

ifeq ($(CONFIG_DEADLOCK_DETECT),y)
RUST_FEATURES += deadlock-detect
endif

ifeq ($(CONFIG_NOLOCK),y)
RUST_FEATURES += nolock
endif

ifeq ($(CONFIG_BACKTRACE),y)
RUST_FEATURES += backtrace
RUSTFLAGS += -C force-frame-pointers=yes
endif

CARGO_FLAGS += --target $(RUST_TARGET)
CARGO_FLAGS += --no-default-features --features "$(RUST_FEATURES)"
ifeq ($(COMPILE_MODE),release)
CARGO_FLAGS += --release
endif

all: kernel

kernel: clib vdso $(RUST_KERNEL)
	@ mkdir -p $(BUILD)
	@ cp $(RUST_KERNEL) $(KERNEL_VM)
	@ $(OBJCOPY) -O binary $(RUST_KERNEL) $(KERNEL_IMAGE)

$(KERNEL_VM): $(RUST_KERNEL)
	@ mkdir -p $(BUILD)
	@ cp $(RUST_KERNEL) $(KERNEL_VM)

image: $(KERNEL_IMAGE)

$(KERNEL_IMAGE): $(RUST_KERNEL)
	@ mkdir -p $(BUILD)
	echo "+ OBJCOPY $(RUST_KERNEL) $(KERNEL_IMAGE)"
	@ $(OBJCOPY) -O binary $(RUST_KERNEL) $(KERNEL_IMAGE)

$(CLIB): clib

clib: patch-lwext4
	@ $(BUILD_ENV) make -C clib all

$(VDSO): vdso

vdso:
	@ $(BUILD_ENV) make -C vdso all

$(RUST_KERNEL): $(CLIB) $(VDSO)
	@ mkdir -p build/$(ARCH)$(ARCH_BITS)
	@ test -f build/$(ARCH)$(ARCH_BITS)/symbols.bin || touch build/$(ARCH)$(ARCH_BITS)/symbols.bin
	@ $(BUILD_ENV) cargo build $(CARGO_FLAGS)
ifeq ($(CONFIG_BACKTRACE),y)
	@ python3 scripts/gen_symbols.py $(RUST_KERNEL) build/$(ARCH)$(ARCH_BITS)/symbols.bin
	@ $(BUILD_ENV) cargo build $(CARGO_FLAGS)
endif

check: patch-lwext4
	@ $(BUILD_ENV) cargo check $(CARGO_FLAGS)

patch-lwext4:
	@if [ -z "$(LWEXT4_PATCHES)" ]; then \
		echo "[lwext4] no local patches to apply"; \
	else \
		for patch in $(LWEXT4_PATCHES); do \
			if git -C $(LWEXT4_SUBMODULE) apply --check --reverse "$$patch" >/dev/null 2>&1; then \
				echo "[lwext4] patch already applied: $$patch"; \
			else \
				echo "[lwext4] applying patch: $$patch"; \
				git -C $(LWEXT4_SUBMODULE) apply "$$patch" || exit $$?; \
			fi; \
		done; \
	fi

clean:
	@ $(BUILD_ENV) make -C clib clean
	@ $(BUILD_ENV) make -C vdso clean
	@ $(BUILD_ENV) cargo clean

.PHONY: all clib vdso image check patch-lwext4 $(RUST_KERNEL)
