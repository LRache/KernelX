ARCH ?= riscv64
ISA ?= $(ARCH)
GUEST_COMPONENTS ?= hello_sbi timer_tick
HOST_IMPL ?= host-rs

BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
HOST_OUTPUT_DIR = $(OUTPUT_DIR)/host
GUEST_OUTPUT_DIR = $(OUTPUT_DIR)/guest
HOST_BUILD_OUTPUT_DIR = $(HOST_IMPL)/build/$(ARCH)/output
GUEST_BUILD_OUTPUT_DIR = guest/build/$(ISA)/output

.PHONY: all host guest guest-linux6.1 download-linux6.1 package image clean

all: host guest
	@ rm -rf $(HOST_OUTPUT_DIR) $(GUEST_OUTPUT_DIR)
	@ mkdir -p $(HOST_OUTPUT_DIR) $(GUEST_OUTPUT_DIR)
	@ cp -r $(HOST_BUILD_OUTPUT_DIR)/* $(HOST_OUTPUT_DIR)/
	@ cp -r $(GUEST_BUILD_OUTPUT_DIR)/* $(GUEST_OUTPUT_DIR)/

host:
	@ $(MAKE) -C $(HOST_IMPL) ARCH=$(ARCH) ISA=$(ISA) all

guest:
	@ rm -rf $(GUEST_BUILD_OUTPUT_DIR)
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) GUEST_COMPONENTS="$(GUEST_COMPONENTS)" all

guest-linux6.1:
	@ $(MAKE) ARCH=$(ARCH) ISA=$(ISA) GUEST_COMPONENTS=linux6.1 guest

download-linux6.1:
	@ $(MAKE) -C guest download-linux6.1

package: all
	@ KVM_SKIP_BUILD=1 ARCH=$(ARCH) ISA=$(ISA) HOST_IMPL=$(HOST_IMPL) GUEST_COMPONENTS="$(GUEST_COMPONENTS)" ./package.sh

image: package

clean:
	@ $(MAKE) -C $(HOST_IMPL) ARCH=$(ARCH) ISA=$(ISA) clean
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) clean
	@ rm -rf $(BUILD_DIR)
