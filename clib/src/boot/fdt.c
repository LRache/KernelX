#include "boot/fdt.h"
#include "libfdt.h"

#include <stddef.h>
#include <stdint.h>

static int is_memory_node(const char *name) {
    const char prefix[] = "memory";

    for (size_t i = 0; prefix[i] != '\0'; i++) {
        if (name[i] != prefix[i]) {
            return 0;
        }
    }

    return name[sizeof(prefix) - 1] == '\0' || name[sizeof(prefix) - 1] == '@';
}

static uint64_t read_cells(const fdt32_t *cells, int count) {
    uint64_t value = 0;

    for (int i = 0; i < count; i++) {
        value = (value << 32) | fdt32_to_cpu(cells[i]);
    }

    return value;
}

uintptr_t kernelx_fdt_memory_top(const void *fdt) {
    int address_cells = fdt_address_cells(fdt, 0);
    int size_cells = fdt_size_cells(fdt, 0);
    if (address_cells <= 0 || address_cells > 2 || size_cells <= 0 || size_cells > 2) {
        return 0;
    }

    int range_cells = address_cells + size_cells;
    int node_offset;

    fdt_for_each_subnode(node_offset, fdt, 0) {
        const char *node_name = fdt_get_name(fdt, node_offset, NULL);
        if (!node_name || !is_memory_node(node_name)) {
            continue;
        }

        int prop_len;
        const fdt32_t *reg = fdt_getprop(fdt, node_offset, "reg", &prop_len);
        if (!reg || prop_len < range_cells * (int)sizeof(fdt32_t)) {
            continue;
        }

        uint64_t base = read_cells(reg, address_cells);
        uint64_t size = read_cells(reg + address_cells, size_cells);
        uint64_t top = base + size;

        if (size == 0 || top < base || top > UINTPTR_MAX) {
            continue;
        }

        return (uintptr_t)top;
    }

    return 0;
}
