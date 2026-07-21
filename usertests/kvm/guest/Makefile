ARCH ?= riscv64
ISA ?= $(ARCH)
GUEST_COMPONENTS ?= hello_sbi timer_tick

CROSS_COMPILER ?= riscv64-unknown-elf-

CC = $(CROSS_COMPILER)gcc
OBJCOPY = $(CROSS_COMPILER)objcopy

BUILD_DIR ?= build/$(ISA)
OUTPUT_DIR = $(BUILD_DIR)/output

HELLO_TARGET = $(BUILD_DIR)/hello_sbi.elf
HELLO_BIN_TARGET = $(BUILD_DIR)/hello_sbi.bin
HELLO_OUTPUT_TARGET = $(OUTPUT_DIR)/hello_sbi/hello_sbi.elf
HELLO_OUTPUT_BIN_TARGET = $(OUTPUT_DIR)/hello_sbi/hello_sbi.bin

TIMER_TICK_TARGET = $(BUILD_DIR)/timer_tick.elf
TIMER_TICK_BIN_TARGET = $(BUILD_DIR)/timer_tick.bin
TIMER_TICK_OUTPUT_TARGET = $(OUTPUT_DIR)/timer_tick/timer_tick.elf
TIMER_TICK_OUTPUT_BIN_TARGET = $(OUTPUT_DIR)/timer_tick/timer_tick.bin

CFLAGS = -Wall -Werror -nostdlib -ffreestanding -static
LDFLAGS = -Wl,-e,_start -Wl,-Ttext=0x80200000

.PHONY: all hello_sbi timer_tick linux6.1 download-linux6.1 clean

all: $(GUEST_COMPONENTS)

hello_sbi: $(HELLO_OUTPUT_TARGET) $(HELLO_OUTPUT_BIN_TARGET)

timer_tick: $(TIMER_TICK_OUTPUT_TARGET) $(TIMER_TICK_OUTPUT_BIN_TARGET)

linux6.1:
	@ $(MAKE) -C linux6.1 ARCH=$(ARCH) ISA=$(ISA) OUTPUT_DIR="$(abspath $(OUTPUT_DIR))/linux6.1" all

download-linux6.1:
	@ ./download.sh linux6.1

$(HELLO_TARGET): hello_sbi.S
	@ mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $(HELLO_TARGET) hello_sbi.S

$(HELLO_BIN_TARGET): $(HELLO_TARGET)
	$(OBJCOPY) -O binary $(HELLO_TARGET) $(HELLO_BIN_TARGET)

$(HELLO_OUTPUT_TARGET): $(HELLO_TARGET)
	@ mkdir -p $(@D)
	cp $(HELLO_TARGET) $(HELLO_OUTPUT_TARGET)

$(HELLO_OUTPUT_BIN_TARGET): $(HELLO_BIN_TARGET)
	@ mkdir -p $(@D)
	cp $(HELLO_BIN_TARGET) $(HELLO_OUTPUT_BIN_TARGET)

$(TIMER_TICK_TARGET): timer_tick.S
	@ mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $(TIMER_TICK_TARGET) timer_tick.S

$(TIMER_TICK_BIN_TARGET): $(TIMER_TICK_TARGET)
	$(OBJCOPY) -O binary $(TIMER_TICK_TARGET) $(TIMER_TICK_BIN_TARGET)

$(TIMER_TICK_OUTPUT_TARGET): $(TIMER_TICK_TARGET)
	@ mkdir -p $(@D)
	cp $(TIMER_TICK_TARGET) $(TIMER_TICK_OUTPUT_TARGET)

$(TIMER_TICK_OUTPUT_BIN_TARGET): $(TIMER_TICK_BIN_TARGET)
	@ mkdir -p $(@D)
	cp $(TIMER_TICK_BIN_TARGET) $(TIMER_TICK_OUTPUT_BIN_TARGET)

clean:
	rm -rf $(BUILD_DIR)
	@ $(MAKE) -C linux6.1 clean
