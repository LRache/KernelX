    .section .text.init
    
    .global __riscv_init_symbol_kpgtable_root
    .type __riscv_init_symbol_kpgtable_root, @function
__riscv_init_symbol_kpgtable_root:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__riscv_kpgtable_root)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_kaddr_offset
    .type __riscv_init_symbol_kaddr_offset, @function
__riscv_init_symbol_kaddr_offset:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__riscv_kaddr_offset)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret
    
    .global __riscv_init_symbol_ktop
    .type __riscv_init_symbol_ktop, @function
__riscv_init_symbol_ktop:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__riscv_ktop)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_copied_fdt
    .type __riscv_init_symbol_copied_fdt, @function
__riscv_init_symbol_copied_fdt:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__riscv_copied_fdt)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret
    
    .global __riscv_init_symbol_init_start
    .type __riscv_init_symbol_init_start, @function
__riscv_init_symbol_init_start:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__init_start)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_init_end
    .type __riscv_init_symbol_init_end, @function
__riscv_init_symbol_init_end:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__init_end)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_text_start
    .type __riscv_init_symbol_text_start, @function
__riscv_init_symbol_text_start:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__text_start)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_text_end
    .type __riscv_init_symbol_text_end, @function
__riscv_init_symbol_text_end:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__text_end)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_data_start
    .type __riscv_init_symbol_data_start, @function
__riscv_init_symbol_data_start:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__data_start)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_kernel_end
    .type __riscv_init_symbol_kernel_end, @function
__riscv_init_symbol_kernel_end:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__kernel_end)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_bss_start
    .type __riscv_init_symbol_bss_start, @function
__riscv_init_symbol_bss_start:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__bss_start)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret

    .global __riscv_init_symbol_bss_end
    .type __riscv_init_symbol_bss_end, @function
__riscv_init_symbol_bss_end:
    .option push
    .option norelax
1:  auipc   a0, %got_pcrel_hi(__bss_end)
    ld      a0, %pcrel_lo(1b)(a0)
    .option pop
    sub a0, a0, tp
    ret