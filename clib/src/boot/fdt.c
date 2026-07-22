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

static void sort_regions(struct kernelx_mem_region *regions, size_t count) {
    for (size_t i = 1; i < count; i++) {
        struct kernelx_mem_region current = regions[i];
        size_t j = i;

        while (j > 0 && regions[j - 1].start > current.start) {
            regions[j] = regions[j - 1];
            j--;
        }
        regions[j] = current;
    }
}

static size_t merge_regions(struct kernelx_mem_region *regions, size_t count) {
    size_t merged = 0;

    for (size_t i = 0; i < count; i++) {
        if (merged > 0 && regions[i].start <= regions[merged - 1].end) {
            if (regions[i].end > regions[merged - 1].end) {
                regions[merged - 1].end = regions[i].end;
            }
            continue;
        }
        regions[merged++] = regions[i];
    }

    return merged;
}

size_t kernelx_fdt_memory_regions(const void *fdt, struct kernelx_mem_region *regions,
                                  size_t capacity) {
    int address_cells = fdt_address_cells(fdt, 0);
    int size_cells = fdt_size_cells(fdt, 0);
    if (!regions || capacity == 0 || address_cells <= 0 || address_cells > 2 ||
        size_cells <= 0 || size_cells > 2) {
        return 0;
    }

    int range_cells = address_cells + size_cells;
    size_t count = 0;
    int node_offset;

    fdt_for_each_subnode(node_offset, fdt, 0) {
        const char *node_name = fdt_get_name(fdt, node_offset, NULL);
        if (!node_name || !is_memory_node(node_name)) {
            continue;
        }

        int prop_len;
        const fdt32_t *reg = fdt_getprop(fdt, node_offset, "reg", &prop_len);
        int range_bytes = range_cells * (int)sizeof(fdt32_t);
        if (!reg || prop_len < range_bytes) {
            continue;
        }

        for (int offset = 0; offset + range_bytes <= prop_len; offset += range_bytes) {
            const fdt32_t *range = reg + offset / (int)sizeof(fdt32_t);
            uint64_t base = read_cells(range, address_cells);
            uint64_t size = read_cells(range + address_cells, size_cells);
            uint64_t top = base + size;

            if (size == 0 || top < base || top > UINTPTR_MAX ||
                base > UINTPTR_MAX - (KERNELX_MEM_REGION_PAGE_SIZE - 1)) {
                continue;
            }

            uintptr_t start = ((uintptr_t)base + KERNELX_MEM_REGION_PAGE_SIZE - 1) &
                              ~(uintptr_t)(KERNELX_MEM_REGION_PAGE_SIZE - 1);
            uintptr_t end = (uintptr_t)top & ~(uintptr_t)(KERNELX_MEM_REGION_PAGE_SIZE - 1);
            if (start >= end) {
                continue;
            }
            if (count == capacity) {
                return 0;
            }

            regions[count++] = (struct kernelx_mem_region){.start = start, .end = end};
        }
    }

    sort_regions(regions, count);
    return merge_regions(regions, count);
}
