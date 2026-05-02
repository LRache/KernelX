ARCH ?= riscv64
ISA ?= $(ARCH)
GUEST_COMPONENTS ?= hello_sbi timer_tick

BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
HOST_OUTPUT_DIR = $(OUTPUT_DIR)/host
GUEST_OUTPUT_DIR = $(OUTPUT_DIR)/guest

.PHONY: all host guest guest-linux5.15 download-linux5.15 package image clean

all: host guest
	@ mkdir -p $(HOST_OUTPUT_DIR) $(GUEST_OUTPUT_DIR)
	@ cp -r host/build/$(ARCH)/output/* $(HOST_OUTPUT_DIR)/
	@ cp -r guest/build/$(ARCH)/output/* $(GUEST_OUTPUT_DIR)/

host:
	@ $(MAKE) -C host ARCH=$(ARCH) ISA=$(ISA) all

guest:
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) GUEST_COMPONENTS="$(GUEST_COMPONENTS)" all

guest-linux5.15:
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) linux5.15

download-linux5.15:
	@ $(MAKE) -C guest download-linux5.15

package: all
	@ KVM_SKIP_BUILD=1 ARCH=$(ARCH) ISA=$(ISA) ./package.sh

image: package

clean:
	@ $(MAKE) -C host ARCH=$(ARCH) ISA=$(ISA) clean
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) clean
	@ rm -rf $(BUILD_DIR)
