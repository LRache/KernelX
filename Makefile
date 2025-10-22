KERNEL = build/$(PLATFORM)/vmkernelx
KERNEL_BIN = build/$(PLATFORM)/kernel.bin

all: kernel

include config/config.mk

init:
	@ git submodule init
	@ git submodule update --remote
	# @ make -C ./lib/opensbi CROSS_COMPILE=riscv64-linux-gnu- PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000

kernel:
	@ $(MAKE) -f build.mk kernel $(KERNEL_CONFIG)

vdso:
	@ make -f build.mk vdso $(KERNEL_CONFIG)

clib:
	@ make -f build.mk clib $(KERNEL_CONFIG)

check:
	@ make -f build.mk check $(KERNEL_CONFIG)

run: kernel
	@ make -f scripts/qemu.mk qemu-run KERNEL=$(KERNEL)

clean:
	@ make -f build.mk clean

qemu-dts:
	@ make -f scripts/qemu.mk qemu-dts KERNEL=$(KERNEL)

gdb: kernel
	@ make -f scripts/qemu.mk qemu-gdb KERNEL=$(KERNEL)

objdump: kernel
	@ $(CROSS_COMPILE)objdump -d $(KERNEL) > kernel.asm
	@ echo "Generated kernel.asm"

$(KERNEL_BIN): kernel
	@ $(CROSS_COMPILE)objcopy -O binary $(KERNEL) $(KERNEL_BIN)
	@ echo "Generated $(KERNEL_BIN)"

package: $(KERNEL_BIN)
	@ KERNEL_IMAGE=$(KERNEL_BIN) IMAGE=$(IMAGE) scripts/package.sh
	@ echo "Packaged image: $(IMAGE)"

count:
	@ find src c/src -type f -name "*.rs" -o -name "*.c" -o -name "*.h" | xargs wc -l

.PHONY: all init run gdb clean count check menuconfig objdump kernel vdso clib
