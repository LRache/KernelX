use core::alloc::Layout;
use crate::klib::{InitedCell, SpinLock};
use crate::arch;

struct FrameAllocator {
    allocator: buddy_system_allocator::FrameAllocator,
    allocated: usize,
    #[cfg(feature = "swap-memory")]
    waterlevel_high: usize,
    #[cfg(feature = "swap-memory")]
    waterlevel_low: usize,
}

impl FrameAllocator {
    #[allow(unused_variables)]
    fn new(allocator: buddy_system_allocator::FrameAllocator, total: usize) -> Self {
        Self {
            allocator,
            allocated: 0,
            #[cfg(feature = "swap-memory")]
            waterlevel_high: total * crate::kernel::config::KERNEL_PAGE_SHRINK_WATERLEVEL_HIGH / 100,
            #[cfg(feature = "swap-memory")]
            waterlevel_low:  total * crate::kernel::config::KERNEL_PAGE_SHRINK_WATERLEVEL_LOW / 100,
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        let layout = Layout::from_size_align(arch::PGSIZE, arch::PGSIZE).unwrap();
        let addr = self.allocator.alloc_aligned(layout)?;
        self.allocated += 1;
        Some(addr)
    }

    fn alloc_contiguous(&mut self, pages: usize) -> Option<usize> {
        let layout = Layout::from_size_align(pages * arch::PGSIZE, arch::PGSIZE).unwrap();
        let addr = self.allocator.alloc_aligned(layout)?;
        self.allocated += pages;
        Some(addr)
    }

    fn free(&mut self, addr: usize) {
        let layout = Layout::from_size_align(arch::PGSIZE, arch::PGSIZE).unwrap();
        self.allocator.dealloc_aligned(addr, layout);
        self.allocated -= 1;
    }

    fn free_contiguous(&mut self, addr: usize, pages: usize) {
        let layout = Layout::from_size_align(pages * arch::PGSIZE, arch::PGSIZE).unwrap();
        self.allocator.dealloc_aligned(addr, layout);
        self.allocated -= pages;
    }
}

static FRAME_ALLOCATOR: InitedCell<SpinLock<FrameAllocator>> = InitedCell::uninit();
// static META_PTR_BASE: InitedCell<usize> = InitedCell::uninit();
// static FRAME_BASE: InitedCell<usize> = InitedCell::uninit();

#[unsafe(link_section = ".text.init")]
pub fn init(frame_start: usize, frame_end: usize) {
    // let zone_size = frame_end - frame_start;
    // let ptr_zone_size = zone_size / (arch::PGSIZE + core::mem::size_of::<*const u8>()) * core::mem::size_of::<*const u8>();
    // META_PTR_BASE.init(frame_start);
    
    // let frame_base = (frame_start + ptr_zone_size + arch::PGSIZE - 1) & !(arch::PGSIZE - 1);
    // FRAME_BASE.init(frame_base);

    let frame_base = frame_start;
    let total = (frame_end - frame_start) / arch::PGSIZE;
    
    let mut allocator = buddy_system_allocator::FrameAllocator::new();
    allocator.add_frame(frame_base, frame_end);
    FRAME_ALLOCATOR.init(SpinLock::new(FrameAllocator::new(allocator, total)));
}

// fn page_meta_ref(page: usize) -> &'static mut *const () {
//     let index = (page - *FRAME_BASE) / arch::PGSIZE;
//     let meta_ptr_addr = *META_PTR_BASE + index * core::mem::size_of::<*const ()>();
//     unsafe { &mut *(meta_ptr_addr as *mut *const ()) }
// }

#[cfg(feature = "swap-memory")]
pub fn need_to_shrink() -> bool {
    let allocator = FRAME_ALLOCATOR.lock();
    allocator.allocated >= allocator.waterlevel_high
}

pub fn alloc() -> usize {
    if let Some(page) = FRAME_ALLOCATOR.lock().alloc() {
        page
    } else {
        panic!("Out of physical memory");
    }
}

#[cfg(feature = "swap-memory")]
pub fn alloc_with_shrink() -> usize {
    let mut allocator = FRAME_ALLOCATOR.lock();

    if allocator.allocated >= allocator.waterlevel_high {
        let to_shrink = allocator.allocated - allocator.waterlevel_low + 1;
        let min_to_shrink = to_shrink / 4 + 1;
        drop(allocator);

        crate::kernel::mm::swappable::shrink(to_shrink, min_to_shrink);
        
        return FRAME_ALLOCATOR.lock().alloc().unwrap()
    }
        
    allocator.alloc().unwrap()
}

pub fn alloc_zero() -> usize {
    let page = FRAME_ALLOCATOR.lock().alloc().unwrap();
    zero(page);
    page
}

#[cfg(feature = "swap-memory")]
pub fn alloc_with_shrink_zero() -> usize {
    let page = alloc_with_shrink();
    zero(page);
    page
}

pub fn alloc_contiguous(pages: usize) -> usize {
    let page = FRAME_ALLOCATOR.lock().alloc_contiguous(pages).unwrap();
    page
}

// pub fn alloc_with_meta<T>(meta: T) -> usize {
//     let page = alloc();
//     let meta_ptr = page_meta_ref(page);   
//     unsafe {
//         let ptr = alloc::alloc::alloc(Layout::new::<T>()) as *mut T;
//         if ptr.is_null() {
//             panic!("Failed to allocate memory for page meta");
//         }
//         *meta_ptr = ptr as *const ();
//         core::ptr::write(ptr, meta);
//     }
//     page
// }

pub fn free(page: usize) {
    FRAME_ALLOCATOR.lock().free(page);
}

pub fn free_contiguous(addr: usize, pages: usize) {
    FRAME_ALLOCATOR.lock().free_contiguous(addr, pages);
}

// fn free_with_meta<T>(page: usize) {
//     let meta_ptr = page_meta_ref(page);
//     unsafe {
//         let ptr = *meta_ptr as *mut T;
//         if !ptr.is_null() {
//             core::ptr::drop_in_place(ptr);
//             alloc::alloc::dealloc(ptr as *mut u8, Layout::new::<T>());
//         }
//     }
//     free(page);
// }

// fn meta_of<T>(page: usize) -> &'static mut T {
//     let meta_ptr = page_meta_ref(page);
//     unsafe {
//         let ptr = *meta_ptr as *mut T;
//         debug_assert!(!ptr.is_null(), "No meta data found for page {:#x}", page);
//         &mut *ptr
//     }
// }

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

#[derive(Debug)]
pub struct PhysPageFrame {
    page: usize,
}

impl PhysPageFrame {
    pub fn new(page: usize) -> Self {
        Self { page }
    }

    pub fn alloc() -> Self {
        Self::new(alloc())
    }

    pub fn alloc_zeroed() -> Self {
        Self::new(alloc_zero())
    }

    #[cfg(feature = "swap-memory")]
    pub fn alloc_with_shrink_zeroed() -> Self {
        Self::new(alloc_with_shrink_zero())
    }

    pub fn copy(&self) -> PhysPageFrame {
        let new_frame = PhysPageFrame::alloc();
        copy(self.page, new_frame.page);
        new_frame
    }

    pub fn copy_from_slice(&self, offset: usize, src: &[u8]) {
        safe_page_write!(self.page + offset, src);
    }

    pub fn copy_to_slice(&self, offset: usize, dst: &mut [u8]) {
        debug_assert!(offset + dst.len() <= arch::PGSIZE, "Slice exceeds page frame bounds");
        unsafe {
            let src_ptr = (self.page + offset) as *const u8;
            let dst_ptr = dst.as_mut_ptr();
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, dst.len());
        }
    }

    pub fn slice(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.page as *mut u8, arch::PGSIZE) }
    }

    pub fn get_page(&self) -> usize {
        self.page
    }

    pub fn ptr(&self) -> *mut u8 {
        self.page as *mut u8
    }
}

impl Drop for PhysPageFrame {
    fn drop(&mut self) {
        free(self.page);
    }
}

// pub struct PageFrameWithMeta<T> {
//     page: usize,
//     _marker: core::marker::PhantomData<T>,
// }

// impl<T> PageFrameWithMeta<T> {
//     pub fn new(page: usize) -> Self {
//         Self { page, _marker: core::marker::PhantomData }
//     }

//     pub fn alloc(meta: T) -> Self {
//         let page = alloc_with_meta(meta);
//         Self::new(page)
//     }

//     pub fn meta(&self) -> &'static mut T {
//         meta_of::<T>(self.page)
//     }

//     pub fn get_page(&self) -> usize {
//         self.page
//     }
// }
