#ifndef __ULIB_H__
#define __ULIB_H__

#include <stdint.h>

uintptr_t __syscall(uintptr_t number, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t arg4);

#endif