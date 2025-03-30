#include <kernelx/arch/riscv64/riscv64.hpp>
#include <kernelx/arch/riscv64/trap.hpp>
#include <kernelx/progress/trap.hpp>
#include <kernelx/syscall.hpp>
#include <kernelx/klib.h>

using namespace kernelx;

extern "C" void usertrap_entry();

void progress::init_usertrap() {
    arch::csrw_stvec((uint64_t)usertrap_entry);
}

progress::TrapContext *progress::usertrap_handler(TrapContext *context) {
    uint64_t cause = arch::csrr_scause();
    
    if (cause == 8) {
        // User software interrupt
        // Handle the syscall
        uint64_t syscall_num = context->gpr[17];
        kernelx::syscall::SyscallArgs args;
        args.args[0] = context->gpr[10];
        args.args[1] = context->gpr[11];
        args.args[2] = context->gpr[12];
        args.args[3] = context->gpr[13];
        args.args[4] = context->gpr[14];
        args.args[5] = context->gpr[15];

        uint64_t ret = kernelx::syscall::syscall(syscall_num, args);
        context->gpr[10] = ret;
    } else {
        printf("Unhandled trap: scause = %lx\n", cause);
    }

    return context;
}
