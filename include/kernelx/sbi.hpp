#ifndef __KERNELX_SBI_H__
#define __KERNELX_SBI_H__

namespace kernelx::sbi {
    void console_putchar(char c);
    void console_getchar(char *c);
    void shutdown();
}

#endif
