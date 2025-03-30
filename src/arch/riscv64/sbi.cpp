#include <kernelx/sbi.hpp>
#include <cstdint>

using namespace kernelx;

#define SBI_SUCCESS            0
#define SBI_ERR_FAILED        -1
#define SBI_ERR_NOT_SUPPORTED -2
#define SBI_ERR_INVALID_PARAM -3

struct sbiret {
    long error;
    long value;
};

static sbiret sbi_call(uintptr_t fid, uintptr_t eid, uintptr_t arg0 = 0, uintptr_t arg1 = 0, uintptr_t arg2 = 0, uintptr_t arg3 = 0) {
    register uintptr_t a0 asm("a0") = arg0;
    register uintptr_t a1 asm("a1") = arg1;
    register uintptr_t a2 asm("a2") = arg2;
    register uintptr_t a3 asm("a3") = arg3;
    register uintptr_t a6 asm("a6") = fid;
    register uintptr_t a7 asm("a7") = eid;
    asm volatile("ecall" \
                 : "+r"(a0), "+r"(a1)
                 : "r"(a2), "r"(a3), "r"(a6), "r"(a7)
                 : "memory");
    return sbiret{(long)a0, (long)a1};
}

void sbi::console_putchar(char c) {
    sbi_call(0, 1, (uintptr_t)c);
}

void sbi::shutdown() {
    struct sbiret ret = sbi_call(0, 8);
    if (ret.error != SBI_SUCCESS) {
        while(1);
    }
}
