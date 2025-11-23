use core::alloc::Layout;
use buddy_system_allocator::LockedFrameAllocator;
// use alloc::collections::VecDeque;
// use spin::Mutex;

use crate::klib::InitedCell;
use crate::arch;

// struct PageAllocator {
//     freed: VecDeque<usize>,
//     bottom: usize,
//     top: usize,
// }

// impl PageAllocator {
//     pub const fn new() -> Self {
//         PageAllocator {
//             freed: VecDeque::new(),
//             bottom: 0,
//             top: 0,
//         }
//     }

//     pub fn init(&mut self, bottom: usize) {
//         self.bottom = bottom;
//         self.top = bottom + arch::PGSIZE;
//     }

//     pub fn alloc(&mut self) -> usize {
//         match self.freed.pop_back() {
//             Some(page) => page,
//             None => {
//                 debug_assert!(self.top < 0xffffffff88000000, "Page top overflow: {:#x}", self.top);
//                 let page = self.top;
//                 self.top += arch::PGSIZE;
//                 page
//             }
//         }
//     }

//     pub fn alloc_contiguous(&mut self, pages: usize) -> usize {
//         let addr = self.top;
//         self.top += pages * arch::PGSIZE;
//         debug_assert!(self.top < 0xffffffff88000000, "Page top overflow: {:#x}", self.top);
//         addr
//     }

//     pub fn free(&mut self, addr: usize) {
//         debug_assert!(addr % arch::PGSIZE == 0, "Address must be page-aligned: {:#x}", addr);
//         debug_assert!(addr < self.top, "Attempted to free an invalid address: {:#x}", addr);
//         debug_assert!(self.freed.iter().find(|&x| *x == addr).is_none(), "Address {:#x} is already freed", addr);

//         // fill freed page with 0xff in debug mode
//         if cfg!(debug_assertions) {
//             unsafe { core::ptr::write_bytes(addr as *mut u8, 'A' as u8, arch::PGSIZE); }
//         }
        
//         self.freed.push_back(addr);
//     }
// }

// static ALLOCATOR: Mutex<PageAllocator> = Mutex::new(PageAllocator::new());
static FRAME_ALLOCATOR: InitedCell<LockedFrameAllocator> = InitedCell::uninit();

#[unsafe(link_section = ".text.init")]
pub fn init(frame_start: usize, frame_end: usize) {
    // let mut allocator = ALLOCATOR.lock();
    // allocator.init(heap_end);
    let allocator = LockedFrameAllocator::new();
    allocator.lock().add_frame(frame_start, frame_end);
    FRAME_ALLOCATOR.init(allocator);
}

pub fn alloc() -> usize {
    // ALLOCATOR.lock().alloc()
    let page = FRAME_ALLOCATOR.lock().alloc_aligned(Layout::from_size_align(arch::PGSIZE, arch::PGSIZE).unwrap()).unwrap();
    // kinfo!("alloc page at {:#x}", page);
    page
}

pub fn alloc_zero() -> usize {
    // let addr = ALLOCATOR.lock().alloc();
    let page = FRAME_ALLOCATOR.lock().alloc_aligned(Layout::from_size_align(arch::PGSIZE, arch::PGSIZE).unwrap()).unwrap();
    zero(page);
    page
}

pub fn alloc_contiguous(pages: usize) -> usize {
    // ALLOCATOR.lock().alloc_contiguous(pages)
    let layout = Layout::from_size_align(pages * arch::PGSIZE, arch::PGSIZE).unwrap();
    let page = FRAME_ALLOCATOR.lock().alloc_aligned(layout).unwrap();
    // kinfo!("alloc contiguous {} pages at {:#x}", pages, page);
    page
}

pub fn free(page: usize) {
    // ALLOCATOR.lock().free(addr);
    let layout = Layout::from_size_align(arch::PGSIZE, arch::PGSIZE).unwrap();
    FRAME_ALLOCATOR.lock().dealloc_aligned(page, layout);
}

pub fn free_contiguous(addr: usize, pages: usize) {
    // let mut allocator = ALLOCATOR.lock();
    // for i in 0..pages {
    //     allocator.free(addr + i * arch::PGSIZE);
    // }
    let layout = Layout::from_size_align(pages * arch::PGSIZE, arch::PGSIZE).unwrap();
    FRAME_ALLOCATOR.lock().dealloc_aligned(addr, layout);
}

pub fn copy(src: usize, dst: usize) {
    debug_assert!(src % arch::PGSIZE == 0, "Source address must be page-aligned: {:#x}", src);
    debug_assert!(dst % arch::PGSIZE == 0, "Destination address must be page-aligned: {:#x}", dst);
    debug_assert!(src != dst, "Source and destination addresses must be different: {:#x}", src);
    
    unsafe {
        core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, arch::PGSIZE);
    }
}

pub fn zero(addr: usize) {
    unsafe { 
        core::ptr::write_bytes(addr as *mut u8, 0, arch::PGSIZE);
    }
}

#[macro_export]
macro_rules! safe_page_write {
    ($addr:expr, $buffer:expr) => {
        {
            let addr = $addr;
            let buffer = $buffer;
            
            // Only perform bounds checking in debug mode
            if cfg!(debug_assertions) {
                if (addr & $crate::arch::PGMASK) + buffer.len() > $crate::arch::PGSIZE {
                    panic!(
                        "Buffer exceeds page size at {}:{}:{}\n  addr = {:#x}, len = {:#x}",
                        file!(),
                        line!(),
                        column!(),
                        addr,
                        buffer.len()
                    );
                }
            }

            unsafe {
                core::slice::from_raw_parts_mut(addr as *mut u8, buffer.len())
                    .copy_from_slice(buffer);
            }
        }
    };
}
