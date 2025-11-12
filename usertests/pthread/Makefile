ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

CC = $(CROSS_COMPILE)gcc

SRC_DIR = .
BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output

CFLAGS += -Wall -Wextra
CFLAGS += --sysroot /home/rache/code/glibc-2.42/build/riscv64/install
CFLAGS += -static
CFLAGS += -O0 -Og -g

TESTS = $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
TEST_TARGETS = $(addprefix $(OUTPUT_DIR)/, $(TESTS))

$(BUILD_DIR)/%.o: $(SRC_DIR)/%.c
	$(info + CC $<)
	@ mkdir -p $(OUTPUT_DIR)
	@ $(CC) $(CFLAGS) -c $< -o $@

$(OUTPUT_DIR)/%: $(BUILD_DIR)/%.o
	$(info + LD $<)
	@ mkdir -p $(OUTPUT_DIR)
	@ $(CC) $(CFLAGS) $< -o $@

all: $(TEST_TARGETS)

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
