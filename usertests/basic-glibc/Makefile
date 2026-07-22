ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

CFLAGS += -Wall -Wextra
LDLIBS ?=

PTHREAD_TESTS = thread_fork_mmap_leak

ALL_CASES := $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case).c))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(case)))
include ../mk/case-selection.mk

TESTS = $(CASES)
TEST_TARGETS = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test))

$(addprefix $(BUILD_DIR)/, $(addsuffix .o, $(PTHREAD_TESTS))): CFLAGS += -pthread
$(foreach test,$(PTHREAD_TESTS),$(OUTPUT_DIR)/$(test)/$(test)): LDLIBS += -pthread

$(BUILD_DIR)/%.o: $(SRC_DIR)/%.c
	$(info + CC $<)
	@ mkdir -p $(@D)
	@ $(CC) $(CFLAGS) -c $< -o $@

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(BUILD_DIR)/$(1).o
	$$(info + LD $$<)
	@ mkdir -p $$(@D)
	@ $$(CC) $$(CFLAGS) $$< -o $$@ $$(LDLIBS)
endef

$(foreach test,$(TESTS),$(eval $(call TEST_TARGET_RULE,$(test))))

all: $(TEST_TARGETS)
ifneq ($(filter unlink,$(TESTS)),)
	@ echo "File to unlink" > $(OUTPUT_DIR)/unlink/to_unlink.txt
endif

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
