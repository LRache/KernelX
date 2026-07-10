#ifndef __ARCH_RISCV_TRAP_H__
#define __ARCH_RISCV_TRAP_H__

#if __riscv_xlen == 64
    #define LOAD(dest, n)  ld dest, (n*8)(a0)
    #define STORE(src, n)  sd src , (n*8)(a0)
#else
    #define LOAD(dest, n)  lw dest, (n*4)(a0)
    #define STORE(src, n)  sw src , (n*4)(a0)
#endif

#endif // __ARCH_RISCV_TRAP_H__
