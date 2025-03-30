#ifndef __KERNELX_SYSCALL_HPP__
#define __KERNELX_SYSCALL_HPP__

#include <cstdint>

namespace kernelx::syscall {

struct SyscallArgs {
    uint64_t args[6];
};

uint64_t syscall(uint64_t num, const SyscallArgs &args);

}

#endif // __KERNELX_SYSCALL_HPP__
