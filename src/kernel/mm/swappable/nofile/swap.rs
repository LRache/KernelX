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

struct SwapperDisk {
    bitvec: SpinLock<BitVec>,
    driver: Arc<dyn BlockDriverOps>,
    block_per_slot: usize,
}

impl SwapperDisk {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self {
        let block_size = driver.get_block_size() as usize;
        let len = block_size * driver.get_block_count() as usize / arch::PGSIZE;
        let bitvec = SpinLock::new(BitVec::repeat(false, len));
        let block_per_slot = arch::PGSIZE / block_size;
        
        debug_assert!(block_per_slot * block_size == arch::PGSIZE);
        crate::kinfo!("Anonymous swapper: {} pages ({} KB) available.", len, len * arch::PGSIZE / 1024);
        
        Self {
            bitvec,
            driver, 
            block_per_slot,
        }
    }

    pub fn read_page(&self, slot: usize, frame: &PhysPageFrame) {
        let start = crate::kernel::event::timer::now();
        self.driver.read_blocks(
            slot * self.block_per_slot,
            frame.slice()
        ).expect("Failed to read swapped out page from block device");
        let end = crate::kernel::event::timer::now();
        counter_swap_in(end - start);
    }

    pub fn write_page(&self, pos: usize, frame: &PhysPageFrame) {
        self.driver.write_blocks(pos * self.block_per_slot, frame.slice()).expect("Failed to write back");
    }

    pub fn alloc_slot(&self) -> Option<usize> {
        // let array = self.array.lock();
        // let mut first_zero = None;
        // let mut last_cached = None;
        // for (index, &value) in array.iter().enumerate() {
        //     if value == 0 {
        //         first_zero = Some(index);
        //         break;
        //     } else if value & 1 == 1 && last_cached.is_none() {
        //         last_cached = Some(index);
        //     }
        // }
        // if let Some(pos) = first_zero {
        //     Some(pos)
        // } else {
        //     last_cached
        // }
        let mut bitvec = self.bitvec.lock();
        bitvec.first_zero().map(|pos| {
            bitvec.set(pos, true);
            pos
        })
    }

    // pub fn use_slot(&self, pos: usize, kpage: usize) {
    //     let mut array = self.array.lock();
    //     // array.set(pos, kpage);
    //     array[pos] = kpage;
    // }

    pub fn free_slot(&self,slot: usize) {
        // let mut array = self.array.lock();
        // array[pos] = 0;
        let mut bitvec = self.bitvec.lock();
        bitvec.set(slot, false);
    }

    // pub fn set_cached(&self, pos: usize, cached: bool) {
    //     let mut array = self.array.lock();
    //     let value = array[pos];
    //     if cached {
    //         array[pos] = value | 1;
    //     } else {
    //         array[pos] = value & !1;
    //     }
    // }

    // pub fn is_cached(&self, pos: usize, kpage: usize) -> bool {
    //     let array = self.array.lock();
    //     let value = array[pos];
    //     value == kpage && (value & 1) == 1
    // }
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

                let slot = state.disk_slot;

                let frame = PhysPageFrame::alloc_with_shrink_zeroed();
                SWAPPER.read_page(slot, &frame);

                // Don't free the slot here

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
                debug_assert!(state.disk_slot != NO_DISK_BLOCK);
                SWAPPER.read_page(state.disk_slot, &new_frame);
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
                SWAPPER.free_slot(state.disk_slot);
            }
        }
    }
}

impl SwappableFrame for SwappableNoFileFrameInner {
    fn swap_out(&self, dirty: bool) -> bool {
        let mut state = self.state.lock();
        
        let disk_slot = state.disk_slot;

        let allocated = match &mut state.state {
            State::Allocated(allocated) => { allocated },
            State::SwappedOut => { return false; }
        };
        
        let slot = if disk_slot != NO_DISK_BLOCK {
            disk_slot
        } else if let Some(slot) = SWAPPER.alloc_slot() {
            slot
        } else {
            return false;
        };

        let kpage = allocated.frame.get_page();
        // let need_write = dirty || !SWAPPER.is_cached(pos, kpage);

        // set cached here
        // SWAPPER.use_slot(pos, kpage);

        if dirty {
            SWAPPER.write_page(slot, &allocated.frame);
        }

        self.family_chain.lock().retain(|member| {
            if let Some(addrspace) = member.upgrade() {
                addrspace.unmap_swap_page(self.uaddr, kpage);
                true
            } else {
                false
            }
        });

        state.state = State::SwappedOut;
        state.disk_slot = slot;

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
            // Clear cache in the disk
            if state.disk_slot != NO_DISK_BLOCK {
                SWAPPER.free_slot(state.disk_slot);
            }
        }

        Some((accessed, dirty))
    }
}
