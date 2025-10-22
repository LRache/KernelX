#ifndef __ARCH_RISCV_ENTEY_H__
#define __ARCH_RISCV_ENTEY_H__

#include <stdint.h>

#define PGSIZE 4096

uintptr_t __riscv_load_fdt(const void *fdt);
uintptr_t __riscv_map_kaddr(uintptr_t kaddr_offset, uintptr_t memory_top);

uintptr_t *__riscv_init_load_kernel_end();
uintptr_t *__riscv_init_load_kpgtable_root();
void **__riscv_init_load_copied_fdt();
uintptr_t *__riscv_init_load_kaddr_offset();

void __riscv_init_die();

#endif // __ARCH_RISCV_ENTRY_H__
