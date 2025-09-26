include config/config.mk

KERNEL = build/$(PLATFORM)/kernelx

KERNEL_CONFIG = \
	PLATFORM=$(PLATFORM) \
	ARCH=$(ARCH) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	INITPATH=$(INITPATH) \
	INITCWD=$(INITCWD) \
	RELEASE=$(KERNELX_RELEASE)

all: run

init:
	@ git submodule init
	@ git submodule update --remote
	# @ make -C ./lib/opensbi CROSS_COMPILE=riscv64-linux-gnu- PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000

kernel:
	make -f build.mk kernel $(KERNEL_CONFIG)

run: kernel
	@ make -f scripts/qemu.mk qemu-run KERNEL=$(KERNEL)

clean:
	make -f build.mk clean

qemu-dts:
	@ make -f scripts/qemu.mk qemu-dts KERNEL=$(KERNEL)

gdb: kernel
	@ make -f scripts/qemu.mk qemu-gdb KERNEL=$(KERNEL)

objcopy:
	@ $(CROSS_COMPILE)objcopy -O binary $(KERNEL) kernel.bin
	@ echo "Generated kernel.bin"

objdump:
	@ $(CROSS_COMPILE)objdump -d $(KERNEL) > kernel.asm
	@ echo "Generated kernel.asm"

count:
	@ find src c/src -type f -name "*.rs" -o -name "*.c" -o -name "*.h" | xargs wc -l

.PHONY: all init run gdb clean count check menuconfig objcopy objdump kernel
