ARCH ?= riscv64
ISA ?= $(ARCH)
HOST_IMPL ?= host-rs

BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
HOST_OUTPUT_DIR = $(OUTPUT_DIR)/host
GUEST_OUTPUT_DIR = $(OUTPUT_DIR)/guest
HOST_BUILD_OUTPUT_DIR = $(HOST_IMPL)/build/$(ARCH)/output
GUEST_BUILD_OUTPUT_DIR = guest/build/$(ISA)/output

ALL_CASES := host guest/hello_sbi guest/timer_tick
CASES ?= host guest/hello_sbi guest/timer_tick
CASE_SOURCES_host := host host-rs
CASE_SOURCES_guest/hello_sbi := guest/hello_sbi.S
CASE_SOURCES_guest/timer_tick := guest/timer_tick.S
include ../mk/case-selection.mk

GUEST_COMPONENTS := $(patsubst guest/%,%,$(filter guest/%,$(CASES)))
SELECTED_TARGETS := $(if $(filter host,$(CASES)),host)
SELECTED_TARGETS += $(if $(GUEST_COMPONENTS),guest)

.PHONY: all host guest guest-linux6.1 download-linux6.1 package image clean

all: $(SELECTED_TARGETS)

host:
	@ $(MAKE) -C $(HOST_IMPL) ARCH=$(ARCH) ISA=$(ISA) all
	@ rm -rf $(HOST_OUTPUT_DIR)
	@ mkdir -p $(HOST_OUTPUT_DIR)
	@ cp -r $(HOST_BUILD_OUTPUT_DIR)/* $(HOST_OUTPUT_DIR)/

guest:
	@ $(MAKE) -C guest ARCH=$(ARCH) ISA=$(ISA) GUEST_COMPONENTS="$(GUEST_COMPONENTS)" all
	@ mkdir -p $(GUEST_OUTPUT_DIR)
	@ for component in $(GUEST_COMPONENTS); do \
		rm -rf "$(GUEST_OUTPUT_DIR)/$$component"; \
		cp -r "$(GUEST_BUILD_OUTPUT_DIR)/$$component" "$(GUEST_OUTPUT_DIR)/$$component"; \
	done

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
