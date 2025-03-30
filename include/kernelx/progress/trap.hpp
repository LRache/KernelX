#ifndef __KERNELX_PROGRESS_TRAP_HPP__
#define __KERNELX_PROGRESS_TRAP_HPP__

namespace kernelx::progress {

struct TrapContext;

TrapContext *usertrap_handler(TrapContext *context);

void init_usertrap();

}

#endif // __KERNELX_PROGRESS_TRAP_HPP__
