KERNEL = build/$(ARCH)$(ARCH_BITS)/vmkernelx
KERNEL_IMAGE = build/$(ARCH)$(ARCH_BITS)/Image

OBJDUMP ?= llvm-objdump-21

all: kernel

include config/config.mk

init:
	@ git submodule init
	@ git submodule update --remote

kernel:
	@ $(MAKE) -f build.mk kernel $(KERNEL_CONFIG)

image:
	@ $(MAKE) -f build.mk image $(KERNEL_CONFIG)

vdso:
	@ make -f build.mk vdso $(KERNEL_CONFIG)

clib:
	@ make -f build.mk clib $(KERNEL_CONFIG)

check:
	@ make -f build.mk check $(KERNEL_CONFIG)

run: kernel
	@ make -f scripts/qemu.mk qemu-run $(QEMU_ARGS)

run-bt: kernel
	@ make -f scripts/qemu.mk qemu-run-bt $(QEMU_ARGS)

clean:
	@ make -f build.mk clean $(KERNEL_CONFIG)

qemu-dts:
	@ make -f scripts/qemu.mk qemu-dts $(QEMU_ARGS)

gdb: kernel
	@ make -f scripts/qemu.mk qemu-gdb $(QEMU_ARGS)

objdump: kernel
	@ $(OBJDUMP) -d $(KERNEL) > kernel.asm
	@ echo "Generated kernel.asm"

package: kernel
	@ KERNEL_IMAGE=$(KERNEL_IMAGE) IMAGE=$(IMAGE) scripts/package.sh
	@ echo "Packaged image: $(IMAGE)"

count:
	@ find src clib/src -type f -name "*.rs" -o -name "*.c" -o -name "*.h" -o -name "*.S" | xargs wc -l

.PHONY: all init run run-bt gdb clean count check defconfig savedefconfig exportconfig importconfig menuconfig objdump kernel vdso clib
