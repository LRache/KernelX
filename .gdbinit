# file ./target/riscv64gc-unknown-none-elf/debug/kernelx
add-symbol-file ./build/qemu-virt-riscv64/vmkernelx -s .init 0x80200000
target remote 127.0.0.1:1234
break *0x80200000
layout asm
c