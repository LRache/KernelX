#include <kernelx/console.hpp>
#include <kernelx/sbi.hpp>

void kernelx::console::putc(char c) {
    sbi::console_putchar(c);
}

void kernelx::console::puts(const char *str) {
    while (*str) {
        console::putc(*str++);
    }
}
