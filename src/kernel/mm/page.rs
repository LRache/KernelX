use buddy_slab_allocator::SlabPoolTrait;
use buddy_slab_allocator::eii::{slab_pool_impl, virt_to_phys_impl};
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::mutex::SpinMutex;

use crate::arch;
use crate::kernel::mm::MemRegion;
use crate::klib::InitedCell;

#[virt_to_phys_impl]
fn buddy_slab_virt_to_phys(vaddr: usize) -> usize {
    arch::kaddr_to_paddr(vaddr)
}

// buddy-slab-allocator requires this EII hook even when only BuddyAllocator is used.
#[slab_pool_impl]
fn buddy_slab_pool() -> &'static dyn SlabPoolTrait {
    unreachable!("the KernelX page allocator does not use the slab allocator")
}

struct FrameAllocator {
    allocator: buddy_slab_allocator::BuddyAllocator<{ arch::PGSIZE }>,
    waterlevel_high: usize,
    waterlevel_low: usize,
}

impl FrameAllocator {
    fn new(allocator: buddy_slab_allocator::BuddyAllocator<{ arch::PGSIZE }>, total: usize) -> Self {
        Self {
            allocator,
            waterlevel_high: total * crate::kernel::config::KERNEL_PAGE_SHRINK_WATERLEVEL_HIGH / 100,
            waterlevel_low: total * crate::kernel::config::KERNEL_PAGE_SHRINK_WATERLEVEL_LOW / 100,
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        let addr = self.allocator.alloc_pages(1, arch::PGSIZE).ok()?;
        ALLOCATED_PAGES.fetch_add(1, Ordering::Relaxed);
        Some(addr)
    }

    fn alloc_contiguous(&mut self, pages: usize) -> Option<usize> {
        self.alloc_contiguous_aligned(pages, arch::PGSIZE)
    }

    fn alloc_contiguous_aligned(&mut self, pages: usize, align: usize) -> Option<usize> {
        let addr = self.allocator.alloc_pages(pages, align.max(arch::PGSIZE)).ok()?;
        ALLOCATED_PAGES.fetch_add(pages.next_power_of_two(), Ordering::Relaxed);
        Some(addr)
    }

    fn free(&mut self, addr: usize) {
        self.allocator.dealloc_pages(addr, 1);
        ALLOCATED_PAGES.fetch_sub(1, Ordering::Relaxed);
    }

    fn free_contiguous(&mut self, addr: usize, pages: usize) {
        self.free_contiguous_aligned(addr, pages, arch::PGSIZE);
    }

    fn free_contiguous_aligned(&mut self, addr: usize, pages: usize, _align: usize) {
        self.allocator.dealloc_pages(addr, pages);
        ALLOCATED_PAGES.fetch_sub(pages.next_power_of_two(), Ordering::Relaxed);
    }
}

static FRAME_ALLOCATOR: InitedCell<SpinMutex<FrameAllocator>> = InitedCell::uninit();
static TOTAL_PAGES: InitedCell<usize> = InitedCell::uninit();
static ALLOCATED_PAGES: AtomicUsize = AtomicUsize::new(0);

#[unsafe(link_section = ".text.init")]
pub fn init(bootstrap_end: usize, regions: &[MemRegion]) {
    let mut allocator = buddy_slab_allocator::BuddyAllocator::new();

    let bootstrap_end_paddr = arch::kaddr_to_paddr(bootstrap_end);
    assert_eq!(bootstrap_end_paddr % arch::PGSIZE, 0);
    let mut initialized = false;

    for region in regions {
        let start = region.start.max(bootstrap_end_paddr);
        if start >= region.end {
            continue;
        }

        let kaddr = arch::paddr_to_kaddr(start);
        // SAFETY: The range is page-aligned RAM above all bootstrap allocations.
        // Each normalized memory region is disjoint and is handed exclusively to
        // the page allocator for the lifetime of the kernel.
        let memory = unsafe { core::slice::from_raw_parts_mut(kaddr as *mut u8, region.end - start) };

        // SAFETY: `memory` is a disjoint, writable RAM region that remains valid
        // for the lifetime of the allocator.
        unsafe {
            if initialized {
                allocator.add_region(memory)
            } else {
                allocator.init(memory)
            }
            .expect("Failed to register page allocator region");
        }
        initialized = true;
    }

    assert!(initialized, "No memory remains after bootstrap allocations");
    let total = allocator.total_pages();
    TOTAL_PAGES.init(total);
    FRAME_ALLOCATOR.init(SpinMutex::new(FrameAllocator::new(allocator, total)));
}

pub fn total_pages() -> usize {
    *TOTAL_PAGES
}

pub fn free_pages() -> usize {
    *TOTAL_PAGES - allocated_pages()
}

pub fn allocated_pages() -> usize {
    ALLOCATED_PAGES.load(Ordering::Relaxed)
}

fn allocation_failed(kind: &str, requested_pages: usize) -> ! {
    crate::kwarn!(
        "OOM: {} allocation failed: requested_pages={}, allocated_pages={}, free_pages={}, total_pages={}",
        kind,
        requested_pages,
        allocated_pages(),
        free_pages(),
        total_pages(),
    );
    crate::kernel::mm::swappable::print_perf_info();
    panic!("Out of physical memory");
}

pub fn need_to_shrink() -> bool {
    let allocator = FRAME_ALLOCATOR.lock();
    allocated_pages() >= allocator.waterlevel_high
}

pub fn alloc() -> usize {
    FRAME_ALLOCATOR.lock().alloc().expect("Out of physical memory")
}

pub fn alloc_with_shrink() -> usize {
    let mut allocator = FRAME_ALLOCATOR.lock();
    let allocated = allocated_pages();

    if allocated >= allocator.waterlevel_high {
        let to_shrink = allocated - allocator.waterlevel_low + 1;
        let min_to_shrink = to_shrink / 4 + 1;
        drop(allocator);

        crate::kernel::mm::swappable::shrink(to_shrink, min_to_shrink);

        return FRAME_ALLOCATOR
            .lock()
            .alloc()
            .unwrap_or_else(|| allocation_failed("shrink-aware", 1));
    }

    allocator
        .alloc()
        .unwrap_or_else(|| allocation_failed("shrink-aware", 1))
}

pub fn alloc_zero() -> usize {
    let page = FRAME_ALLOCATOR
        .lock()
        .alloc()
        .unwrap_or_else(|| allocation_failed("single-page", 1));
    zero(page);
    page
}

pub fn alloc_with_shrink_zero() -> usize {
    let page = alloc_with_shrink();
    zero(page);
    page
}

pub fn alloc_contiguous(pages: usize) -> usize {
    try_alloc_contiguous(pages).unwrap_or_else(|| allocation_failed("contiguous", pages))
}

pub fn try_alloc_contiguous(pages: usize) -> Option<usize> {
    FRAME_ALLOCATOR.lock().alloc_contiguous(pages)
}

pub fn alloc_contiguous_zero(pages: usize) -> usize {
    let page = FRAME_ALLOCATOR
        .lock()
        .alloc_contiguous(pages)
        .unwrap_or_else(|| allocation_failed("contiguous", pages));
    // SAFETY: `page` is an exclusive allocation of `pages` contiguous, writable pages,
    // and `pages * arch::PGSIZE` is exactly the allocation size.
    unsafe {
        core::ptr::write_bytes(
            page as *mut usize,
            0,
            pages * arch::PGSIZE / core::mem::size_of::<usize>(),
        );
    }
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

pub fn copy(src: usize, dst: usize) {
    debug_assert!(
        src % arch::PGSIZE == 0,
        "Source address must be page-aligned: {:#x}",
        src
    );
    debug_assert!(
        dst % arch::PGSIZE == 0,
        "Destination address must be page-aligned: {:#x}",
        dst
    );
    debug_assert!(
        src != dst,
        "Source and destination addresses must be different: {:#x}",
        src
    );

    unsafe {
        core::ptr::copy_nonoverlapping(
            src as *const usize,
            dst as *mut usize,
            arch::PGSIZE / core::mem::size_of::<usize>(),
        );
    }
}

pub fn zero(kpage: usize) {
    unsafe {
        if kpage & arch::PGMASK != 0 {
            core::hint::unreachable_unchecked();
        }
        core::ptr::write_bytes(kpage as *mut usize, 0, arch::PGSIZE / core::mem::size_of::<usize>());
    }
}

#[macro_export]
macro_rules! safe_page_write {
    ($addr:expr, $buffer:expr) => {{
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
            core::slice::from_raw_parts_mut(addr as *mut u8, buffer.len()).copy_from_slice(buffer);
        }
    }};
}

#[derive(Debug)]
pub struct PhysPageFrame {
    page: usize,
}

impl PhysPageFrame {
    fn new(page: usize) -> Self {
        Self { page }
    }

    pub fn alloc() -> Self {
        Self::new(alloc())
    }

    pub fn alloc_zeroed() -> Self {
        Self::new(alloc_zero())
    }

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
}

impl Drop for PhysPageFrame {
    fn drop(&mut self) {
        free(self.page);
    }
}

#[derive(Debug)]
pub struct ContiguousPhysPageFrame {
    page: usize,
    n: usize,
}

impl ContiguousPhysPageFrame {
    pub fn new(page: usize, n: usize) -> Self {
        debug_assert!(n > 0, "Page count must be greater than zero");
        Self { page, n }
    }

    pub fn alloc(n: usize) -> Self {
        Self::new(alloc_contiguous(n), n)
    }

    pub fn size(&self) -> usize {
        self.n * arch::PGSIZE
    }

    pub fn slice(&self) -> &mut [u8] {
        // SAFETY: `page` points to `n` contiguous page frames owned by this object until Drop.
        unsafe { core::slice::from_raw_parts_mut(self.page as *mut u8, self.size()) }
    }

    pub fn get_page(&self) -> usize {
        self.page
    }

    pub fn ptr(&self) -> *mut u8 {
        self.get_page() as *mut u8
    }
}

impl Drop for ContiguousPhysPageFrame {
    fn drop(&mut self) {
        free_contiguous(self.page, self.n);
    }
}

#[derive(Debug)]
pub struct FixedContiguousPhysPageFrame<const N: usize> {
    page: usize,
}

impl<const N: usize> FixedContiguousPhysPageFrame<N> {
    pub fn new(page: usize) -> Self {
        debug_assert!(N > 0, "Page count must be greater than zero");
        Self { page }
    }

    pub fn alloc() -> Self {
        Self::new(alloc_contiguous(N))
    }

    pub fn slice(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.page as *mut u8, N * arch::PGSIZE) }
    }

    pub fn get_page(&self) -> usize {
        self.page
    }

    pub fn ptr(&self) -> *mut u8 {
        self.get_page() as *mut u8
    }
}

impl<const N: usize> Drop for FixedContiguousPhysPageFrame<N> {
    fn drop(&mut self) {
        free_contiguous(self.page, N);
    }
}
