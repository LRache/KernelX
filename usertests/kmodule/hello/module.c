#include "kmodule.h"

static int hello_init(void) {
    kinfo("hello");
    return 0;
}

static void hello_exit(void) {
    kinfo("goodbye");
}

KERNELX_MODULE("hello", hello_init, hello_exit);
