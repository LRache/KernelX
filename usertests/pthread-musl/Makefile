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

TESTS = $(basename $(notdir $(wildcard $(SRC_DIR)/*.c)))
TEST_TARGETS = $(addprefix $(OUTPUT_DIR)/, $(TESTS))

$(OUTPUT_DIR)/%: $(SRC_DIR)/%.c
	$(info + CC $<)
	@ mkdir -p $(OUTPUT_DIR)
	@ $(CC) $(CFLAGS) $< $(LDFLAGS) -o $@

# $(OUTPUT_DIR)/%: $(BUILD_DIR)/%.o
# 	$(info + LD $<)
# 	@ mkdir -p $(OUTPUT_DIR)
# 	@ $(CC) $(CFLAGS) $< -o $@

all: $(TEST_TARGETS)

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
