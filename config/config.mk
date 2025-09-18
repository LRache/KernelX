# Include configuration if it exists
CONFIG_FILE := config/.config
-include $(CONFIG_FILE)

# Default values with Kconfig support
# INITPATH ?= $(CONFIG_INITPATH)
INITPATH ?= /init

# INITPWD ?= $(CONFIG_INITPWD)
INITPWD ?= /

PLATFORM ?= $(CONFIG_PLATFORM)
PLATFORM ?= qemu-virt-riscv64

CROSS_COMPILE ?= $(CONFIG_CROSS_COMPILE)
CROSS_COMPILE ?= riscv64-unknown-elf-

# Log level control: trace, debug, info, warn, none
# LOG_LEVEL ?= $(CONFIG_LOG_LEVEL)
LOG_LEVEL ?= trace

# QEMU Configuration
QEMU_MACHINE ?= $(CONFIG_QEMU_MACHINE)
QEMU_MACHINE ?= virt

QEMU_MEMORY ?= $(CONFIG_QEMU_MEMORY)
QEMU_MEMORY ?= 256M

QEMU_CPUS ?= $(CONFIG_QEMU_CPUS)
QEMU_CPUS ?= 1

DISK ?= $(CONFIG_DISK_IMAGE)
DISK ?= ./sdcard-rv.img

BIOS_FIRMWARE ?= $(CONFIG_BIOS_FIRMWARE)
BIOS_FIRMWARE ?= ./lib/opensbi/build/platform/generic/firmware/fw_jump.bin

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

defconfig:
	@if command -v kconfig-conf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=config/.config kconfig-conf --defconfig=config/defconfig config/Kconfig; \
	elif command -v conf >/dev/null 2>&1; then \
		KCONFIG_CONFIG=config/.config conf --defconfig=config/defconfig config/Kconfig; \
	else \
		echo "Creating default config manually..."; \
		echo "# Default KernelX Configuration" > config/.config; \
		echo "CONFIG_PLATFORM=\"qemu-virt-riscv64\"" >> config/.config; \
		echo "CONFIG_COMPILE_MODE=\"debug\"" >> config/.config; \
		echo "CONFIG_LOG_LEVEL=\"trace\"" >> config/.config; \
		echo "CONFIG_INITPATH=\"/init\"" >> config/.config; \
		echo "CONFIG_INITPWD=\"/\"" >> config/.config; \
		echo "CONFIG_ENABLE_GDB=y" >> config/.config; \
	fi
