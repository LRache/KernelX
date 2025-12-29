    .extern __riscv_kernel_end
    .extern __riscv_kpgtable_root

    .section .text.init

    .global __riscv_init_load_kernel_end
    .type __riscv_init_load_kernel_end, @function

__riscv_init_load_kernel_end:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__riscv_kernel_end)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    
    sub a0, a0, tp
    
    ret
    
    .global __riscv_init_load_kpgtable_root
    .type __riscv_init_load_kpgtable_root, @function
__riscv_init_load_kpgtable_root:
    .option push
    .option norelax
2:  auipc   a0, %got_pcrel_hi(__riscv_kpgtable_root)
    ld      a0, %pcrel_lo(2b)(a0)
    .option pop
    
    sub a0, a0, tp
    
    ret

    .global __riscv_init_load_kaddr_offset
    .type __riscv_init_load_kaddr_offset, @function
__riscv_init_load_kaddr_offset:
    .option push
    .option norelax
3:  auipc   a0, %got_pcrel_hi(__riscv_kaddr_offset)
    ld      a0, %pcrel_lo(3b)(a0)
    .option pop

    sub a0, a0, tp
    
    ret

    .global __riscv_init_load_copied_fdt
    .type __riscv_init_load_copied_fdt, @function
__riscv_init_load_copied_fdt:
    .option push
    .option norelax
4:  auipc   a0, %got_pcrel_hi(__riscv_copied_fdt)
    ld      a0, %pcrel_lo(4b)(a0)
    .option pop

    sub a0, a0, tp

    ret

    .global __riscv_init_load_bss_start
    .type __riscv_init_load_bss_start, @function
__riscv_init_load_bss_start:
    .option push
    .option norelax
5:  auipc   a0, %got_pcrel_hi(__bss_start)
    ld      a0, %pcrel_lo(5b)(a0)
    .option pop

    sub a0, a0, tp

    .global __riscv_init_load_bss_end
    .type __riscv_init_load_bss_end, @function
__riscv_init_load_bss_end:
    .option push
    .option norelax
6:  auipc   a0, %got_pcrel_hi(__bss_end)
    ld      a0, %pcrel_lo(6b)(a0)
    .option pop

    sub a0, a0, tp

    ret