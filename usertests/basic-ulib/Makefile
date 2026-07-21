ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

ULIB = ulib/build/$(ARCH)/ulib.a

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

INC_DIR += ulib/include

CFLAGS += -Wall -Wextra
CFLAGS += -nostdlib -static

ALL_CASES := $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case).c))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(case)))
include ../mk/case-selection.mk

TESTS = $(CASES)
TEST_TARGETS = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test))

$(BUILD_DIR)/%.o: $(SRC_DIR)/%.c
	@ mkdir -p $(@D)
	@ $(CC) $(CFLAGS) -c $< -o $@

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(BUILD_DIR)/$(1).o $(ULIB)
	@ mkdir -p $$(@D)
	@ $$(CC) $$(CFLAGS) $$< -o $$@ $$(ULIB)
endef

$(foreach test,$(TESTS),$(eval $(call TEST_TARGET_RULE,$(test))))

all: ulib-all $(TEST_TARGETS)
ifneq ($(filter read,$(TESTS)),)
	@ echo "Hello from basic-ulib read test" > $(OUTPUT_DIR)/read/test.txt
endif
ifneq ($(filter write,$(TESTS)),)
	@ : > $(OUTPUT_DIR)/write/test-write.txt
endif

ulib-all:
	$(MAKE) -C ulib all

show-tests:
	@echo "Auto-discovered tests:"
	@for test in $(ALL_CASES); do echo "  - $$test"; done
