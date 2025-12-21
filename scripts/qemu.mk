include config/config.mk

IMAGE = build/$(ARCH)$(ARCH_BITS)/Image
VMKERNELX = build/$(ARCH)$(ARCH_BITS)/vmkernelx

TMPDISK_SIZE ?= 1G
TMPDISK      := $(shell mktemp /tmp/qemu-tmpdisk-XXXXXX)

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(CONFIG_QEMU_MACHINE) -m $(CONFIG_QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(IMAGE)
QEMU_FLAGS += -drive file=$(CONFIG_DISK_IMAGE),if=none,id=x0,format=raw
QEMU_FLAGS += -drive file=$(TMPDISK),if=none,id=x1,format=raw
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
QEMU_FLAGS += -smp $(CONFIG_QEMU_CPUS)

BOOTARGS += $(CONFIG_BOOTARGS)

# Set bootargs
ifneq ($(CONFIG_INITPATH),)
BOOTARGS += init=$(CONFIG_INITPATH)
endif

ifneq ($(CONFIG_INITARGS),)
BOOTARGS += initargs=$(CONFIG_INITARGS)
endif

ifneq ($(CONFIG_INITCWD),)
BOOTARGS += initcwd=$(CONFIG_INITCWD)
endif

ifneq ($(CONFIG_ROOT_DEVICE),)
BOOTARGS += root=$(CONFIG_ROOT_DEVICE)
endif

ifneq ($(CONFIG_ROOT_FSTYPE),)
BOOTARGS += rootfstype=$(CONFIG_ROOT_FSTYPE)
endif

QEMU_FLAGS += -append "$(BOOTARGS)"

qemu-run:
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
	$(QEMU) $(QEMU_FLAGS)
	@ rm -f $(TMPDISK)

qemu-gdb:
	@ truncate -s $(TMPDISK_SIZE) $(TMPDISK)
	$(QEMU) $(QEMU_FLAGS) -s -S
	@ rm -f $(TMPDISK)

qemu-dts:
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-riscv64.dtb
	@ dtc -I dtb -O dts qemu-virt-riscv64.dtb -o qemu-virt-riscv64.dts

.PHONY: qemu-run qemu-gdb qemu-dts
