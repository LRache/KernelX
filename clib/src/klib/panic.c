#include <klib/klib.h>

/* Implemented in Rust; see src/klib/klog.rs */
extern void rust_kpanic(const char *file, int line, const char *msg)
    __attribute__((noreturn));

__attribute__((noreturn)) void kpanic(const char *file, int line, const char *msg) {
    rust_kpanic(file, line, msg);
}
