ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

CFLAGS += -Wall -Wextra -O2 -pthread
LDFLAGS += -pthread

ALL_CASES := $(patsubst %.c,%,$(wildcard *.c bugs/*.c))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case).c swap_test_common.h))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(notdir $(case))))
include ../mk/case-selection.mk

TEST_TARGETS := $(foreach case,$(CASES),$(OUTPUT_DIR)/$(case)/$(notdir $(case)))

$(BUILD_DIR)/obj/%.o: %.c swap_test_common.h
	$(info + CC $<)
	@ mkdir -p $(@D)
	@ $(CC) $(CFLAGS) -I. -c $< -o $@

define CASE_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(notdir $(1)): $(BUILD_DIR)/obj/$(1).o
	$$(info + LD $$<)
	@ mkdir -p $$(@D)
	@ $$(CC) $$(CFLAGS) $$< -o $$@ $$(LDFLAGS)
endef

$(foreach case,$(ALL_CASES),$(eval $(call CASE_TARGET_RULE,$(case))))

all: $(TEST_TARGETS)

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
