ARCH ?= riscv64
ISA ?= $(ARCH)
GUEST_COMPONENTS ?= hello_sbi

CROSS_COMPILER ?= riscv64-unknown-elf-

CC = $(CROSS_COMPILER)gcc
OBJCOPY = $(CROSS_COMPILER)objcopy

BUILD_DIR ?= build/$(ISA)
OUTPUT_DIR = $(BUILD_DIR)/output

HELLO_TARGET = $(BUILD_DIR)/hello_sbi.elf
HELLO_BIN_TARGET = $(BUILD_DIR)/hello_sbi.bin
HELLO_OUTPUT_TARGET = $(OUTPUT_DIR)/hello_sbi.elf
HELLO_OUTPUT_BIN_TARGET = $(OUTPUT_DIR)/hello_sbi.bin

SRC = hello_sbi.S

CFLAGS = -Wall -Werror -nostdlib -ffreestanding -static
LDFLAGS = -Wl,-e,_start -Wl,-Ttext=0x80200000

.PHONY: all hello_sbi linux5.15 download-linux5.15 clean

all: $(GUEST_COMPONENTS)

hello_sbi: $(HELLO_OUTPUT_TARGET) $(HELLO_OUTPUT_BIN_TARGET)

linux5.15:
	@ $(MAKE) -C linux5.15 ARCH=$(ARCH) ISA=$(ISA) OUTPUT_DIR="$(abspath $(OUTPUT_DIR))/linux5.15" all

download-linux5.15:
	@ ./download.sh linux5.15

$(HELLO_TARGET): $(SRC)
	@ mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $(HELLO_TARGET) $(SRC)

$(HELLO_BIN_TARGET): $(HELLO_TARGET)
	$(OBJCOPY) -O binary $(HELLO_TARGET) $(HELLO_BIN_TARGET)

$(HELLO_OUTPUT_TARGET): $(HELLO_TARGET)
	@ mkdir -p $(OUTPUT_DIR)
	cp $(HELLO_TARGET) $(HELLO_OUTPUT_TARGET)

$(HELLO_OUTPUT_BIN_TARGET): $(HELLO_BIN_TARGET)
	@ mkdir -p $(OUTPUT_DIR)
	cp $(HELLO_BIN_TARGET) $(HELLO_OUTPUT_BIN_TARGET)

clean:
	rm -rf $(BUILD_DIR)
	@ $(MAKE) -C linux5.15 clean
