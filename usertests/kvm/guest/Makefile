CROSS_COMPILER ?= riscv64-unknown-elf-
ISA ?= riscv64

CC = $(CROSS_COMPILER)gcc
OBJCOPY = $(CROSS_COMPILER)objcopy

BUILD_DIR ?= build/$(ISA)
OUTPUT_DIR = $(BUILD_DIR)/output

TARGET = $(BUILD_DIR)/hello_sbi.elf
BIN_TARGET = $(BUILD_DIR)/hello_sbi.bin
OUTPUT_TARGET = $(OUTPUT_DIR)/hello_sbi.elf
OUTPUT_BIN_TARGET = $(OUTPUT_DIR)/hello_sbi.bin

SRC = hello_sbi.S

CFLAGS = -Wall -Werror -nostdlib -ffreestanding -static
LDFLAGS = -Wl,-e,_start -Wl,-Ttext=0x80200000

.PHONY: all clean

$(TARGET): $(SRC)
	@ mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $(TARGET) $(SRC)

$(BIN_TARGET): $(TARGET)
	$(OBJCOPY) -O binary $(TARGET) $(BIN_TARGET)

$(OUTPUT_TARGET): $(TARGET)
	@ mkdir -p $(OUTPUT_DIR)
	cp $(TARGET) $(OUTPUT_TARGET)

$(OUTPUT_BIN_TARGET): $(BIN_TARGET)
	@ mkdir -p $(OUTPUT_DIR)
	cp $(BIN_TARGET) $(OUTPUT_BIN_TARGET)

all: $(OUTPUT_TARGET) $(OUTPUT_BIN_TARGET)

clean:
	rm -rf $(BUILD_DIR)
