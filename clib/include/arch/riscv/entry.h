#ifndef __ARCH_RISCV_ENTEY_H__
#define __ARCH_RISCV_ENTEY_H__

#include <stdint.h>

#define __init_text __attribute__((section(".text.init")))
#define __init_data __attribute__((section(".data.init")))

#define PGSIZE 4096

uintptr_t __riscv_load_fdt(const void *fdt);
uintptr_t __riscv_map_kaddr(uintptr_t kaddr_offset, uintptr_t memory_top);

uintptr_t *__riscv_init_load_kernel_end();
uintptr_t *__riscv_init_load_kpgtable_root();
void **__riscv_init_load_copied_fdt();
uintptr_t *__riscv_init_load_kaddr_offset();

void **__riscv_init_load_bss_start();
void **__riscv_init_load_bss_end();

void __riscv_init_die(const char *reason);

#endif // __ARCH_RISCV_ENTRY_H__
