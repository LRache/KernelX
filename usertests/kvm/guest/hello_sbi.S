.section .text
.global _start

_start:
    la t0, hello

1:
    lbu a0, 0(t0)
    beqz a0, 2f
    li a6, 0
    li a7, 1
    ecall
    addi t0, t0, 1
    j 1b

2:
    wfi
    j 2b

.section .rodata
hello:
    .asciz "Hello, world from KVM guest via SBI\n"
