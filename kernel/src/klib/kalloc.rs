use spin::Mutex;
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use crate::println;

unsafe extern "C" {
    fn init_heap(start: *mut c_void, size: usize);
    fn malloc_aligned(align: usize, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);

    static __heap_start: u8;
    static __heap_end: u8;
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

    pub fn init(&self) {
        unsafe {
            let heap_start = core::ptr::addr_of!(__heap_start) as *mut c_void;
            let heap_end = core::ptr::addr_of!(__heap_end) as *mut c_void;
            let heap_size = heap_end as usize - heap_start as usize;

            println!("Initializing heap: 0x{:x} - 0x{:x} (size: {} bytes)", 
                    heap_start as usize, heap_end as usize, heap_size);

            init_heap(heap_start, heap_size);
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

// #[global_allocator]
// static ALLOCATOR: LockedHeap<32> = LockedHeap::new();

// #[alloc_error_handler]
// fn alloc_error_handler(layout: Layout) -> ! {
//     println!("Allocation error: {:?}", layout);
//     panic!("Heap allocation failed");
// }

pub fn init() {
    ALLOCATOR.init();
    // let start = addr_of!(__heap_start) as usize;
    // let end = addr_of!(__heap_end) as usize;
    // let size = end - start;
    // unsafe { ALLOCATOR.lock().init(start, size) };
}
