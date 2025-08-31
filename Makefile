PLATFORM = qemu-virt-riscv64
ARCH = riscv
CROSS_COMPILE = riscv64-unknown-elf-
KERNELX_RELEASE ?= 5.0

# Log level control: trace, debug, info, warn, none
LOG_LEVEL ?= info

KERNEL = target/riscv64gc-unknown-none-elf/debug/kernelx

BIOS_FIRMWARE = ./lib/opensbi/build/platform/generic/firmware/fw_jump.bin

# DISK = ./tests/build/riscv64.ext4
DISK = ./sdcard-rv.img

QEMU_MACHINE = virt

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(QEMU_MACHINE) -m 256M -nographic
QEMU_FLAGS += -kernel $(KERNEL)
QEMU_FLAGS += -drive file=$(DISK),if=none,id=x0,format=raw 
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0

# Environment variables for builds
BUILD_ENV = PLATFORM=$(PLATFORM) ARCH=$(ARCH) CROSS_COMPILE=$(CROSS_COMPILE) KERNELX_INITPATH=$(INITPATH) KERNELX_RELEASE=$(KERNELX_RELEASE)

RUST_FEATURES += platform-$(PLATFORM)

# Configure log level features using a more elegant lookup
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
# RUST_FEATURES += log-trace-syscall

all: $(KERNEL)

init:
	@ git submodule init
	@ git submodule update --remote
	# @ make -C ./lib/opensbi CROSS_COMPILE=riscv64-linux-gnu- PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000 

$(KERNEL):
	@ $(BUILD_ENV) make -C ./c all
	$(BUILD_ENV) cargo build --features "$(RUST_FEATURES)"

run: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS)

check:
	@ $(BUILD_ENV) cargo check --target riscv64gc-unknown-none-elf --features "$(RUST_FEATURES)"

gdb: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS) -s -S

clean:
	@ cargo clean
	@ make -C ./c clean

count:
	@ find src c/src -type f -name "*.rs" -o -name "*.c" -o -name "*.h" | xargs wc -l

.PHONY: all init run gdb clean count check $(KERNEL)
