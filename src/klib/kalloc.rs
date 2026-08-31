use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use spin::Mutex;

use crate::kernel::config;
use crate::kernel::mm::page;
use crate::klib;

unsafe extern "C" {
    fn init_heap(start: *mut c_void, size: usize);
    fn add_heap_pool(start: *mut c_void, size: usize) -> i32;
    fn try_malloc_aligned(align: usize, size: usize) -> *mut c_void;
    fn free_heap_ptr(ptr: *mut c_void);
    fn realloc_heap_ptr(ptr: *mut c_void, size: usize) -> *mut c_void;
}

struct HeapState {
    pool_pages: usize,
    max_pool_pages: usize,
}

impl HeapState {
    const fn new() -> Self {
        Self {
            pool_pages: 0,
            max_pool_pages: 0,
        }
    }
}

struct HeapAllocator {
    state: Mutex<HeapState>,
}

impl HeapAllocator {
    pub const fn new() -> Self {
        HeapAllocator {
            state: Mutex::new(HeapState::new()),
        }
    }

    fn preferred_pool_pages(minimum_pages: usize, available_pages: usize) -> Option<usize> {
        let minimum_pages = minimum_pages.checked_next_power_of_two()?;
        if available_pages < minimum_pages || available_pages == 0 {
            return None;
        }

        let default_pages = config::KERNEL_HEAP_POOL_SIZE / crate::arch::PGSIZE;
        let preferred_pages = default_pages.max(minimum_pages);
        Some(if available_pages >= preferred_pages {
            preferred_pages
        } else {
            // Only allocate available pages rounded down to the nearest power of two, to avoid fragmentation.
            1usize << available_pages.ilog2()
        })
    }

    pub fn init(&self) {
        let total_pages = page::total_pages();
        let max_pool_pages = total_pages * config::KERNEL_HEAP_MAX_PERCENT / 100;
        let default_pages = config::KERNEL_HEAP_POOL_SIZE / crate::arch::PGSIZE;
        let pool_pages = Self::preferred_pool_pages(default_pages, max_pool_pages)
            .expect("Insufficient page allocator space for the initial kernel heap pool");

        let heap_start =
            page::try_alloc_contiguous(pool_pages).expect("Failed to allocate the initial kernel heap pool");
        let heap_size = pool_pages * crate::arch::PGSIZE;

        // SAFETY: `heap_start..heap_start + heap_size` is an exclusive contiguous page
        // allocation that remains reserved for TLSF for the lifetime of the kernel.
        unsafe {
            init_heap(heap_start as *mut c_void, heap_size);
        }

        let mut state = self.state.lock();
        state.pool_pages = pool_pages;
        state.max_pool_pages = max_pool_pages;
    }

    fn alloc(&self, align: usize, size: usize) -> *mut u8 {
        if size == 0 || !align.is_power_of_two() {
            return core::ptr::null_mut();
        }

        let mut state = self.state.lock();
        if state.max_pool_pages == 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: TLSF has been initialized and all access is serialized by `state`.
        let ptr = unsafe { try_malloc_aligned(align, size) as *mut u8 };
        if !ptr.is_null() {
            return ptr;
        }

        let Some(required_bytes) = size
            .checked_add(align)
            .and_then(|bytes| bytes.checked_add(crate::arch::PGSIZE))
        else {
            return core::ptr::null_mut();
        };
        let minimum_pages = crate::arch::page_count(required_bytes);
        let available_pages = state.max_pool_pages.saturating_sub(state.pool_pages);
        let Some(pool_pages) = Self::preferred_pool_pages(minimum_pages, available_pages) else {
            return core::ptr::null_mut();
        };
        let Some(pool_start) = crate::kernel::mm::page::try_alloc_contiguous(pool_pages) else {
            return core::ptr::null_mut();
        };
        let pool_size = pool_pages * crate::arch::PGSIZE;

        // SAFETY: The new page allocation is exclusive, suitably aligned, and remains
        // reserved while it is registered as a TLSF pool. The heap lock is held.
        let added = unsafe { add_heap_pool(pool_start as *mut c_void, pool_size) } != 0;
        if !added {
            crate::kernel::mm::page::free_contiguous(pool_start, pool_pages);
            return core::ptr::null_mut();
        }
        state.pool_pages += pool_pages;

        // SAFETY: The new pool has been registered and the heap lock remains held.
        unsafe { try_malloc_aligned(align, size) as *mut u8 }
    }

    unsafe fn dealloc(&self, ptr: *mut u8) {
        let _state = self.state.lock();
        // SAFETY: The caller guarantees `ptr` was allocated from this TLSF instance.
        unsafe { free_heap_ptr(ptr as *mut c_void) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(core::mem::align_of::<usize>(), size);
        }
        if size == 0 {
            // SAFETY: The caller guarantees `ptr` was allocated from this TLSF instance.
            unsafe { self.dealloc(ptr) };
            return core::ptr::null_mut();
        }

        let _state = self.state.lock();
        // SAFETY: The caller guarantees `ptr` was allocated from this TLSF instance,
        // and all access to the allocator is serialized by `state`.
        unsafe { realloc_heap_ptr(ptr as *mut c_void, size) as *mut u8 }
    }
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= 1 * 1024 * 1024 {
            // 1 MiB threshold for large allocations
            crate::kwarn!("Large allocation: {} bytes", layout.size());
            klib::backtrace::print_backtrace();
        }

        // if layout.size() > crate::arch::PGSIZE {
        //     let pages = crate::arch::page_count(layout.size());
        //     let align = layout.align().max(crate::arch::PGSIZE);
        //     return crate::kernel::mm::page::alloc_contiguous_aligned(pages, align) as *mut u8;
        // }

        let ptr = self.alloc(layout.align(), layout.size());
        debug_assert!(!ptr.is_null());
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // if layout.size() > crate::arch::PGSIZE {
        //     let pages = crate::arch::page_count(layout.size());
        //     let align = layout.align().max(crate::arch::PGSIZE);
        //     crate::kernel::mm::page::free_contiguous_aligned(ptr as usize, pages, align);
        //     return;
        // }

        let _ = layout;
        unsafe { self.dealloc(ptr) };
    }
}

#[global_allocator]
static ALLOCATOR: HeapAllocator = HeapAllocator::new();

#[unsafe(link_section = ".text.init")]
pub fn init() {
    ALLOCATOR.init();
}

#[unsafe(no_mangle)]
extern "C" fn kernel_heap_alloc(align: usize, size: usize) -> *mut c_void {
    ALLOCATOR.alloc(align, size) as *mut c_void
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kernel_heap_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        // SAFETY: C `free` requires that `ptr` came from the same heap allocator.
        unsafe { ALLOCATOR.dealloc(ptr as *mut u8) };
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kernel_heap_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    // SAFETY: C `realloc` requires that a non-null `ptr` came from the same heap
    // allocator and has not already been freed.
    unsafe { ALLOCATOR.realloc(ptr as *mut u8, size) as *mut c_void }
}
