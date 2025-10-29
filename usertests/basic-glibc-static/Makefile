ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

GLIBC_DIR = /home/rache/code/glibc/build/riscv64-linux-gnu
CRT_FILES_BEGIN = $(GLIBC_DIR)/csu/crt1.o $(GLIBC_DIR)/csu/crti.o
CRT_FILES_END = $(GLIBC_DIR)/csu/crtn.o

CFLAGS += -Wall -Wextra -static -nostdlib -nostartfiles
LDFLAGS += -L$(GLIBC_DIR) -lc -lgcc -lgcc_eh

TESTS = $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
TEST_TARGETS = $(addprefix $(OUTPUT_DIR)/, $(TESTS))

$(OUTPUT_DIR)/%: $(SRC_DIR)/%.c
	$(info + LD $<)
	@ mkdir -p $(OUTPUT_DIR)
	$(CC) $(CFLAGS) $(CRT_FILES_BEGIN) $< -o $@ $(LDFLAGS) $(CRT_FILES_END)

all: $(TEST_TARGETS)

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)

show-tests:
	@echo "Auto-discovered tests:"
	@for test in $(TESTS); do echo "  - $$test"; done
	@for test in $(TESTS); do echo "  - $$test"; done
