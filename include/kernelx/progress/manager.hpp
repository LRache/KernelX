#ifndef __KERNELX_PROGRESS_MANAGER_H__
#define __KERNELX_PROGRESS_MANAGER_H__

#include <etl/queue.h>

#include <kernelx/progress/pcb.hpp>

namespace kernelx::progress::manager {
    extern etl::queue<PCB *, 10> readyQueue;

    void load();
}

#endif
