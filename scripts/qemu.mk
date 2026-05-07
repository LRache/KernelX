include config/config.mk

IMAGE = build/$(ARCH)$(ARCH_BITS)/Image
VMKERNELX = build/$(ARCH)$(ARCH_BITS)/vmkernelx

TMPDISK_SIZE ?= 1G
TMPDISK      := $(shell mktemp /tmp/qemu-tmpdisk-XXXXXX)
SECOND_DISK_IMAGE := $(subst ",,$(CONFIG_SECOND_DISK_IMAGE))
SECOND_DISK := $(if $(SECOND_DISK_IMAGE),$(SECOND_DISK_IMAGE),$(TMPDISK))

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(CONFIG_QEMU_MACHINE) -m $(CONFIG_QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(IMAGE)
QEMU_FLAGS += -drive file=$(CONFIG_DISK_IMAGE),if=none,id=x0,format=raw
QEMU_FLAGS += -drive file=$(SECOND_DISK),if=none,id=x1,format=raw
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
QEMU_FLAGS += -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.2
QEMU_FLAGS += -netdev user,id=net0
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

ifneq ($(CONFIG_ROOT_FS_TYPE),)
BOOTARGS += rootfstype=$(CONFIG_ROOT_FS_TYPE)
endif

QEMU_FLAGS += -append '$(BOOTARGS)'

qemu-run:
ifeq ($(SECOND_DISK_IMAGE),)
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
endif
	$(QEMU) $(QEMU_FLAGS)
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

qemu-run-bt:
ifeq ($(SECOND_DISK_IMAGE),)
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
endif
	python3 scripts/backtrace_run.py \
		--elf $(VMKERNELX) \
		--cross-compile $(CROSS_COMPILE) \
		-- $(QEMU) $(QEMU_FLAGS)
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

qemu-gdb:
ifeq ($(SECOND_DISK_IMAGE),)
	@ truncate -s $(TMPDISK_SIZE) $(TMPDISK)
endif
	$(QEMU) $(QEMU_FLAGS) -s -S
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

qemu-dts:
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-riscv64.dtb
	@ dtc -I dtb -O dts qemu-virt-riscv64.dtb -o qemu-virt-riscv64.dts

.PHONY: qemu-run qemu-gdb qemu-dts
