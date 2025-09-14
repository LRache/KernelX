#!/bin/bash

# riscv64-linux-gnu-gcc helloworld.S -o helloworld.elf -nostdlib -ffreestanding -static
riscv64-linux-gnu-gcc helloworld.S -o helloworld.elf -nostdlib -ffreestanding -static -T linker.ld
riscv64-linux-gnu-objcopy -O binary helloworld.elf helloworld.bin

riscv64-linux-gnu-gcc helloworld.S -o helloworld.elf -nostdlib -ffreestanding -static
