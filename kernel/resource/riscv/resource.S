.global __bin_resource_start
.global __bin_resource_end
.section .data
__bin_resource_start:
    .incbin "./resource/riscv/helloworld/helloworld.elf"
__bin_resource_end:
