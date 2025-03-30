#ifndef __KERNELX_ARCH_RISCV64_TRAP_H__
#define __KERNELX_ARCH_RISCV64_TRAP_H__

#include <cstdint>

namespace kernelx::progress {

class PCB;

struct TrapContext {
    uint64_t gpr[32]; // 0-31
    TrapContext (*usertrap_handler)(TrapContext *);
    PCB *pcb; // Pointer to the process control block
};

struct TaskContext {
    uint64_t ra;
    uint64_t sp;
    uint64_t s[12]; // s0-s11
};

}

#endif // __KERNELX_PROGRESS_CONTEXT_H__
