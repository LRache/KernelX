file ./target/riscv64gc-unknown-none-elf/debug/kernelx
target remote 127.0.0.1:1234
break *0x80200000
layout asm
c