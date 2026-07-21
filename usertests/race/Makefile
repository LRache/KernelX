ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-unknown-linux-gnu-

CC = $(CROSS_COMPILE)gcc

# Case ID 保留源码相对路径，例如 ipc/case01_wakeup_reschedule。
SRC_DIRS = ipc vfs
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

CFLAGS += -Wall -Wextra -pthread -g -O2
LDFLAGS += -pthread

ALL_CASES := $(foreach dir,$(SRC_DIRS),$(patsubst %.c,%,$(wildcard $(dir)/*.c)))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case).c $(dir $(case))smp_check.h))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(notdir $(case))))
include ../mk/case-selection.mk

TEST_TARGETS := $(foreach case,$(CASES),$(OUTPUT_DIR)/$(case)/$(notdir $(case)))

$(BUILD_DIR)/obj/%.o: %.c
	$(info + CC $<)
	@ mkdir -p $(@D)
	@ $(CC) $(CFLAGS) -c $< -o $@

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
