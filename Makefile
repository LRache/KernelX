include config/config.mk

KERNEL = target/$(RUST_TARGET)/$(COMPILE_MODE)/kernelx

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(QEMU_MACHINE) -m $(QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(KERNEL)
QEMU_FLAGS += -drive file=$(DISK),if=none,id=x0,format=raw 
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -smp $(QEMU_CPUS)

BUILD_ENV = \
	PLATFORM=$(PLATFORM) \
	ARCH=$(ARCH) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	KERNELX_INITPATH=$(INITPATH) \
	KERNELX_INITPWD=$(INITPWD) \
	KERNELX_RELEASE=$(KERNELX_RELEASE)

RUST_TARGET=riscv64gc-unknown-none-elf

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

# RUST_FEATURES += log-trace-syscall

all: $(KERNEL)

init:
	@ git submodule init
	@ git submodule update --remote
	# @ make -C ./lib/opensbi CROSS_COMPILE=riscv64-linux-gnu- PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000 

$(KERNEL):
	@ $(BUILD_ENV) make -C clib all
	@ $(BUILD_ENV) cargo build --target $(RUST_TARGET) --features "$(RUST_FEATURES)"
	@ mkdir -p build/$(PLATFORM)
	@ cp $(KERNEL) build/$(PLATFORM)/kernelx

clean:
	@ cargo clean
	@ make -C ./clib clean

count:
	@ find src c/src -type f -name "*.rs" -o -name "*.c" -o -name "*.h" | xargs wc -l

.PHONY: $(KERNEL)

run: $(KERNEL)
	$(QEMU) $(QEMU_FLAGS)

qemu-dts:
	@ $(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-riscv64.dtb
	@ dtc -I dtb -O dts qemu-virt-riscv64.dtb -o qemu-virt-riscv64.dts

gdb: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS) -s -S

# Optional targets based on config
objcopy:
	@ $(CROSS_COMPILE)objcopy -O binary $(KERNEL) kernel.bin
	@ echo "Generated kernel.bin"

objdump:
	@ $(CROSS_COMPILE)objdump -d $(KERNEL) > kernel.asm
	@ echo "Generated kernel.asm"

.PHONY: all init run gdb clean count check menuconfig defconfig objcopy objdump help $(KERNEL)
