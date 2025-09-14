INITPATH ?= /init
INITPWD ?= /
PLATFORM ?= qemu-virt-riscv64
CROSS_COMPILE = riscv64-unknown-elf-

# Log level control: trace, debug, info, warn, none
LOG_LEVEL ?= trace

KERNEL = kernel/build/$(PLATFORM)/kernelx

BIOS_FIRMWARE = ./lib/opensbi/build/platform/generic/firmware/fw_jump.bin

# DISK = ./tests/build/riscv64.ext4
DISK = ./sdcard-rv.img

QEMU_MACHINE = virt

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(QEMU_MACHINE) -m 256M -nographic
QEMU_FLAGS += -kernel $(KERNEL)
QEMU_FLAGS += -drive file=$(DISK),if=none,id=x0,format=raw 
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -smp 1

all: $(KERNEL)

init:
	@ git submodule init
	@ git submodule update --remote
	# @ make -C ./lib/opensbi CROSS_COMPILE=riscv64-linux-gnu- PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000 

$(KERNEL):
	@ make -C ./kernel all INITPATH=$(INITPATH) INITPWD=$(INITPWD) LOG_LEVEL=$(LOG_LEVEL) PLATFORM=$(PLATFORM)

run: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS)

objcopy:
	@ $(CROSS_COMPILE)objcopy -O binary $(KERNEL) kernel.bin
	@ echo "Generated kernel.bin"

objdump:
	@ $(CROSS_COMPILE)objdump -d $(KERNEL) > kernel.asm
	@ echo "Generated kernel.asm"

gdb: $(KERNEL)
	@ $(QEMU) $(QEMU_FLAGS) -s -S

clean:
	@ make -C ./kernel clean

.PHONY: all init run gdb clean count check $(KERNEL)
