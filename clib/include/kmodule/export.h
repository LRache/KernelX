#ifndef __KMODULE_EXPORT_H__
#define __KMODULE_EXPORT_H__

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

struct kmodule_export {
    const unsigned char *name;
    size_t name_len;
    const void *addr;
};

#define KMODULE_EXPORT(name_literal, symbol)                                      \
    static const struct kmodule_export __kmodule_export_##symbol                 \
        __attribute__((used, section(".kmodule.exports"))) = {                   \
            (const unsigned char *)(name_literal), sizeof(name_literal) - 1,      \
            (const void *)(symbol),                                              \
        }

#ifdef __cplusplus
}
#endif

#endif
