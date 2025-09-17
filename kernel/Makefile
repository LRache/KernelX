PLATFORM ?= qemu-virt-riscv64
ARCH = riscv
CROSS_COMPILE = riscv64-unknown-elf-
RUST_TARGET = riscv64gc-unknown-none-elf
COMPILE_MODE ?= debug
LOG_LEVEL ?= trace

KERNELX_RELEASE ?= 5.0
INITPATH ?= /init
INITPWD ?= /

KERNEL = target/$(RUST_TARGET)/$(COMPILE_MODE)/kernelx

BUILD_ENV = \
	PLATFORM=$(PLATFORM) \
	ARCH=$(ARCH) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	KERNELX_INITPATH=$(INITPATH) \
	KERNELX_INITPWD=$(INITPWD) \
	KERNELX_RELEASE=$(KERNELX_RELEASE)

RUST_FEATURES += platform-$(PLATFORM)

# ------ Configure log level features using a more elegant lookup ------
LOG_FEATURES_trace = log-trace
LOG_FEATURES_debug = log-debug
LOG_FEATURES_info = log-info
LOG_FEATURES_warn = log-warn
LOG_FEATURES_syscall = log-trace-syscall
LOG_FEATURES_none = 

ifneq ($(LOG_FEATURES_$(LOG_LEVEL)),)
RUST_FEATURES += $(LOG_FEATURES_$(LOG_LEVEL))
else ifeq ($(LOG_LEVEL),none)
# none is valid, no features to add
else
$(warning Invalid LOG_LEVEL: $(LOG_LEVEL). Valid values: trace, debug, info, warn, syscall, none)
RUST_FEATURES += $(LOG_FEATURES_trace)
endif

RUST_FEATURES += log-trace-syscall

all: $(KERNEL)

$(KERNEL):
	@ $(BUILD_ENV) make -C ./clib all
	@ $(BUILD_ENV) cargo build --target $(RUST_TARGET) --features "$(RUST_FEATURES)"
	@ mkdir -p build/$(PLATFORM)
	@ cp $(KERNEL) build/$(PLATFORM)/kernelx

clean:
	@ cargo clean
	@ make -C ./clib clean

count:
	@ find src c/src -type f -name "*.rs" -o -name "*.c" -o -name "*.h" | xargs wc -l

.PHONY: $(KERNEL)
