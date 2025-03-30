#include <etl/queue.h>
#include <kernelx/mm/page.hpp>

using namespace kernelx;

static char *top;
static etl::queue<void *, 128> freed;

void init_page() {
    extern char __heap_end[];
    top = __heap_end;
}

void *page::alloc() {
    if (!freed.empty()) {
        void *ptr = freed.front();
        freed.pop();
        return ptr;
    } else {
        char *ptr = top;
        top += PAGE_SIZE;
        return ptr;
    }
}

void page::free(void *ptr) {
    freed.push(ptr);
}
