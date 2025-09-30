COMPILE_MODE ?= debug

KERNEL = target/$(RUST_TARGET)/$(COMPILE_MODE)/kernelx

BUILD_ENV = \
	PLATFORM=$(PLATFORM) \
	ARCH=$(ARCH) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	KERNELX_INITPATH=$(INITPATH) \
	KERNELX_INITCWD=$(INITCWD) \
	KERNELX_RELEASE=$(KERNELX_RELEASE)

RUST_TARGET = riscv64gc-unknown-none-elf

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

kernel: $(KERNEL)
	cp $(KERNEL) build/$(PLATFORM)/kernelx

$(KERNEL):
	@ $(BUILD_ENV) make -C clib all
	@ $(BUILD_ENV) cargo build --target $(RUST_TARGET) --features "$(RUST_FEATURES)"
	@ mkdir -p build/$(PLATFORM)
	@ cp $(KERNEL) build/$(PLATFORM)/kernelx

check:
	@ $(BUILD_ENV) cargo check --target $(RUST_TARGET) --features "$(RUST_FEATURES)"

.PHONY: $(KERNEL)
