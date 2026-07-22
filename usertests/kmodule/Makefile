ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-unknown-linux-gnu-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
REPO_ROOT = ../..
KMODULE_SDK_DIR = $(REPO_ROOT)/kmodule
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
KMODULE_CMAKE_BUILD_DIR = $(abspath $(BUILD_DIR)/cmake)
KMODULE_OUTPUT_DIR = $(abspath $(BUILD_DIR)/kmodule-output)
KMODULE_CMAKE_STAMP = $(KMODULE_CMAKE_BUILD_DIR)/CMakeCache.txt

KMODULE_ARCH_riscv = riscv
KMODULE_ARCH_riscv64 = riscv
KMODULE_ARCH_loongarch = loongarch
KMODULE_ARCH_loongarch64 = loongarch
KMODULE_ARCH = $(KMODULE_ARCH_$(ARCH))
KMODULE_CC ?= $(CC)

ifeq ($(KMODULE_ARCH),)
$(error unsupported kmodule test ARCH=$(ARCH))
endif

CFLAGS += -Wall -Wextra

TEST_DIRS = $(sort $(dir $(wildcard $(SRC_DIR)/*/loader.c)))
ALL_CASES := $(notdir $(patsubst %/,%,$(TEST_DIRS)))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case)/loader.c $(case)/module.c $(case)/CMakeLists.txt CMakeLists.txt))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(case)))
include ../mk/case-selection.mk

TESTS = $(CASES)
TEST_BINS = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test))
TEST_MODULES = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test).ko)

all: $(TEST_BINS) $(TEST_MODULES)

$(BUILD_DIR)/%/loader.o: $(SRC_DIR)/%/loader.c
	$(info + CC $<)
	@ mkdir -p $(@D)
	@ $(CC) $(CFLAGS) -DMODULE_PATH=\"/tests/kmodule/$*.ko\" -c $< -o $@

define TEST_PROGRAM_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(BUILD_DIR)/$(1)/loader.o
	$$(info + LD $$<)
	@ mkdir -p $$(@D)
	@ $$(CC) $$(CFLAGS) $$< -o $$@
endef

$(foreach test,$(TESTS),$(eval $(call TEST_PROGRAM_RULE,$(test))))

$(KMODULE_CMAKE_STAMP): CMakeLists.txt $(wildcard $(SRC_DIR)/*/CMakeLists.txt)
	$(info + CMAKE kmodule tests)
	@ mkdir -p $(KMODULE_CMAKE_BUILD_DIR) $(KMODULE_OUTPUT_DIR)
	@ cmake -S $(KMODULE_SDK_DIR) -B $(KMODULE_CMAKE_BUILD_DIR) \
		-DCMAKE_C_COMPILER=$(KMODULE_CC) \
		-DKMODULE_ARCH=$(KMODULE_ARCH) \
		-DKMODULE_IMPLS_DIR=$(abspath $(SRC_DIR)) \
		-DKMODULE_OUTPUT_DIR=$(KMODULE_OUTPUT_DIR)

define TEST_MODULE_RULE
$(KMODULE_OUTPUT_DIR)/$(1).ko: $(KMODULE_CMAKE_STAMP) $(SRC_DIR)/$(1)/module.c $(SRC_DIR)/$(1)/CMakeLists.txt
	$$(info + KMODULE $(1))
	@ cmake --build $(KMODULE_CMAKE_BUILD_DIR) --target $(1)

$(OUTPUT_DIR)/$(1)/$(1).ko: $(KMODULE_OUTPUT_DIR)/$(1).ko
	$$(info + CP $$<)
	@ mkdir -p $$(@D)
	@ cp $$< $$@
endef

$(foreach test,$(TESTS),$(eval $(call TEST_MODULE_RULE,$(test))))

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
