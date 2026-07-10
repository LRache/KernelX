#ifndef __KERNELX_ARCH_LOONGARCH_ENTRY_H__
#define __KERNELX_ARCH_LOONGARCH_ENTRY_H__

#include <stdint.h>

#define __init_text __attribute__((section(".text.init")))
#define __init_data __attribute__((section(".data.init")))

uintptr_t __la_fdt_memory_top(void);

#endif /* __KERNELX_ARCH_LOONGARCH_ENTRY_H__ */
