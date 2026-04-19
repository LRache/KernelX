ARCH ?= riscv64
ISA ?= $(ARCH)

BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
HOST_OUTPUT_DIR = $(OUTPUT_DIR)/host
GUEST_OUTPUT_DIR = $(OUTPUT_DIR)/guest

.PHONY: all host guest clean

all: host guest
	@ mkdir -p $(HOST_OUTPUT_DIR) $(GUEST_OUTPUT_DIR)
	@ cp -r host/build/$(ARCH)/output/* $(HOST_OUTPUT_DIR)/
	@ cp -r guest/build/$(ARCH)/output/* $(GUEST_OUTPUT_DIR)/

host:
	@ $(MAKE) -C host ARCH=$(ARCH) ISA=$(ISA) all

guest:
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) all

clean:
	@ $(MAKE) -C host ARCH=$(ARCH) ISA=$(ISA) clean
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) clean
	@ rm -rf $(BUILD_DIR)
