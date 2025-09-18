include config/config.mk

KERNEL = kernel/build/$(PLATFORM)/kernelx

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(QEMU_MACHINE) -m $(QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(KERNEL)
QEMU_FLAGS += -drive file=$(DISK),if=none,id=x0,format=raw 
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -smp $(QEMU_CPUS)

ENV = \
	PLATFORM=$(PLATFORM) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	INITPATH=$(INITPATH) \
	INITPWD=$(INITPWD) \
	LOG_LEVEL=$(LOG_LEVEL) \
	COMPILE_MODE=$(COMPILE_MODE)

all: $(KERNEL)

init:
	@ git submodule init
	@ git submodule update --remote
	# @ make -C ./lib/opensbi CROSS_COMPILE=riscv64-linux-gnu- PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000 

$(KERNEL):
	@ make -C ./kernel all INITPATH=$(INITPATH) INITPWD=$(INITPWD) LOG_LEVEL=$(LOG_LEVEL) PLATFORM=$(PLATFORM)

run: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS)

gdb: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS) -s -S

# Optional targets based on config
objcopy:
	@ $(CROSS_COMPILE)objcopy -O binary $(KERNEL) kernel.bin
	@ echo "Generated kernel.bin"

objdump:
	@ $(CROSS_COMPILE)objdump -d $(KERNEL) > kernel.asm
	@ echo "Generated kernel.asm"

clean:
	@ make -C ./kernel clean

.PHONY: all init run gdb clean count check menuconfig defconfig objcopy objdump help $(KERNEL)
