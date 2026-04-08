# Include configuration if it exists
CONFIG_FILE := config/.config
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

SYSROOT ?= $(CONFIG_SYSROOT)

# Log level control: trace, debug, info, warn, none
LOG_LEVEL ?= $(CONFIG_LOG_LEVEL)
LOG_LEVEL ?= trace

BIOS_FIRMWARE ?= $(CONFIG_BIOS_FIRMWARE)
BIOS_FIRMWARE ?= ./lib/opensbi/build/platform/generic/firmware/fw_jump.bin

KERNEL_CONFIG = \
	ARCH=$(ARCH) \
	ARCH_BITS=$(ARCH_BITS) \
	CROSS_COMPILE=$(CROSS_COMPILE) \
	KERNELX_RELEASE=$(KERNELX_RELEASE) \
	CONFIG_LOG_LEVEL=$(LOG_LEVEL) \
	CONFIG_LOG_SYSCALL=$(CONFIG_LOG_SYSCALL) \
	CONFIG_WARN_UNIMPLEMENTED_SYSCALL=$(CONFIG_WARN_UNIMPLEMENTED_SYSCALL) \
	CONFIG_ENABLE_SWAP_MEMORY=$(CONFIG_ENABLE_SWAP_MEMORY) \
	CONFIG_BACKTRACE=$(CONFIG_BACKTRACE) \
	CONFIG_DEADLOCK_DETECT=$(CONFIG_DEADLOCK_DETECT) \
	CONFIG_NO_SMP=$(CONFIG_NO_SMP) \
	CONFIG_NOLOCK=$(CONFIG_NOLOCK) \
	SYSROOT=$(SYSROOT) \
	COMPILE_MODE=$(COMPILE_MODE)

# Configuration targets
menuconfig:
	@if command -v kconfig-mconf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=config/.config kconfig-mconf config/Kconfig; \
	elif command -v menuconfig >/dev/null 2>&1; then \
		KCONFIG_CONFIG=config/.config menuconfig config/Kconfig; \
	elif python3 -c "import menuconfig" 2>/dev/null; then \
		KCONFIG_CONFIG=config/.config python3 -m menuconfig config/Kconfig; \
	else \
		echo "Error: menuconfig not found. Please install one of:"; \
		echo "  kconfig-frontends: sudo apt-get install kconfig-frontends"; \
		echo "  kconfiglib:        pip3 install kconfiglib"; \
		exit 1; \
	fi
