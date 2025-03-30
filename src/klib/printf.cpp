#include <kernelx/console.hpp>
#include <tinyprintf.h>

static void printf_putc_helper(void *, char c) {
    kernelx::console::putc(c);
}

void printf_init() {
    init_printf(nullptr, printf_putc_helper);
}
