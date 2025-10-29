COMPILE_MODE ?= debug

KERNELX_HOME := $(strip $(patsubst %/, %, $(dir $(abspath $(lastword $(MAKEFILE_LIST))))))

BUILD = $(abspath build/$(PLATFORM))
KERNEL_VM = $(BUILD)/vmkernelx
KERNEL_IMAGE = $(BUILD)/Image

CLIB = clib/build/$(ARCH)$(ARCH_BITS)/libkernelx_clib.a
VDSO = vdso/build/$(ARCH)$(ARCH_BITS)/vdso.o

BUILD_ENV = \
	PLATFORM=$(PLATFORM) \
	ARCH=$(ARCH) \
	ARCH_BITS=$(ARCH_BITS) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	KERNELX_INITPATH=$(INITPATH) \
	KERNELX_INITCWD=$(INITCWD) \
	KERNELX_RELEASE=$(KERNELX_RELEASE) \
	KERNELX_HOME=$(KERNELX_HOME)

RUST_TARGET = riscv64gc-unknown-none-elf
RUST_TARGET_DIR ?= $(abspath target/$(RUST_TARGET)/$(COMPILE_MODE))
RUST_KERNEL ?= $(RUST_TARGET_DIR)/kernelx
RUST_DEPENDENCIES = $(RUST_TARGET_DIR)/kernelx.d

RUST_FEATURES += platform-$(PLATFORM)

# ------ Configure log level features using a more elegant lookup ------ #
LOG_FEATURES_trace = log-trace
LOG_FEATURES_debug = log-debug
LOG_FEATURES_info = log-info
LOG_FEATURES_warn = log-warn

ifeq ($(LOG_LEVEL),)
RUST_FEATURES += log-info
else ifneq ($(LOG_FEATURES_$(LOG_LEVEL)),)
RUST_FEATURES += $(LOG_FEATURES_$(LOG_LEVEL))
else
$(warning Invalid LOG_LEVEL: $(LOG_LEVEL). Valid values: trace, debug, info, warn)
endif
# ------ Configure log level features using a more elegant lookup ------ #

ifeq ($(LOG_SYSCALL),y)
RUST_FEATURES += log-trace-syscall
endif

ifeq ($(WARN_UNIMPLEMENTED_SYSCALL),y)
RUST_FEATURES += warn-unimplemented-syscall
endif

all: kernel

kernel: $(RUST_KERNEL)
	@ mkdir -p $(BUILD)
	@ cp $(RUST_KERNEL) $(KERNEL_VM)
	@ $(CROSS_COMPILE)objcopy -O binary $(RUST_KERNEL) $(KERNEL_IMAGE)

$(KERNEL_VM): $(RUST_KERNEL)
	@ mkdir -p $(BUILD)
	@ cp $(RUST_KERNEL) $(KERNEL_VM)

$(KERNEL_IMAGE): $(RUST_KERNEL)
	@ mkdir -p $(BUILD)
	@ $(CROSS_COMPILE)objcopy -O binary $(RUST_KERNEL) $(KERNEL_IMAGE)

clib: $(CLIB)

$(CLIB):
	@ echo $(KERNELX_HOME)
	@ $(BUILD_ENV) make -C clib all

vdso: $(VDSO)

$(VDSO):
	@ $(BUILD_ENV) make -C vdso all

$(RUST_KERNEL): $(CLIB) $(VDSO)
	$(BUILD_ENV) cargo build --target $(RUST_TARGET) --features "$(RUST_FEATURES)"

check:
	@ $(BUILD_ENV) cargo check --target $(RUST_TARGET) --features "$(RUST_FEATURES)"

objcopy:
	@ $(CROSS_COMPILE)objcopy -O binary $(KERNEL) build/$(PLATFORM)/kernel.bin
	@ echo "Generated kernel.bin"

clean:
	@ $(BUILD_ENV) make -C clib clean 
	@ $(BUILD_ENV) cargo clean

.PHONY: all $(CLIB) $(VDSO) $(RUST_KERNEL)
