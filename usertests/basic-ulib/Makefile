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

TESTS = $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
TEST_TARGETS = $(addprefix $(OUTPUT_DIR)/, $(TESTS))

$(BUILD_DIR)/%.o: $(SRC_DIR)/%.c
	@ mkdir -p $(OUTPUT_DIR)/$(ARCH)
	@ $(CC) $(CFLAGS) -c $< -o $@

$(OUTPUT_DIR)/%: $(BUILD_DIR)/%.o $(ULIB)
	@ mkdir -p $(OUTPUT_DIR)/$(ARCH)
	@ $(CC) $(CFLAGS) $< -o $@ $(ULIB)

all: ulib-all $(TEST_TARGETS)

ulib-all:
	$(MAKE) -C ulib all

show-tests:
	@echo "Auto-discovered tests:"
	@for test in $(TESTS); do echo "  - $$test"; done
