include config/config.mk

# Kconfig stores `string` options with surrounding double quotes, e.g.
# CONFIG_ARCH="riscv". That reaches us here as $(ARCH)=="riscv" (with the
# quotes), which trips up `ifeq ($(ARCH),riscv)`, `ifneq ($(VAR),)`
# (quoted empty string is non-empty!), and `build/$(ARCH)$(ARCH_BITS)/...`
# path interpolation. Strip every quote character once, locally.
# (`config/config.mk` is left as-is so we don't perturb other consumers
# that already tolerate the quoted form — cargo / CMake pass the quoted
# value to shells which unquote naturally.)
ARCH             := $(subst ",,$(ARCH))
ARCH_BITS        := $(subst ",,$(ARCH_BITS))
CONFIG_QEMU_BIOS := $(subst ",,$(CONFIG_QEMU_BIOS))
qemu_unquote = $(subst ",,$(1))

IMAGE = build/$(ARCH)$(ARCH_BITS)/Image
VMKERNELX = build/$(ARCH)$(ARCH_BITS)/vmkernelx

TMPDISK_SIZE ?= 1G
TMPDISK      := $(shell mktemp /tmp/qemu-tmpdisk-XXXXXX)
SECOND_DISK_IMAGE := $(subst ",,$(CONFIG_SECOND_DISK_IMAGE))
SECOND_DISK := $(if $(SECOND_DISK_IMAGE),$(SECOND_DISK_IMAGE),$(TMPDISK))
QEMU_DISK_OPTIONS :=
ifeq ($(CONFIG_QEMU_SNAPSHOT),y)
QEMU_DISK_OPTIONS += ,snapshot=on
endif

# ---------------------------------------------------------------
# Per-arch QEMU binary / transport choice.
# RISC-V virt uses virtio-mmio + loads the kernel as a raw binary.
# LoongArch virt exposes virtio-pci + requires the ELF directly
# (QEMU's LoongArch loader reads the ELF program headers to decide
# where to place each segment).
# ---------------------------------------------------------------
ifeq ($(ARCH),riscv)
QEMU = qemu-system-riscv64
QEMU_KERNEL = $(IMAGE)
QEMU_DEVICES += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_DEVICES += -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
QEMU_DEVICES += -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.2
else ifeq ($(ARCH),loongarch)
QEMU = qemu-system-loongarch64
QEMU_KERNEL = $(VMKERNELX)
QEMU_DEVICES += -device virtio-blk-pci,drive=x0
QEMU_DEVICES += -device virtio-blk-pci,drive=x1
QEMU_DEVICES += -device virtio-net-pci,netdev=net0
else
$(error Unsupported ARCH=$(ARCH) for QEMU target)
endif

QEMU_FLAGS += -M $(CONFIG_QEMU_MACHINE) -m $(CONFIG_QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(QEMU_KERNEL)
QEMU_FLAGS += -drive file=$(CONFIG_DISK_IMAGE),if=none,id=x0,format=raw$(QEMU_DISK_OPTIONS)
QEMU_FLAGS += -drive file=$(SECOND_DISK),if=none,id=x1,format=raw$(QEMU_DISK_OPTIONS)
QEMU_FLAGS += $(QEMU_DEVICES)
QEMU_FLAGS += -netdev user,id=net0
QEMU_FLAGS += -smp $(CONFIG_QEMU_CPUS)

# LoongArch: omit -bios entirely so QEMU writes the aux_boot_code shim into
# flash0 and BSP jumps to our ELF entry. Passing anything (even /dev/null)
# loads it as firmware and skips the shim, leaving flash0 filled with zeros
# so the CPU executes `illegal` right out of reset.
#
# RISC-V: honor CONFIG_QEMU_BIOS if set (typically empty → QEMU uses its
# built-in OpenSBI).
ifneq ($(CONFIG_QEMU_BIOS),)
QEMU_FLAGS += -bios $(CONFIG_QEMU_BIOS)
endif

CONFIG_BOOTARGS_UNQUOTED    := $(call qemu_unquote,$(CONFIG_BOOTARGS))
CONFIG_INITPATH_UNQUOTED    := $(call qemu_unquote,$(CONFIG_INITPATH))
CONFIG_INITARGS_UNQUOTED    := $(call qemu_unquote,$(CONFIG_INITARGS))
CONFIG_INITCWD_UNQUOTED     := $(call qemu_unquote,$(CONFIG_INITCWD))
CONFIG_ROOT_DEVICE_UNQUOTED := $(call qemu_unquote,$(CONFIG_ROOT_DEVICE))
CONFIG_ROOT_FSTYPE_UNQUOTED := $(call qemu_unquote,$(CONFIG_ROOT_FSTYPE))

BOOTARGS += $(CONFIG_BOOTARGS_UNQUOTED)

# Set bootargs
ifneq ($(CONFIG_INITPATH_UNQUOTED),)
BOOTARGS += init="$(CONFIG_INITPATH_UNQUOTED)"
endif

ifneq ($(CONFIG_INITARGS_UNQUOTED),)
BOOTARGS += initargs="$(CONFIG_INITARGS_UNQUOTED)"
endif

ifneq ($(CONFIG_INITCWD_UNQUOTED),)
BOOTARGS += initcwd="$(CONFIG_INITCWD_UNQUOTED)"
endif

ifneq ($(CONFIG_ROOT_DEVICE_UNQUOTED),)
BOOTARGS += root="$(CONFIG_ROOT_DEVICE_UNQUOTED)"
endif

ifneq ($(CONFIG_ROOT_FSTYPE_UNQUOTED),)
BOOTARGS += rootfstype="$(CONFIG_ROOT_FSTYPE_UNQUOTED)"
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
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-$(ARCH)$(ARCH_BITS).dtb
	@ dtc -I dtb -O dts qemu-virt-$(ARCH)$(ARCH_BITS).dtb -o qemu-virt-$(ARCH)$(ARCH_BITS).dts

.PHONY: qemu-run qemu-gdb qemu-dts
