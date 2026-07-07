#ifndef __KLIB_KLIB_H__
#define __KLIB_KLIB_H__

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void klib_init();

long klog_info(const char *file, unsigned int line, unsigned int column, const char *fmt, ...);

/* Kernel panic — implemented in Rust (rust_kpanic), called from C */
__attribute__((noreturn)) void kpanic(const char *file, int line, const char *msg);

#define kassert(cond) \
    do { if (!(cond)) kpanic(__FILE__, __LINE__, "assertion failed: " #cond); } while (0)

#ifdef __cplusplus
}
#endif

#endif
