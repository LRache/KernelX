#ifndef __ARCH_RISCV_ENTEY_H__
#define __ARCH_RISCV_ENTEY_H__

#include <stdint.h>

#define __init_text __attribute__((section(".text.init")))
#define __init_data __attribute__((section(".data.init")))

uintptr_t __riscv_load_fdt(const void *fdt);
uintptr_t __riscv_map_kaddr(uintptr_t kaddr_offset, uintptr_t memory_top);

void **__riscv_init_symbol_ktop();
uintptr_t *__riscv_init_symbol_kpgtable_root();
void **__riscv_init_symbol_copied_fdt();
uintptr_t *__riscv_init_symbol_kaddr_offset();

void *__riscv_init_symbol_kernel_end();
void *__riscv_init_symbol_init_start();
void *__riscv_init_symbol_init_end();
void *__riscv_init_symbol_text_start();
void *__riscv_init_symbol_text_end();
void *__riscv_init_symbol_data_start();
void *__riscv_init_symbol_bss_start();
void *__riscv_init_symbol_bss_end();

void __riscv_init_die(const char *reason);

#endif // __ARCH_RISCV_ENTRY_H__
