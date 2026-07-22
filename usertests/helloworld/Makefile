CROSS_COMPILER ?= riscv64-unknown-elf-
ISA ?= riscv64

BUILD_DIR ?= build/$(ISA)
OUTPUT_DIR = $(BUILD_DIR)/output

TARGET = $(BUILD_DIR)/helloworld.elf
OUTPUT_TARGET = $(OUTPUT_DIR)/helloworld/helloworld.elf

SRC = helloworld.S

CC = $(CROSS_COMPILER)gcc
CFLAGS = -Wall -Werror -nostdlib -ffreestanding -static

ALL_CASES := helloworld
CASE_SOURCES_helloworld := $(SRC)
CASE_COMMAND_helloworld := helloworld.elf
include ../mk/case-selection.mk

SELECTED_TARGETS := $(if $(filter helloworld,$(CASES)),$(OUTPUT_TARGET))

$(TARGET): $(SRC)
	@mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) -o $(TARGET) $(SRC)

$(OUTPUT_TARGET): $(TARGET)
	@mkdir -p $(@D)
	cp $(TARGET) $(OUTPUT_TARGET)

all: $(SELECTED_TARGETS)

clean:
	rm -rf $(BUILD_DIR)
