ARCH ?= riscv64
CROSS_COMPILE ?= /opt/riscv64-linux-musl-cross/bin/riscv64-linux-musl-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

# MUSL_TMP = /home/rache/code/musl-1.2.5/build-riscv64-debug

# CFLAGS += -nostdinc
# CFLAGS += -I$(MUSL_TMP)/include

# LDFLAGS += -nostdlib
# LDFLAGS += $(MUSL_TMP)/lib/crt1.o
# LDFLAGS += $(MUSL_TMP)/lib/crti.o
# LDFLAGS += -L$(MUSL_TMP)/lib
# LDFLAGS += -lc
# LDFLAGS += $(MUSL_TMP)/lib/crtn.o

CFLAGS += -Wl,-t

CFLAGS += -Wall -Wextra
# CFLAGS += --sysroot /home/rache/code/musl-1.2.5/build-riscv64-debug
CFLAGS += -static -no-pie
CFLAGS += -O0 -Og -g -v

ALL_CASES := $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
$(foreach case,$(ALL_CASES),$(eval CASE_SOURCES_$(case) := $(case).c))
$(foreach case,$(ALL_CASES),$(eval CASE_COMMAND_$(case) := $(case)))
include ../mk/case-selection.mk

TESTS = $(CASES)
TEST_TARGETS = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test))

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(SRC_DIR)/$(1).c
	$$(info + CC $$<)
	@ mkdir -p $$(@D)
	@ $$(CC) $$(CFLAGS) $$< $$(LDFLAGS) -o $$@
endef

$(foreach test,$(TESTS),$(eval $(call TEST_TARGET_RULE,$(test))))

# $(OUTPUT_DIR)/%: $(BUILD_DIR)/%.o
# 	$(info + LD $<)
# 	@ mkdir -p $(OUTPUT_DIR)
# 	@ $(CC) $(CFLAGS) $< -o $@

all: $(TEST_TARGETS)

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
