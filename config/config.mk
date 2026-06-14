# Include configuration if it exists
KCONFIG_FILE ?= config/Kconfig
KCONFIG_CONFIG ?= config/.config
DEFCONFIG ?= config/defconfig

CONFIG_FILE := $(KCONFIG_CONFIG)
-include $(CONFIG_FILE)

ifeq ($(origin ARCH),command line)
CONFIG_ARCH_SOURCE := $(ARCH)
else
CONFIG_ARCH_SOURCE := $(or $(CONFIG_ARCH),$(ARCH),riscv)
endif
CONFIG_ARCH_UNQUOTED := $(subst ",,$(CONFIG_ARCH_SOURCE))
EXPORT_CONFIG ?= config/$(CONFIG_ARCH_UNQUOTED)
IMPORT_CONFIG ?= $(EXPORT_CONFIG)

ARCH = $(CONFIG_ARCH)
ARCH_BITS = $(CONFIG_ARCH_BITS)

COMPILE_MODE ?= $(CONFIG_COMPILE_MODE)
COMPILE_MODE ?= debug

KERNELX_RELEASE ?= $(CONFIG_KERNELX_RELEASE)
KERNELX_RELEASE ?= 5.0

# Default values with Kconfig support
INITPATH ?= $(CONFIG_INITPATH)
INITPATH ?= /init

INITCWD ?= $(CONFIG_INITCWD)
INITCWD ?= /

CROSS_COMPILE ?= $(CONFIG_CROSS_COMPILE)
CROSS_COMPILE ?= riscv64-unknown-elf-

# Rust target triple, chosen per architecture via Kconfig.
RUST_TARGET ?= $(CONFIG_RUST_TARGET)
ifeq ($(RUST_TARGET),)
RUST_TARGET := riscv64gc-unknown-none-elf
endif

SYSROOT ?= $(CONFIG_SYSROOT)

# Log level control: trace, debug, info, warn, none
LOG_LEVEL ?= $(CONFIG_LOG_LEVEL)
LOG_LEVEL ?= trace

CONFIG_DEFAULT_BOOTARGS_UNQUOTED := $(subst ",,$(CONFIG_DEFAULT_BOOTARGS))
CONFIG_SECOND_DEVICE_UNQUOTED := $(subst ",,$(CONFIG_SECOND_DEVICE))
CONFIG_SECOND_FSTYPE_UNQUOTED := $(subst ",,$(CONFIG_SECOND_FSTYPE))
CONFIG_SECOND_FSTYPE_UNQUOTED := $(or $(CONFIG_SECOND_FSTYPE_UNQUOTED),ext4)
CONFIG_SECOND_MOUNTPOINT_UNQUOTED := $(subst ",,$(CONFIG_SECOND_MOUNTPOINT))
CONFIG_SECOND_MOUNTPOINT_UNQUOTED := $(or $(CONFIG_SECOND_MOUNTPOINT_UNQUOTED),/mnt)

BIOS_FIRMWARE ?= $(CONFIG_BIOS_FIRMWARE)
BIOS_FIRMWARE ?= ./lib/opensbi/build/platform/generic/firmware/fw_jump.bin

CONFIG_OBJCOPY ?= objcopy
CONFIG_AR ?= ar
CONFIG_READELF ?= readelf

ifeq ($(origin AR),default)
AR = $(CONFIG_AR)
endif

KERNEL_CONFIG = \
	ARCH=$(ARCH) \
	ARCH_BITS=$(ARCH_BITS) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	RUST_TARGET=$(RUST_TARGET) \
	KERNELX_RELEASE=$(KERNELX_RELEASE) \
	CONFIG_LOG_LEVEL=$(LOG_LEVEL) \
	CONFIG_LOG_SYSCALL=$(CONFIG_LOG_SYSCALL) \
	CONFIG_LOG_SYSCALL_CPU_TIME=$(CONFIG_LOG_SYSCALL_CPU_TIME) \
	CONFIG_WARN_UNIMPLEMENTED_SYSCALL=$(CONFIG_WARN_UNIMPLEMENTED_SYSCALL) \
	CONFIG_ENABLE_SWAP_MEMORY=$(CONFIG_ENABLE_SWAP_MEMORY) \
	CONFIG_KVM=$(CONFIG_KVM) \
	CONFIG_BACKTRACE=$(CONFIG_BACKTRACE) \
	CONFIG_LOCKDEP=$(CONFIG_LOCKDEP) \
	CONFIG_SPINLOCK_CHECK=$(CONFIG_SPINLOCK_CHECK) \
	CONFIG_ENABLE_WATCHDOG=$(CONFIG_ENABLE_WATCHDOG) \
	CONFIG_NO_SMP=$(CONFIG_NO_SMP) \
	CONFIG_NOLOCK=$(CONFIG_NOLOCK) \
	CONFIG_DEFAULT_BOOTARGS="$(CONFIG_DEFAULT_BOOTARGS_UNQUOTED)" \
	CONFIG_SECOND_DEVICE="$(CONFIG_SECOND_DEVICE_UNQUOTED)" \
	CONFIG_SECOND_FSTYPE="$(CONFIG_SECOND_FSTYPE_UNQUOTED)" \
	CONFIG_SECOND_MOUNTPOINT="$(CONFIG_SECOND_MOUNTPOINT_UNQUOTED)" \
	SYSROOT=$(SYSROOT) \
	COMPILE_MODE=$(COMPILE_MODE) \
	READELF=$(CONFIG_READELF) \
	AR=$(AR) \
	OBJCOPY=$(CONFIG_OBJCOPY)

# Configuration targets
defconfig:
	@if command -v kconfig-conf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) kconfig-conf --defconfig=$(DEFCONFIG) $(KCONFIG_FILE); \
	else \
		echo "Error: kconfig-conf not found. Please install one of:"; \
		echo "  kconfig-frontends: sudo apt-get install kconfig-frontends"; \
		exit 1; \
	fi

savedefconfig:
	@if command -v kconfig-conf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) kconfig-conf --savedefconfig=$(DEFCONFIG) $(KCONFIG_FILE); \
	else \
		echo "Error: kconfig-conf not found. Please install one of:"; \
		echo "  kconfig-frontends: sudo apt-get install kconfig-frontends"; \
		exit 1; \
	fi

exportconfig:
	@if command -v kconfig-conf >/dev/null 2>&1; then \
		mkdir -p $(dir $(EXPORT_CONFIG)); \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) kconfig-conf --savedefconfig=$(EXPORT_CONFIG) $(KCONFIG_FILE); \
		echo "Exported config to $(EXPORT_CONFIG)"; \
	else \
		echo "Error: kconfig-conf not found. Please install one of:"; \
		echo "  kconfig-frontends: sudo apt-get install kconfig-frontends"; \
		exit 1; \
	fi

importconfig:
	@if command -v kconfig-conf >/dev/null 2>&1; then \
		if [ ! -f $(IMPORT_CONFIG) ]; then \
			echo "Error: config file not found: $(IMPORT_CONFIG)"; \
			exit 1; \
		fi; \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) kconfig-conf --defconfig=$(IMPORT_CONFIG) $(KCONFIG_FILE); \
		echo "Imported config from $(IMPORT_CONFIG) to $(KCONFIG_CONFIG)"; \
	else \
		echo "Error: kconfig-conf not found. Please install one of:"; \
		echo "  kconfig-frontends: sudo apt-get install kconfig-frontends"; \
		exit 1; \
	fi

menuconfig:
	@if command -v kconfig-mconf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) kconfig-mconf $(KCONFIG_FILE); \
	elif command -v menuconfig >/dev/null 2>&1; then \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) menuconfig $(KCONFIG_FILE); \
	elif python3 -c "import menuconfig" 2>/dev/null; then \
		KCONFIG_CONFIG=$(KCONFIG_CONFIG) python3 -m menuconfig $(KCONFIG_FILE); \
	else \
		echo "Error: menuconfig not found. Please install one of:"; \
		echo "  kconfig-frontends: sudo apt-get install kconfig-frontends"; \
		echo "  kconfiglib:        pip3 install kconfiglib"; \
		exit 1; \
	fi
