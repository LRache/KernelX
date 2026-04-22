include config/config.mk

IMAGE = build/$(ARCH)$(ARCH_BITS)/Image
VMKERNELX = build/$(ARCH)$(ARCH_BITS)/vmkernelx

TMPDISK_SIZE ?= 1G
TMPDISK      := $(shell mktemp /tmp/qemu-tmpdisk-XXXXXX)

# ---------------------------------------------------------------
# Per-arch QEMU binary / transport choice.
# RISC-V virt uses virtio-mmio; LoongArch virt exposes virtio-pci.
# ---------------------------------------------------------------
ifeq ($(ARCH),riscv)
QEMU = qemu-system-riscv64
QEMU_DEVICES += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_DEVICES += -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
QEMU_DEVICES += -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.2
else ifeq ($(ARCH),loongarch)
QEMU = qemu-system-loongarch64
QEMU_DEVICES += -device virtio-blk-pci,drive=x0
QEMU_DEVICES += -device virtio-blk-pci,drive=x1
QEMU_DEVICES += -device virtio-net-pci,netdev=net0
else
$(error Unsupported ARCH=$(ARCH) for QEMU target)
endif

QEMU_FLAGS += -M $(CONFIG_QEMU_MACHINE) -m $(CONFIG_QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(IMAGE)
QEMU_FLAGS += -drive file=$(CONFIG_DISK_IMAGE),if=none,id=x0,format=raw
QEMU_FLAGS += -drive file=$(TMPDISK),if=none,id=x1,format=raw
QEMU_FLAGS += $(QEMU_DEVICES)
QEMU_FLAGS += -netdev user,id=net0
QEMU_FLAGS += -smp $(CONFIG_QEMU_CPUS)

# LoongArch qemu 10.2+ does not accept `-bios none`; expose whatever Kconfig gave us.
ifneq ($(CONFIG_QEMU_BIOS),)
QEMU_FLAGS += -bios $(CONFIG_QEMU_BIOS)
endif

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

qemu-run-bt:
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
	python3 scripts/backtrace_run.py \
		--elf $(VMKERNELX) \
		--cross-compile $(CROSS_COMPILE) \
		-- $(QEMU) $(QEMU_FLAGS)
	@ rm -f $(TMPDISK)

qemu-gdb:
	@ truncate -s $(TMPDISK_SIZE) $(TMPDISK)
	$(QEMU) $(QEMU_FLAGS) -s -S
	@ rm -f $(TMPDISK)

qemu-dts:
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-$(ARCH)$(ARCH_BITS).dtb
	@ dtc -I dtb -O dts qemu-virt-$(ARCH)$(ARCH_BITS).dtb -o qemu-virt-$(ARCH)$(ARCH_BITS).dts

.PHONY: qemu-run qemu-gdb qemu-dts
