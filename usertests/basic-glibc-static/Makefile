ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

GLIBC_DIR ?= /home/rache/code/glibc/build/riscv64-linux-gnu
CRT_FILES_BEGIN ?= $(GLIBC_DIR)/csu/crt1.o $(GLIBC_DIR)/csu/crti.o
CRT_FILES_END ?= $(GLIBC_DIR)/csu/crtn.o

CFLAGS += -Wall -Wextra -static -nostdlib -nostartfiles
LDFLAGS += -L$(GLIBC_DIR) -lc -lgcc -lgcc_eh

TESTS = $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
TEST_TARGETS = $(foreach test,$(TESTS),$(OUTPUT_DIR)/$(test)/$(test))

define TEST_TARGET_RULE
$(OUTPUT_DIR)/$(1)/$(1): $(SRC_DIR)/$(1).c
	$$(info + LD $$<)
	@ mkdir -p $$(@D)
	$$(CC) $$(CFLAGS) $$(CRT_FILES_BEGIN) $$< -o $$@ $$(LDFLAGS) $$(CRT_FILES_END)
endef

$(foreach test,$(TESTS),$(eval $(call TEST_TARGET_RULE,$(test))))

all: $(TEST_TARGETS)

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)

show-tests:
	@echo "Auto-discovered tests:"
	@for test in $(TESTS); do echo "  - $$test"; done
	@for test in $(TESTS); do echo "  - $$test"; done
