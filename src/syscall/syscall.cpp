#include <cstdint>
#include <kernelx/syscall.hpp>
#include <kernelx/console.hpp>

using namespace kernelx;

static uint64_t syscall_read(const kernelx::syscall::SyscallArgs &) {
    return -1;
}

static uint64_t syscall_write(const kernelx::syscall::SyscallArgs &args) {
    uint64_t fd = args.args[0];
    uint64_t buf = args.args[1];
    uint64_t count = args.args[2];
    
    if (fd == 1) { // STDOUT
        for (uint64_t i = 0; i < count; ++i) {
            char c = *(char *)(buf + i);
            console::putc(c);
        }
        return count;
    }

    return 0;
}

static uint64_t (*syscall_table[])(const kernelx::syscall::SyscallArgs &) = {
    [0] = syscall_read,
    [1] = syscall_write,
};

uint64_t syscall::syscall(uint64_t num, const SyscallArgs &args) {
    if (num >= sizeof(syscall_table) / sizeof(syscall_table[0])) {
        return -1;
    }
    if (syscall_table[num] == nullptr) {
        return -1;
    }
    return syscall_table[num](args);
}
