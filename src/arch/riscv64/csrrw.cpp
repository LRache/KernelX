#include <cstdint>
#include <kernelx/arch/riscv64/riscv64.hpp>

using namespace kernelx;

void arch::csrw_stvec(uint64_t value) {
    asm volatile("csrw stvec, %0" : : "r"(value));
}

uint64_t arch::csrr_scause() {
    uint64_t value;
    asm volatile("csrr %0, scause" : "=r"(value));
    return value;
}
