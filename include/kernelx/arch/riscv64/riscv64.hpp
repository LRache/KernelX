#ifndef __KERNELX_ARCH_RISCV64_RISCV64_H__
#define __KERNELX_ARCH_RISCV64_RISCV64_H__

#include <cstdint>

namespace kernelx::arch {
    void csrw_stvec(uint64_t value);
    
    uint64_t csrr_scause();
    uint64_t csrr_sepc();
};

#endif
