CROSS_COMPILER ?= riscv64-unknown-elf-
ISA ?= riscv64

BUILD_DIR ?= build/$(ISA)
OUTPUT_DIR = $(BUILD_DIR)/output

TARGET = $(BUILD_DIR)/helloworld.elf
OUTPUT_TARGET = $(OUTPUT_DIR)/helloworld.elf

SRC = helloworld.S

CC = $(CROSS_COMPILER)gcc
CFLAGS = -Wall -Werror -nostdlib -ffreestanding -static

$(TARGET): $(SRC)
	@mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) -o $(TARGET) $(SRC)

$(OUTPUT_TARGET): $(TARGET)
	@mkdir -p $(OUTPUT_DIR)
	cp $(TARGET) $(OUTPUT_TARGET)

all: $(OUTPUT_TARGET)

clean:
	rm -rf $(BUILD_DIR)