use alloc::sync::Arc;
use bitvec::vec::BitVec;

use crate::driver::BlockDriverOps;
use crate::arch;
use crate::kernel::mm::swappable::swapper::counter_swap_in;
use crate::kernel::mm::swappable::swapper;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::mm::swappable::swappable::SwappableFrame;
use crate::klib::{InitedCell, SpinLock};

use super::frame::{SwappableNoFileFrameInner, State, AllocatedFrame, NO_DISK_BLOCK};

struct BitMap {
    allocated: BitVec,
    cached: BitVec, // `1` means this block has a cached page in memory `0` means this block only has a swapped out page on disk
}

struct SwapperDisk {
    bitmap: SpinLock<BitMap>,
    driver: Arc<dyn BlockDriverOps>,
    block_per_page: usize,
}

impl SwapperDisk {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self {
        let block_size = driver.get_block_size() as usize;
        let len = block_size * driver.get_block_count() as usize / arch::PGSIZE;
        let bitmap = SpinLock::new(BitMap { allocated: BitVec::repeat(false, len), cached: BitVec::repeat(false, len) });
        let block_per_page = arch::PGSIZE / block_size;
        
        debug_assert!(block_per_page * block_size == arch::PGSIZE);
        crate::kinfo!("Anonymous swapper: {} pages ({} KB) available.", len, len * arch::PGSIZE / 1024);
        
        Self {
            bitmap, 
            driver, 
            block_per_page,
        }
    }

    pub fn block_per_page(&self) -> usize {
        self.block_per_page
    }

    pub fn read_page(&self, block_start: usize, frame: &PhysPageFrame) {
        let start = crate::kernel::event::timer::now();
        self.driver.read_blocks(
            block_start, 
            frame.slice()
        ).expect("Failed to read swapped out page from block device");
        let end = crate::kernel::event::timer::now();
        counter_swap_in(end - start);
    }

    pub fn write_page(&self, block_start: usize, frame: &PhysPageFrame) {
        self.driver.write_blocks(block_start, frame.slice()).expect("Failed to write back");
    }

    pub fn alloc_slot(&self) -> Option<usize> {
        let bitmap = self.bitmap.lock();
        if let Some(pos) = bitmap.allocated.first_zero() {
            Some(pos)
        } else if let Some(pos) = bitmap.cached.first_one() {
            Some(pos)
        } else {
            None
        }
    }

    pub fn use_slot(&self, pos: usize) {
        let mut bitmap = self.bitmap.lock();
        bitmap.allocated.set(pos, true);
        bitmap.cached.set(pos, false);
    }

    pub fn free_slot(&self, pos: usize) {
        let mut bitmap = self.bitmap.lock();
        bitmap.allocated.set(pos, false);
        bitmap.cached.set(pos, false);
    }

    pub fn set_cached(&self, pos: usize, cached: bool) {
        self.bitmap.lock().cached.set(pos, cached);
    }

    pub fn is_cached(&self, pos: usize) -> bool {
        self.bitmap.lock().cached.get(pos).map(|b| *b).unwrap_or(false)
    }
}

static SWAPPER: InitedCell<SwapperDisk> = InitedCell::uninit();

pub fn init_swapper(driver: Arc<dyn BlockDriverOps>) {
    SWAPPER.init(SwapperDisk::new(driver));
}

impl SwappableNoFileFrameInner {
    pub fn get_page_swap_in(self: &Arc<Self>) -> usize {
        let mut state = self.state.lock();
        match &state.state {
            State::Allocated(allocated) => {
                allocated.frame.get_page()
            },
            State::SwappedOut => {
                let start = crate::kernel::event::timer::now();
                
                let disk_block = state.disk_block;

                let frame = PhysPageFrame::alloc_with_shrink_zeroed();
                SWAPPER.read_page(disk_block, &frame);

                // Cached the swap page in disk
                let pos = disk_block / SWAPPER.block_per_page();
                SWAPPER.set_cached(pos, true);
                
                let kpage = frame.get_page();
                state.state = State::Allocated(AllocatedFrame{
                    frame,
                    dirty: false,
                });
                swapper::push_lru(kpage, self.clone()); // Note: This clone might be problematic if self is not Arc

                let end = crate::kernel::event::timer::now();
                counter_swap_in(end - start);
                
                kpage
            }
        }
    }

    pub fn copy(&self, addrspace: &AddrSpace) -> (Arc<SwappableNoFileFrameInner>, usize) {
        let state = self.state.lock();
        let new_frame = match &state.state {
            State::Allocated(allocated) => {
                allocated.frame.copy()
            }
            State::SwappedOut => {
                let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
                debug_assert!(state.disk_block != NO_DISK_BLOCK);
                SWAPPER.read_page(state.disk_block, &new_frame);
                new_frame
            }
        };
        let kpage = new_frame.get_page();
        let allocated = Arc::new(SwappableNoFileFrameInner::allocated(self.uaddr, new_frame, addrspace.family_chain().clone()));
        swapper::push_lru(kpage, allocated.clone());
        (allocated, kpage)
    }

    pub fn free(&self) {
        let state = self.state.lock();
        match &state.state {
            State::Allocated(allocated) => {
                let key = allocated.frame.get_page();
                swapper::remove_lru(key);
            }
            State::SwappedOut => {
                let pos = state.disk_block / SWAPPER.block_per_page();
                SWAPPER.free_slot(pos);
            }
        }
    }
}

impl SwappableFrame for SwappableNoFileFrameInner {
    fn swap_out(&self, dirty: bool) -> bool {
        let mut state = self.state.lock();
        
        let disk_block = state.disk_block;

        let allocated = match &mut state.state {
            State::Allocated(allocated) => { allocated },
            State::SwappedOut => { return false; }
        };
        
        let pos = if disk_block != NO_DISK_BLOCK {
            disk_block / SWAPPER.block_per_page()
        } else if let Some(pos) = SWAPPER.alloc_slot() {
            pos
        } else {
            return false;
        };

        let block_start = pos * SWAPPER.block_per_page();
        let need_write = dirty || !SWAPPER.is_cached(pos);

        SWAPPER.use_slot(pos);

        if need_write {
            SWAPPER.write_page(block_start, &allocated.frame);
        }

        let kpage = allocated.frame.get_page();
        self.family_chain.lock().retain(|member| {
            if let Some(addrspace) = member.upgrade() {
                addrspace.unmap_swap_page(self.uaddr, kpage);
                true
            } else {
                false
            }
        });

        state.state = State::SwappedOut;
        state.disk_block = block_start;

        true
    }

    fn take_access_dirty_bit(&self) -> Option<(bool, bool)> {
        let mut state = self.state.lock();
        let allocated = match &mut state.state {
            State::Allocated(allocated) => { allocated },
            State::SwappedOut => { return None; }
        };

        let mut accessed = false;
        let mut dirty = allocated.dirty;

        for member in &*self.family_chain.lock() {
            if let Some(addrspace) = member.upgrade() {
                if let Some((a, d)) = addrspace.take_page_access_dirty_bit(self.uaddr) {
                    accessed |= a;
                    dirty |= d;
                }
            }
        }

        if dirty {
            allocated.dirty = true;
        }

        Some((accessed, dirty))
    }
}
