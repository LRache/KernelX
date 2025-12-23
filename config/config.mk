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
	COMPILE_MODE=$(COMPILE_MODE)

# Configuration targets
menuconfig:
	@if command -v kconfig-mconf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=config/.config kconfig-mconf config/Kconfig; \
	elif command -v menuconfig >/dev/null 2>&1; then \
		KCONFIG_CONFIG=config/.config menuconfig config/Kconfig; \
	else \
		echo "Error: menuconfig not found. Please install kconfig-frontends:"; \
		echo "  Ubuntu/Debian: sudo apt-get install kconfig-frontends"; \
		exit 1; \
	fi
