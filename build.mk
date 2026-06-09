COMPILE_MODE ?= debug

OBJCOPY ?= objcopy
AR ?= ar

KERNELX_HOME := $(strip $(patsubst %/, %, $(dir $(abspath $(lastword $(MAKEFILE_LIST))))))

BUILD = $(abspath build/$(ARCH)$(ARCH_BITS))
KERNEL_VM = $(BUILD)/vmkernelx
KERNEL_IMAGE = $(BUILD)/Image

CLIB = clib/build/$(ARCH)$(ARCH_BITS)/libkernelx_clib.a
VDSO = vdso/build/$(ARCH)$(ARCH_BITS)/vdso.o

CONFIG_SECOND_FSTYPE := $(or $(CONFIG_SECOND_FSTYPE),ext4)
CONFIG_SECOND_MOUNTPOINT := $(or $(CONFIG_SECOND_MOUNTPOINT),/mnt)

BUILD_ENV = \
	ARCH=$(ARCH) \
	ARCH_BITS=$(ARCH_BITS) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	AR=$(AR) \
	RUST_TARGET=$(RUST_TARGET) \
	KERNELX_INITPATH=$(INITPATH) \
	KERNELX_INITCWD=$(INITCWD) \
	KERNELX_RELEASE=$(KERNELX_RELEASE) \
	KERNELX_HOME=$(KERNELX_HOME) \
	CONFIG_DEFAULT_BOOT_DEVICE=$(CONFIG_DEFAULT_BOOT_ROOT_DEVICE) \
	CONFIG_SECOND_DEVICE=$(CONFIG_SECOND_DEVICE) \
	CONFIG_SECOND_FSTYPE=$(CONFIG_SECOND_FSTYPE) \
	CONFIG_SECOND_MOUNTPOINT=$(CONFIG_SECOND_MOUNTPOINT) \
	CONFIG_DEFAULT_INITPATH=$(CONFIG_DEFAULT_INITPATH) \
	CONFIG_DEFAULT_BOOTARGS="$(CONFIG_DEFAULT_BOOTARGS)" \
	SYSROOT=$(SYSROOT) \
	COMPILE_MODE=$(COMPILE_MODE) \
	RUSTFLAGS="$(RUSTFLAGS)"

# Rust target triple — passed in from config/config.mk based on Kconfig.
# Fallback for direct `make -f build.mk ...` invocations.
RUST_TARGET ?= riscv64gc-unknown-none-elf
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

ifeq ($(CONFIG_LOCKDEP),y)
RUST_FEATURES += lockdep
endif

ifeq ($(CONFIG_SPINLOCK_CHECK),y)
RUST_FEATURES += spinlock-check
endif

ifeq ($(CONFIG_ENABLE_WATCHDOG),y)
RUST_FEATURES += watchdog
endif

ifeq ($(CONFIG_NOLOCK),y)
RUST_FEATURES += nolock
endif

ifeq ($(CONFIG_BACKTRACE),y)
RUST_FEATURES += backtrace
RUSTFLAGS += -C force-frame-pointers=yes
endif

-include $(KERNELX_HOME)/scripts/$(ARCH)$(ARCH_BITS).mk

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

clib:
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

check: clib vdso
	@ $(BUILD_ENV) cargo check $(CARGO_FLAGS)

clean:
	@ $(BUILD_ENV) make -C clib clean
	@ $(BUILD_ENV) make -C vdso clean
	@ $(BUILD_ENV) cargo clean

.PHONY: all clib vdso image check $(RUST_KERNEL)
