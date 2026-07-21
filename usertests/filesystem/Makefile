ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

CFLAGS += -Wall -Wextra

ALL_CASES := $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case).c))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(case)))
include ../mk/case-selection.mk

TESTS = $(CASES)
TEST_TARGETS = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test))

$(BUILD_DIR)/%.o: $(SRC_DIR)/%.c
	$(info + CC $<)
	@ mkdir -p $(@D)
	@ $(CC) $(CFLAGS) -c $< -o $@

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(BUILD_DIR)/$(1).o
	$$(info + LD $$<)
	@ mkdir -p $$(@D)
	@ $$(CC) $$(CFLAGS) $$< -o $$@
endef

$(foreach test,$(TESTS),$(eval $(call TEST_TARGET_RULE,$(test))))

all: $(TEST_TARGETS)
ifneq ($(filter write,$(TESTS)),)
	@ : > $(OUTPUT_DIR)/write/a.txt
endif

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
