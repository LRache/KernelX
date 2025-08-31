ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-linux-gnu-

INC_DIR = include
SRC_DIR = src
ARCH_DIR = $(SRC_DIR)/arch/$(ARCH)

BUILD_DIR = build/$(ARCH)
TARGET = $(BUILD_DIR)/ulib.a

CC = $(CROSS_COMPILE)gcc
AR = $(CROSS_COMPILE)ar

CFLAGS += -Wall -Wextra -ffreestanding
CFLAGS += $(addprefix -I, $(INC_DIR))

SRCS += $(wildcard $(SRC_DIR)/*.c)
SRCS += $(wildcard $(ARCH_DIR)/*.c)
SRCS += $(wildcard $(ARCH_DIR)/*.S)

OBJS = $(patsubst $(SRC_DIR)/%, $(BUILD_DIR)/%, $(SRCS:%=%.o))

$(TARGET): $(OBJS)
	@ mkdir -p $(dir $@)
	@ $(AR) rcs $@ $^

$(BUILD_DIR)/%.c.o: $(SRC_DIR)/%.c
	@ mkdir -p $(dir $@)
	@ $(CC) $(CFLAGS) -c $< -o $@

$(BUILD_DIR)/%.S.o: $(SRC_DIR)/%.S
	@ mkdir -p $(dir $@)
	@ $(CC) $(CFLAGS) -c $< -o $@

all: $(TARGET)

clean:
	rm -rf $(BUILD_DIR)
	@ echo "Cleaned build directory"

.PHONY: all clean
