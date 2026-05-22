# Include configuration if it exists
KCONFIG_FILE ?= config/Kconfig
KCONFIG_CONFIG ?= config/.config
DEFCONFIG ?= config/defconfig

CONFIG_FILE := $(KCONFIG_CONFIG)
-include $(CONFIG_FILE)

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
	CONFIG_WARN_UNIMPLEMENTED_SYSCALL=$(CONFIG_WARN_UNIMPLEMENTED_SYSCALL) \
	CONFIG_ENABLE_SWAP_MEMORY=$(CONFIG_ENABLE_SWAP_MEMORY) \
	CONFIG_BACKTRACE=$(CONFIG_BACKTRACE) \
	CONFIG_DEADLOCK_DETECT=$(CONFIG_DEADLOCK_DETECT) \
	CONFIG_NO_SMP=$(CONFIG_NO_SMP) \
	CONFIG_NOLOCK=$(CONFIG_NOLOCK) \
	CONFIG_DEFAULT_BOOTARGS="$(CONFIG_DEFAULT_BOOTARGS_UNQUOTED)" \
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
