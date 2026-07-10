#ifndef __KERNELX_ARCH_LOONGARCH_ENTRY_H__
#define __KERNELX_ARCH_LOONGARCH_ENTRY_H__

/*
 * Phase 1 skeleton: no prototypes yet. Phase 2 will add the `_entry` /
 * `__la_init(...)` declarations that mirror `clib/include/arch/riscv/entry.h`.
 */

#define __init_text __attribute__((section(".text.init")))
#define __init_data __attribute__((section(".data.init")))

#endif /* __KERNELX_ARCH_LOONGARCH_ENTRY_H__ */
