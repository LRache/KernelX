use spin::Mutex;
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use crate::println;

unsafe extern "C" {
    fn init_heap(start: *mut c_void, size: usize);
    fn malloc_aligned(align: usize, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
}

struct HeapAllocator {
    mutex: Mutex<()>,
}

impl HeapAllocator {
    pub const fn new() -> Self {
        HeapAllocator {
            mutex: Mutex::new(()),
        }
    }

    pub fn init(&self, heap_start: usize, heap_size: usize) {
        unsafe {
            init_heap(heap_start as *mut c_void, heap_size);
            println!("Heap initialized successfully!");
        }
    }
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = self.mutex.lock();
        unsafe { malloc_aligned(layout.align(), layout.size()) as *mut u8 }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let _ = self.mutex.lock();
        unsafe { free(ptr as *mut c_void) };
    }
}

#[global_allocator]
static ALLOCATOR: HeapAllocator = HeapAllocator::new();

pub fn init(heap_start: usize, heap_size: usize) {
    ALLOCATOR.init(heap_start, heap_size);
}
