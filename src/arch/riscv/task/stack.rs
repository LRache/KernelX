use core::alloc::Layout;

use crate::arch;
use crate::kernel::mm::{MapPerm, PhysPageFrame};
use crate::klib::{InitedCell, SpinLock};

use super::super::pagetable::kernelpagetable;
use super::super::{KERNEL_STACK_ARENA_END, KERNEL_STACK_ARENA_START, PGSIZE};

struct KernelStackAddressAllocator {
    allocator: buddy_system_allocator::FrameAllocator,
}

impl KernelStackAddressAllocator {
    fn new() -> Self {
        let mut allocator = buddy_system_allocator::FrameAllocator::new();
        allocator.add_frame(KERNEL_STACK_ARENA_START, KERNEL_STACK_ARENA_END);
        Self { allocator }
    }

    fn alloc(&mut self, page_count: usize) -> usize {
        let size = page_count.checked_mul(PGSIZE).expect("Kernel stack size overflow");
        let layout = Layout::from_size_align(size, size).expect("Invalid kernel stack layout");
        self.allocator
            .alloc_aligned(layout)
            .expect("Kernel stack virtual address space exhausted")
    }

    fn free(&mut self, base: usize, page_count: usize) {
        let size = page_count.checked_mul(PGSIZE).expect("Kernel stack size overflow");
        let layout = Layout::from_size_align(size, size).expect("Invalid kernel stack layout");
        self.allocator.dealloc_aligned(base, layout);
    }
}

static KERNEL_STACK_ADDRESS_ALLOCATOR: InitedCell<SpinLock<KernelStackAddressAllocator>> = InitedCell::uninit();

pub(in crate::arch::riscv) fn init_kernel_stack_allocator() {
    KERNEL_STACK_ADDRESS_ALLOCATOR.init(SpinLock::new(
        KernelStackAddressAllocator::new(),
        "KERNEL_STACK_ADDRESS_ALLOCATOR",
    ));
}

pub struct KernelStack<const MAPPED_PAGE_COUNT: usize> {
    base: usize,
    _frames: [PhysPageFrame; MAPPED_PAGE_COUNT],
}

impl<const MAPPED_PAGE_COUNT: usize> KernelStack<MAPPED_PAGE_COUNT> {
    pub fn new() -> Self {
        let page_count = MAPPED_PAGE_COUNT
            .checked_add(1)
            .expect("Kernel stack page count overflow");
        assert!(
            page_count >= 2 && page_count.is_power_of_two(),
            "Kernel stack allocation must contain 2^n pages including its guard page"
        );

        let base = KERNEL_STACK_ADDRESS_ALLOCATOR.lock().alloc(page_count);
        let frames = core::array::from_fn(|_| PhysPageFrame::alloc_zeroed());
        kernelpagetable::map_kernel_pages(
            base + PGSIZE,
            frames.iter().map(|frame| arch::kaddr_to_paddr(frame.get_page())),
            MapPerm::RW,
        );

        Self { base, _frames: frames }
    }

    pub fn get_top(&self) -> usize {
        self.base + (MAPPED_PAGE_COUNT + 1) * PGSIZE
    }

    pub fn check_stack_overflow(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.base + PGSIZE
    }
}

impl<const MAPPED_PAGE_COUNT: usize> Drop for KernelStack<MAPPED_PAGE_COUNT> {
    fn drop(&mut self) {
        let mapped_base = self.base + PGSIZE;
        let mapped_size = MAPPED_PAGE_COUNT * PGSIZE;
        // SAFETY: This stack is no longer reachable by a task when its owning
        // task object is dropped, and the unmap completes its global TLB
        // invalidation before the backing frames and VA slot are recycled.
        unsafe { arch::unmap_kernel_addr(mapped_base, mapped_size) };
        KERNEL_STACK_ADDRESS_ALLOCATOR
            .lock()
            .free(self.base, MAPPED_PAGE_COUNT + 1);
    }
}
