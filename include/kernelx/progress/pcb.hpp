#ifndef __KERNELX_PROGRESS_PCB_H__
#define __KERNELX_PROGRESS_PCB_H__

#include "kernelx/arch/riscv64/trap.hpp"
#include <kernelx/progress/trap.hpp>

#include <cstdint>

namespace kernelx::progress {

// Process Control Block (PCB) structure
// This structure holds the context of a process, including its trap context,
class PCB {
public:
    TrapContext *userContext; // Context of the process
    uint64_t pc;

    TaskContext *taskContext; // Pointer to the task context

    int coreid; // Core ID where the process is running
};

}

#endif // __KERNELX_PROGRESS_PCB_H__
