.section .text
.global _start

_start:
    li a0, 1      # stdout
    la a1, hello
    li a2, 24
    li a7, 64

    ecall
    
    li a7, 93
    ecall

.data
hello:
    .string "Hello, World! from user\n"

