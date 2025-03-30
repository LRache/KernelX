#include "kernelx/progress/manager.hpp"
#include <kernelx/sbi.hpp>
#include <kernelx/klib.h>
#include <kernelx/progress/trap.hpp>

static void cpp_init() {
    extern char __init_array_start[];
    extern char __init_array_end[];
    for (char **p = (char **)__init_array_start; p < (char **)__init_array_end; ++p) {
        ((void (*)())*p)();
    }
}

void heap_init();
void printf_init();

static void init() {
    cpp_init();
    printf_init();
    heap_init();

    kernelx::progress::init_usertrap();
}

int main() {
    init();
    
    printf("Hello, World!\n");

    kernelx::progress::manager::load();

    kernelx::sbi::shutdown();
    
    return 0;
}
