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

QPERF_DIR ?= tools/qperf
QPERF_PLUGIN ?= $(QPERF_DIR)/target/release/libqperf.so
QPERF_ANALYZER_MANIFEST ?= $(QPERF_DIR)/analyzer/Cargo.toml
QPERF_RUN_TIMESTAMP ?= $(shell date +%Y%m%d-%H%M%S)
QPERF_FREQ ?= 99
QPERF_OUT ?= build/$(ARCH)$(ARCH_BITS)/qperf.bin
QPERF_OUT_DIR := $(dir $(QPERF_OUT))
QPERF_FOLDED_OUTPUT_DIR ?= output/qperf
QPERF_FOLDED ?= $(QPERF_FOLDED_OUTPUT_DIR)/kernelx-qperf-$(QPERF_RUN_TIMESTAMP).folded
QPERF_FOLDED_DIR := $(dir $(QPERF_FOLDED))
QPERF_SVG ?= $(basename $(QPERF_FOLDED)).svg
QPERF_SVG_DIR := $(dir $(QPERF_SVG))
QPERF_CONSOLE_LOG ?= $(QPERF_FOLDED_OUTPUT_DIR)/kernelx-qperf-$(QPERF_RUN_TIMESTAMP).console.log
QPERF_CONSOLE_LOG_DIR := $(dir $(QPERF_CONSOLE_LOG))
QPERF_FLAMEGRAPH ?= tools/FlameGraph/flamegraph.pl
QPERF_FLAGS = -plugin file=$(QPERF_PLUGIN),freq=$(QPERF_FREQ),out=$(QPERF_OUT)

TMPDISK_SIZE ?= 1G
TMPDISK      := $(shell mktemp /tmp/qemu-tmpdisk-XXXXXX)
SECOND_DISK_IMAGE := $(subst ",,$(CONFIG_SECOND_DISK_IMAGE))
SECOND_DISK := $(if $(SECOND_DISK_IMAGE),$(SECOND_DISK_IMAGE),$(TMPDISK))
CONFIG_QEMU_DEBUG_CONSOLE_DEVICE := $(call qemu_unquote,$(CONFIG_QEMU_DEBUG_CONSOLE_DEVICE))
CONFIG_QEMU_DEBUG_CONSOLE_LOG := $(call qemu_unquote,$(CONFIG_QEMU_DEBUG_CONSOLE_LOG))
QEMU_DEBUG_CONSOLE_DEVICE ?= $(or $(CONFIG_QEMU_DEBUG_CONSOLE_DEVICE),/dev/hvc0)
QEMU_DEBUG_CONSOLE_LOG ?= $(or $(CONFIG_QEMU_DEBUG_CONSOLE_LOG),build/$(ARCH)$(ARCH_BITS)/debug-console.log)
QEMU_DEBUG_CONSOLE_CHARDEV := kdebug0
QEMU_DEBUG_CONSOLE_LOG_DIR := $(dir $(QEMU_DEBUG_CONSOLE_LOG))
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
QEMU_DEVICES += -device virtio-serial-device,bus=virtio-mmio-bus.3
QEMU_DEVICES += -device virtconsole,chardev=$(QEMU_DEBUG_CONSOLE_CHARDEV)
else ifeq ($(ARCH),loongarch)
QEMU = qemu-system-loongarch64
QEMU_KERNEL = $(VMKERNELX)
QEMU_DEVICES += -device virtio-blk-pci,drive=x0
QEMU_DEVICES += -device virtio-blk-pci,drive=x1
QEMU_DEVICES += -device virtio-net-pci,netdev=net0
QEMU_DEVICES += -device virtio-serial-pci
QEMU_DEVICES += -device virtconsole,chardev=$(QEMU_DEBUG_CONSOLE_CHARDEV)
else
$(error Unsupported ARCH=$(ARCH) for QEMU target)
endif

QEMU_FLAGS += -M $(CONFIG_QEMU_MACHINE) -m $(CONFIG_QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(QEMU_KERNEL)
QEMU_FLAGS += -drive file=$(CONFIG_DISK_IMAGE),if=none,id=x0,format=raw$(QEMU_DISK_OPTIONS)
QEMU_FLAGS += -drive file=$(SECOND_DISK),if=none,id=x1,format=raw$(QEMU_DISK_OPTIONS)
QEMU_FLAGS += -chardev file,id=$(QEMU_DEBUG_CONSOLE_CHARDEV),path=$(QEMU_DEBUG_CONSOLE_LOG),append=off
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

ifneq ($(QEMU_DEBUG_CONSOLE_DEVICE),)
BOOTARGS += kdebug_console="$(QEMU_DEBUG_CONSOLE_DEVICE)"
endif
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
	@ mkdir -p $(QEMU_DEBUG_CONSOLE_LOG_DIR)
	$(QEMU) $(QEMU_FLAGS)
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

qperf-plugin:
	@test -f $(QPERF_DIR)/Cargo.toml || { \
		echo "Missing $(QPERF_DIR). Run: git submodule update --init tools/qperf"; \
		exit 1; \
	}
	cargo build --release --manifest-path $(QPERF_DIR)/Cargo.toml

qperf-analyzer:
	@test -f $(QPERF_ANALYZER_MANIFEST) || { \
		echo "Missing $(QPERF_ANALYZER_MANIFEST). Run: git submodule update --init tools/qperf"; \
		exit 1; \
	}
	cargo build --release --manifest-path $(QPERF_ANALYZER_MANIFEST)

qperf-flamegraph:
	@test -f $(QPERF_FLAMEGRAPH) || { \
		echo "Missing $(QPERF_FLAMEGRAPH). Run: git submodule update --init tools/FlameGraph"; \
		exit 1; \
	}

qemu-run-qperf: QEMU_DEBUG_CONSOLE_LOG = $(QPERF_CONSOLE_LOG)
qemu-run-qperf: qperf-plugin qperf-analyzer qperf-flamegraph
ifeq ($(SECOND_DISK_IMAGE),)
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
endif
	@ mkdir -p $(QEMU_DEBUG_CONSOLE_LOG_DIR) $(QPERF_OUT_DIR) $(QPERF_FOLDED_DIR) $(QPERF_SVG_DIR) $(QPERF_CONSOLE_LOG_DIR)
	$(QEMU) $(QEMU_FLAGS) $(QPERF_FLAGS)
	@ cargo run --release --manifest-path $(QPERF_ANALYZER_MANIFEST) -- --elf $(VMKERNELX) $(QPERF_OUT) $(QPERF_FOLDED)
	@ $(QPERF_FLAMEGRAPH) --title "KernelX qperf $(QPERF_RUN_TIMESTAMP)" $(QPERF_FOLDED) > $(QPERF_SVG)
	@ echo "QPerf folded output: $(QPERF_FOLDED)"
	@ echo "QPerf SVG output: $(QPERF_SVG)"
	@ echo "QPerf console log: $(QPERF_CONSOLE_LOG)"
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

qemu-run-bt:
ifeq ($(SECOND_DISK_IMAGE),)
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
endif
	@ mkdir -p $(QEMU_DEBUG_CONSOLE_LOG_DIR)
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
	@ mkdir -p $(QEMU_DEBUG_CONSOLE_LOG_DIR)
	$(QEMU) $(QEMU_FLAGS) -s -S
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

qemu-dts:
	@ mkdir -p $(QEMU_DEBUG_CONSOLE_LOG_DIR)
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-$(ARCH)$(ARCH_BITS).dtb
	@ dtc -I dtb -O dts qemu-virt-$(ARCH)$(ARCH_BITS).dtb -o qemu-virt-$(ARCH)$(ARCH_BITS).dts

.PHONY: qemu-run qperf-plugin qperf-analyzer qperf-flamegraph qemu-run-qperf qemu-gdb qemu-dts
